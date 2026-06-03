import { AutoformatPlugin, type AutoformatRule } from '@platejs/autoformat';
import {
  BlockquotePlugin,
  BoldPlugin,
  CodePlugin,
  H1Plugin,
  H2Plugin,
  H3Plugin,
  H4Plugin,
  H5Plugin,
  H6Plugin,
  ItalicPlugin,
  StrikethroughPlugin,
  SubscriptPlugin,
  SuperscriptPlugin,
  UnderlinePlugin,
} from '@platejs/basic-nodes/react';
import { CodeBlockPlugin, CodeLinePlugin, CodeSyntaxPlugin } from '@platejs/code-block/react';
import { insertEmptyCodeBlock, toggleCodeBlock } from '@platejs/code-block';
import { DndPlugin, useDraggable, useDropLine } from '@platejs/dnd';
import { DndProvider } from 'react-dnd';
import { HTML5Backend } from 'react-dnd-html5-backend';
import { getLinkAttributes } from '@platejs/link';
import {
  FloatingLinkUrlInput,
  LinkPlugin,
  useFloatingLinkEdit,
  useFloatingLinkEditState,
  useFloatingLinkInsert,
  useFloatingLinkInsertState,
  useLinkToolbarButton,
  useLinkToolbarButtonState,
  type LinkFloatingToolbarState,
} from '@platejs/link/react';
import {
  ListPlugin,
  useIndentTodoToolBarButton,
  useIndentTodoToolBarButtonState,
  useListToolbarButton,
  useListToolbarButtonState,
  useTodoListElement,
  useTodoListElementState,
} from '@platejs/list/react';
import { isOrderedList, toggleList } from '@platejs/list';
import { MarkdownPlugin } from '@platejs/markdown';
import { YjsPlugin } from '@platejs/yjs/react';
import {
  flip,
  offset,
  shift,
  useFloatingToolbar,
  useFloatingToolbarState,
  type UseVirtualFloatingOptions,
} from '@platejs/floating';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import {
  Bold,
  Check,
  ChevronDown,
  ChevronRight,
  Code,
  Copy,
  Heading1,
  Heading2,
  Heading3,
  GripVertical,
  Heading4,
  Heading5,
  Heading6,
  ExternalLink,
  Italic,
  Link,
  List,
  ListOrdered,
  ListTodo,
  MessageSquarePlus,
  Pilcrow,
  Quote,
  SquareCode,
  Strikethrough,
  Subscript,
  Superscript,
  Table,
  Trash2,
  Type,
  Underline,
  Unlink,
  Workflow,
  X,
} from 'lucide-react';
import {
  ElementApi,
  KEYS,
  NodeApi,
  PathApi,
  TrailingBlockPlugin,
  type Descendant,
  type TCodeBlockElement,
  type TElement,
  type TLinkElement,
  type TListElement,
} from 'platejs';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import remarkGfm from 'remark-gfm';
import {
  ParagraphPlugin,
  Plate,
  PlateContainer,
  PlateContent,
  PlateElement,
  useEditorRef,
  useEditorSelection,
  useEventEditorValue,
  useEditorSelector,
  useFormInputProps,
  useMarkToolbarButton,
  useMarkToolbarButtonState,
  usePlateEditor,
  useReadOnly,
  useSelectionFragmentProp,
  type PlateEditor,
  type PlateElementProps,
  type RenderNodeWrapper,
} from 'platejs/react';

import { SuggestionPlugin } from '@platejs/suggestion/react';

import { cn } from '../../lib/utils';
import { type PlateValue } from './markdown-codec';
import { remarkInlineMarks } from './remark-inline-marks';
import { reviewKit } from './review-kit';
import { ImageKit, ImageProvider, type ImageApi } from './image-element';
import { mermaidMdRules, MERMAID_KEY } from './mermaid';
import { MermaidPlugin } from './mermaid-block';
import { tableMdRules, turnIntoTable } from './table';
import { TableKit } from './table-element';
import { wikiLinkMdRules } from './wiki-link';
import { WikiLinkPlugin, WikiLinkProvider, type WikiLinkApi } from './wiki-link-element';
import { startCommentDraft } from '../review/comment-draft';
import { currentAuthor } from '../review/identity';
import { markdownToReview, reviewToMarkdown } from '../review/rfm-codec';
import { syncSuggestionsFromValue, useReviewStore } from '../review/review-store';
import { acceptSuggestionById, rejectSuggestionById } from '../review/accept-reject';
import { ReviewRail } from '../review/ui/ReviewRail';
import { RemoteCursorOverlay } from '../collab/RemoteCursorOverlay';
import {
  clearCollabAwareness,
  collectFlushAcks,
  collectRecoveryErrors,
  type CollabFlushAck,
  type CollabRecoveryError,
  updateCollabAwareness,
} from '../collab/flusher-lease';
import { RUST_WS_PROVIDER_TYPE, registerRustWsProviderType } from '../collab/rust-ws-provider';

registerRustWsProviderType();

