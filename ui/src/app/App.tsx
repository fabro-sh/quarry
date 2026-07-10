import { Dialog } from '@radix-ui/react-dialog';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import * as Tooltip from '@radix-ui/react-tooltip';
import { Command } from 'cmdk';
import {
  AlertTriangle,
  Bot,
  Braces,
  Check,
  CheckCircle2,
  ChevronDown,
  Copy,
  Download,
  Eye,
  FilePlus2,
  FileText,
  FolderInput,
  FolderTree,
  GitBranch,
  Hash,
  Heading1,
  Library,
  Link2,
  MessageSquarePlus,
  Moon,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  PencilLine,
  Plus,
  Loader2,
  RotateCcw,
  Search,
  Settings as SettingsIcon,
  Sun,
  Trash2,
  Unlink,
  Upload,
} from 'lucide-react';
import {
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  useCallback,
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
  type ImperativePanelGroupHandle,
  type ImperativePanelHandle,
} from 'react-resizable-panels';
import { BrowserRouter, useLocation } from 'react-router-dom';
import useSWR, { useSWRConfig } from 'swr';

import {
  ApiError,
  type AgentPresenceDisplayEntry,
  backlinks,
  createCollabInvite,
  fetchAgentPrompt,
  createDocument,
  createGitPeer,
  createTmpDocument,
  deleteDocument,
  diffVersion,
  documentHref,
  documentVersion,
  getCapabilities,
  getDocument,
  getDocumentReview,
  gitExport,
  gitImport,
  gitPull,
  gitPush,
  gitSync,
  isTextContentType,
  listAgentPresence,
  listConflicts,
  listDocuments,
  listGitPeers,
  listLibraries,
  moveDocument,
  outgoingLinks,
  postBlockTransaction,
  promoteTmpDocument,
  putBinaryDocument,
  putDocument,
  resolveConflict,
  restoreVersion,
  searchDocuments,
  tmpDocumentHref,
  versions,
} from '../api/client';
import {
  type DocumentRef,
  documentRefKey,
  documentRefPath,
  documentRefUrl,
  libraryDocumentRef,
  tmpDocumentRef,
} from '../api/document-ref';
import type {
  AgentReviewResponse,
  BlockTransactionRequest,
  ConflictRecord,
  DocumentHistoryEntry,
  DocumentLink,
  DocumentListEntry,
  DocumentVersion,
  DocumentVersionContent,
  Library as LibraryType,
  SearchResult,
  SearchSuggestion,
  VersionDiff,
} from '../api/generated/types';
import type {
  DocumentMutationOptions,
  GitExportResult,
  GitImportResult,
  GitPeer,
  GitSyncResult,
} from '../api/client';
import {
  classifyLiveDocumentEvent,
  type LiveCollabSession,
} from '../features/collab/session-events';
import { collabDebug } from '../features/collab/collab-debug';
import { saveStateLabel, type CollabSaveState } from '../features/collab/save-state';
import { tmpCollabWebSocketBaseUrl } from '../features/collab/rust-ws-provider';
import {
  type EditorMode,
  type ImageApi,
  type WikiLinkApi,
} from '../features/editor/MarkdownEditor';
import { AgentAvatar } from '../features/agents/AgentAvatar';
import { agentKind } from '../features/agents/agents';
import { fileToDataUrl, imageAssetPath, resolveImageSrc } from '../features/editor/image';
import {
  DEFAULT_AUTHOR,
  hasStoredAuthor,
  loadAuthor,
  saveAuthor,
  storedAuthor,
} from '../features/review/identity';
import { CommentsPanel } from '../features/review/ui/CommentsPanel';
import { buildDocumentTree, droppedDocumentPath, type TreeNode } from '../features/tree/tree-model';
import { cn } from '../lib/utils';
import { WELCOME_DOCUMENT } from './welcome-document';
import { useWorkspaceNavigation } from './workspace-navigation';
import { useOpenDocumentController } from './open-document-state';
import { DocumentBody } from './document-body';
import {
  type BrowserEventPayload,
  useWorkspaceEventStream,
} from './workspace-event-stream';

type ThemePreference = 'light' | 'dark';
type TreeOpenState = Record<string, boolean>;
type RightPaneTab = 'links' | 'versions' | 'comments';
const DEFAULT_WORKSPACE_LAYOUT = [22, 54, 24];
const EVENT_POLL_INTERVAL_MS = 5_000;
// How long the settled "Saved" status lingers before it fades away, so the
// header confirms the save and then gets out of the way.
const SAVED_STATUS_LINGER_MS = 2_000;
const RECENT_LIBRARY_LIMIT = 8;
const LIBRARY_EVENT_TYPES = [
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
] as const;
const TMP_EVENT_TYPES = [
  'doc.changed',
  'doc.deleted',
  'doc.moved',
  'conflict.created',
  'conflict.resolved',
  'stream.lagged',
] as const;

interface TreeMenuState {
  node: TreeNode;
  x: number;
  y: number;
}

interface AddAgentModalState {
  open: boolean;
  loading: boolean;
  instructions: string;
  error: string;
  waitingForAgent: boolean;
  knownAgentIds: string[];
}

export function App() {
  return (
    <BrowserRouter>
      <Workspace />
    </BrowserRouter>
  );
}

