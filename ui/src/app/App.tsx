import { Dialog } from '@radix-ui/react-dialog';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { Command } from 'cmdk';
import {
  AlertTriangle,
  Braces,
  CheckCircle2,
  Download,
  Eye,
  FileArchive,
  FilePlus2,
  FileText,
  FolderTree,
  GitBranch,
  Hash,
  Heading1,
  Image as ImageIcon,
  Library,
  Link2,
  Moon,
  MoreHorizontal,
  Network,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Save,
  RotateCcw,
  Search,
  Settings as SettingsIcon,
  Sun,
  Trash2,
  Unlink,
} from 'lucide-react';
import {
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { Tree, type MoveHandler, type RowRendererProps } from 'react-arborist';
import {
  Panel,
  PanelGroup,
  PanelResizeHandle,
  type ImperativePanelHandle,
} from 'react-resizable-panels';
import { BrowserRouter, useLocation, useNavigate } from 'react-router-dom';
import useSWR, { useSWRConfig } from 'swr';

import {
  ApiPreconditionError,
  backlinks,
  createDocument,
  createGitPeer,
  createLibrary,
  deleteDocument,
  diffVersion,
  documentHref,
  documentVersion,
  getDocument,
  gitExport,
  gitImport,
  gitPull,
  gitPush,
  gitSync,
  graph as loadGraph,
  isTextContentType,
  listConflicts,
  listDocuments,
  listGitPeers,
  listLibraries,
  moveDocument,
  outgoingLinks,
  putDocument,
  resolveConflict,
  restoreVersion,
  searchDocuments,
  versions,
} from '../api/client';
import type {
  DocumentLink,
  DocumentListEntry,
  DocumentVersion,
  DocumentVersionContent,
  ConflictRecord,
  GraphResponse,
  Library as LibraryType,
  SearchResult,
  SearchSuggestion,
  VersionDiff,
} from '../api/generated/types';
import type {
  GitExportResult,
  GitImportResult,
  GitPeer,
  GitSyncResult,
} from '../api/client';
import { clearDraft, loadDraft, saveDraft } from '../features/editor/drafts';
import { MarkdownEditor } from '../features/editor/MarkdownEditor';
import { layoutGraphNodes } from '../features/graph/layout';
import { buildDocumentTree, droppedDocumentPath, type TreeNode } from '../features/tree/tree-model';
import { cn } from '../lib/utils';

type SaveState = 'clean' | 'dirty' | 'drafted' | 'saving' | 'saved' | 'stale' | 'failed';
type EventState = 'idle' | 'connecting' | 'open' | 'polling' | 'error';
type ThemePreference = 'light' | 'dark';
type TreeOpenState = Record<string, boolean>;
type RightPaneTab = 'links' | 'backlinks' | 'properties' | 'graph' | 'versions' | 'conflicts';
type GraphScope = 'focused' | 'full';
type GraphDepth = 1 | 2 | 3;
type GraphLinkKindFilter = 'all' | 'wiki_link' | 'markdown_link' | 'embed' | 'heading' | 'tag';
type GraphResolutionFilter = 'all' | 'resolved' | 'unresolved';
const EVENT_POLL_INTERVAL_MS = 5_000;
const RECENT_LIBRARY_LIMIT = 8;

interface BrowserEventPayload {
  type: string;
  path?: string | null;
  from?: string | null;
  to?: string | null;
  peer_id?: string | null;
  applied?: number | null;
  conflicts?: number | null;
}

interface SaveConflictDetails {
  baseEtag: string;
  path: string;
  remoteEtag: string;
}

interface TreeMenuState {
  node: TreeNode;
  x: number;
  y: number;
}

type SigmaRenderer = {
  kill: () => void;
  on: (event: 'clickNode', handler: (payload: { node: string }) => void) => void;
};

export function App() {
  return (
    <BrowserRouter>
      <Workspace />
    </BrowserRouter>
  );
}

function Workspace() {
  const location = useLocation();
  const navigate = useNavigate();
  const routeSelection = useMemo(() => parseWorkspaceRoute(location.pathname), [location.pathname]);
  const { mutate } = useSWRConfig();
  const { data: libraries = [] } = useSWR('/v1/libraries', listLibraries);
  const [activeLibrary, setActiveLibrary] = useState<string>(() => {
    return routeSelection.library ?? localStorage.getItem('quarry:active-library') ?? '';
  });
  const [treeOpenState, setTreeOpenState] = useState<TreeOpenState>(() =>
    loadTreeOpenState(activeLibrary)
  );
  const [rightPaneTab, setRightPaneTab] = useState<RightPaneTab>(() => loadRightPaneTab(activeLibrary));
  const [graphScope, setGraphScope] = useState<GraphScope>(() => loadGraphScope(activeLibrary));
  const [graphDepth, setGraphDepth] = useState<GraphDepth>(() => loadGraphDepth(activeLibrary));
  const [graphFolder, setGraphFolder] = useState(() => loadGraphFolder(activeLibrary));
  const [graphTag, setGraphTag] = useState(() => loadGraphTag(activeLibrary));
  const [graphLinkKind, setGraphLinkKind] = useState<GraphLinkKindFilter>(() => loadGraphLinkKind(activeLibrary));
  const [graphResolution, setGraphResolution] = useState<GraphResolutionFilter>(() =>
    loadGraphResolution(activeLibrary)
  );
  const [selectedPath, setSelectedPath] = useState(routeSelection.path ?? '');
  const [searchQuery, setSearchQuery] = useState('');
  const [content, setContent] = useState('');
  const [etag, setEtag] = useState('');
  const [contentType, setContentType] = useState('text/markdown');
  const [saveState, setSaveState] = useState<SaveState>('clean');
  const [conflictRemote, setConflictRemote] = useState<string | null>(null);
  const [conflictDetails, setConflictDetails] = useState<SaveConflictDetails | null>(null);
  const [selectedVersionId, setSelectedVersionId] = useState<string | null>(null);
  const [compareVersionId, setCompareVersionId] = useState<string | null>(null);
  const [currentDiffOpen, setCurrentDiffOpen] = useState(false);
  const [eventState, setEventState] = useState<EventState>('idle');
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState('');
  const [gitOpen, setGitOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [lastSyncResult, setLastSyncResult] = useState('');
  const [theme, setTheme] = useState<ThemePreference>(() =>
    localStorage.getItem('quarry:theme') === 'light' ? 'light' : 'dark'
  );
  const [mergeConflictId, setMergeConflictId] = useState<string | null>(null);
  const [treeMenu, setTreeMenu] = useState<TreeMenuState | null>(null);
  const leftPanelRef = useRef<ImperativePanelHandle>(null);
  const rightPanelRef = useRef<ImperativePanelHandle>(null);
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [resizingPanels, setResizingPanels] = useState(false);
  const selectedPathRef = useRef(selectedPath);
  const saveStateRef = useRef(saveState);
  const loadedDocumentRef = useRef<{ library: string; path: string; etag: string } | null>(null);
  const searchQueryRef = useRef(searchQuery);
  const appliedRouteRef = useRef(location.pathname);

  useEffect(() => {
    if (!activeLibrary && libraries.length >= 1) {
      const nextLibrary = orderLibrariesByRecent(libraries, '')[0]?.slug ?? libraries[0].slug;
      setActiveLibrary(nextLibrary);
      setTreeOpenState(loadTreeOpenState(nextLibrary));
      setRightPaneTab(loadRightPaneTab(nextLibrary));
      setGraphScope(loadGraphScope(nextLibrary));
      setGraphDepth(loadGraphDepth(nextLibrary));
      setGraphFolder(loadGraphFolder(nextLibrary));
      setGraphTag(loadGraphTag(nextLibrary));
      setGraphLinkKind(loadGraphLinkKind(nextLibrary));
      setGraphResolution(loadGraphResolution(nextLibrary));
    }
    if (activeLibrary && libraries.length > 0 && libraries.every((library) => library.slug !== activeLibrary)) {
      const nextLibrary = libraries[0]?.slug ?? '';
      setActiveLibrary(nextLibrary);
      setTreeOpenState(loadTreeOpenState(nextLibrary));
      setRightPaneTab(loadRightPaneTab(nextLibrary));
      setGraphScope(loadGraphScope(nextLibrary));
      setGraphDepth(loadGraphDepth(nextLibrary));
      setGraphFolder(loadGraphFolder(nextLibrary));
      setGraphTag(loadGraphTag(nextLibrary));
      setGraphLinkKind(loadGraphLinkKind(nextLibrary));
      setGraphResolution(loadGraphResolution(nextLibrary));
    }
  }, [activeLibrary, libraries]);

  useEffect(() => {
    if (appliedRouteRef.current === location.pathname) return;
    appliedRouteRef.current = location.pathname;
    const selection = parseWorkspaceRoute(location.pathname);
    if (selection.library) {
      setActiveLibrary(selection.library);
      setTreeOpenState(loadTreeOpenState(selection.library));
      setRightPaneTab(loadRightPaneTab(selection.library));
      setGraphScope(loadGraphScope(selection.library));
      setGraphDepth(loadGraphDepth(selection.library));
      setGraphFolder(loadGraphFolder(selection.library));
      setGraphTag(loadGraphTag(selection.library));
      setGraphLinkKind(loadGraphLinkKind(selection.library));
      setGraphResolution(loadGraphResolution(selection.library));
    }
    if (selection.path !== undefined) setSelectedPath(selection.path);
  }, [location.pathname]);

  useEffect(() => {
    const nextPath = workspaceRoute(activeLibrary, selectedPath);
    if (nextPath && location.pathname !== nextPath) {
      navigate(nextPath, { replace: location.pathname === '/' });
    }
  }, [activeLibrary, selectedPath, location.pathname, navigate]);

  useEffect(() => {
    if (activeLibrary) {
      localStorage.setItem('quarry:active-library', activeLibrary);
      persistRecentLibrary(
        activeLibrary,
        libraries.map((library) => library.slug)
      );
    }
  }, [activeLibrary, libraries]);

  useEffect(() => {
    localStorage.setItem('quarry:theme', theme);
    window.document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    setLastSyncResult('');
  }, [activeLibrary]);

  useEffect(() => {
    selectedPathRef.current = selectedPath;
  }, [selectedPath]);

  useEffect(() => {
    saveStateRef.current = saveState;
  }, [saveState]);

  useEffect(() => {
    searchQueryRef.current = searchQuery;
  }, [searchQuery]);

  useEffect(() => {
    function handleKeyboard(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setPaletteOpen(true);
        return;
      }
      if (event.key === 'Escape') {
        setPaletteOpen(false);
        setPaletteQuery('');
      }
    }

    window.addEventListener('keydown', handleKeyboard);
    return () => window.removeEventListener('keydown', handleKeyboard);
  }, []);

  useEffect(() => {
    if (!activeLibrary) {
      setEventState('idle');
      return;
    }
    let pollingTimer: number | null = null;
    const eventTypes = [
      'doc.changed',
      'doc.deleted',
      'doc.moved',
      'directory.changed',
      'stream.lagged',
      'links.indexed',
      'library.reindexed',
      'git.sync.completed',
      'conflict.created',
      'conflict.resolved',
    ];
    const handleEvent = (event: MessageEvent) => {
      const payload = parseBrowserEvent(event);
      if (payload) invalidateFromEvent(payload);
    };

    function invalidateDocumentState(path: string) {
      void mutate(['/v1/document', activeLibrary, path]);
      void mutate(['/v1/versions', activeLibrary, path]);
      void mutate(['/v1/outgoing', activeLibrary, path]);
      void mutate(['/v1/backlinks', activeLibrary, path]);
      void mutateGraphState();
    }

    function invalidateCurrentBacklinks() {
      const currentPath = selectedPathRef.current;
      if (currentPath) void mutate(['/v1/backlinks', activeLibrary, currentPath]);
    }

    function invalidateSearch() {
      const query = searchQueryRef.current;
      if (query) void mutate(['/v1/search', activeLibrary, query]);
    }

    function invalidateIndexedState() {
      const currentPath = selectedPathRef.current;
      if (currentPath) {
        void mutate(['/v1/outgoing', activeLibrary, currentPath]);
        void mutate(['/v1/backlinks', activeLibrary, currentPath]);
      }
      void mutateGraphState();
      invalidateSearch();
    }

    function invalidateGitSyncState(payload: BrowserEventPayload) {
      const currentPath = selectedPathRef.current;
      if (currentPath) {
        invalidateDocumentState(currentPath);
      } else {
        void mutateGraphState();
      }
      void mutate(['/v1/conflicts', activeLibrary]);
      void mutate(['/v1/git-peers', activeLibrary]);
      setLastSyncResult(gitSyncEventSummary(payload));
    }

    function invalidateFromEvent(payload: BrowserEventPayload) {
      void mutate(['/v1/documents', activeLibrary]);
      invalidateSearch();

      if (payload.type === 'stream.lagged' || payload.type === 'directory.changed') {
        const currentPath = selectedPathRef.current;
        if (currentPath) invalidateDocumentState(currentPath);
        void mutate(['/v1/conflicts', activeLibrary]);
        return;
      }

      if (payload.type === 'links.indexed' || payload.type === 'library.reindexed') {
        invalidateIndexedState();
        return;
      }

      if (payload.type === 'git.sync.completed') {
        invalidateGitSyncState(payload);
        return;
      }

      if (payload.type === 'conflict.created' || payload.type === 'conflict.resolved') {
        void mutate(['/v1/conflicts', activeLibrary]);
        return;
      }

      const currentPath = selectedPathRef.current;
      if (payload.type === 'doc.deleted' && payload.path === currentPath) {
        setSelectedPath('');
        return;
      }
      if (payload.type === 'doc.moved' && payload.from === currentPath && payload.to) {
        setSelectedPath(payload.to);
        invalidateDocumentState(payload.to);
        return;
      }
      if (payload.path && payload.path === currentPath) {
        invalidateDocumentState(payload.path);
        return;
      }
      invalidateCurrentBacklinks();
    }

    function pollServerState() {
      void mutate(['/v1/documents', activeLibrary]);
      const currentPath = selectedPathRef.current;
      if (currentPath) {
        invalidateDocumentState(currentPath);
      } else {
        void mutateGraphState();
      }
      void mutate(['/v1/conflicts', activeLibrary]);
      void mutate(['/v1/git-peers', activeLibrary]);
      invalidateSearch();
    }

    function startPollingFallback() {
      setEventState('polling');
      void pollServerState();
      if (pollingTimer !== null) return;
      pollingTimer = window.setInterval(() => {
        void pollServerState();
      }, EVENT_POLL_INTERVAL_MS);
    }

    function stopPollingFallback() {
      if (pollingTimer === null) return;
      window.clearInterval(pollingTimer);
      pollingTimer = null;
    }

    if (typeof EventSource === 'undefined') {
      startPollingFallback();
      return stopPollingFallback;
    }

    setEventState('connecting');
    const source = new EventSource(`/v1/events?library=${encodeURIComponent(activeLibrary)}`);
    for (const eventType of eventTypes) {
      source.addEventListener(eventType, handleEvent);
    }
    source.onopen = () => {
      stopPollingFallback();
      setEventState('open');
    };
    source.onerror = () => startPollingFallback();

    return () => {
      for (const eventType of eventTypes) {
        source.removeEventListener(eventType, handleEvent);
      }
      source.close();
      stopPollingFallback();
    };
  }, [activeLibrary, mutate]);

  const { data: documents = [] } = useSWR(
    activeLibrary ? ['/v1/documents', activeLibrary] : null,
    () => listDocuments(activeLibrary)
  );
  const { data: document } = useSWR(
    activeLibrary && selectedPath ? ['/v1/document', activeLibrary, selectedPath] : null,
    () => getDocument(activeLibrary, selectedPath)
  );
  const { data: search = { results: [], cursor: null } } = useSWR(
    activeLibrary && searchQuery ? ['/v1/search', activeLibrary, searchQuery] : null,
    () => searchDocuments(activeLibrary, searchQuery)
  );
  const { data: outgoing = { path: selectedPath, links: [] } } = useSWR(
    activeLibrary && selectedPath ? ['/v1/outgoing', activeLibrary, selectedPath] : null,
    () => outgoingLinks(activeLibrary, selectedPath)
  );
  const { data: incoming = { path: selectedPath, links: [] } } = useSWR(
    activeLibrary && selectedPath ? ['/v1/backlinks', activeLibrary, selectedPath] : null,
    () => backlinks(activeLibrary, selectedPath)
  );
  const graphRoot = graphScope === 'focused' ? selectedPath : undefined;
  const focusedGraphDepth = graphScope === 'focused' ? graphDepth : undefined;
  const graphFilters = useMemo(
    () => ({
      folder: graphFolder || undefined,
      tag: graphTag || undefined,
      linkKind: graphLinkKind === 'all' ? undefined : graphLinkKind,
      resolution: graphResolution === 'all' ? undefined : graphResolution,
    }),
    [graphFolder, graphTag, graphLinkKind, graphResolution]
  );
  const canLoadGraph = Boolean(activeLibrary && (graphScope === 'full' || selectedPath));
  const { data: graphData = { nodes: [], edges: [], truncated: false } } = useSWR(
    canLoadGraph
      ? graphCacheKey(
          activeLibrary,
          graphScope,
          selectedPath,
          graphDepth,
          graphFolder,
          graphTag,
          graphLinkKind,
          graphResolution
        )
      : null,
    () => loadGraph(activeLibrary, graphRoot, focusedGraphDepth, graphFilters)
  );
  const { data: versionList = [] } = useSWR(
    activeLibrary && selectedPath ? ['/v1/versions', activeLibrary, selectedPath] : null,
    () => versions(activeLibrary, selectedPath)
  );
  const headVersionId = versionList[0]?.id;
  const { data: selectedVersionContent } = useSWR(
    activeLibrary && selectedPath && selectedVersionId
      ? ['/v1/version-content', activeLibrary, selectedPath, selectedVersionId]
      : null,
    () => documentVersion(activeLibrary, selectedPath, selectedVersionId!)
  );
  const selectedDiffAgainstVersionId = compareVersionId ?? headVersionId;
  const { data: selectedVersionDiff } = useSWR(
    activeLibrary && selectedPath && selectedVersionId
      ? ['/v1/version-diff', activeLibrary, selectedPath, selectedVersionId, selectedDiffAgainstVersionId ?? '']
      : null,
    () => diffVersion(activeLibrary, selectedPath, selectedVersionId!, selectedDiffAgainstVersionId)
  );
  const currentEditorDiff = useMemo(
    () => unifiedLineDiff(document?.content ?? '', content, 'latest server', 'current editor'),
    [content, document?.content]
  );
  const { data: conflicts = [] } = useSWR(
    activeLibrary ? ['/v1/conflicts', activeLibrary] : null,
    () => listConflicts(activeLibrary)
  );
  const { data: gitPeers = [] } = useSWR(
    activeLibrary ? ['/v1/git-peers', activeLibrary] : null,
    () => listGitPeers(activeLibrary)
  );

  useEffect(() => {
    if (!document) return;
    const loadedDocument = loadedDocumentRef.current;
    const sameDocument =
      loadedDocument?.library === activeLibrary && loadedDocument.path === selectedPath;
    if (sameDocument && hasUnsavedEditorState(saveStateRef.current)) {
      if (loadedDocument.etag !== document.etag) {
        loadedDocumentRef.current = { library: activeLibrary, path: selectedPath, etag: document.etag };
        setEtag(document.etag);
        setContentType(document.contentType);
        setConflictDetails({
          baseEtag: loadedDocument.etag,
          path: selectedPath,
          remoteEtag: document.etag,
        });
        setConflictRemote(document.content);
        transitionSaveState('stale');
      }
      return;
    }

    const preserveSavedState =
      sameDocument && loadedDocument?.etag === document.etag && saveStateRef.current === 'saved';
    const draft = loadDraft(activeLibrary, selectedPath, document.etag);
    loadedDocumentRef.current = { library: activeLibrary, path: selectedPath, etag: document.etag };
    setContent(draft?.content ?? document.content);
    setEtag(document.etag);
    setContentType(document.contentType);
    transitionSaveState(draft ? 'drafted' : preserveSavedState ? 'saved' : 'clean');
    setConflictRemote(null);
    setConflictDetails(null);
    setSelectedVersionId(null);
    setCompareVersionId(null);
    setCurrentDiffOpen(false);
  }, [activeLibrary, document, selectedPath]);

  const tree = useMemo(
    () =>
      buildDocumentTree(
        documents.map((entry) => ({
          id: entry.id,
          path: entry.path,
          title: documentTitle(entry),
        }))
      ),
    [documents]
  );

  const activeLibraryRecord = libraries.find((library) => library.slug === activeLibrary);
  const selectedEntry = documents.find((entry) => entry.path === selectedPath);
  const loadedDocumentContentType = document?.path === selectedPath ? document.contentType : undefined;
  const selectedContentType = loadedDocumentContentType ?? selectedEntry?.content_type ?? contentType;
  const layoutStorageKey = activeLibrary ? `quarry:layout:${activeLibrary}` : 'quarry:layout:workspace';
  const mergeConflict = conflicts.find((conflict) => conflict.id === mergeConflictId) ?? null;
  const saveConflictDialogRef = useDialogFocusTrap(Boolean(conflictRemote), closeSaveConflictDialog);

  async function save() {
    if (!activeLibrary || !selectedPath || !etag) return;
    if (!isTextContentType(contentType)) return;
    transitionSaveState('saving');
    try {
      const saved = await putDocument(activeLibrary, selectedPath, content, etag, contentType);
      const savedEtag = saved.etag || `"${saved.outcome.version.id}"`;
      clearDraft(activeLibrary, selectedPath, etag);
      loadedDocumentRef.current = { library: activeLibrary, path: selectedPath, etag: savedEtag };
      setEtag(savedEtag);
      transitionSaveState('saved');
      await Promise.all([
        mutate(
          ['/v1/document', activeLibrary, selectedPath],
          { content, contentType, etag: savedEtag, path: selectedPath },
          { revalidate: false }
        ),
        mutate(['/v1/documents', activeLibrary]),
        mutate(['/v1/versions', activeLibrary, selectedPath]),
        mutate(['/v1/outgoing', activeLibrary, selectedPath]),
        mutate(['/v1/backlinks', activeLibrary, selectedPath]),
        mutateGraphState(),
        searchQuery ? mutate(['/v1/search', activeLibrary, searchQuery]) : Promise.resolve(),
      ]);
    } catch (error) {
      if (error instanceof ApiPreconditionError) {
        const baseEtag = etag;
        transitionSaveState('stale');
        const remote = await getDocument(activeLibrary, selectedPath);
        setConflictDetails({ baseEtag, path: selectedPath, remoteEtag: remote.etag });
        setConflictRemote(remote.content);
        setEtag(remote.etag);
        return;
      }
      transitionSaveState('failed');
    }
  }

  function changeContent(next: string) {
    setContent(next);
    if (activeLibrary && selectedPath && etag) {
      saveDraft(activeLibrary, selectedPath, etag, next);
      transitionSaveState('drafted');
    } else {
      transitionSaveState('dirty');
    }
  }

  async function createNewDocument(defaultPath = 'untitled.md') {
    if (!activeLibrary) return;
    const path = window.prompt('New document path', defaultPath);
    if (!path) return;
    await createDocument(activeLibrary, path, '# Untitled\n');
    await mutate(['/v1/documents', activeLibrary]);
    setSelectedPath(path);
  }

  async function createDocumentFromLink(link: DocumentLink) {
    if (!activeLibrary) return;
    const defaultPath = defaultDocumentPathForLink(link);
    const path = window.prompt('New document path', defaultPath);
    if (!path) return;
    if (path !== selectedPath && selectedPath && hasUnsavedEditorState(saveState)) {
      const confirmed = window.confirm(
        `Create ${path} and keep your unsaved draft for ${selectedPath}?`
      );
      if (!confirmed) return;
    }
    await createDocument(activeLibrary, path, '# Untitled\n');
    await Promise.all([
      mutate(['/v1/documents', activeLibrary]),
      selectedPath ? mutate(['/v1/outgoing', activeLibrary, selectedPath]) : Promise.resolve(),
      mutateGraphState(),
    ]);
    setSelectedPath(path);
  }

  async function renameCurrent() {
    if (!activeLibrary || !selectedPath) return;
    const toPath = window.prompt('Move document to path', selectedPath);
    if (!toPath || toPath === selectedPath) return;
    await moveDocument(activeLibrary, selectedPath, toPath);
    await mutate(['/v1/documents', activeLibrary]);
    setSelectedPath(toPath);
  }

  async function moveDocumentPath(fromPath: string) {
    if (!activeLibrary) return;
    const toPath = window.prompt('Move document to path', fromPath);
    if (!toPath || toPath === fromPath) return;
    await moveDocument(activeLibrary, fromPath, toPath);
    await mutate(['/v1/documents', activeLibrary]);
    if (selectedPath === fromPath) setSelectedPath(toPath);
  }

  const moveDroppedTreeDocuments: MoveHandler<TreeNode> = async ({ dragNodes, parentNode }) => {
    if (!activeLibrary) return;
    const parent = parentNode?.data ?? null;
    const moves = dragNodes
      .map((node) => node.data)
      .filter((node) => node.kind === 'document')
      .map((node) => ({ from: node.path, to: droppedDocumentPath(node, parent) }))
      .filter((move) => move.from !== move.to);
    if (!moves.length) return;
    await Promise.all(moves.map((move) => moveDocument(activeLibrary, move.from, move.to)));
    await mutate(['/v1/documents', activeLibrary]);
    const movedSelection = moves.find((move) => move.from === selectedPath);
    if (movedSelection) setSelectedPath(movedSelection.to);
  };

  async function deleteCurrent() {
    if (!activeLibrary || !selectedPath) return;
    if (!window.confirm(`Delete ${selectedPath}?`)) return;
    await deleteDocument(activeLibrary, selectedPath);
    await mutate(['/v1/documents', activeLibrary]);
    setSelectedPath('');
  }

  function downloadCurrentMarkdown() {
    if (!selectedPath) return;
    const blob = new Blob([content], { type: 'text/markdown;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const anchor = window.document.createElement('a');
    anchor.href = url;
    anchor.download = documentBasename(selectedPath);
    anchor.click();
    URL.revokeObjectURL(url);
  }

  async function deleteDocumentPath(path: string) {
    if (!activeLibrary) return;
    if (!window.confirm(`Delete ${path}?`)) return;
    await deleteDocument(activeLibrary, path);
    await mutate(['/v1/documents', activeLibrary]);
    if (selectedPath === path) setSelectedPath('');
  }

  async function restoreSelectedVersion(versionId: string) {
    if (!activeLibrary || !selectedPath) return;
    const restored = await restoreVersion(activeLibrary, selectedPath, versionId);
    setEtag(restored.etag || `"${restored.outcome.version.id}"`);
    transitionSaveState('saved');
    setSelectedVersionId(null);
    setCompareVersionId(null);
    await Promise.all([
      mutate(['/v1/document', activeLibrary, selectedPath]),
      mutate(['/v1/documents', activeLibrary]),
      mutate(['/v1/versions', activeLibrary, selectedPath]),
      mutate(['/v1/outgoing', activeLibrary, selectedPath]),
      mutate(['/v1/backlinks', activeLibrary, selectedPath]),
      mutateGraphState(),
    ]);
  }

  async function resolveOpenConflict(conflictId: string) {
    if (!activeLibrary) return;
    await resolveConflict(activeLibrary, conflictId);
    await mutate(['/v1/conflicts', activeLibrary]);
  }

  function openDocument(path: string) {
    if (!path || path === selectedPath) return;
    if (hasUnsavedEditorState(saveState) && selectedPath) {
      const confirmed = window.confirm(
        `Open ${path} and keep your unsaved draft for ${selectedPath}?`
      );
      if (!confirmed) return;
    }
    setSelectedPath(path);
  }

  function openTreeContextMenu(node: TreeNode, event: ReactKeyboardEvent | ReactMouseEvent) {
    event.preventDefault();
    const clientX = 'clientX' in event ? event.clientX : 32;
    const clientY = 'clientY' in event ? event.clientY : 48;
    setTreeMenu({ node, x: clientX, y: clientY });
  }

  function closeTreeContextMenu() {
    setTreeMenu(null);
  }

  function defaultChildDocumentPath(node: TreeNode) {
    const folder = node.kind === 'folder' ? node.path : node.path.split('/').slice(0, -1).join('/');
    return folder ? `${folder}/untitled.md` : 'untitled.md';
  }

  async function createTreeDocument(node: TreeNode) {
    closeTreeContextMenu();
    await createNewDocument(defaultChildDocumentPath(node));
  }

  async function moveTreeDocument(node: TreeNode) {
    closeTreeContextMenu();
    if (node.kind === 'document') await moveDocumentPath(node.path);
  }

  async function deleteTreeDocument(node: TreeNode) {
    closeTreeContextMenu();
    if (node.kind === 'document') await deleteDocumentPath(node.path);
  }

  function copyTreePath(node: TreeNode) {
    closeTreeContextMenu();
    void navigator.clipboard?.writeText(node.path);
  }

  function revealTreeDocument(node: TreeNode) {
    closeTreeContextMenu();
    if (node.kind === 'document') openDocument(node.path);
  }

  function changeActiveLibrary(slug: string) {
    setActiveLibrary(slug);
    setTreeOpenState(loadTreeOpenState(slug));
    setRightPaneTab(loadRightPaneTab(slug));
    setGraphScope(loadGraphScope(slug));
    setGraphDepth(loadGraphDepth(slug));
    setGraphFolder(loadGraphFolder(slug));
    setGraphTag(loadGraphTag(slug));
    setGraphLinkKind(loadGraphLinkKind(slug));
    setGraphResolution(loadGraphResolution(slug));
    setSelectedPath('');
    navigate(workspaceRoute(slug, ''), { replace: false });
  }

  function changeTreeOpenState(id: string) {
    setTreeOpenState((current) => {
      const next = { ...current, [id]: !(current[id] ?? true) };
      persistTreeOpenState(activeLibrary, next);
      return next;
    });
  }

  function changeRightPaneTab(tab: RightPaneTab) {
    setRightPaneTab(tab);
    persistRightPaneTab(activeLibrary, tab);
  }

  function changeGraphScope(scope: GraphScope) {
    setGraphScope(scope);
    persistGraphScope(activeLibrary, scope);
  }

  function changeGraphDepth(depth: GraphDepth) {
    setGraphDepth(depth);
    persistGraphDepth(activeLibrary, depth);
  }

  function changeGraphFolder(folder: string) {
    const nextFolder = normalizeGraphTextFilter(folder);
    setGraphFolder(nextFolder);
    persistGraphFolder(activeLibrary, nextFolder);
  }

  function changeGraphTag(tag: string) {
    const nextTag = normalizeGraphTextFilter(tag).replace(/^#/, '');
    setGraphTag(nextTag);
    persistGraphTag(activeLibrary, nextTag);
  }

  function changeGraphLinkKind(linkKind: GraphLinkKindFilter) {
    setGraphLinkKind(linkKind);
    persistGraphLinkKind(activeLibrary, linkKind);
  }

  function changeGraphResolution(resolution: GraphResolutionFilter) {
    setGraphResolution(resolution);
    persistGraphResolution(activeLibrary, resolution);
  }

  function viewSelectedVersion(versionId: string) {
    setCurrentDiffOpen(false);
    setSelectedVersionId(versionId);
    setCompareVersionId(null);
  }

  function diffCurrentEditor() {
    setSelectedVersionId(null);
    setCompareVersionId(null);
    setCurrentDiffOpen(true);
  }

  function mutateGraphState() {
    if (!activeLibrary) return Promise.resolve();
    return mutate((key) => isGraphCacheKey(key, activeLibrary));
  }

  function toggleLeftPane() {
    const panel = leftPanelRef.current;
    if (!panel) return;
    if (panel.isCollapsed()) panel.expand();
    else panel.collapse();
  }

  function toggleRightPane() {
    const panel = rightPanelRef.current;
    if (!panel) return;
    if (panel.isCollapsed()) panel.expand();
    else panel.collapse();
  }

  function closePalette() {
    setPaletteOpen(false);
    setPaletteQuery('');
  }

  function closeSaveConflictDialog() {
    setConflictRemote(null);
    setConflictDetails(null);
  }

  function transitionSaveState(next: SaveState) {
    saveStateRef.current = next;
    setSaveState(next);
  }

  return (
    <main
      className="isolate flex h-screen min-h-0 flex-col overflow-hidden bg-canvas text-ink antialiased"
      data-theme={theme}
    >
      <h1 className="sr-only">Quarry</h1>

      <PanelGroup
        aria-label="Workspace layout"
        autoSaveId={layoutStorageKey}
        className="min-h-0 flex-1"
        data-layout-storage-key={layoutStorageKey}
        direction="horizontal"
      >
        <Panel
          className={cn(!resizingPanels && 'transition-[flex] duration-200 ease-out')}
          collapsedSize={3}
          collapsible
          defaultSize={22}
          minSize={16}
          onCollapse={() => setLeftCollapsed(true)}
          onExpand={() => setLeftCollapsed(false)}
          ref={leftPanelRef}
        >
          <LeftPane
            active={activeLibraryRecord}
            collapsed={leftCollapsed}
            libraries={libraries}
            onCreate={() => void createNewDocument()}
            onCreateChild={(node) => void createNewDocument(defaultChildDocumentPath(node))}
            onLibraryChange={changeActiveLibrary}
            onMove={moveDroppedTreeDocuments}
            onOpen={openDocument}
            onOpenContextMenu={openTreeContextMenu}
            onRename={moveDocumentPath}
            onSearchChange={setSearchQuery}
            onToggleCollapsed={toggleLeftPane}
            searchQuery={searchQuery}
            searchResults={search.results}
            selectedPath={selectedPath}
            tree={tree}
            treeKey={activeLibrary}
            treeOpenState={treeOpenState}
            onTreeToggle={changeTreeOpenState}
          />
        </Panel>
        <PanelResizeHandle className="w-px bg-line" onDragging={setResizingPanels} />
        <Panel defaultSize={54} minSize={35}>
          {selectedPath ? (
            <div className="flex h-full min-h-0 flex-col">
              <DocumentToolbar
                disabled={saveState === 'saving'}
                isText={isTextContentType(selectedContentType)}
                path={selectedPath}
                saveState={saveState}
                onDelete={deleteCurrent}
                onDownload={downloadCurrentMarkdown}
                onRename={renameCurrent}
                onSave={save}
              />
              <DocumentBody
                activeLibrary={activeLibrary}
                byteSize={selectedEntry?.byte_size}
                contentHash={selectedEntry?.content_hash}
                content={content}
                contentType={selectedContentType}
                disabled={saveState === 'saving'}
                path={selectedPath}
                saveState={saveState}
                onChange={changeContent}
              />
            </div>
          ) : (
            <EmptyDocument />
          )}
        </Panel>
        <PanelResizeHandle className="w-px bg-line" onDragging={setResizingPanels} />
        <Panel
          className={cn(!resizingPanels && 'transition-[flex] duration-200 ease-out')}
          collapsedSize={3}
          collapsible
          defaultSize={24}
          minSize={18}
          onCollapse={() => setRightCollapsed(true)}
          onExpand={() => setRightCollapsed(false)}
          ref={rightPanelRef}
        >
          <RightPane
            activeTab={rightPaneTab}
            activeLibrary={activeLibrary}
            collapsed={rightCollapsed}
            onToggleCollapsed={toggleRightPane}
            conflicts={conflicts}
            currentDiffOpen={currentDiffOpen}
            currentEditorDiff={currentEditorDiff}
            compareVersionId={compareVersionId}
            graphDepth={graphDepth}
            graphFolder={graphFolder}
            graphTag={graphTag}
            graphLinkKind={graphLinkKind}
            graphResolution={graphResolution}
            graphScope={graphScope}
            graphData={graphData}
            incoming={incoming.links}
            onCompareVersionChange={setCompareVersionId}
            onCreateDocumentFromLink={createDocumentFromLink}
            onDiffCurrent={diffCurrentEditor}
            onOpenDocument={openDocument}
            onOpenConflict={setMergeConflictId}
            onResolveConflict={resolveOpenConflict}
            onRestoreVersion={restoreSelectedVersion}
            onGraphDepthChange={changeGraphDepth}
            onGraphFolderChange={changeGraphFolder}
            onGraphTagChange={changeGraphTag}
            onGraphLinkKindChange={changeGraphLinkKind}
            onGraphResolutionChange={changeGraphResolution}
            onGraphScopeChange={changeGraphScope}
            onViewVersion={viewSelectedVersion}
            outgoing={outgoing.links}
            selectedContentType={selectedContentType}
            selectedEntry={selectedEntry}
            selectedVersionContent={selectedVersionContent}
            selectedVersionDiff={selectedVersionDiff}
            selectedVersionId={selectedVersionId}
            onTabChange={changeRightPaneTab}
            versions={versionList}
          />
        </Panel>
      </PanelGroup>

      {conflictRemote ? (
        <Dialog open>
          <div className="fixed inset-0 z-40 bg-black/20" />
          <div
            aria-label="Save conflict"
            aria-modal="true"
            className="fixed left-1/2 top-1/2 z-50 grid w-[min(900px,92vw)] -translate-x-1/2 -translate-y-1/2 grid-cols-2 gap-3 rounded-md border border-line-strong bg-surface p-4 shadow-xl"
            ref={saveConflictDialogRef}
            role="dialog"
            tabIndex={-1}
          >
            {conflictDetails ? (
              <dl className="col-span-2 grid gap-2 rounded-md border border-line bg-raised px-3 py-2 text-xs text-body sm:grid-cols-3">
                <div className="min-w-0 truncate">
                  <dt className="inline font-semibold uppercase text-muted">Path</dt>{' '}
                  <dd className="inline font-mono">{conflictDetails.path}</dd>
                </div>
                <div className="min-w-0 truncate">
                  <dt className="inline font-semibold uppercase text-muted">Base</dt>{' '}
                  <dd className="inline font-mono">{conflictDetails.baseEtag}</dd>
                </div>
                <div className="min-w-0 truncate">
                  <dt className="inline font-semibold uppercase text-muted">Latest</dt>{' '}
                  <dd className="inline font-mono">{conflictDetails.remoteEtag}</dd>
                </div>
              </dl>
            ) : null}
            <div>
              <h2 className="mb-2 text-sm font-semibold">Local draft</h2>
              <pre className="max-h-[50vh] overflow-auto rounded border border-line bg-raised p-3 text-xs">
                {content}
              </pre>
            </div>
            <div>
              <h2 className="mb-2 text-sm font-semibold">Latest remote</h2>
              <pre className="max-h-[50vh] overflow-auto rounded border border-line bg-raised p-3 text-xs">
                {conflictRemote}
              </pre>
            </div>
            <div className="col-span-2 flex justify-end gap-2">
              <button className={secondaryButton} onClick={() => setContent(conflictRemote)}>
                Use remote
              </button>
              <button
                className={primaryButton}
                onClick={closeSaveConflictDialog}
              >
                Keep editing local draft
              </button>
            </div>
          </div>
        </Dialog>
      ) : null}

      <CommandPalette
        documents={documents}
        open={paletteOpen}
        query={paletteQuery}
        selectedPath={selectedPath}
        onClose={closePalette}
        onCreate={() => void createNewDocument()}
        onDelete={deleteCurrent}
        onDownload={downloadCurrentMarkdown}
        onOpenGit={() => setGitOpen(true)}
        onOpenSettings={() => setSettingsOpen(true)}
        onMove={renameCurrent}
        onOpenDocument={openDocument}
        onQueryChange={setPaletteQuery}
        onSearch={setSearchQuery}
        onToggleTheme={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
        theme={theme}
      />

      <SettingsDialog
        activeLibrary={activeLibrary}
        layoutStorageKey={layoutStorageKey}
        open={settingsOpen}
        theme={theme}
        onClose={() => setSettingsOpen(false)}
        onResetLayout={() => localStorage.removeItem(layoutStorageKey)}
        onThemeChange={setTheme}
      />

      <GitPanel
        activeLibrary={activeLibrary}
        open={gitOpen}
        peers={gitPeers}
        onClose={() => setGitOpen(false)}
        onSyncResult={setLastSyncResult}
      />

      <TreeContextMenu
        menu={treeMenu}
        onClose={closeTreeContextMenu}
        onCopyPath={copyTreePath}
        onCreateDocument={createTreeDocument}
        onDeleteDocument={deleteTreeDocument}
        onMoveDocument={moveTreeDocument}
        onRevealDocument={revealTreeDocument}
      />

      {mergeConflict ? (
        <ConflictMergeDialog
          activeLibrary={activeLibrary}
          conflict={mergeConflict}
          onClose={() => setMergeConflictId(null)}
        />
      ) : null}
    </main>
  );
}

function DocumentBody({
  activeLibrary,
  byteSize,
  contentHash,
  content,
  contentType,
  disabled,
  path,
  saveState,
  onChange,
}: {
  activeLibrary: string;
  byteSize?: number;
  contentHash?: string | null;
  content: string;
  contentType: string;
  disabled: boolean;
  path: string;
  saveState: SaveState;
  onChange: (content: string) => void;
}) {
  if (isTextContentType(contentType)) {
    return (
      <MarkdownEditor
        content={content}
        disabled={disabled}
        status={statusText(saveState)}
        onChange={onChange}
      />
    );
  }

  if (isImageContentType(contentType)) {
    return (
      <ImagePreview
        byteSize={byteSize}
        contentType={contentType}
        href={documentHref(activeLibrary, path)}
        path={path}
      />
    );
  }

  return (
    <BinaryPreview
      byteSize={byteSize}
      contentHash={contentHash}
      contentType={contentType}
      href={documentHref(activeLibrary, path)}
      path={path}
    />
  );
}

function ImagePreview({
  byteSize,
  contentType,
  href,
  path,
}: {
  byteSize?: number;
  contentType: string;
  href: string;
  path: string;
}) {
  return (
    <section aria-label="Image preview" className="flex min-h-0 flex-1 flex-col bg-surface">
      <div className="flex h-11 shrink-0 items-center gap-3 border-b border-line px-3 text-sm text-body">
        <ImageIcon size={15} className="shrink-0 text-accent" />
        <span className="min-w-0 flex-1 truncate">{path}</span>
        <span className="shrink-0 text-xs text-muted">{contentType}</span>
        {typeof byteSize === 'number' ? (
          <span className="shrink-0 text-xs tabular-nums text-muted">{formatBytes(byteSize)}</span>
        ) : null}
      </div>
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto p-6">
        <img
          alt={`${path} preview`}
          className="max-h-full max-w-full rounded-sm object-contain outline-1 -outline-offset-1 outline-black/10"
          src={href}
        />
      </div>
    </section>
  );
}

function BinaryPreview({
  byteSize,
  contentHash,
  contentType,
  href,
  path,
}: {
  byteSize?: number;
  contentHash?: string | null;
  contentType: string;
  href: string;
  path: string;
}) {
  return (
    <section
      aria-label="Binary document preview"
      className="flex min-h-0 flex-1 items-center justify-center bg-surface p-6"
    >
      <div className="w-full max-w-xl rounded-md border border-line bg-raised p-5">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-accent-tint text-accent">
            <FileArchive size={20} />
          </div>
          <div className="min-w-0 flex-1">
            <h2 className="truncate text-sm font-semibold text-ink">{path}</h2>
            <p className="mt-1 text-sm text-muted">This binary document is available for download.</p>
          </div>
          <a className={secondaryButton} download={path.split('/').at(-1)} href={href}>
            <Download size={15} />
            Download
          </a>
        </div>

        <dl className="mt-5 grid grid-cols-[120px_1fr] gap-x-3 gap-y-2 text-sm">
          <dt className="text-muted">Path</dt>
          <dd className="min-w-0 truncate font-mono text-body">{path}</dd>
          <dt className="text-muted">Content type</dt>
          <dd className="min-w-0 truncate font-mono text-body">{contentType}</dd>
          <dt className="text-muted">Size</dt>
          <dd className="tabular-nums text-body">
            {typeof byteSize === 'number' ? formatBytes(byteSize) : 'Unknown'}
          </dd>
          {contentHash ? (
            <>
              <dt className="text-muted">Hash</dt>
              <dd className="min-w-0 truncate font-mono text-body">{contentHash}</dd>
            </>
          ) : null}
        </dl>
      </div>
    </section>
  );
}

function CommandPalette({
  documents,
  open,
  query,
  selectedPath,
  theme,
  onClose,
  onCreate,
  onDelete,
  onDownload,
  onOpenGit,
  onOpenSettings,
  onMove,
  onOpenDocument,
  onQueryChange,
  onSearch,
  onToggleTheme,
}: {
  documents: DocumentListEntry[];
  open: boolean;
  query: string;
  selectedPath: string;
  theme: ThemePreference;
  onClose: () => void;
  onCreate: () => void;
  onDelete: () => void;
  onDownload: () => void;
  onOpenGit: () => void;
  onOpenSettings: () => void;
  onMove: () => void;
  onOpenDocument: (path: string) => void;
  onQueryChange: (query: string) => void;
  onSearch: (query: string) => void;
  onToggleTheme: () => void;
}) {
  const dialogRef = useDialogFocusTrap(open, onClose);

  if (!open) return null;

  function run(action: () => void) {
    onClose();
    action();
  }

  const trimmedQuery = query.trim();
  const quickOpenDocuments = documents.slice(0, 25);

  return (
    <div className="fixed inset-0 z-50 bg-black/20 p-4" onMouseDown={onClose}>
      <div
        aria-label="Command palette"
        aria-modal="true"
        className="mx-auto mt-[10vh] w-full max-w-2xl overflow-hidden rounded-md border border-line-strong bg-surface shadow-xl"
        onMouseDown={(event) => event.stopPropagation()}
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <Command
          className="flex max-h-[70vh] flex-col"
          label="Command palette"
        >
          <div className="flex h-12 items-center gap-2 border-b border-line px-3">
            <Search size={16} className="shrink-0 text-muted" />
            <Command.Input
              aria-label="Command palette"
              autoFocus
              className="min-w-0 flex-1 border-0 bg-transparent text-sm outline-none"
              placeholder="Open, search, or run a command"
              value={query}
              onValueChange={onQueryChange}
            />
            <kbd className="rounded border border-line px-1.5 py-0.5 text-xs text-muted">Esc</kbd>
          </div>
          {trimmedQuery ? (
            <div className="border-b border-line p-2">
              <button
                className={`${commandItem} w-full text-left hover:bg-accent-tint focus:bg-accent-tint`}
                type="button"
                onClick={() => run(() => onSearch(trimmedQuery))}
              >
                <span className="min-w-0 flex-1 truncate">Search server for "{trimmedQuery}"</span>
              </button>
            </div>
          ) : null}
          <Command.List className="min-h-0 overflow-auto p-2">
            <Command.Empty className="px-2 py-6 text-center text-sm text-muted">
              No matching commands
            </Command.Empty>
            <Command.Group
              className="pb-2 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:text-muted"
              heading="Documents"
            >
              {quickOpenDocuments.map((entry) => (
                <Command.Item
                  className={commandItem}
                  key={entry.id}
                  value={`open ${documentTitle(entry)} ${entry.path}`}
                  onSelect={() => run(() => onOpenDocument(entry.path))}
                >
                  <span className="min-w-0 flex-1 truncate">Open {documentTitle(entry)}</span>
                  <span className="max-w-[45%] shrink-0 truncate font-mono text-xs text-muted">
                    {entry.path}
                  </span>
                </Command.Item>
              ))}
            </Command.Group>
            <Command.Group
              className="border-t border-line pt-2 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:text-muted"
              heading="Actions"
            >
              <Command.Item className={commandItem} value="new create document" onSelect={() => run(onCreate)}>
                <span className="min-w-0 flex-1 truncate">Create document</span>
              </Command.Item>
              <Command.Item
                className={commandItem}
                disabled={!selectedPath}
                value="download export markdown current document"
                onSelect={() => run(onDownload)}
              >
                <span className="min-w-0 flex-1 truncate">Download as Markdown</span>
                {selectedPath ? <span className="shrink-0 truncate text-xs text-muted">{selectedPath}</span> : null}
              </Command.Item>
              <Command.Item
                className={commandItem}
                disabled={!selectedPath}
                value="rename move current document"
                onSelect={() => run(onMove)}
              >
                <span className="min-w-0 flex-1 truncate">Move current document</span>
                {selectedPath ? <span className="shrink-0 truncate text-xs text-muted">{selectedPath}</span> : null}
              </Command.Item>
              <Command.Item
                className={commandItem}
                disabled={!selectedPath}
                value="delete remove current document"
                onSelect={() => run(onDelete)}
              >
                <span className="min-w-0 flex-1 truncate">Delete current document</span>
              </Command.Item>
              <Command.Item className={commandItem} value="sync git pull push peers" onSelect={() => run(onOpenGit)}>
                <span className="min-w-0 flex-1 truncate">Sync with Git peer</span>
              </Command.Item>
              <Command.Item
                className={commandItem}
                value="theme dark light mode appearance toggle"
                onSelect={() => run(onToggleTheme)}
              >
                <span className="min-w-0 flex-1 truncate">
                  {theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
                </span>
              </Command.Item>
              <Command.Item
                className={commandItem}
                value="settings preferences library"
                onSelect={() => run(onOpenSettings)}
              >
                <span className="min-w-0 flex-1 truncate">Open settings</span>
              </Command.Item>
            </Command.Group>
          </Command.List>
        </Command>
      </div>
    </div>
  );
}

function SettingsDialog({
  activeLibrary,
  layoutStorageKey,
  open,
  theme,
  onClose,
  onResetLayout,
  onThemeChange,
}: {
  activeLibrary: string;
  layoutStorageKey: string;
  open: boolean;
  theme: ThemePreference;
  onClose: () => void;
  onResetLayout: () => void;
  onThemeChange: (theme: ThemePreference) => void;
}) {
  const dialogRef = useDialogFocusTrap(open, onClose);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 bg-black/20 p-4" onMouseDown={onClose}>
      <div
        aria-label="Workspace settings"
        aria-modal="true"
        className="mx-auto mt-[12vh] w-full max-w-xl overflow-hidden rounded-md border border-line-strong bg-surface shadow-xl"
        onMouseDown={(event) => event.stopPropagation()}
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex h-12 items-center gap-2 border-b border-line px-4">
          <SettingsIcon size={16} className="text-accent" />
          <h2 className="min-w-0 flex-1 truncate text-sm font-semibold text-ink">
            Workspace settings
          </h2>
          <button className={secondaryButton} onClick={onClose} type="button">
            Close settings
          </button>
        </div>

        <div className="space-y-5 p-4">
          <section>
            <h3 className="text-xs font-semibold uppercase text-muted">Library</h3>
            <dl className="mt-2 grid grid-cols-[120px_1fr] gap-x-3 gap-y-2 text-sm">
              <dt className="text-muted">Active library</dt>
              <dd className="min-w-0 truncate font-mono text-body">
                {activeLibrary || 'No library selected'}
              </dd>
            </dl>
          </section>

          <section>
            <h3 className="text-xs font-semibold uppercase text-muted">Theme</h3>
            <div className="mt-2 flex flex-wrap gap-2">
              <button
                className={theme === 'light' ? primaryButton : secondaryButton}
                disabled={theme === 'light'}
                onClick={() => onThemeChange('light')}
                type="button"
              >
                <Sun size={15} />
                Use light theme
              </button>
              <button
                className={theme === 'dark' ? primaryButton : secondaryButton}
                disabled={theme === 'dark'}
                onClick={() => onThemeChange('dark')}
                type="button"
              >
                <Moon size={15} />
                Use dark theme
              </button>
            </div>
          </section>

          <section>
            <h3 className="text-xs font-semibold uppercase text-muted">Layout</h3>
            <dl className="mt-2 grid grid-cols-[120px_1fr] gap-x-3 gap-y-2 text-sm">
              <dt className="text-muted">Storage key</dt>
              <dd className="min-w-0 truncate font-mono text-body">{layoutStorageKey}</dd>
            </dl>
            <button className={`${secondaryButton} mt-3`} onClick={onResetLayout} type="button">
              <RotateCcw size={15} />
              Reset workspace layout
            </button>
          </section>
        </div>
      </div>
    </div>
  );
}

function ConflictMergeDialog({
  activeLibrary,
  conflict,
  onClose,
}: {
  activeLibrary: string;
  conflict: ConflictRecord;
  onClose: () => void;
}) {
  const { mutate } = useSWRConfig();
  const [manualContent, setManualContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const dialogRef = useDialogFocusTrap(true, onClose);
  const theirsPath = conflict.conflict_path ?? conflict.path;
  const { data: head } = useSWR(
    activeLibrary ? ['/v1/conflict-head', activeLibrary, conflict.path, conflict.id] : null,
    () => getDocument(activeLibrary, conflict.path)
  );
  const { data: ours } = useSWR(
    activeLibrary && conflict.ours_version_id
      ? ['/v1/conflict-version', activeLibrary, conflict.path, conflict.ours_version_id]
      : null,
    () => documentVersion(activeLibrary, conflict.path, conflict.ours_version_id!)
  );
  const { data: theirs } = useSWR(
    activeLibrary && conflict.theirs_version_id
      ? ['/v1/conflict-version', activeLibrary, theirsPath, conflict.theirs_version_id]
      : null,
    () => documentVersion(activeLibrary, theirsPath, conflict.theirs_version_id!)
  );

  useEffect(() => {
    setManualContent(ours?.content ?? head?.content ?? '');
  }, [head?.content, ours?.content, conflict.id]);

  async function refreshConflictState() {
    await Promise.all([
      mutate(['/v1/document', activeLibrary, conflict.path]),
      mutate(['/v1/documents', activeLibrary]),
      mutate(['/v1/conflicts', activeLibrary]),
      mutate(['/v1/versions', activeLibrary, conflict.path]),
      mutate(['/v1/outgoing', activeLibrary, conflict.path]),
      mutate(['/v1/backlinks', activeLibrary, conflict.path]),
      mutate((key) => isGraphCacheKey(key, activeLibrary)),
    ]);
  }

  async function resolveWith(content: string) {
    if (!head?.etag) return;
    setBusy(true);
    setError('');
    try {
      await putDocument(activeLibrary, conflict.path, content, head.etag, head.contentType);
      await resolveConflict(activeLibrary, conflict.id);
      await refreshConflictState();
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function resolveWithDelete() {
    setBusy(true);
    setError('');
    try {
      await deleteDocument(activeLibrary, conflict.path);
      await resolveConflict(activeLibrary, conflict.id);
      await refreshConflictState();
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 bg-black/20 p-4" onMouseDown={onClose}>
      <div
        aria-label="Resolve conflict"
        aria-modal="true"
        className="mx-auto mt-[5vh] flex max-h-[88vh] w-full max-w-5xl flex-col overflow-hidden rounded-md border border-line-strong bg-surface shadow-xl"
        onMouseDown={(event) => event.stopPropagation()}
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-4">
          <AlertTriangle size={16} className="text-warn-ink" />
          <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Resolve {conflict.path}</h2>
          <button className={secondaryButton} onClick={onClose} type="button">
            Close
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-4">
          {error ? (
            <div className="mb-3 rounded-md border border-warn-line bg-warn-tint px-3 py-2 text-sm text-warn-ink">
              {error}
            </div>
          ) : null}
          <div className="grid gap-3 md:grid-cols-2">
            <ConflictVersionPanel label="Ours" content={ours?.content} version={conflict.ours_version_id} />
            <ConflictVersionPanel label="Theirs" content={theirs?.content} version={conflict.theirs_version_id} />
          </div>
          <label className="mt-4 block text-xs font-semibold uppercase text-muted">
            Manual resolution
            <textarea
              aria-label="Manual resolution"
              className="mt-2 min-h-40 w-full resize-y rounded-md border border-line bg-raised p-3 font-mono text-[14px] leading-6 text-ink outline-none focus:border-accent"
              value={manualContent}
              onChange={(event) => setManualContent(event.target.value)}
            />
          </label>
        </div>
        <div className="flex shrink-0 items-center justify-end gap-2 border-t border-line bg-surface px-4 py-3">
          <button
            className={secondaryButton}
            disabled={busy || !ours}
            onClick={() => void resolveWith(ours?.content ?? '')}
            type="button"
          >
            Use ours
          </button>
          <button
            className={secondaryButton}
            disabled={busy || !theirs}
            onClick={() => void resolveWith(theirs?.content ?? '')}
            type="button"
          >
            Use theirs
          </button>
          <button
            className={secondaryButton}
            disabled={busy}
            onClick={() => void resolveWithDelete()}
            type="button"
          >
            Delete document
          </button>
          <button
            className={primaryButton}
            disabled={busy || !head}
            onClick={() => void resolveWith(manualContent)}
            type="button"
          >
            Save manual
          </button>
        </div>
      </div>
    </div>
  );
}

function ConflictVersionPanel({
  content,
  label,
  version,
}: {
  content?: string;
  label: string;
  version: string | null;
}) {
  return (
    <section className="min-w-0 rounded-md border border-line bg-raised">
      <div className="border-b border-line px-3 py-2">
        <h3 className="text-xs font-semibold uppercase text-muted">{label}</h3>
        <div className="mt-1 truncate font-mono text-xs text-muted">{version ?? 'No version'}</div>
      </div>
      <pre className="max-h-64 min-h-32 overflow-auto p-3 text-sm text-body">
        {content ?? 'Loading...'}
      </pre>
    </section>
  );
}

function GitPanel({
  activeLibrary,
  open,
  peers,
  onClose,
  onSyncResult,
}: {
  activeLibrary: string;
  open: boolean;
  peers: GitPeer[];
  onClose: () => void;
  onSyncResult: (result: string) => void;
}) {
  const { mutate } = useSWRConfig();
  const [peerRepo, setPeerRepo] = useState('');
  const [peerBranch, setPeerBranch] = useState('main');
  const [peerRemote, setPeerRemote] = useState('');
  const [importRepo, setImportRepo] = useState('');
  const [exportRepo, setExportRepo] = useState('');
  const [exportBranch, setExportBranch] = useState('main');
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState('');
  const [error, setError] = useState('');
  const dialogRef = useDialogFocusTrap(open, onClose);

  if (!open) return null;

  async function refreshAfterGit() {
    if (!activeLibrary) return;
    await Promise.all([
      mutate(['/v1/documents', activeLibrary]),
      mutate(['/v1/conflicts', activeLibrary]),
      mutate(['/v1/git-peers', activeLibrary]),
    ]);
  }

  async function runGit<T>(
    label: string,
    operation: () => Promise<T>,
    summarize: (result: T) => string
  ) {
    if (!activeLibrary) return;
    setBusy(label);
    setResult('');
    setError('');
    try {
      const outcome = await operation();
      const summary = summarize(outcome);
      setResult(summary);
      if (isSyncOperationLabel(label)) onSyncResult(summary);
      await refreshAfterGit();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      if (isSyncOperationLabel(label)) onSyncResult(`Failed ${label}: ${message}`);
    } finally {
      setBusy(null);
    }
  }

  async function createPeer(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const repo = peerRepo.trim();
    if (!repo) return;
    await runGit(
      'create peer',
      () =>
        createGitPeer(activeLibrary, {
          repo,
          branch: peerBranch.trim() || 'main',
          remote: peerRemote.trim() || undefined,
        }),
      (peer) => `Created peer ${peer.id}`
    );
    setPeerRepo('');
    setPeerRemote('');
  }

  async function importWorktree(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const repo = importRepo.trim();
    if (!repo) return;
    await runGit('import', () => gitImport(activeLibrary, repo), gitImportSummary);
  }

  async function exportWorktree(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const repo = exportRepo.trim();
    if (!repo) return;
    await runGit(
      'export',
      () => gitExport(activeLibrary, { repo, branch: exportBranch.trim() || 'main' }),
      gitExportSummary
    );
  }

  return (
    <div className="fixed inset-0 z-50 bg-black/20 p-4" onMouseDown={onClose}>
      <div
        aria-label="Git operations"
        aria-modal="true"
        className="mx-auto mt-[6vh] flex max-h-[84vh] w-full max-w-4xl flex-col overflow-hidden rounded-md border border-line-strong bg-surface shadow-xl"
        onMouseDown={(event) => event.stopPropagation()}
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-4">
          <GitBranch size={16} className="text-accent" />
          <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Git operations</h2>
          <button className={secondaryButton} onClick={onClose} type="button">
            Close
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-4">
          {busy ? (
            <div className="mb-3 rounded-md border border-line bg-raised px-3 py-2 text-sm text-body">
              Running {busy}...
            </div>
          ) : null}
          {result ? (
            <div className="mb-3 rounded-md border border-accent-line bg-accent-tint px-3 py-2 text-sm text-accent-ink">
              {result}
            </div>
          ) : null}
          {error ? (
            <div className="mb-3 rounded-md border border-warn-line bg-warn-tint px-3 py-2 text-sm text-warn-ink">
              {error}
            </div>
          ) : null}

          <section className="border-b border-line pb-4">
            <h3 className="mb-2 text-xs font-semibold uppercase text-muted">Peers</h3>
            {peers.length ? (
              <ul className="space-y-2" role="list">
                {peers.map((peer) => (
                  <li className="rounded-md border border-line bg-raised p-3" key={peer.id}>
                    <div className="flex items-start gap-3">
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-sm font-semibold text-body">{peer.id}</div>
                        <div className="mt-1 truncate font-mono text-xs text-muted">
                          {gitPeerRepo(peer)}
                        </div>
                        <div className="mt-1 text-xs text-muted">
                          Branch {gitPeerBranch(peer)}{gitPeerRemote(peer) ? ` · Remote ${gitPeerRemote(peer)}` : ''}
                        </div>
                      </div>
                      <div className="flex shrink-0 gap-2">
                        <button
                          aria-label={`Pull peer ${peer.id}`}
                          className={secondaryButton}
                          disabled={Boolean(busy)}
                          onClick={() =>
                            void runGit(
                              'pull',
                              () => gitPull(activeLibrary, peer.id),
                              gitSyncSummary
                            )
                          }
                          type="button"
                        >
                          Pull
                        </button>
                        <button
                          aria-label={`Push peer ${peer.id}`}
                          className={secondaryButton}
                          disabled={Boolean(busy)}
                          onClick={() =>
                            void runGit(
                              'push',
                              () => gitPush(activeLibrary, peer.id),
                              gitSyncSummary
                            )
                          }
                          type="button"
                        >
                          Push
                        </button>
                        <button
                          aria-label={`Sync peer ${peer.id}`}
                          className={primaryButton}
                          disabled={Boolean(busy)}
                          onClick={() =>
                            void runGit(
                              'sync',
                              () => gitSync(activeLibrary, peer.id),
                              gitSyncSummary
                            )
                          }
                          type="button"
                        >
                          Sync
                        </button>
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-sm text-muted">No Git peers configured</p>
            )}
          </section>

          <div className="grid gap-4 pt-4 md:grid-cols-3">
            <form className="space-y-2" onSubmit={createPeer}>
              <h3 className="text-xs font-semibold uppercase text-muted">Create peer</h3>
              <GitTextInput label="Repo path" value={peerRepo} onChange={setPeerRepo} />
              <GitTextInput label="Branch" value={peerBranch} onChange={setPeerBranch} />
              <GitTextInput label="Remote" value={peerRemote} onChange={setPeerRemote} />
              <button className={secondaryButton} disabled={Boolean(busy) || !peerRepo.trim()} type="submit">
                Add peer
              </button>
            </form>
            <form className="space-y-2" onSubmit={importWorktree}>
              <h3 className="text-xs font-semibold uppercase text-muted">Import</h3>
              <GitTextInput label="Import repo path" value={importRepo} onChange={setImportRepo} />
              <button className={secondaryButton} disabled={Boolean(busy) || !importRepo.trim()} type="submit">
                Import
              </button>
            </form>
            <form className="space-y-2" onSubmit={exportWorktree}>
              <h3 className="text-xs font-semibold uppercase text-muted">Export</h3>
              <GitTextInput label="Export repo path" value={exportRepo} onChange={setExportRepo} />
              <GitTextInput label="Export branch" value={exportBranch} onChange={setExportBranch} />
              <button className={secondaryButton} disabled={Boolean(busy) || !exportRepo.trim()} type="submit">
                Export
              </button>
            </form>
          </div>
        </div>
      </div>
    </div>
  );
}

function GitTextInput({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="block text-xs font-medium text-muted">
      {label}
      <input
        className="mt-1 h-8 w-full rounded-md border border-line-strong bg-raised px-2 text-sm text-body outline-none focus:border-accent"
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

function DocumentTreeRow({ attrs, children, innerRef, node }: RowRendererProps<TreeNode>) {
  function handleKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    attrs.onKeyDown?.(event);
  }

  return (
    <div
      {...attrs}
      data-tree-kind={node.data.kind}
      data-tree-path={node.data.path}
      ref={innerRef}
      onClick={node.handleClick}
      onFocus={(event) => event.stopPropagation()}
      onKeyDown={handleKeyDown}
    >
      {children}
    </div>
  );
}

function findTreeNodeByPath(nodes: TreeNode[], path: string): TreeNode | null {
  for (const node of nodes) {
    if (node.path === path) return node;
    const childMatch = node.children ? findTreeNodeByPath(node.children, path) : null;
    if (childMatch) return childMatch;
  }
  return null;
}

function LeftPane({
  active,
  collapsed,
  libraries,
  searchQuery,
  searchResults,
  selectedPath,
  tree,
  treeKey,
  treeOpenState,
  onCreate,
  onCreateChild,
  onLibraryChange,
  onMove,
  onOpen,
  onOpenContextMenu,
  onRename,
  onSearchChange,
  onToggleCollapsed,
  onTreeToggle,
}: {
  active?: LibraryType;
  collapsed: boolean;
  libraries: LibraryType[];
  searchQuery: string;
  searchResults: SearchResult[];
  selectedPath: string;
  tree: TreeNode[];
  treeKey: string;
  treeOpenState: TreeOpenState;
  onCreate: () => void;
  onCreateChild: (node: TreeNode) => void;
  onLibraryChange: (slug: string) => void;
  onMove: MoveHandler<TreeNode>;
  onOpen: (path: string) => void;
  onOpenContextMenu: (node: TreeNode, event: ReactKeyboardEvent | ReactMouseEvent) => void;
  onRename: (path: string) => void;
  onSearchChange: (query: string) => void;
  onToggleCollapsed: () => void;
  onTreeToggle: (id: string) => void;
}) {
  if (collapsed) {
    return (
      <aside
        aria-label="Document tree"
        className="flex h-full flex-col items-center border-r border-line bg-surface py-2"
      >
        <button
          aria-label="Expand sidebar"
          className={cn(ghostIconButton, 'size-8')}
          onClick={onToggleCollapsed}
          type="button"
        >
          <PanelLeftOpen size={16} />
        </button>
      </aside>
    );
  }
  const [activeSearchIndex, setActiveSearchIndex] = useState(0);
  const [searchOpen, setSearchOpen] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const activeSearchResult = searchResults[Math.min(activeSearchIndex, searchResults.length - 1)];

  function toggleSearch() {
    setSearchOpen((open) => {
      const next = !open;
      if (!next) onSearchChange('');
      else window.requestAnimationFrame(() => searchInputRef.current?.focus());
      return next;
    });
  }

  function handleLibraryKeyDown(event: ReactKeyboardEvent<HTMLSelectElement>) {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const values = [...(active ? [] : ['']), ...libraries.map((library) => library.slug)];
    const currentIndex = Math.max(0, values.indexOf(active?.slug ?? ''));
    let nextIndex = currentIndex;
    if (event.key === 'ArrowDown') nextIndex = Math.min(currentIndex + 1, values.length - 1);
    if (event.key === 'ArrowUp') nextIndex = Math.max(currentIndex - 1, 0);
    if (event.key === 'Home') nextIndex = 0;
    if (event.key === 'End') nextIndex = values.length - 1;
    const next = values[nextIndex];
    if (next !== (active?.slug ?? '')) onLibraryChange(next);
  }

  useEffect(() => {
    setActiveSearchIndex(0);
  }, [searchResults]);

  function selectSearchResult(index: number) {
    if (!searchResults.length) return;
    setActiveSearchIndex(Math.max(0, Math.min(index, searchResults.length - 1)));
  }

  function handleSearchResultsKeyDown(event: ReactKeyboardEvent) {
    if (!searchResults.length) return;
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      selectSearchResult(activeSearchIndex + 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      selectSearchResult(activeSearchIndex - 1);
    } else if (event.key === 'Home') {
      event.preventDefault();
      selectSearchResult(0);
    } else if (event.key === 'End') {
      event.preventDefault();
      selectSearchResult(searchResults.length - 1);
    } else if (event.key === 'Enter' && activeSearchResult) {
      event.preventDefault();
      onOpen(activeSearchResult.path);
    }
  }

  function handleTreeKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.defaultPrevented) return;

    const row = treeRowFromKeyboardEvent(event);
    const path = row?.dataset.treePath;
    if (!path) return;

    if ((event.key === 'Enter' || event.key === ' ') && row.dataset.treeKind === 'document') {
      event.preventDefault();
      onOpen(path);
    } else if (event.key === 'F2' && row.dataset.treeKind === 'document') {
      event.preventDefault();
      onRename(path);
    } else if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) {
      const node = findTreeNodeByPath(tree, path);
      if (!node) return;
      event.preventDefault();
      onOpenContextMenu(node, event);
    }
  }

  function treeRowFromKeyboardEvent(event: ReactKeyboardEvent<HTMLDivElement>) {
    const target = event.target instanceof HTMLElement ? event.target : null;
    const active = event.currentTarget.ownerDocument.activeElement;
    return (
      target?.closest<HTMLElement>('[data-tree-path]') ??
      (active instanceof HTMLElement ? active.closest<HTMLElement>('[data-tree-path]') : null)
    );
  }

  return (
    <aside aria-label="Document tree" className="flex h-full min-h-0 flex-col border-r border-line bg-surface">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-2.5">
        <select
          aria-label="Library switcher"
          className="h-8 min-w-0 flex-1 rounded-md border border-line-strong bg-raised px-2 text-sm font-medium text-body"
          value={active?.slug ?? ''}
          onChange={(event) => onLibraryChange(event.target.value)}
          onKeyDown={handleLibraryKeyDown}
        >
          {active ? null : <option value="">Select library…</option>}
          {libraries.map((library) => (
            <option key={library.id} value={library.slug}>
              {library.slug}
            </option>
          ))}
        </select>
      </div>
      <div className="flex h-9 shrink-0 items-center justify-between pr-1.5 pl-3">
        <span className="text-[0.6875rem] font-semibold uppercase tracking-wider text-faint">Documents</span>
        <div className="flex items-center gap-0.5">
          <button
            aria-expanded={searchOpen}
            aria-label="Search"
            className={cn(ghostIconButton, 'size-7', searchOpen && 'bg-well text-body')}
            onClick={toggleSearch}
            type="button"
          >
            <Search size={15} />
          </button>
          <button aria-label="Create document" className={cn(ghostIconButton, 'size-7')} onClick={onCreate} type="button">
            <FilePlus2 size={15} />
          </button>
          <button
            aria-label="Collapse sidebar"
            className={cn(ghostIconButton, 'size-7')}
            onClick={onToggleCollapsed}
            type="button"
          >
            <PanelLeftClose size={15} />
          </button>
        </div>
      </div>
      {searchOpen ? (
        <div className="px-2.5 pb-2">
          <label className="flex h-8 items-center gap-2 rounded-md border border-line-strong bg-raised px-2.5 text-sm transition-colors focus-within:border-accent focus-within:ring-2 focus-within:ring-accent-tint">
            <Search className="shrink-0 text-muted" size={15} />
            <input
              aria-label="Search"
              className="min-w-0 flex-1 border-0 bg-transparent text-body outline-none placeholder:text-faint"
              placeholder="Search documents…"
              ref={searchInputRef}
              value={searchQuery}
              onChange={(event) => onSearchChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Escape') toggleSearch();
              }}
            />
          </label>
        </div>
      ) : null}
      {searchResults.length ? (
        <section className="border-b border-line p-2">
          <div
            aria-label="Search results"
            className="space-y-1 outline-none"
            role="listbox"
            tabIndex={0}
            onKeyDown={handleSearchResultsKeyDown}
          >
            {searchResults.map((result) => (
              <button
                aria-selected={result.path === activeSearchResult?.path}
                key={result.path}
                className={cn(
                  'block w-full rounded px-2 py-1.5 text-left text-sm hover:bg-well',
                  result.path === activeSearchResult?.path && 'bg-accent-tint text-accent-ink'
                )}
                role="option"
                type="button"
                onClick={() => onOpen(result.path)}
                onFocus={() => selectSearchResult(searchResults.indexOf(result))}
                onMouseEnter={() => selectSearchResult(searchResults.indexOf(result))}
              >
                <span className="block truncate font-medium">{result.title || result.path}</span>
                <span className="block truncate text-xs text-muted">{result.path}</span>
              </button>
            ))}
          </div>
          {activeSearchResult ? (
            <div
              aria-label="Search result preview"
              className="mt-2 rounded border border-line bg-raised px-2 py-1.5 text-xs text-body"
            >
              <div className="truncate font-mono text-muted">{activeSearchResult.path}</div>
              <p className="mt-1 line-clamp-3">
                {activeSearchResult.snippet ||
                  activeSearchResult.matched_fields.join(', ') ||
                  activeSearchResult.path}
              </p>
            </div>
          ) : null}
        </section>
      ) : null}
      <div className="min-h-0 flex-1" onKeyDown={handleTreeKeyDown}>
        <Tree<TreeNode>
          data={tree}
          height={800}
          indent={16}
          initialOpenState={treeOpenState}
          key={treeKey}
          disableDrag={(node) => node.kind === 'folder'}
          onMove={onMove}
          onActivate={(node) => {
            if (node.data.kind === 'folder') node.toggle();
            else onOpen(node.data.path);
          }}
          onToggle={onTreeToggle}
          openByDefault
          renderRow={DocumentTreeRow}
          rowHeight={28}
          width="100%"
        >
          {({ node, style }) => (
            <div
              className={cn(
                'group/row flex cursor-default items-center gap-1 truncate pr-1 pl-2 text-sm hover:bg-well',
                node.data.path === selectedPath && 'bg-accent-tint text-accent-ink'
              )}
              style={style}
              onContextMenu={(event) => {
                onOpenContextMenu(node.data, event);
              }}
            >
              <span className="shrink-0 text-faint">{node.data.kind === 'folder' ? '▸' : '·'}</span>
              <span className="min-w-0 flex-1 truncate">{node.data.name}</span>
              <button
                aria-label={`Add page in ${node.data.name}`}
                className="hidden shrink-0 rounded p-0.5 text-muted hover:bg-line-strong hover:text-body focus-visible:block group-hover/row:block"
                onClick={(event) => {
                  event.stopPropagation();
                  onCreateChild(node.data);
                }}
                type="button"
              >
                <Plus size={14} />
              </button>
            </div>
          )}
        </Tree>
      </div>
    </aside>
  );
}

function TreeContextMenu({
  menu,
  onClose,
  onCopyPath,
  onCreateDocument,
  onDeleteDocument,
  onMoveDocument,
  onRevealDocument,
}: {
  menu: TreeMenuState | null;
  onClose: () => void;
  onCopyPath: (node: TreeNode) => void;
  onCreateDocument: (node: TreeNode) => void;
  onDeleteDocument: (node: TreeNode) => void;
  onMoveDocument: (node: TreeNode) => void;
  onRevealDocument: (node: TreeNode) => void;
}) {
  if (!menu) return null;
  return (
    <div className="fixed inset-0 z-40" onMouseDown={onClose}>
      <div
        aria-label={`Actions for ${menu.node.path}`}
        className="fixed z-50 min-w-44 overflow-hidden rounded-md border border-line-strong bg-surface py-1 text-sm shadow-lg"
        role="menu"
        style={{ left: menu.x, top: menu.y }}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button className={treeMenuItem} role="menuitem" type="button" onClick={() => onCopyPath(menu.node)}>
          Copy path
        </button>
        <button className={treeMenuItem} role="menuitem" type="button" onClick={() => onCreateDocument(menu.node)}>
          New document here
        </button>
        {menu.node.kind === 'document' ? (
          <>
            <button className={treeMenuItem} role="menuitem" type="button" onClick={() => onRevealDocument(menu.node)}>
              Reveal in graph
            </button>
            <button className={treeMenuItem} role="menuitem" type="button" onClick={() => onMoveDocument(menu.node)}>
              Move
            </button>
            <button className={cn(treeMenuItem, 'text-warn-ink')} role="menuitem" type="button" onClick={() => onDeleteDocument(menu.node)}>
              Delete
            </button>
          </>
        ) : null}
      </div>
    </div>
  );
}

function DocumentToolbar({
  disabled,
  isText,
  path,
  saveState,
  onDelete,
  onDownload,
  onRename,
  onSave,
}: {
  disabled: boolean;
  isText: boolean;
  path: string;
  saveState: SaveState;
  onDelete: () => void;
  onDownload: () => void;
  onRename: () => void;
  onSave: () => void;
}) {
  return (
    <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line bg-surface px-3">
      <h1 className="min-w-0 flex-1 truncate text-sm">
        {documentDirname(path) ? (
          <span className="text-muted">{documentDirname(path)}/</span>
        ) : null}
        <span className="font-semibold text-ink">{documentBasename(path)}</span>
      </h1>
      {saveState === 'stale' ? <AlertTriangle className="shrink-0 text-warn-ink" size={16} /> : null}
      {isText ? (
        <button
          aria-label="Save document"
          className="inline-flex h-8 items-center gap-1.5 rounded-md bg-accent py-2 pr-3 pl-2.5 text-sm font-medium text-on-accent shadow-sm transition-colors hover:bg-accent-strong focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50"
          disabled={disabled}
          onClick={onSave}
          type="button"
        >
          <Save size={16} />
          Save
        </button>
      ) : null}
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button aria-label="Document actions" className={iconButton} type="button">
            <MoreHorizontal size={16} />
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            className="z-50 min-w-40 rounded-md border border-line bg-raised p-1 shadow-lg"
            sideOffset={6}
          >
            {isText ? (
              <DropdownMenu.Item className={menuItem} onSelect={onDownload}>
                <Download className="shrink-0" size={15} />
                Download as Markdown
              </DropdownMenu.Item>
            ) : null}
            <DropdownMenu.Item className={menuItem} onSelect={onRename}>
              Move…
            </DropdownMenu.Item>
            <DropdownMenu.Separator className="my-1 h-px bg-line" />
            <DropdownMenu.Item className={cn(menuItem, 'text-danger')} onSelect={onDelete}>
              <Trash2 className="shrink-0" size={15} />
              Delete
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  );
}

function RightPane({
  activeTab,
  activeLibrary,
  collapsed,
  compareVersionId,
  conflicts,
  currentDiffOpen,
  currentEditorDiff,
  graphData,
  graphDepth,
  graphFolder,
  graphTag,
  graphLinkKind,
  graphResolution,
  graphScope,
  incoming,
  onCompareVersionChange,
  onCreateDocumentFromLink,
  onDiffCurrent,
  onGraphDepthChange,
  onGraphFolderChange,
  onGraphTagChange,
  onGraphLinkKindChange,
  onGraphResolutionChange,
  onGraphScopeChange,
  onOpenDocument,
  onOpenConflict,
  onResolveConflict,
  onRestoreVersion,
  onToggleCollapsed,
  onViewVersion,
  outgoing,
  selectedContentType,
  selectedEntry,
  selectedVersionContent,
  selectedVersionDiff,
  selectedVersionId,
  onTabChange,
  versions,
}: {
  activeTab: RightPaneTab;
  activeLibrary: string;
  collapsed: boolean;
  compareVersionId: string | null;
  conflicts: ConflictRecord[];
  currentDiffOpen: boolean;
  currentEditorDiff: string;
  graphData: GraphResponse;
  graphDepth: GraphDepth;
  graphFolder: string;
  graphTag: string;
  graphLinkKind: GraphLinkKindFilter;
  graphResolution: GraphResolutionFilter;
  graphScope: GraphScope;
  incoming: DocumentLink[];
  onCompareVersionChange: (version: string | null) => void;
  onCreateDocumentFromLink: (link: DocumentLink) => void;
  onDiffCurrent: () => void;
  onGraphDepthChange: (depth: GraphDepth) => void;
  onGraphFolderChange: (folder: string) => void;
  onGraphTagChange: (tag: string) => void;
  onGraphLinkKindChange: (linkKind: GraphLinkKindFilter) => void;
  onGraphResolutionChange: (resolution: GraphResolutionFilter) => void;
  onGraphScopeChange: (scope: GraphScope) => void;
  onOpenDocument: (path: string) => void;
  onOpenConflict: (conflict: string) => void;
  onResolveConflict: (conflict: string) => void;
  onRestoreVersion: (version: string) => void;
  onToggleCollapsed: () => void;
  onViewVersion: (version: string) => void;
  outgoing: DocumentLink[];
  selectedContentType: string;
  selectedEntry?: DocumentListEntry;
  selectedVersionContent?: DocumentVersionContent;
  selectedVersionDiff?: VersionDiff;
  selectedVersionId: string | null;
  onTabChange: (tab: RightPaneTab) => void;
  versions: DocumentVersion[];
}) {
  const selectedTab = rightPaneTabs.some((tab) => tab.key === activeTab) ? activeTab : 'links';
  const selectedTabLabel = rightPaneTabs.find((tab) => tab.key === selectedTab)?.label ?? 'Links';

  if (collapsed) {
    return (
      <aside
        aria-label="Document details"
        className="flex h-full flex-col items-center border-l border-line bg-surface py-2"
      >
        <button
          aria-label="Expand details"
          className={cn(ghostIconButton, 'size-8')}
          onClick={onToggleCollapsed}
          type="button"
        >
          <PanelRightOpen size={16} />
        </button>
      </aside>
    );
  }

  return (
    <aside aria-label="Document details" className="flex h-full min-h-0 flex-col bg-surface">
      <div className="flex h-10 shrink-0 items-center gap-1 border-b border-line bg-panel px-2">
        <button
          aria-label="Collapse details"
          className={cn(ghostIconButton, 'size-7 shrink-0')}
          onClick={onToggleCollapsed}
          type="button"
        >
          <PanelRightClose size={15} />
        </button>
        <div
          aria-label="Right pane sections"
          className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
          role="tablist"
        >
          {rightPaneTabs.map((tab) => (
          <button
            aria-controls={`right-pane-panel-${tab.key}`}
            aria-selected={selectedTab === tab.key}
            className={cn(
              'h-7 shrink-0 rounded-md px-2.5 text-xs font-medium transition-colors hover:text-body focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-tint',
              selectedTab === tab.key ? 'bg-raised text-ink shadow-sm' : 'text-muted'
            )}
            id={`right-pane-tab-${tab.key}`}
            key={tab.key}
            onClick={() => onTabChange(tab.key)}
            role="tab"
            tabIndex={selectedTab === tab.key ? 0 : -1}
            type="button"
          >
            {tab.label}
          </button>
          ))}
        </div>
      </div>
      <section
        aria-labelledby={`right-pane-tab-${selectedTab}`}
        className="min-h-0 flex-1 overflow-auto p-3"
        id={`right-pane-panel-${selectedTab}`}
        role="tabpanel"
      >
        {selectedTab === 'links' ? (
          <>
            <h2 className={rightHeading}>
              <Link2 size={14} />
              {selectedTabLabel}
            </h2>
            <LinkList
              activeLibrary={activeLibrary}
              direction="outgoing"
              links={outgoing}
              onCreateDocument={onCreateDocumentFromLink}
              onOpenDocument={onOpenDocument}
            />
          </>
        ) : null}
        {selectedTab === 'backlinks' ? (
          <>
            <h2 className={rightHeading}>{selectedTabLabel}</h2>
            <LinkList
              activeLibrary={activeLibrary}
              direction="incoming"
              links={incoming}
              onOpenDocument={onOpenDocument}
            />
          </>
        ) : null}
        {selectedTab === 'properties' ? (
          <>
            <h2 className={rightHeading}>{selectedTabLabel}</h2>
            <DocumentProperties contentType={selectedContentType} entry={selectedEntry} />
          </>
        ) : null}
        {selectedTab === 'graph' ? (
          <>
            <h2 className={rightHeading}>
              <Network size={14} />
              {selectedTabLabel}
            </h2>
            <GraphPanel
              depth={graphDepth}
              graphData={graphData}
              folder={graphFolder}
              tag={graphTag}
              linkKind={graphLinkKind}
              resolution={graphResolution}
              scope={graphScope}
              onDepthChange={onGraphDepthChange}
              onFolderChange={onGraphFolderChange}
              onTagChange={onGraphTagChange}
              onLinkKindChange={onGraphLinkKindChange}
              onOpenDocument={onOpenDocument}
              onResolutionChange={onGraphResolutionChange}
              onScopeChange={onGraphScopeChange}
            />
          </>
        ) : null}
        {selectedTab === 'versions' ? (
          <>
            <h2 className={rightHeading}>{selectedTabLabel}</h2>
            <button className={`${secondaryButton} mb-2 w-full justify-center`} onClick={onDiffCurrent} type="button">
              Diff editor against latest
            </button>
            <VersionList
              onRestore={onRestoreVersion}
              onView={onViewVersion}
              selectedVersionId={selectedVersionId}
              versions={versions}
            />
            <VersionDetails
              compareVersionId={compareVersionId}
              content={selectedVersionContent}
              currentDiffOpen={currentDiffOpen}
              currentEditorDiff={currentEditorDiff}
              diff={selectedVersionDiff}
              onCompareVersionChange={onCompareVersionChange}
              selectedVersionId={selectedVersionId}
              versions={versions}
            />
          </>
        ) : null}
        {selectedTab === 'conflicts' ? (
          <>
            <h2 className={rightHeading}>{selectedTabLabel}</h2>
            <ConflictList conflicts={conflicts} onOpen={onOpenConflict} onResolve={onResolveConflict} />
          </>
        ) : null}
      </section>
    </aside>
  );
}

function DocumentProperties({
  contentType,
  entry,
}: {
  contentType: string;
  entry?: DocumentListEntry;
}) {
  if (!entry) return <p className="text-xs text-muted">No document selected</p>;

  const rows = [
    { label: 'Path', value: entry.path },
    { label: 'Type', value: contentType || entry.content_type },
    { label: 'Size', value: formatBytes(entry.byte_size) },
    { label: 'Version', value: entry.head_version_id },
    { label: 'Updated', value: entry.updated_at },
    ...metadataPropertyRows(entry.metadata),
  ].filter((row) => row.value.trim().length > 0);

  return (
    <dl className="space-y-1 text-xs">
      {rows.map((row) => (
        <div className="grid grid-cols-[72px_minmax(0,1fr)] gap-2" key={row.label}>
          <dt className="text-muted">{row.label}</dt>
          <dd className="min-w-0 truncate font-mono text-body" title={row.value}>
            {row.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function ConflictList({
  conflicts,
  onOpen,
  onResolve,
}: {
  conflicts: ConflictRecord[];
  onOpen: (conflict: string) => void;
  onResolve: (conflict: string) => void;
}) {
  if (!conflicts.length) return <p className="text-xs text-muted">None</p>;
  return (
    <ul className="space-y-1 text-xs">
      {conflicts.map((conflict) => (
        <li className="rounded bg-raised px-2 py-1 text-body" key={conflict.id}>
          <div className="flex items-center gap-2">
            <span className="min-w-0 flex-1 truncate">
              {conflict.path} {conflict.status}
            </span>
            <button
              aria-label={`Open conflict ${conflict.id}`}
              className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-line text-body hover:bg-well disabled:opacity-40"
              disabled={conflict.status !== 'open'}
              onClick={() => onOpen(conflict.id)}
              type="button"
            >
              <Eye size={13} />
            </button>
            <button
              aria-label={`Resolve conflict ${conflict.id}`}
              className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-line text-body hover:bg-well disabled:opacity-40"
              disabled={conflict.status !== 'open'}
              onClick={() => onResolve(conflict.id)}
              type="button"
            >
              <CheckCircle2 size={13} />
            </button>
          </div>
          {(conflict.ours_version_id || conflict.theirs_version_id) && (
            <div className="mt-1 truncate text-[10px] uppercase text-muted">
              {conflict.ours_version_id ?? 'ours?'} / {conflict.theirs_version_id ?? 'theirs?'}
            </div>
          )}
          {conflict.conflict_path ? (
            <div className="mt-1 truncate text-[10px] text-muted">
              Sibling {conflict.conflict_path}
            </div>
          ) : null}
          <div className="mt-1 truncate text-[10px] text-muted">
            Discovered {conflict.discovered_at}
          </div>
        </li>
      ))}
    </ul>
  );
}

function GraphPanel({
  depth,
  folder,
  graphData,
  tag,
  linkKind,
  resolution,
  scope,
  onDepthChange,
  onFolderChange,
  onTagChange,
  onLinkKindChange,
  onOpenDocument,
  onResolutionChange,
  onScopeChange,
}: {
  depth: GraphDepth;
  folder: string;
  graphData: GraphResponse;
  tag: string;
  linkKind: GraphLinkKindFilter;
  resolution: GraphResolutionFilter;
  scope: GraphScope;
  onDepthChange: (depth: GraphDepth) => void;
  onFolderChange: (folder: string) => void;
  onTagChange: (tag: string) => void;
  onLinkKindChange: (linkKind: GraphLinkKindFilter) => void;
  onOpenDocument: (path: string) => void;
  onResolutionChange: (resolution: GraphResolutionFilter) => void;
  onScopeChange: (scope: GraphScope) => void;
}) {
  const visibleEdges = graphData.edges.slice(0, 6);
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <div aria-label="Graph scope" className="inline-flex rounded-md border border-line-strong bg-raised p-0.5">
          {graphScopes.map((option) => (
            <button
              aria-pressed={scope === option.key}
              className={cn(
                'h-7 rounded px-2 text-xs font-medium text-muted hover:bg-well focus:outline-none focus:ring-1 focus:ring-accent-ring',
                scope === option.key && 'bg-accent-tint text-accent-ink'
              )}
              key={option.key}
              onClick={() => onScopeChange(option.key)}
              type="button"
            >
              {option.label}
            </button>
          ))}
        </div>
        {scope === 'focused' ? (
          <label className="flex h-8 items-center gap-1 rounded-md border border-line-strong bg-raised px-2 text-xs text-muted">
            Depth
            <select
              aria-label="Graph depth"
              className="bg-transparent font-medium text-body outline-none"
              onChange={(event) => onDepthChange(Number(event.target.value) as GraphDepth)}
              value={depth}
            >
              {graphDepthOptions.map((option) => (
                <option key={option} value={option}>
                  {option} hop{option === 1 ? '' : 's'}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        <label className="flex h-8 min-w-[132px] items-center gap-1 rounded-md border border-line-strong bg-raised px-2 text-xs text-muted">
          Folder
          <input
            aria-label="Graph folder"
            className="min-w-0 flex-1 bg-transparent font-medium text-body outline-none placeholder:text-faint"
            onChange={(event) => onFolderChange(event.target.value)}
            placeholder="path"
            value={folder}
          />
        </label>
        <label className="flex h-8 min-w-[116px] items-center gap-1 rounded-md border border-line-strong bg-raised px-2 text-xs text-muted">
          Tag
          <input
            aria-label="Graph tag"
            className="min-w-0 flex-1 bg-transparent font-medium text-body outline-none placeholder:text-faint"
            onChange={(event) => onTagChange(event.target.value)}
            placeholder="tag"
            value={tag}
          />
        </label>
        <label className="flex h-8 items-center gap-1 rounded-md border border-line-strong bg-raised px-2 text-xs text-muted">
          Kind
          <select
            aria-label="Graph link kind"
            className="bg-transparent font-medium text-body outline-none"
            onChange={(event) => onLinkKindChange(event.target.value as GraphLinkKindFilter)}
            value={linkKind}
          >
            {graphLinkKindOptions.map((option) => (
              <option key={option.key} value={option.key}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <label className="flex h-8 items-center gap-1 rounded-md border border-line-strong bg-raised px-2 text-xs text-muted">
          State
          <select
            aria-label="Graph resolution"
            className="bg-transparent font-medium text-body outline-none"
            onChange={(event) => onResolutionChange(event.target.value as GraphResolutionFilter)}
            value={resolution}
          >
            {graphResolutionOptions.map((option) => (
              <option key={option.key} value={option.key}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
      </div>
      {graphData.nodes.length ? (
        <>
          <GraphCanvas graphData={graphData} onOpenDocument={onOpenDocument} />
          {graphData.truncated ? (
            <p className="rounded border border-warn-line bg-warn-tint px-2 py-1 text-xs text-warn-ink">
              Full graph is too large for the current limit; use focused mode or filters to narrow it.
            </p>
          ) : null}
          {visibleEdges.length ? (
            <ul className="space-y-1 text-xs">
              {visibleEdges.map((edge) => (
                <li
                  className="flex items-center gap-2 rounded bg-raised px-2 py-1 text-body"
                  key={edge.id}
                >
                  <button
                    className="min-w-0 flex-1 truncate text-left hover:text-accent"
                    onClick={() => edge.target_path && onOpenDocument(edge.target_path)}
                    type="button"
                  >
                    {edge.source_path} -&gt; {edge.target_path ?? edge.target_text}
                  </button>
                  <span className="shrink-0 rounded border border-line px-1.5 py-0.5 text-[10px] uppercase text-muted">
                    {linkKindLabel(edge.target_kind)}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-xs text-muted">No document edges</p>
          )}
        </>
      ) : (
        <p className="text-xs text-muted">None</p>
      )}
    </div>
  );
}

function GraphCanvas({
  graphData,
  onOpenDocument,
}: {
  graphData: GraphResponse;
  onOpenDocument: (path: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !graphData.nodes.length) return;
    container.replaceChildren();
    let cancelled = false;
    let renderer: SigmaRenderer | null = null;
    void (async () => {
      try {
        const [{ default: GraphologyGraph }, { default: Sigma }] = await Promise.all([
          import('graphology'),
          import('sigma'),
        ]);
        if (cancelled || !containerRef.current) return;
        const positions = await layoutGraphNodes(graphData.nodes);
        if (cancelled || !containerRef.current) return;
        const positionById = new Map(positions.map((position) => [position.id, position]));
        const graph = new GraphologyGraph();
        for (const node of graphData.nodes) {
          const position = positionById.get(node.id) ?? { x: 0, y: 0 };
          graph.addNode(node.id, {
            label: node.title || node.path,
            path: node.path,
            x: position.x,
            y: position.y,
            size: 7,
            color: '#256f64',
          });
        }
        for (const edge of graphData.edges) {
          if (!edge.target || !graph.hasNode(edge.source) || !graph.hasNode(edge.target)) continue;
          graph.addDirectedEdgeWithKey(edge.id, edge.source, edge.target, {
            size: 2,
            color: edge.resolved ? '#7aa69e' : '#b9804c',
          });
        }

        renderer = new Sigma(graph, containerRef.current, {
          allowInvalidContainer: true,
          renderEdgeLabels: false,
          renderLabels: true,
        }) as SigmaRenderer;
        renderer.on('clickNode', ({ node }) => {
          const path = graph.getNodeAttribute(node, 'path');
          if (typeof path === 'string') onOpenDocument(path);
        });
      } catch {
        container.replaceChildren();
      }
    })();
    return () => {
      cancelled = true;
      renderer?.kill();
    };
  }, [graphData, onOpenDocument]);

  return (
    <div
      aria-label="Graph view"
      className="h-36 overflow-hidden rounded border border-line bg-raised"
      ref={containerRef}
    />
  );
}

function VersionList({
  versions,
  selectedVersionId,
  onView,
  onRestore,
}: {
  versions: DocumentVersion[];
  selectedVersionId: string | null;
  onView: (version: string) => void;
  onRestore: (version: string) => void;
}) {
  if (!versions.length) return <p className="text-xs text-muted">None</p>;
  return (
    <ul className="space-y-1 text-xs">
      {versions.map((version) => {
        const metadataSummary = versionMetadataSummary(version);
        return (
          <li
            className={cn(
              'flex min-h-10 items-start gap-2 rounded bg-raised px-2 py-1 text-body',
              selectedVersionId === version.id && 'outline outline-1 outline-accent-ring'
            )}
            key={version.id}
          >
            <span className="min-w-0 flex-1 space-y-0.5">
              <span className="block truncate">
                <span className="font-mono">{version.id.slice(0, 8)}</span> {version.created_at}
              </span>
              <span className="block truncate text-muted">
                {version.content_type} · {formatBytes(version.byte_size)} · {versionTransactionLabel(version)}
              </span>
              {metadataSummary ? <span className="block truncate text-muted">{metadataSummary}</span> : null}
            </span>
            <button
              aria-label={`View version ${version.id}`}
              className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-line text-body hover:bg-well"
              onClick={() => onView(version.id)}
              type="button"
            >
              <Eye size={13} />
            </button>
            <button
              aria-label={`Restore version ${version.id}`}
              className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-line text-body hover:bg-well"
              onClick={() => onRestore(version.id)}
              type="button"
            >
              <RotateCcw size={13} />
            </button>
          </li>
        );
      })}
    </ul>
  );
}

function versionTransactionLabel(version: DocumentVersion) {
  const source = version.transaction_source ?? formatMetadataValue(version.metadata.source);
  return source ? `Source ${source} · Transaction ${version.tx_id}` : `Transaction ${version.tx_id}`;
}

function versionMetadataSummary(version: DocumentVersion) {
  const transactionRows = [
    { key: 'message', value: version.transaction_message },
    { key: 'actor', value: version.transaction_actor },
    { key: 'provenance', value: version.transaction_provenance },
  ]
    .map(({ key, value }) => ({ key, value: formatMetadataValue(value) }))
    .filter((row): row is { key: string; value: string } => Boolean(row.value));
  if (transactionRows.length) {
    return transactionRows.map((row) => `${metadataLabel(row.key)} ${row.value}`).join(' · ');
  }

  const preferredKeys = ['message', 'summary', 'source', 'actor', 'provenance'];
  const rows = preferredKeys
    .map((key) => ({ key, value: formatMetadataValue(version.metadata[key]) }))
    .filter((row): row is { key: string; value: string } => Boolean(row.value));
  return rows.map((row) => `${metadataLabel(row.key)} ${row.value}`).join(' · ');
}

function VersionDetails({
  compareVersionId,
  selectedVersionId,
  content,
  currentDiffOpen,
  currentEditorDiff,
  diff,
  onCompareVersionChange,
  versions,
}: {
  compareVersionId: string | null;
  selectedVersionId: string | null;
  content?: DocumentVersionContent;
  currentDiffOpen: boolean;
  currentEditorDiff: string;
  diff?: VersionDiff;
  onCompareVersionChange: (version: string | null) => void;
  versions: DocumentVersion[];
}) {
  if (currentDiffOpen) {
    return (
      <div className="mt-3 space-y-2 text-xs">
        <h3 className="font-semibold text-body">Current editor vs latest server</h3>
        <pre className="max-h-52 overflow-auto rounded border border-line bg-surface p-2 text-body">
          {currentEditorDiff}
        </pre>
      </div>
    );
  }
  if (!selectedVersionId) return null;
  return (
    <div className="mt-3 space-y-2 text-xs">
      <h3 className="font-semibold text-body">Version {selectedVersionId.slice(0, 8)}</h3>
      {versions.length > 1 ? (
        <label className="flex items-center gap-2 text-muted">
          <span className="shrink-0">Compare against</span>
          <select
            aria-label="Compare version against"
            className="h-7 min-w-0 flex-1 rounded border border-line bg-raised px-2 text-body"
            onChange={(event) => onCompareVersionChange(event.target.value || null)}
            value={compareVersionId ?? ''}
          >
            <option value="">Latest server</option>
            {versions
              .filter((version) => version.id !== selectedVersionId)
              .map((version) => (
                <option key={version.id} value={version.id}>
                  {version.id.slice(0, 8)} {version.created_at}
                </option>
              ))}
          </select>
        </label>
      ) : null}
      <pre className="max-h-28 overflow-auto rounded border border-line bg-raised p-2 text-body">
        {content?.content ?? 'Loading version...'}
      </pre>
      <pre className="max-h-36 overflow-auto rounded border border-line bg-surface p-2 text-body">
        {diff?.unified_diff ?? 'Loading diff...'}
      </pre>
    </div>
  );
}

function LinkList({
  activeLibrary,
  links,
  direction,
  onCreateDocument,
  onOpenDocument,
}: {
  activeLibrary: string;
  links: DocumentLink[];
  direction: 'incoming' | 'outgoing';
  onCreateDocument?: (link: DocumentLink) => void;
  onOpenDocument: (path: string) => void;
}) {
  const [previewPath, setPreviewPath] = useState<string | null>(null);
  const { data: previewDocument } = useSWR(
    activeLibrary && previewPath ? ['/v1/link-preview', activeLibrary, previewPath] : null,
    () => getDocument(activeLibrary, previewPath!)
  );

  if (!links.length) return <p className="text-xs text-muted">None</p>;
  return (
    <ul className="space-y-1 text-xs">
      {links.map((link) => {
        const destination = direction === 'incoming' ? link.src_path : link.target_path;
        const label = linkLabel(link, direction);
        const visiblePreviewDocument = destination && previewPath === destination ? previewDocument : undefined;
        return (
          <li
            className="rounded bg-raised px-2 py-1 text-body"
            key={`${link.src_doc_id}:${link.start_offset}:${link.end_offset}:${link.target_kind}`}
          >
            <div className="flex min-h-6 items-center gap-2">
              <LinkKindIcon kind={link.target_kind} resolved={link.resolved} />
              {destination ? (
                <button
                  className="min-w-0 flex-1 truncate text-left hover:text-accent"
                  onBlur={() => setPreviewPath(null)}
                  onClick={() => onOpenDocument(destination)}
                  onFocus={() => setPreviewPath(destination)}
                  onMouseEnter={() => setPreviewPath(destination)}
                  onMouseLeave={() => setPreviewPath(null)}
                  type="button"
                >
                  {label}
                </button>
              ) : (
                <span className="min-w-0 flex-1 truncate">{label}</span>
              )}
              {!destination && direction === 'outgoing' && onCreateDocument && canCreateDocumentFromLink(link) ? (
                <button
                  aria-label={`Create document for ${label}`}
                  className="inline-flex h-6 shrink-0 items-center gap-1 rounded border border-line px-1.5 text-[10px] uppercase text-body hover:bg-well"
                  type="button"
                  onClick={() => onCreateDocument(link)}
                >
                  <FilePlus2 aria-hidden size={11} />
                  Create
                </button>
              ) : null}
              <span className="shrink-0 rounded border border-line px-1.5 py-0.5 text-[10px] uppercase text-muted">
                {linkKindLabel(link.target_kind)}
              </span>
              {linkStatus(link) ? (
                <span className="shrink-0 rounded bg-warn-tint px-1.5 py-0.5 text-[10px] uppercase text-warn-ink">
                  {linkStatus(link)}
                </span>
              ) : null}
            </div>
            {visiblePreviewDocument ? (
              <div
                aria-label="Link preview"
                className="mt-1 rounded border border-line bg-surface p-2 text-muted"
                role="tooltip"
              >
                <div className="truncate font-mono text-[10px] uppercase text-muted">
                  {visiblePreviewDocument.path}
                </div>
                <p className="mt-1 line-clamp-3 text-body">
                  {linkPreviewText(visiblePreviewDocument.content)}
                </p>
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

function linkPreviewText(content: string) {
  const text = content
    .split(/\r?\n/)
    .map((line) => line.replace(/^#{1,6}\s+/, '').trim())
    .filter(Boolean)
    .slice(0, 3)
    .join(' ');
  if (!text) return 'Empty document';
  return text.length > 180 ? `${text.slice(0, 177)}...` : text;
}

function LinkKindIcon({ kind, resolved }: { kind: string; resolved: boolean }) {
  if (!resolved && kind !== 'tag') return <Unlink aria-hidden size={13} className="shrink-0 text-warn-ink" />;
  if (kind === 'tag') return <Hash aria-hidden size={13} className="shrink-0 text-accent" />;
  if (kind === 'heading') return <Heading1 aria-hidden size={13} className="shrink-0 text-accent" />;
  return <Link2 aria-hidden size={13} className="shrink-0 text-accent" />;
}

function linkLabel(link: DocumentLink, direction: 'incoming' | 'outgoing') {
  if (direction === 'incoming') return link.src_path;
  if (link.target_kind === 'heading') return `# ${link.target_text}`;
  if (link.target_kind === 'tag') return `#${link.target_text}`;
  const target = link.target_path ?? link.target_text;
  return link.target_anchor ? `${target}#${link.target_anchor}` : target;
}

function canCreateDocumentFromLink(link: DocumentLink) {
  return linkStatus(link) === 'Unresolved' && link.target_kind !== 'heading' && link.target_kind !== 'tag';
}

function defaultDocumentPathForLink(link: DocumentLink) {
  const raw = (link.target_path ?? link.target_text).split('#', 1)[0]?.split('^', 1)[0]?.trim();
  if (!raw) return 'untitled.md';
  return /\.[^/]+$/.test(raw) ? raw : `${raw}.md`;
}

function linkKindLabel(kind: string) {
  switch (kind) {
    case 'embed':
      return 'Embed';
    case 'heading':
      return 'Heading';
    case 'markdown_link':
      return 'Link';
    case 'tag':
      return 'Tag';
    case 'wiki_link':
      return 'Wiki';
    default:
      return kind;
  }
}

function linkStatus(link: DocumentLink) {
  if (link.target_kind === 'tag' || link.target_kind === 'heading') return null;
  if (link.resolution_status === 'ambiguous') return 'Ambiguous';
  return link.resolved ? null : 'Unresolved';
}

function EmptyDocument() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 bg-surface px-6 text-center">
      <div className="flex size-12 items-center justify-center rounded-xl bg-well text-faint">
        <FileText size={22} />
      </div>
      <div>
        <p className="text-sm font-medium text-body">No document open</p>
        <p className="mt-1 text-sm text-muted">
          Select a document from the tree, or press{' '}
          <kbd className="rounded border border-line-strong bg-raised px-1.5 py-0.5 font-mono text-xs text-body">
            ⌘K
          </kbd>{' '}
          to search.
        </p>
      </div>
    </div>
  );
}

function List({ items }: { items: string[] }) {
  if (!items.length) return <p className="text-xs text-muted">None</p>;
  return (
    <ul className="space-y-1 text-xs">
      {items.map((item) => (
        <li className="truncate rounded bg-raised px-2 py-1 text-body" key={item}>
          {item}
        </li>
      ))}
    </ul>
  );
}

function documentTitle(entry: DocumentListEntry) {
  const title = entry.metadata.title;
  return typeof title === 'string' && title.trim() ? title : entry.path.split('/').at(-1) ?? entry.path;
}

function documentDirname(path: string) {
  return path.split('/').slice(0, -1).join('/');
}

function documentBasename(path: string) {
  return path.split('/').at(-1) ?? path;
}

function metadataPropertyRows(metadata: Record<string, unknown>) {
  const hiddenMetadataKeys = new Set(['byte_size', 'content_type', 'head_version_id', 'id', 'path', 'updated_at']);
  return Object.entries(metadata)
    .filter(([key]) => !hiddenMetadataKeys.has(key))
    .map(([key, value]) => ({ label: metadataLabel(key), value: formatMetadataValue(value) }))
    .filter((row): row is { label: string; value: string } => typeof row.value === 'string' && row.value.length > 0);
}

function useDialogFocusTrap(open: boolean, onClose: () => void) {
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);

  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    if (!open) return;
    const dialog = dialogRef.current;
    if (!dialog) return;

    restoreFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusTimer = window.requestAnimationFrame(() => {
      const focusable = dialogFocusableElements(dialog);
      (focusable[0] ?? dialog).focus();
    });

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }

      if (event.key !== 'Tab' || !dialog) return;
      const focusable = dialogFocusableElements(dialog);
      if (!focusable.length) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && (active === first || !(active instanceof Node && dialog.contains(active)))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (active === last || !(active instanceof Node && dialog.contains(active)))) {
        event.preventDefault();
        first.focus();
      }
    }

    document.addEventListener('keydown', handleKeyDown);
    return () => {
      window.cancelAnimationFrame(focusTimer);
      document.removeEventListener('keydown', handleKeyDown);
      const restoreTarget = restoreFocusRef.current;
      restoreFocusRef.current = null;
      if (restoreTarget && document.contains(restoreTarget)) {
        restoreTarget.focus();
      }
    };
  }, [open]);

  return dialogRef;
}

function dialogFocusableElements(root: HTMLElement) {
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      [
        'a[href]',
        'button:not([disabled])',
        'input:not([disabled])',
        'select:not([disabled])',
        'textarea:not([disabled])',
        '[tabindex]:not([tabindex="-1"])',
      ].join(',')
    )
  ).filter((element) => !element.hasAttribute('disabled') && element.getAttribute('aria-hidden') !== 'true');
}

function metadataLabel(key: string) {
  return key
    .replace(/[_-]+/g, ' ')
    .replace(/\w\S*/g, (word) => word.charAt(0).toUpperCase() + word.slice(1));
}

function formatMetadataValue(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string') return value.trim() ? value : null;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) {
    const values = value.map(formatMetadataValue).filter((item): item is string => Boolean(item));
    return values.length ? values.join(', ') : null;
  }
  if (typeof value === 'object') {
    try {
      return JSON.stringify(value);
    } catch {
      return null;
    }
  }
  return null;
}

function unifiedLineDiff(base: string, against: string, baseLabel = 'base', againstLabel = 'against') {
  const baseLines = base.split(/\r?\n/);
  const againstLines = against.split(/\r?\n/);
  const max = Math.max(baseLines.length, againstLines.length);
  const lines = [`--- ${baseLabel}`, `+++ ${againstLabel}`];

  for (let index = 0; index < max; index += 1) {
    const baseLine = baseLines[index];
    const againstLine = againstLines[index];
    if (baseLine === againstLine && baseLine !== undefined) {
      lines.push(` ${baseLine}`);
    } else {
      if (baseLine !== undefined) lines.push(`-${baseLine}`);
      if (againstLine !== undefined) lines.push(`+${againstLine}`);
    }
  }

  return lines.join('\n');
}

function statusText(state: SaveState) {
  const label: Record<SaveState, string> = {
    clean: 'Saved',
    dirty: 'Unsaved changes',
    drafted: 'Draft saved locally',
    saving: 'Saving…',
    saved: 'Saved',
    stale: 'Stale',
    failed: 'Failed',
  };
  return label[state];
}

function hasUnsavedEditorState(state: SaveState) {
  return state === 'dirty' || state === 'drafted' || state === 'stale' || state === 'failed';
}

function orderLibrariesByRecent(libraries: LibraryType[], activeLibrary: string) {
  const recentIndex = new Map<string, number>();
  for (const slug of [activeLibrary, ...loadRecentLibraries()]) {
    if (!slug || recentIndex.has(slug)) continue;
    recentIndex.set(slug, recentIndex.size);
  }

  return [...libraries].sort((left, right) => {
    const leftIndex = recentIndex.get(left.slug) ?? Number.MAX_SAFE_INTEGER;
    const rightIndex = recentIndex.get(right.slug) ?? Number.MAX_SAFE_INTEGER;
    return leftIndex - rightIndex;
  });
}

function loadRecentLibraries() {
  try {
    const parsed = JSON.parse(localStorage.getItem('quarry:recent-libraries') ?? '[]') as unknown;
    if (!Array.isArray(parsed)) return [];
    return Array.from(
      new Set(parsed.filter((slug): slug is string => typeof slug === 'string' && slug.trim().length > 0))
    ).slice(0, RECENT_LIBRARY_LIMIT);
  } catch {
    return [];
  }
}

function persistRecentLibrary(slug: string, knownLibraries: string[]) {
  const known = new Set(knownLibraries);
  const previous = loadRecentLibraries().filter(
    (recent) => recent !== slug && (!known.size || known.has(recent))
  );
  localStorage.setItem(
    'quarry:recent-libraries',
    JSON.stringify([slug, ...previous].slice(0, RECENT_LIBRARY_LIMIT))
  );
}

function loadTreeOpenState(library: string): TreeOpenState {
  if (!library) return {};
  try {
    const parsed = JSON.parse(localStorage.getItem(treeOpenStorageKey(library)) ?? '{}') as unknown;
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, boolean] => typeof entry[1] === 'boolean')
    );
  } catch {
    return {};
  }
}

function persistTreeOpenState(library: string, state: TreeOpenState) {
  if (!library) return;
  localStorage.setItem(treeOpenStorageKey(library), JSON.stringify(state));
}

function treeOpenStorageKey(library: string) {
  return `quarry:tree-open:${library}`;
}

function loadRightPaneTab(library: string): RightPaneTab {
  if (!library) return 'links';
  const stored = localStorage.getItem(rightPaneTabStorageKey(library));
  return isRightPaneTab(stored) ? stored : 'links';
}

function persistRightPaneTab(library: string, tab: RightPaneTab) {
  if (!library) return;
  localStorage.setItem(rightPaneTabStorageKey(library), tab);
}

function rightPaneTabStorageKey(library: string) {
  return `quarry:right-pane-tab:${library}`;
}

function isRightPaneTab(value: unknown): value is RightPaneTab {
  return typeof value === 'string' && rightPaneTabs.some((tab) => tab.key === value);
}

function loadGraphScope(library: string): GraphScope {
  if (!library) return 'focused';
  const stored = localStorage.getItem(graphScopeStorageKey(library));
  return isGraphScope(stored) ? stored : 'focused';
}

function persistGraphScope(library: string, scope: GraphScope) {
  if (!library) return;
  localStorage.setItem(graphScopeStorageKey(library), scope);
}

function graphScopeStorageKey(library: string) {
  return `quarry:graph-scope:${library}`;
}

function isGraphScope(value: unknown): value is GraphScope {
  return typeof value === 'string' && graphScopes.some((scope) => scope.key === value);
}

function loadGraphDepth(library: string): GraphDepth {
  if (!library) return 1;
  const stored = Number(localStorage.getItem(graphDepthStorageKey(library)));
  return isGraphDepth(stored) ? stored : 1;
}

function persistGraphDepth(library: string, depth: GraphDepth) {
  if (!library) return;
  localStorage.setItem(graphDepthStorageKey(library), String(depth));
}

function graphDepthStorageKey(library: string) {
  return `quarry:graph-depth:${library}`;
}

function isGraphDepth(value: unknown): value is GraphDepth {
  return typeof value === 'number' && graphDepthOptions.includes(value as GraphDepth);
}

function loadGraphFolder(library: string) {
  if (!library) return '';
  return normalizeGraphTextFilter(localStorage.getItem(graphFolderStorageKey(library)) ?? '');
}

function persistGraphFolder(library: string, folder: string) {
  if (!library) return;
  localStorage.setItem(graphFolderStorageKey(library), folder);
}

function graphFolderStorageKey(library: string) {
  return `quarry:graph-folder:${library}`;
}

function loadGraphTag(library: string) {
  if (!library) return '';
  return normalizeGraphTextFilter(localStorage.getItem(graphTagStorageKey(library)) ?? '').replace(/^#/, '');
}

function persistGraphTag(library: string, tag: string) {
  if (!library) return;
  localStorage.setItem(graphTagStorageKey(library), tag);
}

function graphTagStorageKey(library: string) {
  return `quarry:graph-tag:${library}`;
}

function normalizeGraphTextFilter(value: string) {
  return value.trim();
}

function loadGraphLinkKind(library: string): GraphLinkKindFilter {
  if (!library) return 'all';
  const stored = localStorage.getItem(graphLinkKindStorageKey(library));
  return isGraphLinkKindFilter(stored) ? stored : 'all';
}

function persistGraphLinkKind(library: string, linkKind: GraphLinkKindFilter) {
  if (!library) return;
  localStorage.setItem(graphLinkKindStorageKey(library), linkKind);
}

function graphLinkKindStorageKey(library: string) {
  return `quarry:graph-link-kind:${library}`;
}

function isGraphLinkKindFilter(value: unknown): value is GraphLinkKindFilter {
  return typeof value === 'string' && graphLinkKindOptions.some((option) => option.key === value);
}

function loadGraphResolution(library: string): GraphResolutionFilter {
  if (!library) return 'all';
  const stored = localStorage.getItem(graphResolutionStorageKey(library));
  return isGraphResolutionFilter(stored) ? stored : 'all';
}

function persistGraphResolution(library: string, resolution: GraphResolutionFilter) {
  if (!library) return;
  localStorage.setItem(graphResolutionStorageKey(library), resolution);
}

function graphResolutionStorageKey(library: string) {
  return `quarry:graph-resolution:${library}`;
}

function isGraphResolutionFilter(value: unknown): value is GraphResolutionFilter {
  return typeof value === 'string' && graphResolutionOptions.some((option) => option.key === value);
}

function graphCacheKey(
  library: string,
  scope: GraphScope,
  path: string,
  depth: GraphDepth,
  folder: string,
  tag: string,
  linkKind: GraphLinkKindFilter,
  resolution: GraphResolutionFilter
) {
  return [
    '/v1/graph',
    library,
    scope === 'focused' ? path : '__full__',
    scope === 'focused' ? depth : 0,
    folder,
    tag,
    linkKind,
    resolution,
  ];
}

function isGraphCacheKey(key: unknown, library: string) {
  return Array.isArray(key) && key[0] === '/v1/graph' && key[1] === library;
}


function eventStatusText(state: EventState) {
  const label: Record<EventState, string> = {
    idle: 'Events idle',
    connecting: 'Events connecting',
    open: 'Live',
    polling: 'Polling',
    error: 'Events unavailable',
  };
  return label[state];
}

function isImageContentType(contentType: string) {
  return contentType.split(';', 1)[0]?.trim().toLowerCase().startsWith('image/') ?? false;
}

function formatBytes(bytes: number) {
  if (bytes === 1) return '1 byte';
  if (bytes < 1024) return `${bytes} bytes`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const formatted = Number.isInteger(value) ? String(value) : value.toFixed(1);
  return `${formatted} ${units[unitIndex]}`;
}

function gitPeerRepo(peer: GitPeer) {
  return gitPeerConfig(peer, 'repo') || 'No repository path';
}

function gitPeerBranch(peer: GitPeer) {
  return gitPeerConfig(peer, 'branch') || 'main';
}

function gitPeerRemote(peer: GitPeer) {
  return gitPeerConfig(peer, 'remote');
}

function gitPeerConfig(peer: GitPeer, key: string) {
  const value = peer.config[key];
  return typeof value === 'string' ? value : '';
}

function gitSyncSummary(result: GitSyncResult) {
  const conflictCount = result.conflict_paths.length || result.conflicts.length;
  return `Imported ${result.imported_paths.length} · Exported ${result.exported_paths.length} · Conflicts ${conflictCount}`;
}

function isSyncOperationLabel(label: string) {
  return label === 'sync' || label === 'pull' || label === 'push' || label === 'import' || label === 'export';
}

function gitSyncEventSummary(payload: BrowserEventPayload) {
  const peer = payload.peer_id?.trim() || 'unknown';
  const applied = typeof payload.applied === 'number' ? payload.applied : 0;
  const conflicts = typeof payload.conflicts === 'number' ? payload.conflicts : 0;
  return `Peer ${peer} · Applied ${applied} · Conflicts ${conflicts}`;
}

function gitImportSummary(result: GitImportResult) {
  return `Imported ${result.imported_paths.length} · Transaction ${result.transaction_id}`;
}

function gitExportSummary(result: GitExportResult) {
  return `Exported ${result.exported_paths.length}${result.commit_id ? ` · Commit ${result.commit_id}` : ''}`;
}

function parseWorkspaceRoute(pathname: string) {
  const segments = pathname.split('/').filter(Boolean);
  if (segments[0] !== 'libraries' || !segments[1]) {
    return { library: null, path: undefined };
  }
  const library = safeDecodeSegment(segments[1]);
  if (segments[2] !== 'documents') {
    return { library, path: '' };
  }
  return {
    library,
    path: segments.slice(3).map(safeDecodeSegment).join('/'),
  };
}

function workspaceRoute(library: string, path: string) {
  if (!library) return '';
  const libraryPath = `/libraries/${encodeURIComponent(library)}`;
  if (!path) return libraryPath;
  return `${libraryPath}/documents/${path.split('/').map(encodeURIComponent).join('/')}`;
}

function safeDecodeSegment(segment: string) {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

function parseBrowserEvent(event: MessageEvent): BrowserEventPayload | null {
  try {
    const payload = JSON.parse(String(event.data)) as BrowserEventPayload;
    return typeof payload.type === 'string' ? payload : null;
  } catch {
    return null;
  }
}

const primaryButton =
  'inline-flex h-8 items-center gap-1.5 rounded-md bg-accent py-2 pr-3 pl-2.5 text-sm font-medium text-on-accent shadow-sm transition-colors hover:bg-accent-strong focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent';
const secondaryButton =
  'inline-flex h-8 items-center gap-1.5 rounded-md border border-line-strong bg-raised px-3 text-sm text-body transition-colors hover:bg-well';
const ghostButton =
  'inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-sm font-medium text-muted transition-colors hover:bg-well hover:text-body';
const ghostIconButton =
  'inline-flex h-8 w-8 items-center justify-center rounded-md text-muted transition-colors hover:bg-well hover:text-body';
const iconButton =
  'inline-flex h-8 w-8 items-center justify-center rounded-md border border-line-strong bg-raised text-body transition-colors hover:bg-well';
const commandItem =
  'flex min-h-9 cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm text-body outline-none aria-disabled:cursor-not-allowed aria-disabled:opacity-45 aria-selected:bg-accent-tint';
const treeMenuItem =
  'block w-full px-3 py-1.5 text-left text-sm text-body hover:bg-accent-tint focus:bg-accent-tint focus:outline-none';
const menuItem =
  'flex w-full cursor-pointer items-center gap-2 rounded px-2.5 py-1.5 text-left text-sm text-body outline-none select-none data-highlighted:bg-well';
const rightHeading = 'mb-2.5 flex items-center gap-2 text-[0.6875rem] font-semibold uppercase tracking-wider text-faint';
const rightPaneTabs: Array<{ key: RightPaneTab; label: string }> = [
  { key: 'links', label: 'Links' },
  { key: 'backlinks', label: 'Backlinks' },
  { key: 'properties', label: 'Properties' },
  { key: 'graph', label: 'Graph' },
  { key: 'versions', label: 'Versions' },
  { key: 'conflicts', label: 'Conflicts' },
];
const graphScopes: Array<{ key: GraphScope; label: string }> = [
  { key: 'focused', label: 'Focused' },
  { key: 'full', label: 'Full library' },
];
const graphDepthOptions: GraphDepth[] = [1, 2, 3];
const graphLinkKindOptions: Array<{ key: GraphLinkKindFilter; label: string }> = [
  { key: 'all', label: 'All kinds' },
  { key: 'wiki_link', label: 'Wiki links' },
  { key: 'markdown_link', label: 'Markdown' },
  { key: 'embed', label: 'Embeds' },
  { key: 'heading', label: 'Headings' },
  { key: 'tag', label: 'Tags' },
];
const graphResolutionOptions: Array<{ key: GraphResolutionFilter; label: string }> = [
  { key: 'all', label: 'All states' },
  { key: 'resolved', label: 'Resolved' },
  { key: 'unresolved', label: 'Unresolved' },
];