// Notion-style markdown shortcuts: typing the markdown prefix at the start of a
// block (or wrapping marks) auto-converts it. Scoped to the surface Quarry
// supports so everything round-trips through the markdown codec.
const autoformatRules: AutoformatRule[] = [
  { match: '# ', mode: 'block', type: KEYS.h1 },
  { match: '## ', mode: 'block', type: KEYS.h2 },
  { match: '### ', mode: 'block', type: KEYS.h3 },
  { match: '#### ', mode: 'block', type: KEYS.h4 },
  { match: '##### ', mode: 'block', type: KEYS.h5 },
  { match: '###### ', mode: 'block', type: KEYS.h6 },
  { match: '> ', mode: 'block', type: KEYS.blockquote },
  {
    match: '```',
    mode: 'block',
    type: KEYS.codeBlock,
    format: (editor) => {
      insertEmptyCodeBlock(editor, {
        defaultType: KEYS.p,
        insertNodesOptions: { select: true },
      });
    },
  },
  {
    match: ['* ', '- '],
    mode: 'block',
    type: 'list',
    format: (editor) => {
      toggleList(editor, { listStyleType: KEYS.ul });
    },
  },
  {
    match: [String.raw`^\d+\.$ `, String.raw`^\d+\)$ `],
    matchByRegex: true,
    mode: 'block',
    type: 'list',
    format: (editor, { matchString }) => {
      toggleList(editor, {
        listRestartPolite: Number(matchString) || 1,
        listStyleType: KEYS.ol,
      });
    },
  },
  {
    // Notion-style `[]` and GitHub-style `[ ]` (with the space inside).
    match: ['[] ', '[ ] '],
    mode: 'block',
    type: 'list',
    format: (editor) => {
      toggleList(editor, { listStyleType: KEYS.listTodo });
      editor.tf.setNodes({ checked: false, listStyleType: KEYS.listTodo });
    },
  },
  {
    match: ['[x] ', '[X] '],
    mode: 'block',
    type: 'list',
    format: (editor) => {
      toggleList(editor, { listStyleType: KEYS.listTodo });
      editor.tf.setNodes({ checked: true, listStyleType: KEYS.listTodo });
    },
  },
  { match: '***', mode: 'mark', type: [KEYS.bold, KEYS.italic] },
  { match: '**', mode: 'mark', type: KEYS.bold },
  { match: '*', mode: 'mark', type: KEYS.italic },
  { match: '_', mode: 'mark', type: KEYS.italic },
  { match: '~~', mode: 'mark', type: KEYS.strikethrough },
  { match: '`', mode: 'mark', type: KEYS.code },
];

// Renders the list marker for an indent-list item: native disc/decimal markers
// for bullet/numbered lists, and an interactive checkbox for to-do items.
const BlockList: RenderNodeWrapper = (props) => {
  if (!props.element.listStyleType) return undefined;
  return (childProps) => <ListItemElement {...childProps} />;
};

// WebKit/Safari won't run a native HTML5 drag for a draggable element inside a
// contentEditable region (it fires dragstart then immediately dragend, with no
// dragover/drop), so block dragging can't work there. Hide the handle rather
// than show a dead affordance. `navigator.vendor` is "Apple Computer, Inc." in
// Safari/WebKit and "Google Inc."/"" in Chrome/Firefox.
const supportsBlockDrag =
  typeof navigator !== 'undefined' && !/apple/i.test(navigator.vendor);

// Notion-style drag handle for reordering top-level blocks (Chrome/Firefox).
const BlockDraggable: RenderNodeWrapper = (props) => {
  if (!supportsBlockDrag) return undefined;
  if (props.editor.dom.readOnly) return undefined;
  if (props.path.length !== 1) return undefined;
  return (childProps) => <DraggableBlock {...childProps} />;
};

const plateMarkdownPlugins = [
  ParagraphPlugin,
  // Always keep an editable paragraph at the end, so there's a line to type on
  // below the last block — even an atomic void like a Mermaid diagram or image,
  // which would otherwise leave the document with no place to continue writing.
  // The trailing paragraph is stripped on serialize (stripTrailingEmptyParagraphs).
  TrailingBlockPlugin,
  H1Plugin,
  H2Plugin,
  H3Plugin,
  H4Plugin,
  H5Plugin,
  H6Plugin,
  BlockquotePlugin,
  CodeBlockPlugin.withComponent(CodeBlockElement),
  CodeLinePlugin,
  CodeSyntaxPlugin,
  BoldPlugin,
  ItalicPlugin,
  CodePlugin,
  StrikethroughPlugin,
  UnderlinePlugin,
  SubscriptPlugin,
  SuperscriptPlugin,
  ListPlugin.configure({ render: { belowNodes: BlockList } }),
  LinkPlugin.configure({
    render: { node: LinkElement, afterEditable: () => <LinkFloatingToolbar /> },
  }),
  WikiLinkPlugin,
  MermaidPlugin,
  ...TableKit,
  ...ImageKit,
  DndPlugin.configure({
    render: { aboveNodes: BlockDraggable, aboveSlate: EditorDndProvider },
  }),
  AutoformatPlugin.configure({
    options: {
      enableUndoOnDelete: true,
      rules: autoformatRules.map((rule) => ({
        ...rule,
        query: (editor) =>
          !editor.api.some({ match: { type: editor.getType(KEYS.codeBlock) } }),
      })),
    },
  }),
  ...reviewKit,
  MarkdownPlugin.configure({
    options: { remarkPlugins: [remarkGfm, remarkInlineMarks], rules: { ...wikiLinkMdRules, ...mermaidMdRules, ...tableMdRules } },
  }),
] as const;

// The document interaction mode chosen from the header selector. Viewing is
// read-only; Editing edits directly; Suggesting tracks edits as suggestion marks.
export type EditorMode = 'editing' | 'suggesting' | 'viewing';

export interface CollabEditorConfig {
  documentId: string;
  flushAck?: CollabFlushAck | null;
  onFlushAck?: (ack: CollabFlushAck) => void;
  onFlusherChange?: (isFlusher: boolean) => void;
  onRecoveryError?: (error: CollabRecoveryError) => void;
  sessionId: string;
  token?: string;
}