function Workspace() {
  const location = useLocation();
  const routeCollabToken = useMemo(
    () => new URLSearchParams(location.search).get('token') ?? undefined,
    [location.search]
  );
  const { mutate } = useSWRConfig();
  const { data: capabilities } = useSWR('/v1/capabilities', getCapabilities);
  const capabilitiesLoaded = Boolean(capabilities);
  const tmpDocumentsEnabled = capabilities?.tmp_documents ?? false;
  const libDocumentsEnabled = capabilities?.lib_documents ?? false;
  const { data: libraries = [] } = useSWR(
    libDocumentsEnabled ? '/v1/libraries' : null,
    listLibraries
  );
  const defaultLibrary = orderLibrariesByRecent(libraries, '')[0]?.slug ?? libraries[0]?.slug ?? '';
  const navigation = useWorkspaceNavigation({
    capabilitiesLoaded,
    defaultLibrary,
    libDocumentsEnabled,
    libraries,
    tmpDocumentsEnabled,
  });
  const { activeLibrary, documentScope, routeSelection, selectedPath } = navigation;
  const [treeOpenState, setTreeOpenState] = useState<TreeOpenState>(() =>
    loadTreeOpenState(activeLibrary)
  );
  const [rightPaneTab, setRightPaneTab] = useState<RightPaneTab>(() => loadRightPaneTab(activeLibrary));
  const [searchQuery, setSearchQuery] = useState('');
  // The Phase 5 save state: derived inside the collab editor from
  // connection state + checkpoint-ack coverage; null = no session-backed
  // document open (nothing to save from the browser).
  const [saveState, setSaveState] = useState<CollabSaveState | null>(null);
  const [editorMode, setEditorMode] = useState<EditorMode>('editing');
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState('');
  const [gitOpen, setGitOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [addAgentModal, setAddAgentModal] = useState<AddAgentModalState>({
    open: false,
    loading: false,
    instructions: '',
    error: '',
    waitingForAgent: false,
    knownAgentIds: [],
  });
  const [lastSyncResult, setLastSyncResult] = useState('');
  const [author, setAuthor] = useState(() => loadAuthor());
  // The name prompt is deferred to the moment attribution starts to matter —
  // inviting an agent — instead of gating first load. Skipping proceeds
  // anonymously and stays skipped for this visit.
  const [namePromptOpen, setNamePromptOpen] = useState(false);
  const namePromptSkippedRef = useRef(false);
  const [theme, setTheme] = useState<ThemePreference>(() =>
    localStorage.getItem('quarry:theme') === 'light' ? 'light' : 'dark'
  );
  const [mergeConflictId, setMergeConflictId] = useState<string | null>(null);
  const [treeMenu, setTreeMenu] = useState<TreeMenuState | null>(null);
  const leftPanelRef = useRef<ImperativePanelHandle>(null);
  const workspaceLayoutRef = useRef<ImperativePanelGroupHandle>(null);
  const rightPanelRef = useRef<ImperativePanelHandle>(null);
  const uploadMarkdownInputRef = useRef<HTMLInputElement>(null);
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [resizingPanels, setResizingPanels] = useState(false);
  const selectedPathRef = useRef(selectedPath);
  const openDocumentRef = useRef<(path: string) => void>(() => {});
  const liveCollabSessionRef = useRef<LiveCollabSession | null>(null);
  const collabSessionIdRef = useRef(makeCollabSessionId());
  const searchQueryRef = useRef(searchQuery);

  useEffect(() => {
    setTreeOpenState(loadTreeOpenState(activeLibrary));
    setRightPaneTab(loadRightPaneTab(activeLibrary));
  }, [activeLibrary]);

  // `/tmp/new` creates a welcome-seeded scratch document and replaces itself
  // with the document's real route, so reloads and the back button never
  // mint another one.
  const autoCreatedTmpRef = useRef(false);
  useEffect(() => {
    if (!routeSelection.createTmp || !tmpDocumentsEnabled || autoCreatedTmpRef.current) return;
    autoCreatedTmpRef.current = true;
    void (async () => {
      try {
        await createNewTmpDocument(WELCOME_DOCUMENT, { replace: true });
      } catch {
        navigation.openTmpDocument('', { replace: true });
      }
    })();
  }, [routeSelection.createTmp, tmpDocumentsEnabled]);

  useEffect(() => {
    if (libDocumentsEnabled && activeLibrary) {
      localStorage.setItem('quarry:active-library', activeLibrary);
      persistRecentLibrary(
        activeLibrary,
        libraries.map((library) => library.slug)
      );
    }
  }, [activeLibrary, libDocumentsEnabled, libraries]);

  useEffect(() => {
    localStorage.setItem('quarry:theme', theme);
    window.document.documentElement.dataset.theme = theme;
  }, [theme]);

  function changeAuthor(nextAuthor: string) {
    setAuthor(saveAuthor(nextAuthor));
  }

  useEffect(() => {
    setLastSyncResult('');
  }, [activeLibrary]);

  useEffect(() => {
    selectedPathRef.current = selectedPath;
  }, [selectedPath]);

  useEffect(() => {
    openDocumentRef.current = openDocument;
  });

  function browserMutationOptions() {
    return {
      originId: collabSessionIdRef.current,
      transactionActor: storedAuthor(),
    };
  }

  const clearDeletedDocumentCaches = useCallback(
    (ref: DocumentRef) => {
      const path = documentRefPath(ref);
      const scoped = [
        mutate(documentRefKey('document', ref), undefined, { revalidate: false }),
        mutate(documentRefKey('versions', ref), [], { revalidate: false }),
      ];
      const libraryOnly =
        ref.scope === 'library'
          ? [
              mutate(['/v1/outgoing', ref.library, path], { path, links: [] }, { revalidate: false }),
              mutate(['/v1/backlinks', ref.library, path], { path, links: [] }, { revalidate: false }),
            ]
          : [];
      return Promise.all([...scoped, ...libraryOnly]);
    },
    [mutate]
  );

  const seedCreatedDocumentCaches = useCallback(
    (
      ref: DocumentRef,
      createdContent: string,
      createdContentType: string,
      created: Awaited<ReturnType<typeof createDocument>>
    ) => {
      const createdEtag = created.etag || `"${created.outcome.version.id}"`;
      const documentId = created.outcome.document?.id ?? '';
      return Promise.all([
        mutate(
          documentRefKey('document', ref),
          {
            content: createdContent,
            contentType: createdContentType,
            documentId,
            etag: createdEtag,
            path: documentRefPath(ref),
          },
          { revalidate: false }
        ),
        mutate(
          documentRefKey('versions', ref),
          [historyEntryFromVersion(created.outcome.version)],
          { revalidate: false }
        ),
      ]).then(() => ({ documentId, etag: createdEtag }));
    },
    [mutate]
  );

  // The document-scoped caches a write invalidates in ANY scope: content
  // arrives through the live collab doc, so only the metadata projections
  // (version history, review record) need a refetch.
  const invalidateDocumentScopedState = useCallback(
    (ref: DocumentRef) => {
      void mutate(documentRefKey('versions', ref));
      void mutate(documentRefKey('review', ref));
    },
    [mutate]
  );

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

  const invalidateLibraryDocumentState = useCallback(
    (path: string) => {
      const ref = libraryDocumentRef(activeLibrary, path);
      void mutate(documentRefKey('document', ref));
      void mutate(documentRefKey('versions', ref));
      void mutate(['/v1/outgoing', activeLibrary, path]);
      void mutate(['/v1/backlinks', activeLibrary, path]);
    },
    [activeLibrary, mutate]
  );
  const invalidateLibrarySearch = useCallback(() => {
    const query = searchQueryRef.current;
    if (query) void mutate(['/v1/search', activeLibrary, query]);
  }, [activeLibrary, mutate]);
  const invalidateCurrentBacklinks = useCallback(() => {
    const currentPath = selectedPathRef.current;
    if (currentPath) void mutate(['/v1/backlinks', activeLibrary, currentPath]);
  }, [activeLibrary, mutate]);
  const handleLibraryEvent = useCallback(
    (payload: BrowserEventPayload) => {
      const currentPath = selectedPathRef.current;
      void mutate(['/v1/documents', activeLibrary]);
      invalidateLibrarySearch();

      if (payload.type === 'stream.lagged' || payload.type === 'directory.changed') {
        if (currentPath) invalidateLibraryDocumentState(currentPath);
        void mutate(['/v1/conflicts', activeLibrary]);
        return;
      }
      if (payload.type === 'links.indexed' || payload.type === 'library.reindexed') {
        if (currentPath) {
          void mutate(['/v1/outgoing', activeLibrary, currentPath]);
          void mutate(['/v1/backlinks', activeLibrary, currentPath]);
        }
        invalidateLibrarySearch();
        return;
      }
      if (payload.type === 'git.sync.completed') {
        if (currentPath) invalidateLibraryDocumentState(currentPath);
        void mutate(['/v1/conflicts', activeLibrary]);
        void mutate(['/v1/git-peers', activeLibrary]);
        setLastSyncResult(gitSyncEventSummary(payload));
        return;
      }
      if (payload.type === 'conflict.created' || payload.type === 'conflict.resolved') {
        void mutate(['/v1/conflicts', activeLibrary]);
        return;
      }

      const liveDecision = classifyLiveDocumentEvent(payload, liveCollabSessionRef.current);
      if (liveDecision.action !== 'pass') {
        collabDebug('event.classify', {
          action: liveDecision.action,
          type: payload.type,
          originId: payload.origin_id,
          versionId: payload.version_id,
          etag: payload.etag,
        });
      }
      if (liveDecision.action === 'session_refresh') {
        if (currentPath) {
          invalidateDocumentScopedState(libraryDocumentRef(activeLibrary, currentPath));
          void mutate(['/v1/outgoing', activeLibrary, currentPath]);
          void mutate(['/v1/backlinks', activeLibrary, currentPath]);
        }
        return;
      }
      if (liveDecision.action === 'retarget_move') {
        if (liveCollabSessionRef.current) {
          liveCollabSessionRef.current = {
            ...liveCollabSessionRef.current,
            path: liveDecision.path,
          };
        }
        navigation.openLibraryDocument(activeLibrary, liveDecision.path, { replace: true });
        invalidateCurrentBacklinks();
        return;
      }
      if (payload.type === 'doc.deleted' && payload.path) {
        void clearDeletedDocumentCaches(libraryDocumentRef(activeLibrary, payload.path));
        if (payload.path === currentPath) navigation.closeDocument();
        else invalidateCurrentBacklinks();
        return;
      }
      if (payload.type === 'doc.moved' && payload.from === currentPath && payload.to) {
        navigation.openLibraryDocument(activeLibrary, payload.to, { replace: true });
        invalidateLibraryDocumentState(payload.to);
        return;
      }
      if (payload.path && payload.path === currentPath) {
        invalidateLibraryDocumentState(payload.path);
        return;
      }
      invalidateCurrentBacklinks();
    },
    [
      activeLibrary,
      clearDeletedDocumentCaches,
      invalidateCurrentBacklinks,
      invalidateDocumentScopedState,
      invalidateLibraryDocumentState,
      invalidateLibrarySearch,
      mutate,
      navigation,
    ]
  );
  const pollLibraryState = useCallback(() => {
    void mutate(['/v1/documents', activeLibrary]);
    const currentPath = selectedPathRef.current;
    if (currentPath) invalidateLibraryDocumentState(currentPath);
    void mutate(['/v1/conflicts', activeLibrary]);
    void mutate(['/v1/git-peers', activeLibrary]);
    invalidateLibrarySearch();
  }, [activeLibrary, invalidateLibraryDocumentState, invalidateLibrarySearch, mutate]);
  useWorkspaceEventStream({
    enabled: libDocumentsEnabled && Boolean(activeLibrary),
    eventTypes: LIBRARY_EVENT_TYPES,
    onEvent: handleLibraryEvent,
    onPoll: pollLibraryState,
    pollIntervalMs: EVENT_POLL_INTERVAL_MS,
    url: `/v1/events?library=${encodeURIComponent(activeLibrary)}`,
  });

  const isTmpDocument = documentScope === 'tmp';
  const isLibraryDocument = documentScope === 'library';
  // Whether the active scope's document surface is usable: the tmp surface
  // needs only its capability flag; the library surface needs its flag plus a
  // selected library. Gates every scope-dispatched SWR key below.
  const scopeReady = isTmpDocument
    ? tmpDocumentsEnabled
    : libDocumentsEnabled && Boolean(activeLibrary);
  // The selected document's scope-resolved address — the one value every
  // document-scoped client call and SWR key derives from.
  const documentRef: DocumentRef | null = selectedPath
    ? isTmpDocument
      ? tmpDocumentRef(selectedPath)
      : libraryDocumentRef(activeLibrary, selectedPath)
    : null;

  // Tmp content arrives over the live session; document events refresh only
  // metadata projections and diff3 review records.
  const tmpEventRef = isTmpDocument && selectedPath ? tmpDocumentRef(selectedPath) : null;
  const handleTmpEvent = useCallback(
    (payload: BrowserEventPayload) => {
      if (!tmpEventRef) return;
      if (payload.type === 'doc.deleted') {
        void clearDeletedDocumentCaches(tmpEventRef);
        return;
      }
      invalidateDocumentScopedState(tmpEventRef);
    },
    [clearDeletedDocumentCaches, invalidateDocumentScopedState, tmpEventRef]
  );
  const pollTmpState = useCallback(() => {
    if (tmpEventRef) invalidateDocumentScopedState(tmpEventRef);
  }, [invalidateDocumentScopedState, tmpEventRef]);
  useWorkspaceEventStream({
    enabled: Boolean(tmpEventRef && tmpDocumentsEnabled),
    eventTypes: TMP_EVENT_TYPES,
    onEvent: handleTmpEvent,
    onPoll: pollTmpState,
    pollIntervalMs: EVENT_POLL_INTERVAL_MS,
    url: tmpEventRef ? documentRefUrl(tmpEventRef, '/events/stream') : '',
  });

  const { data: libraryDocuments = [] } = useSWR(
    libDocumentsEnabled && activeLibrary ? ['/v1/documents', activeLibrary] : null,
    () => listDocuments(activeLibrary)
  );
  const documents = isTmpDocument ? [] : libraryDocuments;
  const { data: document } = useSWR(
    documentRef && scopeReady ? documentRefKey('document', documentRef) : null,
    () => getDocument(requireValue(documentRef)),
    { revalidateOnFocus: false }
  );
  const openDocumentState = useOpenDocumentController({
    activeLibrary,
    document,
    documentScope,
    selectedPath,
  });
  const {
    compareVersionId,
    content,
    contentType,
    currentDiffOpen,
    etag,
    selectedVersionId,
  } = openDocumentState;
  const { data: search = { results: [], cursor: null } } = useSWR(
    libDocumentsEnabled && isLibraryDocument && activeLibrary && searchQuery
      ? ['/v1/search', activeLibrary, searchQuery]
      : null,
    () => searchDocuments(activeLibrary, searchQuery)
  );
  const { data: outgoing = { path: selectedPath, links: [] } } = useSWR(
    libDocumentsEnabled && isLibraryDocument && activeLibrary && selectedPath
      ? ['/v1/outgoing', activeLibrary, selectedPath]
      : null,
    () => outgoingLinks(activeLibrary, selectedPath)
  );
  const { data: incoming = { path: selectedPath, links: [] } } = useSWR(
    libDocumentsEnabled && isLibraryDocument && activeLibrary && selectedPath
      ? ['/v1/backlinks', activeLibrary, selectedPath]
      : null,
    () => backlinks(activeLibrary, selectedPath)
  );

  // Resolve editor wiki-links against the backend's outgoing links for the open
  // document, and open the resolved target. Memoized on `outgoing` so the editor
  // chips don't re-render on every keystroke; `open` reads the latest navigate
  // handler via ref. Resolution lags an edit until the next save + reindex.
  const wikiLink = useMemo<WikiLinkApi>(() => {
    const byTarget = new Map<string, DocumentLink>();
    for (const link of outgoing.links) {
      if (link.target_kind === 'wiki_link' || link.target_kind === 'embed') {
        byTarget.set(link.target_text.toLowerCase(), link);
      }
    }
    return {
      resolve: (target) => {
        const link = byTarget.get(target.toLowerCase());
        return link ? { resolved: link.resolved, targetPath: link.target_path } : undefined;
      },
      open: (path) => openDocumentRef.current(path),
    };
  }, [outgoing]);

  // Render image urls (relative asset paths) against the serve endpoint, and
  // store dropped/pasted images as content-addressed `assets/<hash>` documents.
  const imageApi = useMemo<ImageApi>(
    () => ({
      resolveSrc: (url) => resolveImageSrc(url, activeLibrary),
      upload: async (file) => {
        const path = await imageAssetPath(file);
        try {
          await putBinaryDocument(
            activeLibrary,
            path,
            file,
            file.type || 'application/octet-stream',
            browserMutationOptions()
          );
        } catch (error) {
          // 412 means an identical asset is already stored at this path — reuse it.
          if (!(error instanceof ApiError) || error.code !== 'PRECONDITION_FAILED') throw error;
        }
        void mutate(['/v1/documents', activeLibrary]);
        return path;
      },
    }),
    [activeLibrary, mutate]
  );
  const tmpImageApi = useMemo<ImageApi>(
    () => ({
      upload: fileToDataUrl,
    }),
    []
  );
  const { data: versionList = [] } = useSWR(
    documentRef && scopeReady ? documentRefKey('versions', documentRef) : null,
    () => versions(requireValue(documentRef))
  );
  const headVersionId = versionList[0]?.latest_version_id;
  const { data: selectedVersionContent } = useSWR(
    documentRef && selectedVersionId && scopeReady
      ? [...documentRefKey('version-content', documentRef), selectedVersionId]
      : null,
    () => documentVersion(requireValue(documentRef), requireValue(selectedVersionId))
  );
  const selectedDiffAgainstVersionId = compareVersionId ?? headVersionId;
  const { data: selectedVersionDiff } = useSWR(
    documentRef && scopeReady && selectedVersionId
      ? [
          ...documentRefKey('version-diff', documentRef),
          selectedVersionId,
          selectedDiffAgainstVersionId ?? '',
        ]
      : null,
    () =>
      diffVersion(
        requireValue(documentRef),
        requireValue(selectedVersionId),
        selectedDiffAgainstVersionId
      )
  );
  const currentEditorDiff = useMemo(
    () => unifiedLineDiff(document?.content ?? '', content, 'latest server', 'current editor'),
    [content, document?.content]
  );
  const { data: conflicts = [] } = useSWR(
    libDocumentsEnabled && isLibraryDocument && activeLibrary ? ['/v1/conflicts', activeLibrary] : null,
    () => listConflicts(activeLibrary)
  );
  const { data: gitPeers = [] } = useSWR(
    libDocumentsEnabled && isLibraryDocument && activeLibrary ? ['/v1/git-peers', activeLibrary] : null,
    () => listGitPeers(activeLibrary)
  );

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
  const loadedDocumentForSelection = document?.path === selectedPath ? document : undefined;
  const loadedDocumentContentType = loadedDocumentForSelection?.contentType;
  const selectedContentType = loadedDocumentContentType ?? selectedEntry?.content_type ?? contentType;
  const selectedIsMarkdown = Boolean(
    selectedPath && isMarkdownDocument(selectedPath, selectedContentType)
  );
  const activeLoadedDocument =
    openDocumentState.identity?.scope === documentScope &&
    openDocumentState.identity.library === activeLibrary &&
    openDocumentState.identity.path === selectedPath
      ? openDocumentState.identity
      : null;
  const selectedDocumentBodyReady = Boolean(
    loadedDocumentForSelection && activeLoadedDocument
  );
  // Title from the live editor mirror once the body is mounted: collab
  // writes (the only write path for tmp documents) never touch the SWR
  // snapshot, which can stay at its seeded "# Untitled" forever. Before the
  // body is ready `content` may still hold the previous document, so fall
  // back to the snapshot.
  const titleContent = selectedDocumentBodyReady ? content : document?.content;
  useEffect(() => {
    if (!selectedPath) {
      window.document.title = 'Quarry';
      return;
    }
    const h1 = selectedIsMarkdown && titleContent ? extractFirstH1(titleContent) : null;
    window.document.title = `${h1 ?? documentBasename(selectedPath)} · Quarry`;
  }, [selectedIsMarkdown, selectedPath, titleContent]);
  const collabDocumentId = selectedDocumentBodyReady
    ? activeLoadedDocument?.documentId || loadedDocumentForSelection?.documentId || ''
    : '';
  const collabBaseUrl =
    isTmpDocument && selectedPath ? tmpCollabWebSocketBaseUrl(selectedPath) : undefined;
  const collabRoomName = isTmpDocument ? 'content' : undefined;
  const layoutStorageKey = activeLibrary ? `quarry:layout:${activeLibrary}` : 'quarry:layout:workspace';
  // Tmp markdown documents carry the details pane too (the review record —
  // diff3 conflicts especially — must stay visible to the human); the editor
  // panel is only pinned full-width while the pane is absent.
  const rightPaneVisible = !isTmpDocument || isMarkdownDocument(selectedPath, selectedContentType);
  const mergeConflict = conflicts.find((conflict) => conflict.id === mergeConflictId) ?? null;
  const { data: agentPresence = { presence: [] } } = useSWR(
    documentRef && isTextContentType(selectedContentType) && scopeReady
      ? documentRefKey('presence', documentRef)
      : null,
    () => listAgentPresence(requireValue(documentRef)),
    { refreshInterval: 3_000 }
  );
  // The rows-backed review projection feeds the Comments panel (states,
  // orphaned/invalidated badges, diff3 conflict items). Refreshed by the
  // scope's event stream above whenever the document changes.
  const { data: documentReview } = useSWR(
    documentRef && isMarkdownDocument(selectedPath, selectedContentType) && scopeReady
      ? documentRefKey('review', documentRef)
      : null,
    () => getDocumentReview(requireValue(documentRef))
  );

  useEffect(() => {
    if (
      selectedPath &&
      collabDocumentId &&
      isMarkdownDocument(selectedPath, selectedContentType)
    ) {
      liveCollabSessionRef.current = {
        documentId: collabDocumentId,
        path: selectedPath,
      };
    } else {
      liveCollabSessionRef.current = null;
      setSaveState(null);
    }
  }, [collabDocumentId, selectedContentType, selectedPath]);

  const changeSaveState = useCallback((state: CollabSaveState) => {
    setSaveState(state);
  }, []);

  // The editor's serialized mirror: feeds downloads and the current-editor
  // diff. Durability belongs to the session checkpoint, never to this state.
  const changeContent = useCallback(
    (next: string) => openDocumentState.changeContent(next),
    [openDocumentState.changeContent]
  );

  async function createNewDocument(defaultPath = 'untitled.md') {
    if (!libDocumentsEnabled || !activeLibrary) return;
    const path = window.prompt('New document path', defaultPath);
    if (!path) return;
    const initialContent = '# Untitled\n';
    const initialContentType = 'text/markdown';
    const created = await createDocument(
      activeLibrary,
      path,
      initialContent,
      initialContentType,
      browserMutationOptions()
    );
    await seedCreatedDocumentCaches(libraryDocumentRef(activeLibrary, path), initialContent, initialContentType, created);
    await mutate(['/v1/documents', activeLibrary]);
    navigation.openLibraryDocument(activeLibrary, path);
  }

  async function createNewTmpDocument(
    seed = { title: 'Untitled', content: '# Untitled\n' },
    options: { readonly replace?: boolean } = {}
  ) {
    if (!tmpDocumentsEnabled) return;
    const initialContent = seed.content;
    const initialContentType = 'text/markdown';
    const created = await createTmpDocument({
      content: initialContent,
      metadata: { title: seed.title },
    });
    const secret = created.outcome.document?.path ?? '';
    if (!secret) throw new Error('tmp document creation did not return a secret');
    await seedCreatedDocumentCaches(
      tmpDocumentRef(secret),
      initialContent,
      initialContentType,
      created
    );
    navigation.openTmpDocument(secret, options);
    return secret;
  }

  async function createVisibleDocument(defaultPath = 'untitled.md') {
    if (isTmpDocument || !libDocumentsEnabled) await createNewTmpDocument();
    else await createNewDocument(defaultPath);
  }

  // Seeds a fresh scratch document from an uploaded Markdown file — the empty
  // workspace has no open document to replace, unlike startUploadMarkdown.
  async function createTmpDocumentFromFile(file: File | undefined) {
    if (!file || !tmpDocumentsEnabled) return;
    try {
      const text = await file.text();
      const title = extractFirstH1(text) ?? file.name.replace(/\.(md|markdown)$/i, '');
      await createNewTmpDocument({ title: title || 'Untitled', content: text });
    } catch (error) {
      window.alert(
        `Upload Markdown failed: ${error instanceof Error ? error.message : String(error)}`
      );
    }
  }

  async function createDocumentFromLink(link: DocumentLink) {
    if (!libDocumentsEnabled || !isLibraryDocument || !activeLibrary) return;
    const defaultPath = defaultDocumentPathForLink(link);
    const path = window.prompt('New document path', defaultPath);
    if (!path) return;
    const initialContent = '# Untitled\n';
    const initialContentType = 'text/markdown';
    const created = await createDocument(
      activeLibrary,
      path,
      initialContent,
      initialContentType,
      browserMutationOptions()
    );
    await seedCreatedDocumentCaches(libraryDocumentRef(activeLibrary, path), initialContent, initialContentType, created);
    await Promise.all([
      mutate(['/v1/documents', activeLibrary]),
      selectedPath ? mutate(['/v1/outgoing', activeLibrary, selectedPath]) : Promise.resolve(),
    ]);
    navigation.openLibraryDocument(activeLibrary, path);
  }

  async function renameCurrent() {
    if (!libDocumentsEnabled || !isLibraryDocument || !activeLibrary || !selectedPath) return;
    const toPath = window.prompt('Move document to path', selectedPath);
    if (!toPath || toPath === selectedPath) return;
    await moveDocument(activeLibrary, selectedPath, toPath, browserMutationOptions());
    await mutate(['/v1/documents', activeLibrary]);
    navigation.openLibraryDocument(activeLibrary, toPath, { replace: true });
  }

  async function moveDocumentPath(fromPath: string) {
    if (!libDocumentsEnabled || !activeLibrary) return;
    const toPath = window.prompt('Move document to path', fromPath);
    if (!toPath || toPath === fromPath) return;
    await moveDocument(activeLibrary, fromPath, toPath, browserMutationOptions());
    await mutate(['/v1/documents', activeLibrary]);
    if (selectedPath === fromPath) {
      navigation.openLibraryDocument(activeLibrary, toPath, { replace: true });
    }
  }

  const moveDroppedTreeDocuments: MoveHandler<TreeNode> = async ({ dragNodes, parentNode }) => {
    if (!libDocumentsEnabled || !activeLibrary || isTmpDocument) return;
    const parent = parentNode?.data ?? null;
    const moves = dragNodes
      .map((node) => node.data)
      .filter((node) => node.kind === 'document')
      .map((node) => ({ from: node.path, to: droppedDocumentPath(node, parent) }))
      .filter((move) => move.from !== move.to);
    if (!moves.length) return;
    await Promise.all(
      moves.map((move) => moveDocument(activeLibrary, move.from, move.to, browserMutationOptions()))
    );
    await mutate(['/v1/documents', activeLibrary]);
    const movedSelection = moves.find((move) => move.from === selectedPath);
    if (movedSelection) {
      navigation.openLibraryDocument(activeLibrary, movedSelection.to, { replace: true });
    }
  };

  async function deleteCurrent() {
    if (!documentRef || (isLibraryDocument && !activeLibrary)) return;
    const deletingRef = documentRef;
    if (!window.confirm(`Delete ${documentRefPath(deletingRef)}?`)) return;
    await deleteDocument(deletingRef, browserMutationOptions());
    await clearDeletedDocumentCaches(deletingRef);
    if (deletingRef.scope === 'library') await mutate(['/v1/documents', activeLibrary]);
    navigation.closeDocument();
  }

  async function promoteCurrentTmpDocument() {
    if (!libDocumentsEnabled || !isTmpDocument || !selectedPath) return;
    const library = window.prompt('Promote to library', activeLibrary);
    if (!library) return;
    const targetPath = window.prompt('Promote to path', selectedPath);
    if (!targetPath) return;
    await promoteTmpDocument(selectedPath, {
      library,
      path: targetPath,
      ifMatch: etag.trim().replace(/^"|"$/g, ''),
    });
    await clearDeletedDocumentCaches(tmpDocumentRef(selectedPath));
    await mutate(['/v1/documents', library]);
    setTreeOpenState(loadTreeOpenState(library));
    setRightPaneTab(loadRightPaneTab(library));
    navigation.openLibraryDocument(library, targetPath, { replace: true });
  }

  // Downloads serve the canonical export (frontmatter included) — the same
  // bytes Git, FUSE, CLI, and agents see — not the editor's local serializer
  // mirror, which is a second Markdown writer that can drift from canonical.
  // Canonical reflects the last checkpoint; the save indicator already tells
  // the user whether their latest keystrokes are covered.
  async function downloadCurrentMarkdown() {
    if (!selectedPath || (isLibraryDocument && !activeLibrary)) return;
    const response = await fetch(
      isTmpDocument ? tmpDocumentHref(selectedPath) : documentHref(activeLibrary, selectedPath)
    );
    if (!response.ok) return;
    const blob = await response.blob();
    const url = URL.createObjectURL(blob);
    const anchor = window.document.createElement('a');
    anchor.href = url;
    anchor.download = documentBasename(selectedPath);
    anchor.click();
    URL.revokeObjectURL(url);
  }

  function copyCurrentRawLink() {
    if (!selectedPath || (isLibraryDocument && !activeLibrary)) return;
    const relativeHref = isTmpDocument
      ? tmpDocumentHref(selectedPath)
      : documentHref(activeLibrary, selectedPath);
    const rawLink = new URL(relativeHref, window.location.origin).toString();
    void copyText(rawLink, 'Raw document link');
  }

  function canUploadMarkdown() {
    if (!selectedPath || !selectedIsMarkdown) return false;
    if (isLibraryDocument && !activeLibrary) return false;
    if (saveState && saveState !== 'saved') {
      window.alert('Wait for the current document to finish saving before uploading Markdown.');
      return false;
    }
    return true;
  }

  function startUploadMarkdown() {
    if (!canUploadMarkdown()) return;
    uploadMarkdownInputRef.current?.click();
  }

  async function uploadCurrentMarkdownFile(file: File | null | undefined) {
    if (!file) return;
    if (!canUploadMarkdown()) {
      if (uploadMarkdownInputRef.current) uploadMarkdownInputRef.current.value = '';
      return;
    }
    const ref = documentRef;
    if (!ref) return;
    try {
      const [text, latest] = await Promise.all([file.text(), getDocument(ref)]);
      const saved = await putDocument(ref, text, latest.etag, 'text/markdown', browserMutationOptions());
      openDocumentState.adoptHead(saved.etag || `"${saved.outcome.version.id}"`);
      openDocumentState.resetHistoryView();
      const scoped = [
        mutate(documentRefKey('document', ref)),
        mutate(documentRefKey('versions', ref)),
        mutate(documentRefKey('review', ref)),
      ];
      const libraryOnly =
        ref.scope === 'library'
          ? [
              mutate(['/v1/documents', ref.library]),
              mutate(['/v1/outgoing', ref.library, ref.path]),
              mutate(['/v1/backlinks', ref.library, ref.path]),
            ]
          : [];
      await Promise.all([...scoped, ...libraryOnly]);
    } catch (error) {
      window.alert(
        `Upload Markdown failed: ${error instanceof Error ? error.message : String(error)}`
      );
    } finally {
      if (uploadMarkdownInputRef.current) uploadMarkdownInputRef.current.value = '';
    }
  }

  async function openAddAgentModal() {
    if (!selectedPath || !isMarkdownDocument(selectedPath, selectedContentType)) return;
    if (isLibraryDocument && (!libDocumentsEnabled || !activeLibrary)) return;
    if (isTmpDocument && !tmpDocumentsEnabled) return;
    if (!hasStoredAuthor() && !namePromptSkippedRef.current) {
      setNamePromptOpen(true);
      return;
    }
    const path = selectedPath;
    const knownAgentIds = agentPresence.presence.map((e) => e.agentId);
    setAddAgentModal({
      open: true,
      loading: true,
      instructions: '',
      error: '',
      waitingForAgent: false,
      knownAgentIds,
    });
    try {
      const instructions = isTmpDocument
        ? await fetchAgentPrompt({ scope: 'tmp', secret: path })
        : await fetchAgentPrompt({
            scope: 'library',
            library: activeLibrary,
            path,
            token: (
              await createCollabInvite(activeLibrary, path, { byHint: author, role: 'editor' })
            ).id,
          });
      setAddAgentModal({
        open: true,
        loading: false,
        instructions,
        error: '',
        waitingForAgent: false,
        knownAgentIds,
      });
    } catch (error) {
      setAddAgentModal({
        open: true,
        loading: false,
        instructions: '',
        error: error instanceof Error ? error.message : String(error),
        waitingForAgent: false,
        knownAgentIds,
      });
    }
  }

  function closeAddAgentModal() {
    setAddAgentModal((state) => ({ ...state, open: false }));
  }

  useEffect(() => {
    if (!addAgentModal.waitingForAgent) return;
    const newAgent = agentPresence.presence.find(
      (e) => !addAgentModal.knownAgentIds.includes(e.agentId)
    );
    if (newAgent) closeAddAgentModal();
  }, [agentPresence.presence, addAgentModal.waitingForAgent, addAgentModal.knownAgentIds]);

  async function deleteDocumentPath(path: string) {
    if (!libDocumentsEnabled || !activeLibrary) return;
    if (!window.confirm(`Delete ${path}?`)) return;
    await deleteDocument(libraryDocumentRef(activeLibrary, path), browserMutationOptions());
    await clearDeletedDocumentCaches(libraryDocumentRef(activeLibrary, path));
    await mutate(['/v1/documents', activeLibrary]);
    if (selectedPath === path) navigation.closeDocument();
  }

  async function restoreSelectedVersion(versionId: string) {
    if (!documentRef) return;
    const ref = documentRef;
    const restored = await restoreVersion(ref, versionId, browserMutationOptions());
    openDocumentState.adoptHead(restored.etag || `"${restored.outcome.version.id}"`);
    openDocumentState.resetHistoryView();
    const scoped = [
      mutate(documentRefKey('document', ref)),
      mutate(documentRefKey('versions', ref)),
    ];
    const libraryOnly =
      ref.scope === 'library'
        ? [
            mutate(['/v1/documents', ref.library]),
            mutate(['/v1/outgoing', ref.library, ref.path]),
            mutate(['/v1/backlinks', ref.library, ref.path]),
          ]
        : [];
    await Promise.all([...scoped, ...libraryOnly]);
  }

  async function resolveOpenConflict(conflictId: string) {
    if (!libDocumentsEnabled || !isLibraryDocument || !activeLibrary) return;
    await resolveConflict(activeLibrary, conflictId);
    await mutate(['/v1/conflicts', activeLibrary]);
  }

  // Diff3 conflict review items (whole-file merge leftovers) dismiss through
  // the block-transaction gateway; resolution never mutates the document.
  async function dismissReviewConflict(conflictId: string) {
    if (!documentRef) return;
    const request: BlockTransactionRequest = {
      client_tx_id: crypto.randomUUID(),
      actor: { kind: 'user', id: storedAuthor() },
      ops: [{ op: 'comment.resolve', item_id: conflictId }],
    };
    try {
      await postBlockTransaction(documentRef, request);
      await mutate(documentRefKey('review', documentRef));
    } catch (error) {
      window.alert(
        `Dismiss conflict failed: ${error instanceof Error ? error.message : String(error)}`
      );
    }
  }

  function openDocument(path: string) {
    if (!path || (documentScope === 'library' && path === selectedPath)) return;
    if (isTmpDocument || !libDocumentsEnabled) navigation.openTmpDocument(path);
    else navigation.openLibraryDocument(activeLibrary, path);
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
    await createVisibleDocument(defaultChildDocumentPath(node));
  }

  async function moveTreeDocument(node: TreeNode) {
    closeTreeContextMenu();
    if (libDocumentsEnabled && node.kind === 'document') await moveDocumentPath(node.path);
  }

  async function deleteTreeDocument(node: TreeNode) {
    closeTreeContextMenu();
    if (node.kind !== 'document') return;
    if (isTmpDocument) {
      if (!window.confirm(`Delete ${node.path}?`)) return;
      await deleteDocument(tmpDocumentRef(node.path), browserMutationOptions());
      await clearDeletedDocumentCaches(tmpDocumentRef(node.path));
      if (selectedPath === node.path) navigation.closeDocument();
    } else {
      await deleteDocumentPath(node.path);
    }
  }

  function copyTreePath(node: TreeNode) {
    closeTreeContextMenu();
    void copyText(node.path, 'Document path');
  }

  function changeActiveLibrary(slug: string) {
    if (!libDocumentsEnabled) return;
    setTreeOpenState(loadTreeOpenState(slug));
    setRightPaneTab(loadRightPaneTab(slug));
    navigation.selectLibrary(slug);
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

  function resetWorkspaceLayout() {
    localStorage.removeItem(layoutStorageKey);
    leftPanelRef.current?.expand();
    rightPanelRef.current?.expand();
    setLeftCollapsed(false);
    setRightCollapsed(false);
    const panelGroup = workspaceLayoutRef.current;
    if (panelGroup?.getLayout().length === DEFAULT_WORKSPACE_LAYOUT.length) {
      panelGroup.setLayout(DEFAULT_WORKSPACE_LAYOUT);
    }
  }

  function viewSelectedVersion(versionId: string) {
    openDocumentState.viewVersion(versionId);
  }

  function diffCurrentEditor() {
    openDocumentState.diffCurrent();
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

  function copyFuseMountCommand() {
    if (!libDocumentsEnabled || !activeLibrary) return;
    void copyText(
      `mkdir -p ${activeLibrary} && quarry mount ${activeLibrary} ${activeLibrary}`,
      'FUSE mount command'
    );
  }

  return (
    <main
      className="isolate flex h-screen min-h-0 flex-col overflow-hidden bg-canvas text-ink antialiased"
      data-theme={theme}
    >
      <h1 className="sr-only">Quarry</h1>
      <label className="sr-only" htmlFor="upload-markdown-file">
        Upload Markdown file
      </label>
      <input
        accept=".md,.markdown,text/markdown,text/x-markdown"
        className="sr-only"
        id="upload-markdown-file"
        ref={uploadMarkdownInputRef}
        type="file"
        onChange={(event) => void uploadCurrentMarkdownFile(event.currentTarget.files?.[0])}
      />

      <PanelGroup
        aria-label="Workspace layout"
        autoSaveId={layoutStorageKey}
        className="min-h-0 flex-1"
        data-layout-storage-key={layoutStorageKey}
        direction="horizontal"
        ref={workspaceLayoutRef}
      >
        {!isTmpDocument ? (
          <>
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
                canMoveDocuments={libDocumentsEnabled && isLibraryDocument}
                collapsed={leftCollapsed}
                heading={isTmpDocument ? 'Tmp documents' : 'Documents'}
                libraryControlsEnabled={libDocumentsEnabled}
                libraries={libraries}
                onCreate={() => void createVisibleDocument()}
                onCreateChild={(node) => void createVisibleDocument(defaultChildDocumentPath(node))}
                onLibraryChange={changeActiveLibrary}
                onMove={moveDroppedTreeDocuments}
                onOpen={openDocument}
                onOpenContextMenu={openTreeContextMenu}
                onRename={moveDocumentPath}
                onSearchChange={setSearchQuery}
                onToggleCollapsed={toggleLeftPane}
                searchEnabled={libDocumentsEnabled && isLibraryDocument}
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
          </>
        ) : null}
        <Panel
          defaultSize={isTmpDocument ? (rightPaneVisible ? 76 : 100) : 54}
          minSize={isTmpDocument && !rightPaneVisible ? 100 : 35}
        >
          {selectedPath ? (
            <div className="flex h-full min-h-0 flex-col">
              <DocumentToolbar
                agentPresence={agentPresence.presence}
                canPromote={libDocumentsEnabled}
                isMarkdown={isMarkdownDocument(selectedPath, selectedContentType)}
                isText={isTextContentType(selectedContentType)}
                isTmp={isTmpDocument}
                mode={editorMode}
                onModeChange={setEditorMode}
                path={selectedPath}
                saveState={saveState}
                onAddAgent={() => void openAddAgentModal()}
                onDelete={deleteCurrent}
                onDownload={downloadCurrentMarkdown}
                onCopyRawLink={copyCurrentRawLink}
                onPromote={() => void promoteCurrentTmpDocument()}
                onRename={renameCurrent}
                onUploadMarkdown={startUploadMarkdown}
              />
              {selectedDocumentBodyReady ? (
                <DocumentBody
                  author={author}
                  byteSize={selectedEntry?.byte_size}
                  collabEnabled={Boolean(collabDocumentId)}
                  collabBaseUrl={collabBaseUrl}
                  collabRoomName={collabRoomName}
                  collabSessionId={collabSessionIdRef.current}
                  collabToken={isTmpDocument ? undefined : routeCollabToken}
                  contentHash={selectedEntry?.content_hash}
                  content={content}
                  contentType={selectedContentType}
                  documentId={collabDocumentId}
                  href={isTmpDocument ? tmpDocumentHref(selectedPath) : documentHref(activeLibrary, selectedPath)}
                  image={isTmpDocument ? tmpImageApi : isLibraryDocument ? imageApi : undefined}
                  mode={editorMode}
                  path={selectedPath}
                  wikiLink={wikiLink}
                  onChange={changeContent}
                  onSaveStateChange={changeSaveState}
                />
              ) : (
                <LoadingDocument />
              )}
            </div>
          ) : (
            <EmptyDocument
              treeHidden={isTmpDocument}
              onCreate={() => void createVisibleDocument()}
              onUploadFile={(file) => void createTmpDocumentFromFile(file)}
            />
          )}
        </Panel>
        {rightPaneVisible ? (
          <>
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
                libraryControlsEnabled={libDocumentsEnabled && isLibraryDocument}
                onToggleCollapsed={toggleRightPane}
                conflicts={conflicts}
                currentDiffOpen={currentDiffOpen}
                currentEditorDiff={currentEditorDiff}
                compareVersionId={compareVersionId}
                incoming={incoming.links}
                onCompareVersionChange={openDocumentState.changeCompareVersion}
                onCreateDocumentFromLink={createDocumentFromLink}
                onDiffCurrent={diffCurrentEditor}
                onOpenDocument={openDocument}
                onOpenConflict={setMergeConflictId}
                onResolveConflict={resolveOpenConflict}
                onDismissConflict={dismissReviewConflict}
                onRestoreVersion={restoreSelectedVersion}
                onViewVersion={viewSelectedVersion}
                outgoing={outgoing.links}
                review={documentReview}
                reviewEnabled={isMarkdownDocument(selectedPath, selectedContentType)}
                selectedVersionContent={selectedVersionContent}
                selectedVersionDiff={selectedVersionDiff}
                selectedVersionId={selectedVersionId}
                onTabChange={changeRightPaneTab}
                versions={versionList}
              />
            </Panel>
          </>
        ) : null}
      </PanelGroup>

      <CommandPalette
        activeLibrary={activeLibrary}
        documents={documents}
        libraryControlsEnabled={libDocumentsEnabled && isLibraryDocument}
        open={paletteOpen}
        query={paletteQuery}
        selectedPath={selectedPath}
        onClose={closePalette}
        onCopyFuseMount={copyFuseMountCommand}
        onCreate={() => void createVisibleDocument()}
        onDelete={deleteCurrent}
        onDownload={downloadCurrentMarkdown}
        onUploadMarkdown={startUploadMarkdown}
        onOpenGit={() => setGitOpen(true)}
        onOpenSettings={() => setSettingsOpen(true)}
        onMove={renameCurrent}
        onOpenDocument={openDocument}
        onQueryChange={setPaletteQuery}
        onSearch={setSearchQuery}
        onToggleTheme={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
        selectedIsMarkdown={selectedIsMarkdown}
        theme={theme}
      />

      <AddAgentDialog
        error={addAgentModal.error}
        instructions={addAgentModal.instructions}
        loading={addAgentModal.loading}
        open={addAgentModal.open}
        waitingForAgent={addAgentModal.waitingForAgent}
        onClose={closeAddAgentModal}
        onCopied={() => setAddAgentModal((s) => ({ ...s, waitingForAgent: true }))}
      />

      <NamePromptDialog
        open={namePromptOpen}
        onSkip={() => {
          namePromptSkippedRef.current = true;
          setNamePromptOpen(false);
          void openAddAgentModal();
        }}
        onSubmit={(name) => {
          changeAuthor(name);
          setNamePromptOpen(false);
          void openAddAgentModal();
        }}
      />

      <SettingsDialog
        activeLibrary={activeLibrary}
        author={author}
        librarySettingsVisible={libDocumentsEnabled}
        layoutSettingsVisible={libDocumentsEnabled && !isTmpDocument}
        open={settingsOpen}
        theme={theme}
        onClose={() => setSettingsOpen(false)}
        onAuthorChange={changeAuthor}
        onResetLayout={resetWorkspaceLayout}
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
        canMoveDocuments={libDocumentsEnabled && isLibraryDocument}
        menu={treeMenu}
        onClose={closeTreeContextMenu}
        onCopyPath={copyTreePath}
        onCreateDocument={createTreeDocument}
        onDeleteDocument={deleteTreeDocument}
        onMoveDocument={moveTreeDocument}
      />

      {mergeConflict ? (
        <ConflictMergeDialog
          activeLibrary={activeLibrary}
          conflict={mergeConflict}
          mutationOptions={browserMutationOptions()}
          onClose={() => setMergeConflictId(null)}
        />
      ) : null}
    </main>
  );
}

function CommandPalette({
  activeLibrary,
  documents,
  libraryControlsEnabled,
  open,
  query,
  selectedPath,
  theme,
  onClose,
  onCopyFuseMount,
  onCreate,
  onDelete,
  onDownload,
  onUploadMarkdown,
  onOpenGit,
  onOpenSettings,
  onMove,
  onOpenDocument,
  onQueryChange,
  onSearch,
  onToggleTheme,
  selectedIsMarkdown,
}: {
  activeLibrary: string;
  documents: DocumentListEntry[];
  libraryControlsEnabled: boolean;
  open: boolean;
  query: string;
  selectedPath: string;
  theme: ThemePreference;
  onClose: () => void;
  onCopyFuseMount: () => void;
  onCreate: () => void;
  onDelete: () => void;
  onDownload: () => void;
  onUploadMarkdown: () => void;
  onOpenGit: () => void;
  onOpenSettings: () => void;
  onMove: () => void;
  onOpenDocument: (path: string) => void;
  onQueryChange: (query: string) => void;
  onSearch: (query: string) => void;
  onToggleTheme: () => void;
  selectedIsMarkdown: boolean;
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
              className="min-w-0 flex-1 border-0 bg-transparent text-sm text-body outline-none placeholder:text-faint"
              placeholder="Open, search, or run a command"
              value={query}
              onValueChange={onQueryChange}
            />
            <kbd className="rounded border border-line px-1.5 py-0.5 text-xs text-muted">Esc</kbd>
          </div>
          {trimmedQuery && libraryControlsEnabled ? (
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
              {selectedIsMarkdown ? (
                <Command.Item
                  className={commandItem}
                  value="upload import markdown replace current document"
                  onSelect={() => run(onUploadMarkdown)}
                >
                  <span className="min-w-0 flex-1 truncate">Upload Markdown</span>
                  <span className="shrink-0 truncate text-xs text-muted">{selectedPath}</span>
                </Command.Item>
              ) : null}
              {libraryControlsEnabled ? (
                <Command.Item
                  className={commandItem}
                  disabled={!selectedPath}
                  value="rename move current document"
                  onSelect={() => run(onMove)}
                >
                  <span className="min-w-0 flex-1 truncate">Move current document</span>
                  {selectedPath ? <span className="shrink-0 truncate text-xs text-muted">{selectedPath}</span> : null}
                </Command.Item>
              ) : null}
              <Command.Item
                className={commandItem}
                disabled={!selectedPath}
                value="delete remove current document"
                onSelect={() => run(onDelete)}
              >
                <span className="min-w-0 flex-1 truncate">Delete current document</span>
              </Command.Item>
              {libraryControlsEnabled ? (
                <>
                  <Command.Item className={commandItem} value="sync git pull push peers" onSelect={() => run(onOpenGit)}>
                    <span className="min-w-0 flex-1 truncate">Sync with Git peer</span>
                  </Command.Item>
                  <Command.Item
                    className={commandItem}
                    value="fuse mount filesystem copy linux"
                    onSelect={() => run(onCopyFuseMount)}
                  >
                    <span className="min-w-0 flex-1 truncate">Copy FUSE mount command</span>
                    <span className="shrink-0 truncate text-xs text-muted">{activeLibrary}</span>
                  </Command.Item>
                </>
              ) : null}
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

function AddAgentDialog({
  error,
  instructions,
  loading,
  open,
  waitingForAgent,
  onClose,
  onCopied,
}: {
  error: string;
  instructions: string;
  loading: boolean;
  open: boolean;
  waitingForAgent: boolean;
  onClose: () => void;
  onCopied: () => void;
}) {
  const dialogRef = useDialogFocusTrap(open, onClose);

  if (!open) return null;

  return (
    <Dialog open>
      <div className="fixed inset-0 z-40 bg-black/20" onMouseDown={onClose} />
      <div
        aria-label="Add agent"
        aria-modal="true"
        className="fixed left-1/2 top-1/2 z-50 flex max-h-[86vh] w-[min(460px,92vw)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-md border border-line-strong bg-surface shadow-xl"
        onMouseDown={(event) => event.stopPropagation()}
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-4">
          <Bot size={16} className="text-accent" />
          <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">Add agent</h2>
          <button className={secondaryButton} onClick={onClose} type="button">
            Close
          </button>
        </div>
        <div className="min-h-0 flex-1 space-y-4 overflow-auto p-4">
          <p className="text-sm leading-6 text-body">
            Copy these instructions and paste them into your AI agent. It will join this document
            and work alongside you in real time, editing directly or leaving comments and
            suggestions for you to review.
          </p>
          {error ? (
            <p className="rounded-md border border-warn-line bg-warn-tint px-3 py-2 text-sm text-warn-ink">
              {error}
            </p>
          ) : null}
          <div className="space-y-2">
            <pre className="max-h-43 overflow-y-auto whitespace-pre-wrap break-words rounded-md border border-line bg-raised p-3 font-mono text-xs leading-5 text-muted">
              {loading ? 'Preparing instructions...' : instructions}
            </pre>
            <button
              className={`${primaryButton} w-full justify-center`}
              disabled={!instructions || loading || waitingForAgent}
              onClick={() =>
                void copyText(instructions, 'Agent instructions').then(() => onCopied())
              }
              type="button"
            >
              {waitingForAgent ? (
                <Loader2 size={14} className="animate-spin" />
              ) : (
                <Copy size={14} />
              )}
              {waitingForAgent ? 'Waiting for your agent…' : 'Copy instructions'}
            </button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}

async function copyText(text: string, promptLabel: string) {
  if (!text) return;
  try {
    if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable');
    await navigator.clipboard.writeText(text);
  } catch {
    window.prompt(promptLabel, text);
  }
}

// Asks for a name the first time the user invites an agent — the moment
// attribution starts to matter. Never a hard gate: skipping (or Escape)
// proceeds anonymously.
function NamePromptDialog({
  open,
  onSkip,
  onSubmit,
}: {
  open: boolean;
  onSkip: () => void;
  onSubmit: (name: string) => void;
}) {
  const dialogRef = useDialogFocusTrap(open, onSkip);
  const [draftName, setDraftName] = useState('');
  const name = draftName.trim();
  // Reserved: saveAuthor treats the default author as "no stored name", which
  // would bring this prompt back on the next invite.
  const submittable = Boolean(name) && name !== DEFAULT_AUTHOR;

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 bg-black/20 p-4">
      <div
        aria-label="What's your name?"
        aria-modal="true"
        className="mx-auto mt-[12vh] w-full max-w-md overflow-hidden rounded-md border border-line-strong bg-surface shadow-xl"
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="space-y-4 p-6">
          <h2 className="text-lg font-semibold text-ink">What&apos;s your name?</h2>
          <div className="grid gap-1 text-sm">
            <label className="grid gap-1">
              <span className="text-muted">Your name</span>
              <input
                aria-describedby="name-prompt-help"
                className="h-9 rounded-md border border-line bg-raised px-3 text-sm text-body outline-none focus:border-accent-line focus:ring-2 focus:ring-accent-ring"
                maxLength={120}
                onChange={(event) => setDraftName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && submittable) onSubmit(name);
                }}
                value={draftName}
              />
            </label>
            <span className="text-xs text-muted" id="name-prompt-help">
              Quarry stamps a name on your edits, comments, and suggestions so collaborators —
              including your agent — can see who did what.
            </span>
          </div>
          <div className="flex items-center gap-2">
            <button
              className={primaryButton}
              disabled={!submittable}
              onClick={() => onSubmit(name)}
              type="button"
            >
              Continue
            </button>
            <button className={ghostButton} onClick={onSkip} type="button">
              Skip for now
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function SettingsDialog({
  activeLibrary,
  author,
  librarySettingsVisible,
  layoutSettingsVisible,
  open,
  theme,
  onAuthorChange,
  onClose,
  onResetLayout,
  onThemeChange,
}: {
  activeLibrary: string;
  author: string;
  librarySettingsVisible: boolean;
  layoutSettingsVisible: boolean;
  open: boolean;
  theme: ThemePreference;
  onAuthorChange: (author: string) => void;
  onClose: () => void;
  onResetLayout: () => void;
  onThemeChange: (theme: ThemePreference) => void;
}) {
  const dialogRef = useDialogFocusTrap(open, onClose);
  const [draftAuthor, setDraftAuthor] = useState(author);

  useEffect(() => {
    if (open) setDraftAuthor(author);
  }, [author, open]);

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
          {librarySettingsVisible ? (
            <section>
              <h3 className="text-xs font-semibold uppercase text-muted">Library</h3>
              <dl className="mt-2 grid grid-cols-[120px_1fr] gap-x-3 gap-y-2 text-sm">
                <dt className="text-muted">Active library</dt>
                <dd className="min-w-0 truncate font-mono text-body">
                  {activeLibrary || 'No library selected'}
                </dd>
              </dl>
            </section>
          ) : null}

          <section>
            <h3 className="text-xs font-semibold uppercase text-muted">Identity</h3>
            <label className="mt-2 grid gap-1 text-sm">
              <span className="text-muted">Author</span>
              <input
                className="h-9 rounded-md border border-line bg-raised px-3 text-sm text-body outline-none focus:border-accent-line focus:ring-2 focus:ring-accent-ring"
                onBlur={() => onAuthorChange(draftAuthor)}
                onChange={(event) => setDraftAuthor(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') onAuthorChange(draftAuthor);
                }}
                value={draftAuthor}
              />
            </label>
          </section>

          <section>
            <h3 className="text-xs font-semibold uppercase text-muted">Appearance</h3>
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

          {layoutSettingsVisible ? (
            <section>
              <h3 className="text-xs font-semibold uppercase text-muted">Layout</h3>
              <button className={`${secondaryButton} mt-2`} onClick={onResetLayout} type="button">
                <RotateCcw size={15} />
                Reset pane sizes
              </button>
            </section>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function ConflictMergeDialog({
  activeLibrary,
  conflict,
  mutationOptions,
  onClose,
}: {
  activeLibrary: string;
  conflict: ConflictRecord;
  mutationOptions: DocumentMutationOptions;
  onClose: () => void;
}) {
  const { mutate } = useSWRConfig();
  const [manualContent, setManualContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const dialogRef = useDialogFocusTrap(true, onClose);
  const theirsPath = conflict.conflict_path ?? conflict.path;
  const conflictDocumentRef = libraryDocumentRef(activeLibrary, conflict.path);
  const { data: head } = useSWR(
    activeLibrary ? ['/v1/conflict-head', activeLibrary, conflict.path, conflict.id] : null,
    () => getDocument(conflictDocumentRef)
  );
  const { data: ours } = useSWR(
    activeLibrary && conflict.ours_version_id
      ? ['/v1/conflict-version', activeLibrary, conflict.path, conflict.ours_version_id]
      : null,
    () => documentVersion(conflictDocumentRef, requireValue(conflict.ours_version_id))
  );
  const { data: theirs } = useSWR(
    activeLibrary && conflict.theirs_version_id
      ? ['/v1/conflict-version', activeLibrary, theirsPath, conflict.theirs_version_id]
      : null,
    () =>
      documentVersion(
        libraryDocumentRef(activeLibrary, theirsPath),
        requireValue(conflict.theirs_version_id)
      )
  );

  useEffect(() => {
    setManualContent(ours?.content ?? head?.content ?? '');
  }, [head?.content, ours?.content, conflict.id]);

  async function refreshConflictState() {
    await Promise.all([
      mutate(documentRefKey('document', conflictDocumentRef)),
      mutate(['/v1/documents', activeLibrary]),
      mutate(['/v1/conflicts', activeLibrary]),
      mutate(documentRefKey('versions', conflictDocumentRef)),
      mutate(['/v1/outgoing', activeLibrary, conflict.path]),
      mutate(['/v1/backlinks', activeLibrary, conflict.path]),
    ]);
  }

  async function resolveWith(content: string) {
    if (!head?.etag) return;
    setBusy(true);
    setError('');
    try {
      await putDocument(conflictDocumentRef, content, head.etag, head.contentType, mutationOptions);
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
      await deleteDocument(conflictDocumentRef, mutationOptions);
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
  canMoveDocuments,
  collapsed,
  heading,
  libraryControlsEnabled,
  libraries,
  searchQuery,
  searchEnabled,
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
  canMoveDocuments: boolean;
  collapsed: boolean;
  heading: string;
  libraryControlsEnabled: boolean;
  libraries: LibraryType[];
  searchQuery: string;
  searchEnabled: boolean;
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
    } else if (canMoveDocuments && event.key === 'F2' && row.dataset.treeKind === 'document') {
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
      {libraryControlsEnabled ? (
        <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                aria-label="Library switcher"
                className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md border border-line-strong bg-raised pl-2.5 pr-2 text-sm font-medium text-body transition-colors hover:bg-well"
                role="combobox"
                type="button"
              >
                <Library className="shrink-0 text-muted" size={15} />
                <span className="min-w-0 flex-1 truncate text-left">{active?.slug ?? 'Select library…'}</span>
                <ChevronDown className="shrink-0 text-muted" size={14} />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                align="start"
                className="z-50 min-w-[var(--radix-dropdown-menu-trigger-width)] rounded-md border border-line bg-raised p-1 shadow-lg"
                sideOffset={6}
              >
                {libraries.map((library) => (
                  <DropdownMenu.Item
                    className={cn(menuItem, 'justify-between', library.slug === active?.slug && 'text-accent-ink')}
                    key={library.id}
                    onSelect={() => onLibraryChange(library.slug)}
                  >
                    <span className="truncate">{library.slug}</span>
                    {library.slug === active?.slug ? <Check className="shrink-0 text-accent-ink" size={15} /> : null}
                  </DropdownMenu.Item>
                ))}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
      ) : null}
      <div className="flex h-10 shrink-0 items-center justify-between pr-2 pl-3">
        <span className="text-[0.6875rem] font-semibold uppercase tracking-wider text-faint">{heading}</span>
        <div className="flex items-center gap-0.5">
          {searchEnabled ? (
            <button
              aria-expanded={searchOpen}
              aria-label="Search"
              className={cn(ghostIconButton, 'size-7', searchOpen && 'bg-well text-body')}
              onClick={toggleSearch}
              type="button"
            >
              <Search size={15} />
            </button>
          ) : null}
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
      {searchEnabled && searchOpen ? (
        <div className="px-3 pb-2">
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
        <section className="border-b border-line px-3 py-2.5">
          <div
            aria-label="Search results"
            className="space-y-0.5 outline-none"
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
      <div className="min-h-0 flex-1 px-1.5 pt-1" onKeyDown={handleTreeKeyDown}>
        <Tree<TreeNode>
          data={tree}
          height={800}
          indent={16}
          initialOpenState={treeOpenState}
          key={treeKey}
          disableDrag={(node) => !canMoveDocuments || node.kind === 'folder'}
          onMove={onMove}
          onActivate={(node) => {
            if (node.data.kind === 'folder') node.toggle();
            else onOpen(node.data.path);
          }}
          onToggle={onTreeToggle}
          openByDefault
          renderRow={DocumentTreeRow}
          rowHeight={30}
          width="100%"
        >
          {({ node, style }) => (
            <div
              className={cn(
                'group/row flex cursor-default items-center gap-1.5 truncate rounded-md pr-1.5 text-sm transition-colors hover:bg-well',
                node.data.path === selectedPath && 'bg-accent-tint text-accent-ink'
              )}
              style={{
                ...style,
                paddingLeft: (typeof style.paddingLeft === 'number' ? style.paddingLeft : 0) + 8,
              }}
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
  canMoveDocuments,
  menu,
  onClose,
  onCopyPath,
  onCreateDocument,
  onDeleteDocument,
  onMoveDocument,
}: {
  canMoveDocuments: boolean;
  menu: TreeMenuState | null;
  onClose: () => void;
  onCopyPath: (node: TreeNode) => void;
  onCreateDocument: (node: TreeNode) => void;
  onDeleteDocument: (node: TreeNode) => void;
  onMoveDocument: (node: TreeNode) => void;
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
            {canMoveDocuments ? (
              <button className={treeMenuItem} role="menuitem" type="button" onClick={() => onMoveDocument(menu.node)}>
                Move
              </button>
            ) : null}
            <button className={cn(treeMenuItem, 'text-warn-ink')} role="menuitem" type="button" onClick={() => onDeleteDocument(menu.node)}>
              Delete
            </button>
          </>
        ) : null}
      </div>
    </div>
  );
}

const editorModes: ReadonlyArray<{ value: EditorMode; label: string; icon: typeof Eye }> = [
  { value: 'editing', label: 'Editing', icon: PencilLine },
  { value: 'suggesting', label: 'Suggesting', icon: MessageSquarePlus },
  { value: 'viewing', label: 'Viewing', icon: Eye },
];

// Header save status (Phase 5 model): `Saved` means connected with the last
// checkpoint covering everything on screen; `Saving…` means a commit is
// owed; `Reconnecting (read-only)` means no live session. The settled
// "Saved" fades out after a beat; the element stays mounted so it remains a
// stable query target and the layout never jumps.
function SaveStatusIndicator({ saveState }: { saveState: CollabSaveState }) {
  const settled = saveState === 'saved';
  const [faded, setFaded] = useState(false);
  useEffect(() => {
    setFaded(false);
    if (!settled) return;
    const timer = window.setTimeout(() => setFaded(true), SAVED_STATUS_LINGER_MS);
    return () => window.clearTimeout(timer);
  }, [settled, saveState]);
  return (
    <span
      aria-label="Save status"
      className={cn(
        'inline-flex shrink-0 items-center gap-1.5 text-xs text-muted transition-opacity duration-500',
        faded && 'opacity-0'
      )}
    >
      {saveState === 'reconnecting' || saveState === 'save_failed' || saveState === 'refused' ? (
        <AlertTriangle className="shrink-0 text-warn-ink" size={14} />
      ) : null}
      {saveStateLabel(saveState)}
    </span>
  );
}

const PRESENCE_AVATAR_CAP = 3;
const presenceAvatar =
  'flex size-7 shrink-0 items-center justify-center rounded-full ring-2 ring-surface';

function agentPresenceName(entry: AgentPresenceDisplayEntry) {
  const by = entry.by?.trim();
  if (by) return by;
  const parts = entry.agentId.split(':').filter(Boolean);
  return parts.at(-1) ?? entry.agentId;
}

function presenceLabel(entry: AgentPresenceDisplayEntry) {
  return `${agentPresenceName(entry)} · ${entry.status}`;
}

function AgentPresenceAvatar({ entry }: { entry: AgentPresenceDisplayEntry }) {
  const label = presenceLabel(entry);
  return (
    <Tooltip.Root>
      <Tooltip.Trigger aria-label={label} className="flex rounded-full" type="button">
        <AgentAvatar
          className="bg-well text-muted ring-2 ring-surface"
          fallback={<Bot size={14} />}
          kind={agentKind(entry.agentId)}
        />
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content
          className="z-50 rounded-md border border-line bg-raised px-2 py-1 text-xs text-body shadow-lg"
          sideOffset={6}
        >
          {label}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}

function AgentOverflowAvatar({ entries }: { entries: AgentPresenceDisplayEntry[] }) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger
        aria-label={entries.map(presenceLabel).join(', ')}
        className={cn(presenceAvatar, 'bg-well text-xs font-medium text-muted')}
        type="button"
      >
        +{entries.length}
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content
          className="z-50 flex flex-col gap-0.5 rounded-md border border-line bg-raised px-2 py-1 text-xs text-body shadow-lg"
          sideOffset={6}
        >
          {entries.map((entry) => (
            <span key={entry.agentId}>{presenceLabel(entry)}</span>
          ))}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}

function AgentPresencePill({ presence }: { presence: AgentPresenceDisplayEntry[] }) {
  if (!presence.length) return null;
  const shown = presence.slice(0, PRESENCE_AVATAR_CAP);
  const overflow = presence.slice(PRESENCE_AVATAR_CAP);
  return (
    <Tooltip.Provider delayDuration={200}>
      <div aria-label="Agent presence" className="flex shrink-0 -space-x-2">
        {shown.map((entry) => (
          <AgentPresenceAvatar entry={entry} key={entry.agentId} />
        ))}
        {overflow.length ? <AgentOverflowAvatar entries={overflow} /> : null}
      </div>
    </Tooltip.Provider>
  );
}

// Viewing/Editing/Suggesting selector in the document header (à la Google Docs).
// The mode is plain React state; the editor reacts to it, so this control needs
// no editor context.
function DocumentModeSelect({
  mode,
  onModeChange,
}: {
  mode: EditorMode;
  onModeChange: (mode: EditorMode) => void;
}) {
  const active = editorModes.find((option) => option.value === mode) ?? editorModes[0];
  const ActiveIcon = active.icon;
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label="Document mode"
          className="inline-flex h-8 items-center gap-1.5 rounded-md border border-line-strong bg-raised pl-2.5 pr-2 text-sm text-body transition-colors hover:bg-well"
          type="button"
        >
          <ActiveIcon className="text-muted" size={15} />
          {active.label}
          <ChevronDown className="text-muted" size={14} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          className="z-50 min-w-44 rounded-md border border-line bg-raised p-1 shadow-lg"
          sideOffset={6}
        >
          {editorModes.map((option) => (
            <DropdownMenu.Item
              className={cn(menuItem, 'justify-between', option.value === mode && 'text-accent-ink')}
              key={option.value}
              onSelect={() => onModeChange(option.value)}
            >
              <span className="flex items-center gap-2">
                <option.icon className="shrink-0 text-muted" size={15} />
                {option.label}
              </span>
              {option.value === mode ? <Check className="shrink-0 text-accent-ink" size={15} /> : null}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function DocumentToolbar({
  agentPresence,
  canPromote,
  isMarkdown,
  isText,
  isTmp,
  mode,
  onModeChange,
  path,
  saveState,
  onAddAgent,
  onDelete,
  onCopyRawLink,
  onDownload,
  onPromote,
  onRename,
  onUploadMarkdown,
}: {
  agentPresence: AgentPresenceDisplayEntry[];
  canPromote: boolean;
  isMarkdown: boolean;
  isText: boolean;
  isTmp: boolean;
  mode: EditorMode;
  onModeChange: (mode: EditorMode) => void;
  path: string;
  saveState: CollabSaveState | null;
  onAddAgent: () => void;
  onDelete: () => void;
  onCopyRawLink: () => void;
  onDownload: () => void;
  onPromote: () => void;
  onRename: () => void;
  onUploadMarkdown: () => void;
}) {
  return (
    <div className="flex h-12 shrink-0 items-center gap-2 bg-surface px-3">
      {!isTmp ? (
        <h1 className="min-w-0 flex-1 truncate text-sm">
          {path.split('/').map((segment, index, segments) => (
            <span key={index}>
              {index > 0 ? <span className="px-1.5 text-faint">/</span> : null}
              <span className={index === segments.length - 1 ? 'font-semibold text-ink' : 'text-muted'}>
                {segment}
              </span>
            </span>
          ))}
        </h1>
      ) : (
        <div className="min-w-0 flex-1" />
      )}
      {isMarkdown && saveState ? <SaveStatusIndicator saveState={saveState} /> : null}
      {isMarkdown ? <AgentPresencePill presence={agentPresence} /> : null}
      {isMarkdown ? (
        <button
          aria-label="Add agent"
          className={cn(secondaryButton, 'px-2 sm:px-3')}
          onClick={onAddAgent}
          type="button"
        >
          <Bot size={15} />
          <span className="hidden sm:inline">Add agent</span>
        </button>
      ) : null}
      {isMarkdown ? <DocumentModeSelect mode={mode} onModeChange={onModeChange} /> : null}
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
            <DropdownMenu.Item className={menuItem} onSelect={onCopyRawLink}>
              <Copy className="shrink-0" size={15} />
              Copy raw link
            </DropdownMenu.Item>
            {isText ? (
              <DropdownMenu.Item className={menuItem} onSelect={onDownload}>
                <Download className="shrink-0" size={15} />
                Download as Markdown
              </DropdownMenu.Item>
            ) : null}
            {isMarkdown ? (
              <DropdownMenu.Item className={menuItem} onSelect={onUploadMarkdown}>
                <Upload className="shrink-0" size={15} />
                Upload Markdown
              </DropdownMenu.Item>
            ) : null}
            {isTmp ? (
              <>
                {canPromote ? (
                  <DropdownMenu.Item className={menuItem} onSelect={onPromote}>
                    <FolderInput className="shrink-0" size={15} />
                    Promote…
                  </DropdownMenu.Item>
                ) : null}
              </>
            ) : (
              <DropdownMenu.Item className={menuItem} onSelect={onRename}>
                <FolderInput className="shrink-0" size={15} />
                Move…
              </DropdownMenu.Item>
            )}
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
  libraryControlsEnabled,
  compareVersionId,
  conflicts,
  currentDiffOpen,
  currentEditorDiff,
  incoming,
  onCompareVersionChange,
  onCreateDocumentFromLink,
  onDiffCurrent,
  onOpenDocument,
  onOpenConflict,
  onResolveConflict,
  onDismissConflict,
  onRestoreVersion,
  onToggleCollapsed,
  onViewVersion,
  outgoing,
  review,
  reviewEnabled,
  selectedVersionContent,
  selectedVersionDiff,
  selectedVersionId,
  onTabChange,
  versions,
}: {
  activeTab: RightPaneTab;
  activeLibrary: string;
  collapsed: boolean;
  libraryControlsEnabled: boolean;
  compareVersionId: string | null;
  conflicts: ConflictRecord[];
  currentDiffOpen: boolean;
  currentEditorDiff: string;
  incoming: DocumentLink[];
  onCompareVersionChange: (version: string | null) => void;
  onCreateDocumentFromLink: (link: DocumentLink) => void;
  onDiffCurrent: () => void;
  onOpenDocument: (path: string) => void;
  onOpenConflict: (conflict: string) => void;
  onResolveConflict: (conflict: string) => void;
  onDismissConflict: (conflictId: string) => Promise<void>;
  onRestoreVersion: (version: string) => void;
  onToggleCollapsed: () => void;
  onViewVersion: (version: string) => void;
  outgoing: DocumentLink[];
  review?: AgentReviewResponse;
  reviewEnabled: boolean;
  selectedVersionContent?: DocumentVersionContent;
  selectedVersionDiff?: VersionDiff;
  selectedVersionId: string | null;
  onTabChange: (tab: RightPaneTab) => void;
  versions: DocumentHistoryEntry[];
}) {
  // Per-tab gating: the review record and version history travel with every
  // document in either scope; links stay a library-scope feature.
  const visibleTabs = rightPaneTabs.filter((tab) =>
    tab.key === 'links' ? libraryControlsEnabled : tab.key === 'comments' ? reviewEnabled : true
  );
  const selectedTab = visibleTabs.some((tab) => tab.key === activeTab) ? activeTab : visibleTabs[0]?.key ?? 'versions';
  const selectedTabLabel = visibleTabs.find((tab) => tab.key === selectedTab)?.label ?? 'Versions';
  const openConflictCount = (review?.conflicts ?? []).filter(
    (conflict) => conflict.status === 'open'
  ).length;

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
          {visibleTabs.map((tab) => (
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
            {tab.key === 'comments' && openConflictCount > 0 ? (
              <span
                className="ml-1.5 inline-flex min-w-4 items-center justify-center rounded bg-warn-tint px-1 py-0.5 text-[0.625rem] font-semibold leading-none text-warn-ink"
                data-testid="comments-tab-badge"
              >
                {openConflictCount}
              </span>
            ) : null}
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
        {libraryControlsEnabled && selectedTab === 'links' ? (
          <>
            <h2 className={rightHeading}>
              <Link2 size={14} />
              Outgoing
            </h2>
            <LinkList
              activeLibrary={activeLibrary}
              direction="outgoing"
              links={outgoing.filter(isDocumentLink)}
              onCreateDocument={onCreateDocumentFromLink}
              onOpenDocument={onOpenDocument}
            />
            <h2 className={cn(rightHeading, 'mt-6')}>Backlinks</h2>
            <LinkList
              activeLibrary={activeLibrary}
              direction="incoming"
              links={incoming.filter(isDocumentLink)}
              onOpenDocument={onOpenDocument}
            />
          </>
        ) : null}
        {selectedTab === 'versions' ? (
          <>
            {libraryControlsEnabled ? (
              <>
                <h2 className={rightHeading}>Conflicts</h2>
                <ConflictList conflicts={conflicts} onOpen={onOpenConflict} onResolve={onResolveConflict} />
                <h2 className={cn(rightHeading, 'mt-6')}>Versions</h2>
              </>
            ) : (
              <h2 className={rightHeading}>Versions</h2>
            )}
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
        {selectedTab === 'comments' ? (
          <>
            <h2 className={rightHeading}>{selectedTabLabel}</h2>
            <CommentsPanel onDismissConflict={onDismissConflict} review={review} />
          </>
        ) : null}
      </section>
    </aside>
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

function VersionList({
  versions,
  selectedVersionId,
  onView,
  onRestore,
}: {
  versions: DocumentHistoryEntry[];
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
              selectedVersionId === version.latest_version_id && 'outline outline-1 outline-accent-ring'
            )}
            key={version.id}
          >
            <span className="min-w-0 flex-1 space-y-0.5">
              <span className="block truncate">
                <span className="font-mono">{version.latest_version_id.slice(0, 8)}</span>{' '}
                {versionHistoryTitle(version)}
              </span>
              <span className="block truncate text-muted">
                {version.content_type} · {formatBytes(version.byte_size)} · {versionTransactionLabel(version)}
              </span>
              {metadataSummary ? <span className="block truncate text-muted">{metadataSummary}</span> : null}
            </span>
            <button
              aria-label={`View version ${version.latest_version_id}`}
              className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-line text-body hover:bg-well"
              onClick={() => onView(version.latest_version_id)}
              type="button"
            >
              <Eye size={13} />
            </button>
            <button
              aria-label={`Restore version ${version.latest_version_id}`}
              className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-line text-body hover:bg-well"
              onClick={() => onRestore(version.latest_version_id)}
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

function versionHistoryTitle(version: DocumentHistoryEntry) {
  if (isGroupedAutosave(version)) {
    return `Autosaved edits · ${historyTimeRange(version)} · ${version.raw_version_count} revisions`;
  }
  return version.updated_at;
}

function historyEntryFromVersion(version: DocumentVersion): DocumentHistoryEntry {
  return {
    id: version.id,
    document_id: version.document_id,
    latest_version_id: version.id,
    earliest_version_id: version.id,
    raw_version_count: 1,
    source: version.transaction_source,
    actor: version.transaction_actor,
    message: version.transaction_message,
    provenance: version.transaction_provenance,
    checkpoint_reason: historyCheckpointReason(version.transaction_provenance),
    content_type: version.content_type,
    byte_size: version.byte_size,
    created_at: version.created_at,
    updated_at: version.created_at,
  };
}

function historyCheckpointReason(provenance: Record<string, unknown> | null) {
  const history = provenance?.history;
  const reason = history && typeof history === 'object' ? (history as { reason?: unknown }).reason : undefined;
  return typeof reason === 'string' ? reason : null;
}

function versionTransactionLabel(version: DocumentHistoryEntry) {
  const source = version.source;
  return source ? `Source ${source}` : 'History entry';
}

function versionMetadataSummary(version: DocumentHistoryEntry) {
  const transactionRows = [
    { key: 'message', value: version.message },
    { key: 'actor', value: version.actor },
    { key: 'provenance', value: version.provenance },
  ]
    .map(({ key, value }) => ({ key, value: formatMetadataValue(value) }))
    .filter((row): row is { key: string; value: string } => Boolean(row.value));
  if (transactionRows.length) {
    return transactionRows.map((row) => `${metadataLabel(row.key)} ${row.value}`).join(' · ');
  }

  return '';
}

function isGroupedAutosave(version: DocumentHistoryEntry) {
  return (
    version.raw_version_count > 1 &&
    version.provenance &&
    typeof version.provenance === 'object' &&
    (version.provenance.history as { kind?: unknown } | undefined)?.kind === 'autosave'
  );
}

function historyTimeRange(version: DocumentHistoryEntry) {
  const start = formatHistoryTime(version.created_at);
  const end = formatHistoryTime(version.updated_at);
  return start === end ? start : `${start}-${end}`;
}

function formatHistoryTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
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
  versions: DocumentHistoryEntry[];
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
              .filter((version) => version.latest_version_id !== selectedVersionId)
              .map((version) => (
                <option key={version.id} value={version.latest_version_id}>
                  {version.latest_version_id.slice(0, 8)} {versionHistoryTitle(version)}
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
    () => getDocument(libraryDocumentRef(activeLibrary, requireValue(previewPath)))
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

// The Links sidebar shows only links that reference a library document — resolved
// (target exists) or broken (target intended but missing). It excludes headings and
// tags, and links with no document destination (external URLs, same-doc `#fragments`),
// which the backend marks with resolution_status 'external'.
function isDocumentLink(link: DocumentLink) {
  return (
    (link.target_kind === 'wiki_link' ||
      link.target_kind === 'embed' ||
      link.target_kind === 'markdown_link') &&
    link.resolution_status !== 'external'
  );
}

function EmptyDocument({
  treeHidden,
  onCreate,
  onUploadFile,
}: {
  treeHidden: boolean;
  onCreate: () => void;
  onUploadFile: (file: File | undefined) => void;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 bg-surface px-6 text-center">
      <div className="flex size-12 items-center justify-center rounded-xl bg-well text-faint">
        <FileText size={22} />
      </div>
      {treeHidden ? (
        <>
          <div>
            <p className="text-sm font-medium text-body">Start a document</p>
            <p className="mt-1 text-sm text-muted">
              Live, versioned, and shareable by URL — no account needed.
            </p>
          </div>
          <div className="flex flex-wrap items-center justify-center gap-2">
            <button className={primaryButton} onClick={onCreate} type="button">
              <FilePlus2 size={15} />
              New document
            </button>
            <label className={cn(secondaryButton, 'cursor-pointer')}>
              <Upload size={15} />
              Upload Markdown
              <input
                accept=".md,.markdown,text/markdown,text/x-markdown"
                className="sr-only"
                type="file"
                onChange={(event) => {
                  onUploadFile(event.currentTarget.files?.[0]);
                  event.currentTarget.value = '';
                }}
              />
            </label>
          </div>
          <p className="text-xs text-muted">
            Press{' '}
            <kbd className="rounded border border-line-strong bg-raised px-1.5 py-0.5 font-mono text-xs text-body">
              ⌘K
            </kbd>{' '}
            for the command palette.
          </p>
        </>
      ) : (
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
      )}
    </div>
  );
}

function LoadingDocument() {
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-surface" aria-label="Document loading">
      <div className="px-8 py-7 text-sm text-muted">Loading document...</div>
    </section>
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
  return typeof title === 'string' && title.trim() ? title : documentBasename(entry.path);
}

function documentBasename(path: string) {
  return path.split('/').at(-1) ?? path;
}

function extractFirstH1(content: string): string | null {
  const lines = content.split('\n');
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith('# ')) {
      return trimmed.slice(2).trim();
    }
  }
  return null;
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

// Mirrors the server's BlockDocument classification (`document_kind`):
// .md/.markdown paths or a markdown content type edit through live sessions;
// everything else stays on the raw byte path.
function isMarkdownDocument(path: string, contentType: string) {
  const normalized = contentType.split(';', 1)[0]?.trim().toLowerCase() ?? '';
  if (normalized === 'text/markdown' || normalized === 'text/x-markdown') return true;
  const lower = path.toLowerCase();
  return lower.endsWith('.md') || lower.endsWith('.markdown');
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

function formatShortDateTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
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

function makeCollabSessionId() {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return `browser:${crypto.randomUUID()}`;
  }
  return `browser:${Date.now().toString(36)}:${Math.random().toString(36).slice(2)}`;
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
// Bordered field wrapper that delegates focus styling to a single focus-within
// ring, so the inner input/select can safely use `outline-none`.
const filterField =
  'flex h-8 items-center gap-1 rounded-md border border-line-strong bg-raised px-2 text-xs text-muted transition-colors focus-within:border-accent focus-within:ring-2 focus-within:ring-accent-tint';
const commandItem =
  'flex min-h-9 cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm text-body outline-none aria-disabled:cursor-not-allowed aria-disabled:opacity-45 aria-selected:bg-accent-tint';
const treeMenuItem =
  'block w-full px-3 py-1.5 text-left text-sm text-body hover:bg-accent-tint focus:bg-accent-tint focus:outline-none';
const menuItem =
  'flex w-full cursor-pointer items-center gap-2 rounded px-2.5 py-1.5 text-left text-sm text-body outline-none select-none data-highlighted:bg-well';
const rightHeading = 'mb-2.5 flex items-center gap-2 text-[0.6875rem] font-semibold uppercase tracking-wider text-faint';
// Narrows an SWR fetcher's captured value: the fetcher only runs when the key
// is non-null, but TypeScript cannot see that guard from inside the closure.
function requireValue<T>(value: T | null | undefined): T {
  if (value === null || value === undefined) throw new Error('value required by SWR key guard');
  return value;
}

const rightPaneTabs: Array<{ key: RightPaneTab; label: string }> = [
  { key: 'comments', label: 'Comments' },
  { key: 'links', label: 'Links' },
  { key: 'versions', label: 'Versions' },
];