export function PlateMarkdownEditor({
  author = currentAuthor(),
  collab,
  content,
  mode = 'editing',
  wikiLink,
  image,
  onChange,
}: {
  author?: string;
  collab?: CollabEditorConfig;
  content: string;
  mode?: EditorMode;
  wikiLink?: WikiLinkApi;
  image?: ImageApi;
  onChange: (content: string) => void;
}) {
  const storeHydrate = useReviewStore((s) => s.hydrate);
  const storeGetMeta = useReviewStore((s) => s.getMeta);
  const collabEnabled = Boolean(collab?.documentId);
  const collabDocumentId = collab?.documentId ?? '';

  // The review codec serializes both the value (inline CriticMarkup) and the
  // store's metadata (YAML endmatter). `syncSuggestionsFromValue` mirrors any
  // suggestion marks Plate created (via withSuggestion) into the metadata so
  // they survive the round-trip. Shared by every save path.
  const serialize = useCallback(
    (value: PlateValue): string =>
      reviewToMarkdown(value as never, syncSuggestionsFromValue(storeGetMeta(), value as never)),
    [storeGetMeta]
  );

  const initialValueRef = useRef<PlateValue | null>(null);
  if (!initialValueRef.current) {
    const { value, meta } = markdownToReview(content);
    initialValueRef.current = value as PlateValue;
    storeHydrate(meta);
  }
  const lastContentRef = useRef(content);
  const lastSerializedRef = useRef(serialize(initialValueRef.current));
  const editorPlugins = useMemo(() => {
    if (!collabEnabled || !collab) return plateMarkdownPlugins;
    return [
      ...plateMarkdownPlugins,
      YjsPlugin.configure({
        render: {
          afterEditable: RemoteCursorOverlay,
        },
        options: {
          cursors: {
            data: {
              color: collabColor(collab.sessionId),
              name: author,
            },
          },
          providers: [
            {
              options: {
                roomName: collab.documentId,
                token: collab.token,
              },
              type: RUST_WS_PROVIDER_TYPE,
            } as never,
          ],
          userId: collab.sessionId,
        },
      }),
    ] as const;
  }, [author, collab, collabEnabled]);
  const editor = usePlateEditor(
    {
      plugins: editorPlugins as never,
      skipInitialization: collabEnabled,
      value: collabEnabled ? undefined : (initialValueRef.current as never),
    },
    [collabDocumentId]
  );

  // Set the suggesting author before any suggesting can happen; withSuggestion
  // normalizes away suggestion marks that lack a currentUserId.
  useEffect(() => {
    editor.setOption(SuggestionPlugin, 'currentUserId', author);
  }, [author, editor]);

  // The mode selector is the single source of truth for Suggesting: only that
  // mode tracks edits as suggestion marks (via withSuggestion).
  useEffect(() => {
    editor.setOption(SuggestionPlugin, 'isSuggesting', mode === 'suggesting');
  }, [editor, mode]);

  useEffect(() => {
    if (!collabEnabled || !collab) return;
    const { value, meta } = markdownToReview(content);
    lastContentRef.current = content;
    lastSerializedRef.current = reviewToMarkdown(value as never, meta);
    storeHydrate(meta);

    let disposed = false;
    void editor
      .getApi(YjsPlugin)
      .yjs.init({
        autoConnect: false,
        autoSelect: 'end',
        id: collab.documentId,
        value: value as never,
        onReady: () => {
          if (!disposed) {
            editor.getApi(YjsPlugin).yjs.connect(RUST_WS_PROVIDER_TYPE);
          }
        },
      })
      .catch((error: unknown) => {
        console.warn('[collab] failed to initialize Yjs editor', error);
      });

    return () => {
      disposed = true;
      editor.getApi(YjsPlugin).yjs.destroy();
    };
  }, [collabDocumentId, collabEnabled, editor, storeHydrate]);

  useEffect(() => {
    if (collabEnabled) return;
    if (content === lastContentRef.current) return;
    const { value, meta } = markdownToReview(content);
    resetPlateEditor(editor, value as PlateValue);
    // Reseed the refs BEFORE hydrating: `storeHydrate` is a Zustand `set` that
    // synchronously runs the store subscription, which serializes the new
    // editor value and compares it to `lastSerializedRef`. Reseeding first lets
    // that notification short-circuit on the equality guard, so a pure document
    // load doesn't spuriously fire `onChange` and mark the doc dirty.
    lastContentRef.current = content;
    // Reseed from the INCOMING doc's freshly-parsed `meta` — NOT the shared
    // `serialize`, which reads `storeGetMeta()` (still the OUTGOING doc's meta
    // until `storeHydrate` runs on the next line). The store subscription fires
    // synchronously inside `storeHydrate` with the new meta; the baseline must
    // match that, or a pure load spuriously fires onChange.
    lastSerializedRef.current = reviewToMarkdown(value as never, meta);
    storeHydrate(meta);
  }, [collabEnabled, content, editor, storeHydrate]);

  // Replies/resolves and synced suggestions live in the store, not the editor
  // value, so an editor-value change won't fire. Save on store changes too.
  // The review store is a module-global singleton; safe because Quarry mounts
  // exactly one editor at a time (this subscription assumes a single editor).
  useEffect(() => {
    return useReviewStore.subscribe(() => {
      const md = serialize(editor.children as PlateValue);
      if (md === lastSerializedRef.current) return;
      lastContentRef.current = md;
      lastSerializedRef.current = md;
      onChange(md);
    });
  }, [editor, onChange, serialize]);

  // Viewing is the only read-only mode; autosave never freezes the surface.
  const readOnly = mode === 'viewing';

  return (
    <WikiLinkProvider value={wikiLink ?? {}}>
     <ImageProvider value={image ?? {}}>
      <Plate
        editor={editor}
        readOnly={readOnly}
        onValueChange={({ editor, value }) => {
          if (editor.meta.resetting) {
            editor.meta.resetting = undefined;
            return;
          }
          const nextMarkdown = serialize(value as PlateValue);
          if (nextMarkdown === lastSerializedRef.current) return;
          lastContentRef.current = nextMarkdown;
          lastSerializedRef.current = nextMarkdown;
          onChange(nextMarkdown);
        }}
      >
        {collabEnabled && collab ? <CollabAwarenessBridge collab={collab} /> : null}
        {readOnly ? null : <FloatingFormatToolbar />}
        <PlateContainer className="relative flex h-full min-h-0">
          <div className="relative min-w-0 flex-1 overflow-auto">
            <PlateContent
              aria-label="Plate markdown editor"
              className="min-h-full w-full px-[max(2rem,calc((100%-68ch)/2))] pt-16 pb-8 text-[15px] leading-7 text-ink outline-none [&_[data-slate-placeholder=true]]:text-faint"
              disabled={readOnly}
              placeholder="Write markdown…"
              spellCheck={false}
            />
          </div>
          <ReviewRail editor={editor} />
        </PlateContainer>
      </Plate>
     </ImageProvider>
    </WikiLinkProvider>
  );
}

function CollabAwarenessBridge({ collab }: { collab: CollabEditorConfig }) {
  const editor = useEditorRef();
  const flushAckRef = useRef<CollabFlushAck | null>(collab.flushAck ?? null);
  const callbacksRef = useRef({
    onFlushAck: collab.onFlushAck,
    onFlusherChange: collab.onFlusherChange,
    onRecoveryError: collab.onRecoveryError,
  });

  useEffect(() => {
    flushAckRef.current = collab.flushAck ?? null;
    callbacksRef.current = {
      onFlushAck: collab.onFlushAck,
      onFlusherChange: collab.onFlusherChange,
      onRecoveryError: collab.onRecoveryError,
    };
    const awareness = editor.getOption(YjsPlugin, 'awareness');
    if (awareness) {
      const isFlusher = updateCollabAwareness(awareness, collab.sessionId, flushAckRef.current);
      callbacksRef.current.onFlusherChange?.(isFlusher);
    }
  }, [
    collab.flushAck,
    collab.onFlushAck,
    collab.onFlusherChange,
    collab.onRecoveryError,
    collab.sessionId,
    editor,
  ]);

  useEffect(() => {
    const awareness = editor.getOption(YjsPlugin, 'awareness');
    if (!awareness) return;
    let disposed = false;

    const publish = () => {
      if (disposed) return;
      const isFlusher = updateCollabAwareness(awareness, collab.sessionId, flushAckRef.current);
      callbacksRef.current.onFlusherChange?.(isFlusher);
      for (const ack of collectFlushAcks(awareness)) {
        callbacksRef.current.onFlushAck?.(ack);
      }
      for (const error of collectRecoveryErrors(awareness)) {
        callbacksRef.current.onRecoveryError?.(error);
      }
    };
    const awarenessEvents = awareness as typeof awareness & {
      off: (event: 'change', handler: () => void) => void;
      on: (event: 'change', handler: () => void) => void;
    };

    awarenessEvents.on('change', publish);
    publish();
    return () => {
      disposed = true;
      awarenessEvents.off('change', publish);
      clearCollabAwareness(awareness);
      callbacksRef.current.onFlusherChange?.(false);
    };
  }, [collab.documentId, collab.sessionId, editor]);

  return null;
}

function FloatingFormatToolbar() {
  const editor = useEditorRef();
  const focusedEditorId = useEventEditorValue('focus');
  const state = useFloatingToolbarState({
    editorId: editor.id,
    focusedEditorId,
    floatingOptions: {
      placement: 'top',
      middleware: [offset(8), flip({ padding: 8 }), shift({ padding: 8 })],
    },
  });
  const { hidden, props, ref } = useFloatingToolbar(state);
  if (hidden) return null;
  return (
    <div
      aria-label="Formatting"
      className="z-50 flex items-center gap-0.5 rounded-md border border-line bg-raised p-1 shadow-lg"
      ref={ref}
      {...props}
    >
      <TurnIntoButton />
      <div aria-hidden="true" className="mx-0.5 h-5 w-px bg-line" />
      <MarkButton label="Bold" nodeType={KEYS.bold}>
        <Bold size={15} />
      </MarkButton>
      <MarkButton label="Italic" nodeType={KEYS.italic}>
        <Italic size={15} />
      </MarkButton>
      <MarkButton label="Underline" nodeType={KEYS.underline}>
        <Underline size={15} />
      </MarkButton>
      <MarkButton label="Strikethrough" nodeType={KEYS.strikethrough}>
        <Strikethrough size={15} />
      </MarkButton>
      <MarkButton label="Superscript" nodeType={KEYS.sup}>
        <Superscript size={15} />
      </MarkButton>
      <MarkButton label="Subscript" nodeType={KEYS.sub}>
        <Subscript size={15} />
      </MarkButton>
      <MarkButton label="Inline code" nodeType={KEYS.code}>
        <Code size={15} />
      </MarkButton>
      <LinkButton />
      <div aria-hidden="true" className="mx-0.5 h-5 w-px bg-line" />
      <ListButton label="Bullet list" nodeType={KEYS.ul}>
        <List size={15} />
      </ListButton>
      <ListButton label="Numbered list" nodeType={KEYS.ol}>
        <ListOrdered size={15} />
      </ListButton>
      <TodoListButton label="To-do list">
        <ListTodo size={15} />
      </TodoListButton>
      <div aria-hidden="true" className="mx-0.5 h-5 w-px bg-line" />
      <CommentButton />
      <SuggestionActions />
    </div>
  );
}

// When the selection sits inside a suggestion, expose minimal Accept/Reject
// controls that apply or revert it. Plan 3 builds the full per-card review rail;
// this is the minimal reachable surface so the accept/reject behavior exists in
// the editor. The id under the selection drives `acceptSuggestionById` /
// `rejectSuggestionById` from the tested command layer.
function SuggestionActions() {
  const editor = useEditorRef();
  const suggestionId = useEditorSelector((ed) => {
    const entry = ed.getApi(SuggestionPlugin).suggestion.node({ isText: true });
    return entry ? ed.getApi(SuggestionPlugin).suggestion.nodeId(entry[0]) : undefined;
  }, []);
  if (!suggestionId) return null;
  return (
    <>
      <div aria-hidden="true" className="mx-0.5 h-5 w-px bg-line" />
      <button
        aria-label="Accept suggestion"
        className="inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body"
        data-testid="accept-suggestion"
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => {
          acceptSuggestionById(editor, suggestionId);
          editor.tf.focus();
        }}
        title="Accept suggestion"
        type="button"
      >
        <Check size={15} />
      </button>
      <button
        aria-label="Reject suggestion"
        className="inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body"
        data-testid="reject-suggestion"
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => {
          rejectSuggestionById(editor, suggestionId);
          editor.tf.focus();
        }}
        title="Reject suggestion"
        type="button"
      >
        <X size={15} />
      </button>
    </>
  );
}

function CommentButton() {
  const editor = useEditorRef();
  return (
    <button
      aria-label="Comment"
      className="inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body"
      data-testid="comment-button"
      // Preserve the text selection through the click: setDraft marks the
      // selected range, so the selection must survive the mousedown.
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => startCommentDraft(editor)}
      title="Comment"
      type="button"
    >
      <MessageSquarePlus size={15} />
    </button>
  );
}

// Renders an `a` node as a styled, clickable anchor. A plain click places the
// cursor (the link text stays editable); Cmd/Ctrl+click opens the URL in a new
// tab. The floating edit toolbar (below) also exposes Open.
function LinkElement(props: PlateElementProps<TLinkElement>) {
  const attributes = getLinkAttributes(props.editor, props.element);
  return (
    <PlateElement
      {...props}
      as="a"
      className="font-medium text-accent-ink underline decoration-1 underline-offset-2"
      attributes={{
        ...props.attributes,
        ...attributes,
        onClick: (event) => {
          if ((event.metaKey || event.ctrlKey) && attributes.href) {
            window.open(attributes.href, '_blank', 'noopener,noreferrer');
          }
        },
        // Hovering an <a> with an href otherwise steals editor focus.
        onMouseOver: (event) => event.stopPropagation(),
      }}
    >
      {props.children}
    </PlateElement>
  );
}

// Floating toolbar button that opens the link insert popover for the current
// selection (also reachable via Cmd/Ctrl+K, registered by LinkPlugin).
function LinkButton() {
  const state = useLinkToolbarButtonState();
  const { props } = useLinkToolbarButton(state);
  return (
    <button
      aria-label="Link"
      aria-pressed={state.pressed}
      className={cn(
        'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body',
        state.pressed && 'bg-well text-ink'
      )}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => props.onClick()}
      title="Link"
      type="button"
    >
      <Link size={15} />
    </button>
  );
}

const linkPopover = 'z-50 rounded-md border border-line bg-raised p-1 shadow-lg';
const linkInput =
  'h-8 w-full bg-transparent text-sm text-body outline-none placeholder:text-faint';

// Adapted from PlateJS's official LinkFloatingToolbar: a URL input when inserting
// (Cmd/Ctrl+K or the toolbar button), and an Edit / Open / Unlink popover when the
// cursor sits in a link. Hidden in read-only (Viewing) mode.
function LinkFloatingToolbar() {
  const readOnly = useReadOnly();
  const floatingOptions: UseVirtualFloatingOptions = useMemo(
    () => ({
      middleware: [offset(8), flip({ fallbackPlacements: ['bottom-end', 'top-start', 'top-end'], padding: 12 })],
      placement: 'bottom-start',
    }),
    []
  );
  const insertState = useFloatingLinkInsertState({ floatingOptions } satisfies LinkFloatingToolbarState);
  const { hidden, props: insertProps, ref: insertRef, textInputProps } = useFloatingLinkInsert(insertState);
  const editState = useFloatingLinkEditState({ floatingOptions } satisfies LinkFloatingToolbarState);
  const { editButtonProps, props: editProps, ref: editRef, unlinkButtonProps } = useFloatingLinkEdit(editState);
  const inputProps = useFormInputProps({ preventDefaultOnEnterKeydown: true });

  if (readOnly || hidden) return null;

  const input = (
    <div className="flex w-[320px] flex-col" {...inputProps}>
      <div className="flex items-center gap-1.5 px-1.5">
        <Link className="shrink-0 text-muted" size={15} />
        <FloatingLinkUrlInput className={linkInput} placeholder="Paste link" data-plate-focus />
      </div>
      <div className="my-1 h-px bg-line" />
      <div className="flex items-center gap-1.5 px-1.5">
        <Type className="shrink-0 text-muted" size={15} />
        <input className={linkInput} placeholder="Text to display" data-plate-focus {...textInputProps} />
      </div>
    </div>
  );

  const editContent = editState.isEditing ? (
    input
  ) : (
    <div className="flex items-center gap-0.5">
      <button
        className="inline-flex h-7 items-center rounded px-2 text-sm text-body transition-colors hover:bg-well"
        type="button"
        {...editButtonProps}
      >
        Edit
      </button>
      <div aria-hidden="true" className="mx-0.5 h-5 w-px bg-line" />
      <LinkOpenButton />
      <div aria-hidden="true" className="mx-0.5 h-5 w-px bg-line" />
      <button
        aria-label="Remove link"
        className="inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body"
        type="button"
        {...unlinkButtonProps}
      >
        <Unlink size={15} />
      </button>
    </div>
  );

  return (
    <>
      <div className={linkPopover} ref={insertRef} {...insertProps}>
        {input}
      </div>
      <div className={linkPopover} ref={editRef} {...editProps}>
        {editContent}
      </div>
    </>
  );
}

function LinkOpenButton() {
  const editor = useEditorRef();
  const selection = useEditorSelection();
  const attributes = useMemo(() => {
    const entry = editor.api.node<TLinkElement>({ match: { type: editor.getType(KEYS.link) } });
    return entry ? getLinkAttributes(editor, entry[0]) : {};
    // Recompute as the selection moves between links.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor, selection]);
  return (
    <a
      {...attributes}
      aria-label="Open link in a new tab"
      className="inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body"
      onMouseOver={(event) => event.stopPropagation()}
      rel="noreferrer"
      target="_blank"
    >
      <ExternalLink size={15} />
    </a>
  );
}

function TodoListButton({ label, children }: { label: string; children: ReactNode }) {
  const state = useIndentTodoToolBarButtonState({ nodeType: KEYS.listTodo });
  const { props } = useIndentTodoToolBarButton(state);
  return (
    <button
      aria-label={label}
      aria-pressed={state.pressed}
      className={cn(
        'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body',
        state.pressed && 'bg-well text-ink'
      )}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => props.onClick()}
      title={label}
      type="button"
    >
      {children}
    </button>
  );
}

function ListButton({
  label,
  nodeType,
  children,
}: {
  label: string;
  nodeType: string;
  children: ReactNode;
}) {
  const state = useListToolbarButtonState({ nodeType });
  const { props } = useListToolbarButton(state);
  return (
    <button
      aria-label={label}
      aria-pressed={state.pressed}
      className={cn(
        'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body',
        state.pressed && 'bg-well text-ink'
      )}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => props.onClick()}
      title={label}
      type="button"
    >
      {children}
    </button>
  );
}

function MarkButton({
  label,
  nodeType,
  children,
}: {
  label: string;
  nodeType: string;
  children: ReactNode;
}) {
  const state = useMarkToolbarButtonState({ nodeType });
  const { props } = useMarkToolbarButton(state);
  return (
    <button
      aria-label={label}
      aria-pressed={state.pressed}
      className={cn(
        'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body',
        state.pressed && 'bg-well text-ink'
      )}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => props.onClick()}
      title={label}
      type="button"
    >
      {children}
    </button>
  );
}

const TURN_INTO_ITEMS = [
  { icon: Pilcrow, label: 'Text', value: KEYS.p },
  { icon: Heading1, label: 'Heading 1', value: KEYS.h1 },
  { icon: Heading2, label: 'Heading 2', value: KEYS.h2 },
  { icon: Heading3, label: 'Heading 3', value: KEYS.h3 },
  { icon: Heading4, label: 'Heading 4', value: KEYS.h4 },
  { icon: Heading5, label: 'Heading 5', value: KEYS.h5 },
  { icon: Heading6, label: 'Heading 6', value: KEYS.h6 },
  { icon: Quote, label: 'Quote', value: KEYS.blockquote },
  { icon: SquareCode, label: 'Code', value: KEYS.codeBlock },
  { icon: Workflow, label: 'Mermaid', value: 'mermaid' },
  { icon: Table, label: 'Table', value: 'table' },
];

function setBlockType(editor: PlateEditor, type: string) {
  editor.tf.withoutNormalizing(() => {
    for (const [node, path] of editor.api.blocks<TElement>({ mode: 'lowest' })) {
      if (node.type !== type) editor.tf.setNodes({ type }, { at: path });
    }
  });
}

// Convert the current selection's block(s) to `type`, handling the code-block
// wrap/unwrap (a code block holds code_line children, so it can't be a plain
// setNodes). Used by both the floating toolbar and the block handle menu.
function applyBlockType(editor: PlateEditor, type: string) {
  const inCodeBlock = editor.api.some({ match: { type: editor.getType(KEYS.codeBlock) } });
  if (type === KEYS.codeBlock) {
    if (!inCodeBlock) toggleCodeBlock(editor);
  } else if (inCodeBlock) {
    toggleCodeBlock(editor);
    if (type !== KEYS.p) setBlockType(editor, type);
  } else {
    setBlockType(editor, type);
  }
}

// A Mermaid diagram is an atomic void block; the current block's text seeds the
// diagram source.
function turnIntoMermaid(editor: PlateEditor) {
  const entry = editor.api.block({ highest: true });
  if (!entry) return;
  const code = NodeApi.string(entry[0]);
  editor.tf.replaceNodes({ type: MERMAID_KEY, code, children: [{ text: '' }] }, { at: entry[1] });
}

function TurnIntoButton() {
  const editor = useEditorRef();
  const inCodeBlock = useEditorSelector(
    (ed) => ed.api.some({ match: { type: ed.getType(KEYS.codeBlock) } }),
    []
  );
  const value = useSelectionFragmentProp({
    defaultValue: KEYS.p,
    getProp: (node) => node.type,
  });
  // Inside a code block the lowest block is a code_line, so resolve the label
  // from the wrapping code_block instead.
  const currentValue = inCodeBlock ? KEYS.codeBlock : value;
  const active = TURN_INTO_ITEMS.find((item) => item.value === currentValue) ?? TURN_INTO_ITEMS[0];
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label="Turn into"
          className="inline-flex h-7 items-center gap-1 rounded px-2 text-xs font-medium text-body transition-colors hover:bg-well"
          type="button"
        >
          {active.label}
          <ChevronDown className="text-muted" size={13} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          className="z-50 min-w-44 rounded-md border border-line bg-raised p-1 shadow-lg"
          sideOffset={6}
        >
          {TURN_INTO_ITEMS.map((item) => (
            <DropdownMenu.Item
              className={cn(
                'flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-sm text-body outline-none select-none data-highlighted:bg-well',
                item.value === active.value && 'text-accent-ink'
              )}
              key={item.value}
              onSelect={() => {
                if (item.value === 'mermaid') turnIntoMermaid(editor);
                else if (item.value === 'table') turnIntoTable(editor);
                else applyBlockType(editor, item.value);
                editor.tf.focus();
              }}
            >
              <item.icon className="shrink-0 text-muted" size={15} />
              {item.label}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function CodeBlockElement(props: PlateElementProps<TCodeBlockElement>) {
  const [copied, setCopied] = useState(false);
  return (
    <PlateElement {...props} className="group">
      <pre>
        <code>{props.children}</code>
      </pre>
      <button
        aria-label={copied ? 'Copied' : 'Copy code'}
        className="absolute right-1.5 top-1.5 inline-flex items-center justify-center rounded border border-line bg-raised p-1 text-muted opacity-0 transition-opacity hover:text-body group-hover:opacity-100 focus-visible:opacity-100"
        contentEditable={false}
        onClick={() => {
          const text = props.element.children
            .map((child) => NodeApi.string(child))
            .join('\n');
          void navigator.clipboard?.writeText(text)?.catch(() => {});
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        }}
        title="Copy"
        type="button"
      >
        {copied ? <Check size={13} /> : <Copy size={13} />}
      </button>
    </PlateElement>
  );
}

function ListItemElement(props: PlateElementProps) {
  const { listStart, listStyleType } = props.element as TListElement;
  if (listStyleType === KEYS.listTodo) return <TodoListItem {...props} />;
  const ListTag = isOrderedList(props.element) ? 'ol' : 'ul';
  // ps-6 keeps the marker clear of the drag handle in the block's left gutter
  // and aligns list content with the to-do checkbox indent.
  return (
    <ListTag className="relative m-0 ps-6" start={listStart} style={{ listStyleType }}>
      <li>{props.children}</li>
    </ListTag>
  );
}

function TodoListItem(props: PlateElementProps) {
  const state = useTodoListElementState({ element: props.element });
  const { checkboxProps } = useTodoListElement(state);
  const readOnly = useReadOnly();
  const checked = props.element.checked === true;
  return (
    <ul className="relative m-0 list-none p-0">
      <li className={cn('relative pl-6', checked && 'text-muted line-through')}>
        <span className="absolute left-0 top-[0.2em]" contentEditable={false}>
          <input
            aria-label="Toggle to-do"
            checked={checkboxProps.checked}
            className="size-3.5 cursor-pointer accent-accent disabled:cursor-default"
            disabled={readOnly}
            onChange={(event) => checkboxProps.onCheckedChange(event.target.checked)}
            onMouseDown={checkboxProps.onMouseDown}
            type="checkbox"
          />
        </span>
        {props.children}
      </li>
    </ul>
  );
}

// Provides the editor's react-dnd context. react-dnd v14 keeps a single global
// manager/backend, so this coexists with the document tree's own DndProvider
// (react-arborist) without a second HTML5 backend.
function EditorDndProvider({ children }: { children?: ReactNode }) {
  return <DndProvider backend={HTML5Backend}>{children}</DndProvider>;
}

const HANDLE_SIZE = 24;

function DraggableBlock(props: PlateElementProps) {
  const { children, element } = props;
  const editor = useEditorRef();
  // Disable the drag preview (transparent image) — otherwise Chrome renders its
  // default globe icon for the empty preview element. The dragged block fades
  // (opacity-50) and the drop-line shows the target, which is feedback enough.
  const { isDragging, nodeRef, handleRef } = useDraggable({ element, preview: { disable: true } });
  // Center the handle on the block's first line. Blocks (esp. headings) have
  // their own margin-top and line-height, so measure the rendered element rather
  // than assuming a fixed offset. The handle lives in a small left padding
  // *inside* the drop target (nodeRef) — out in the centered-layout margin it
  // would never sit over a drop target, and the drop would never fire.
  const [handleTop, setHandleTop] = useState(0);
  // Clicking the handle (no drag) opens a block-actions menu. A native HTML5
  // drag never fires `click`, so drag and menu don't conflict.
  const [menuRect, setMenuRect] = useState<{ left: number; top: number } | null>(null);
  const alignHandle = () => {
    const dom = editor.api.toDOMNode(element);
    if (!dom) return;
    const style = getComputedStyle(dom);
    const marginTop = Number.parseFloat(style.marginTop) || 0;
    const lineHeight = Number.parseFloat(style.lineHeight) || 0;
    setHandleTop(marginTop + Math.max(0, (lineHeight - HANDLE_SIZE) / 2));
  };
  return (
    <div
      className={cn('group relative flow-root pl-7', isDragging && 'opacity-50')}
      onMouseEnter={alignHandle}
      ref={nodeRef}
    >
      <div
        className="absolute left-0 flex w-7 items-center justify-center opacity-0 transition-opacity group-hover:opacity-100"
        contentEditable={false}
        style={{ height: HANDLE_SIZE, top: handleTop }}
      >
        <button
          aria-label="Drag to move block"
          className="flex size-6 cursor-grab items-center justify-center rounded text-faint transition-colors hover:bg-well hover:text-muted active:cursor-grabbing"
          data-plate-prevent-deselect
          onClick={(event) => {
            const box = event.currentTarget.getBoundingClientRect();
            setMenuRect({ left: box.left, top: box.bottom + 4 });
          }}
          ref={handleRef}
          title="Drag to move · click for actions"
          type="button"
        >
          <GripVertical size={15} />
        </button>
      </div>
      {children}
      <BlockDropLine />
      {menuRect ? (
        <BlockActionsMenu
          editor={editor}
          element={element}
          onClose={() => setMenuRect(null)}
          rect={menuRect}
        />
      ) : null}
    </div>
  );
}

const blockMenuItem =
  'flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-left text-sm text-body outline-none hover:bg-well';

function cloneWithoutIds(node: Descendant): Descendant {
  if (!ElementApi.isElement(node)) return { ...node };
  const { id, ...rest } = node;
  void id;
  return { ...rest, children: node.children.map(cloneWithoutIds) };
}

// Normalize to a paragraph first (unwrap code, drop heading), then toggle the
// list — so any block can become a list cleanly.
function turnIntoList(editor: PlateEditor, listStyleType: string, checked?: boolean) {
  applyBlockType(editor, KEYS.p);
  toggleList(editor, { listStyleType });
  if (checked !== undefined) editor.tf.setNodes({ checked, listStyleType });
}

const BLOCK_TURN_INTO: ReadonlyArray<{
  icon: typeof Pilcrow;
  label: string;
  apply: (editor: PlateEditor) => void;
}> = [
  { icon: Pilcrow, label: 'Text', apply: (editor) => applyBlockType(editor, KEYS.p) },
  { icon: Heading1, label: 'Heading 1', apply: (editor) => applyBlockType(editor, KEYS.h1) },
  { icon: Heading2, label: 'Heading 2', apply: (editor) => applyBlockType(editor, KEYS.h2) },
  { icon: Heading3, label: 'Heading 3', apply: (editor) => applyBlockType(editor, KEYS.h3) },
  { icon: List, label: 'Bulleted list', apply: (editor) => turnIntoList(editor, KEYS.ul) },
  { icon: ListOrdered, label: 'Numbered list', apply: (editor) => turnIntoList(editor, KEYS.ol) },
  { icon: ListTodo, label: 'To-do list', apply: (editor) => turnIntoList(editor, KEYS.listTodo, false) },
  { icon: Quote, label: 'Quote', apply: (editor) => applyBlockType(editor, KEYS.blockquote) },
  { icon: SquareCode, label: 'Code', apply: (editor) => applyBlockType(editor, KEYS.codeBlock) },
  { icon: Workflow, label: 'Mermaid diagram', apply: (editor) => turnIntoMermaid(editor) },
  { icon: Table, label: 'Table', apply: (editor) => turnIntoTable(editor) },
];

function turnBlockInto(editor: PlateEditor, element: TElement, apply: (editor: PlateEditor) => void) {
  const at = editor.api.findPath(element);
  if (!at) return;
  editor.tf.select(at);
  apply(editor);
  editor.tf.focus();
}

function duplicateBlock(editor: PlateEditor, element: TElement) {
  const at = editor.api.findPath(element);
  if (!at) return;
  editor.tf.insertNodes(cloneWithoutIds(element), { at: PathApi.next(at), select: true });
  editor.tf.focus();
}

function deleteBlock(editor: PlateEditor, element: TElement) {
  const at = editor.api.findPath(element);
  if (!at) return;
  editor.tf.removeNodes({ at });
  editor.tf.focus();
}

function BlockActionsMenu({
  editor,
  element,
  onClose,
  rect,
}: {
  editor: PlateEditor;
  element: TElement;
  onClose: () => void;
  rect: { left: number; top: number };
}) {
  return createPortal(
    <div className="fixed inset-0 z-50" onMouseDown={onClose}>
      <div
        aria-label="Block actions"
        className="fixed z-50 min-w-44 rounded-md border border-line bg-raised p-1 shadow-lg"
        onMouseDown={(event) => event.stopPropagation()}
        role="menu"
        style={{ left: rect.left, top: rect.top }}
      >
        <div className="group/turninto relative">
          <button className={cn(blockMenuItem, 'justify-between')} role="menuitem" type="button">
            <span className="flex items-center gap-2">
              <Type className="shrink-0 text-muted" size={15} />
              Turn into
            </span>
            <ChevronRight className="shrink-0 text-muted" size={14} />
          </button>
          <div className="absolute -top-1 left-full z-50 hidden min-w-44 rounded-md border border-line bg-raised p-1 shadow-lg group-hover/turninto:block">
            {BLOCK_TURN_INTO.map((item) => (
              <button
                className={blockMenuItem}
                key={item.label}
                onClick={() => {
                  turnBlockInto(editor, element, item.apply);
                  onClose();
                }}
                role="menuitem"
                type="button"
              >
                <item.icon className="shrink-0 text-muted" size={15} />
                {item.label}
              </button>
            ))}
          </div>
        </div>
        <div className="my-1 h-px bg-line" />
        <button
          className={blockMenuItem}
          onClick={() => {
            duplicateBlock(editor, element);
            onClose();
          }}
          role="menuitem"
          type="button"
        >
          <Copy className="shrink-0 text-muted" size={15} />
          Duplicate
        </button>
        <button
          className={cn(blockMenuItem, 'text-danger')}
          onClick={() => {
            deleteBlock(editor, element);
            onClose();
          }}
          role="menuitem"
          type="button"
        >
          <Trash2 className="shrink-0 text-danger" size={15} />
          Delete
        </button>
      </div>
    </div>,
    document.body
  );
}

function BlockDropLine() {
  const { dropLine } = useDropLine();
  if (!dropLine) return null;
  return (
    <div
      className={cn(
        'absolute inset-x-0 z-10 h-0.5 bg-accent',
        dropLine === 'top' ? '-top-px' : '-bottom-px'
      )}
      contentEditable={false}
    />
  );
}

function resetPlateEditor(editor: PlateEditor, value: PlateValue) {
  editor.tf.replaceNodes(value as never, {
    at: [],
    children: true,
  });
  editor.meta.resetting = true;
  editor.history.undos = [];
  editor.history.redos = [];
  editor.operations = [];
}

function collabColor(seed: string) {
  const colors = ['#2563eb', '#16a34a', '#dc2626', '#9333ea', '#0891b2', '#ca8a04'];
  let hash = 0;
  for (const char of seed) {
    hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  }
  return colors[hash % colors.length];
}
