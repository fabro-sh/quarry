import { createSlateEditor, TextApi, RangeApi } from 'platejs';
import { NativeReviewContext, ReviewHighlightsContext, type NativeReviewState } from '../review/native-review-context';
import { NativeSuggestionActions } from './plate-review-actions';
import { PlatePresence } from './plate-presence';
import { SourceDraftRecovery } from './source-draft-editor';
import type { DocumentSelection } from './document-presence';
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
import { DndPlugin, useDraggable, useDropLine } from '@platejs/dnd';
import { DndProvider } from 'react-dnd';
import { HTML5Backend } from 'react-dnd-html5-backend';
import { createDragDropManager } from 'dnd-core';
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
  useIndentTodoToolBarButtonState,
  useListToolbarButtonState,
  useTodoListElement,
  useTodoListElementState,
} from '@platejs/list/react';
import { isOrderedList } from '@platejs/list';
import { MarkdownPlugin } from '@platejs/markdown';
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
  MoreHorizontal,
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
  type SlateEditor,
  type TCodeBlockElement,
  type TElement,
  type TLinkElement,
  type TListElement,
} from 'platejs';
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal, flushSync } from 'react-dom';
import remarkGfm from 'remark-gfm';
import {
  createPlatePlugin,
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
  usePluginOption,
  useReadOnly,
  useSelectionFragmentProp,
  type PlateEditor,
  type PlateElementProps,
  type RenderNodeWrapper,
} from 'platejs/react';


import { cn } from '../../lib/utils';
import { remarkBreakSemantics } from './remark-break-semantics';
import { remarkInlineMarks } from './remark-inline-marks';
import { ImageKit, ImageProvider, type ImageApi } from './image-element';
import { mermaidMdRules, MERMAID_KEY } from './mermaid';
import { MermaidPlugin } from './mermaid-block';
import { tableMdRules, turnIntoTable } from './table';
import { TableKit } from './table-element';
import { TocSidebar } from './toc-sidebar';
import { wikiLinkMdRules } from './wiki-link';
import { WikiLinkPlugin, WikiLinkProvider, type WikiLinkApi } from './wiki-link-element';
import { RawMarkdownPlugin } from './raw-markdown-block';
import { rawMarkdownMdRules } from './raw-markdown';
import { documentAdapter, PlateDocumentAdapter, type PlateAdapterOptions } from './plate-document-adapter';
import { blockAttrs, blockKinds, inlineText, nativeNodeIdOptions, projectPlate, selectedText } from './plate-document-projection';
import { DocumentModel } from './document-model';
import type { EditorMode } from './editor-types';
import { NativeProposalPlugin, NativeReviewPlugin, useReviewDecoration } from './plate-review-decoration';

const autoformatRules: AutoformatRule[] = [
  { match: '# ', mode: 'block', type: KEYS.h1, format: (editor) => applyBlockType(editor, KEYS.h1) },
  { match: '## ', mode: 'block', type: KEYS.h2, format: (editor) => applyBlockType(editor, KEYS.h2) },
  { match: '### ', mode: 'block', type: KEYS.h3, format: (editor) => applyBlockType(editor, KEYS.h3) },
  { match: '#### ', mode: 'block', type: KEYS.h4, format: (editor) => applyBlockType(editor, KEYS.h4) },
  { match: '##### ', mode: 'block', type: KEYS.h5, format: (editor) => applyBlockType(editor, KEYS.h5) },
  { match: '###### ', mode: 'block', type: KEYS.h6, format: (editor) => applyBlockType(editor, KEYS.h6) },
  { match: '> ', mode: 'block', type: KEYS.blockquote, format: (editor) => applyBlockType(editor, KEYS.blockquote) },
  {
    match: '```',
    mode: 'block',
    type: KEYS.codeBlock,
    format: (editor) => {
      applyBlockType(editor, KEYS.codeBlock);
    },
  },
  {
    match: ['* ', '- '],
    mode: 'block',
    type: 'list',
    format: (editor) => {
      turnIntoList(editor, KEYS.ul);
    },
  },
  {
    match: [String.raw`^\d+\.$ `, String.raw`^\d+\)$ `],
    matchByRegex: true,
    mode: 'block',
    type: 'list',
    format: (editor, { matchString }) => {
      turnIntoList(editor, KEYS.ol, undefined, Number.parseInt(matchString, 10) || 1);
    },
  },
  {
    // Notion-style `[]` and GitHub-style `[ ]` (with the space inside).
    match: ['[] ', '[ ] '],
    mode: 'block',
    type: 'list',
    format: (editor) => {
      turnIntoList(editor, KEYS.listTodo, false);
    },
  },
  {
    match: ['[x] ', '[X] '],
    mode: 'block',
    type: 'list',
    format: (editor) => {
      turnIntoList(editor, KEYS.listTodo, true);
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
// dragover/drop), so Safari uses the handle for block actions only.
// `navigator.vendor` is "Apple Computer, Inc." in
// Safari/WebKit and "Google Inc."/"" in Chrome/Firefox.
const supportsBlockDrag =
  typeof navigator !== 'undefined' && !/apple/i.test(navigator.vendor);

// Notion-style drag handle for reordering top-level blocks (Chrome/Firefox).
const BlockDraggable: RenderNodeWrapper = (props) => {
  if (props.editor.dom.readOnly) return undefined;
  if (props.path.length !== 1) return undefined;
  return (childProps) => <DraggableBlock {...childProps} />;
};

// Headings render as plain <hN> by default, which carry no DOM id. The TOC
// sidebar's scroll-spy reads the active heading from `entry.target.id`, so we
// stamp each heading's node id onto the element. The `slate-hN` class (added
// upstream by Plate's render pipeline, see styles.css) flows through
// `attributes.className`, so appearance is unchanged.
const makeHeadingElement = (as: 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6') =>
  function HeadingElement({ attributes, ...props }: PlateElementProps) {
    const id = typeof props.element.id === 'string' ? props.element.id : undefined;
    return <PlateElement as={as} attributes={{ ...attributes, id }} {...props} />;
  };

// A paste is new content, so it receives new block IDs. Normalize the private
// fragment with the editor's own plugins before native command translation.
const NativeClipboardPlugin = createPlatePlugin({ key: 'quarry_clipboard' }).overrideEditor(({ editor, tf: { insertFragment } }) => {
  const deserialize = editor.api.html.deserialize;
  return {
  api: { html: { deserialize(options: Parameters<typeof deserialize>[0]) {
    const element = typeof options.element === 'string' ? new DOMParser().parseFromString(options.element, 'text/html').body : options.element.cloneNode(true) as HTMLElement;
    element.querySelectorAll('script,style,iframe,object,embed,template,link,meta').forEach((node) => node.remove());
    return deserialize({ ...options, element });
  } } },
  transforms: { insertFragment(fragment, ...args) {
    const scratch = createSlateEditor({
      plugins: plateMarkdownPlugins.filter((plugin) => plugin.key !== TrailingBlockPlugin.key && plugin.key !== 'quarry_clipboard') as never,
      value: fragment.map(cloneWithoutIds) as TElement[], nodeId: nativeNodeIdOptions, shouldNormalizeEditor: true,
    });
    const value = scratch.children;
    const selection = editor.selection;
    if (selection) {
      const [start, end] = RangeApi.edges(selection);
      const first = editor.api.block({ at: start }), last = editor.api.block({ at: end });
      // Slate unwraps non-void containers at a fragment's edges when merging
      // into surrounding text. Empty boundary paragraphs preserve containers;
      // they merge into the existing prefix/tail without adding visible text.
      if (blockKinds.get(String(value[0]?.type))?.content === 'container' && first && !editor.api.isStart(start, first[1])) value.unshift({ type: 'p', children: [{ text: '' }] });
      if (blockKinds.get(String(value.at(-1)?.type))?.content === 'container' && last && !editor.api.isEnd(end, last[1])) value.push({ type: 'p', children: [{ text: '' }] });
    }
    insertFragment(value, ...args);
  } },
}; });

export const plateMarkdownPlugins = [
  NativeClipboardPlugin,
  NativeReviewPlugin,
  NativeProposalPlugin,
  ParagraphPlugin,
  // Always keep an editable paragraph at the end, so there's a line to type on
  // below the last block — even an atomic void like a Mermaid diagram or image,
  // which would otherwise leave the document with no place to continue writing.
  // Empty trailing input is kept local until the user edits it.
  TrailingBlockPlugin,
  H1Plugin.withComponent(makeHeadingElement('h1')),
  H2Plugin.withComponent(makeHeadingElement('h2')),
  H3Plugin.withComponent(makeHeadingElement('h3')),
  H4Plugin.withComponent(makeHeadingElement('h4')),
  H5Plugin.withComponent(makeHeadingElement('h5')),
  H6Plugin.withComponent(makeHeadingElement('h6')),
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
  RawMarkdownPlugin,
  ...TableKit,
  ...ImageKit,
  DndPlugin.configure({
    render: { aboveNodes: BlockDraggable, aboveSlate: EditorDndProvider },
    handlers: {
      onDragStart: ({ event }) => {
        if (!(event.target instanceof Element) || !event.target.closest('[data-quarry-block-handle]')) return;
        // A block move is owned by React DnD. Slate's text-drag path would
        // capture and later delete the current text selection a second time.
        event.dataTransfer.setData('application/x-quarry-block', 'move');
        return true;
      },
      onDrop: ({ event, getOptions }) => event.dataTransfer.types.includes('application/x-quarry-block') || getOptions().isDragging,
    },
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
  MarkdownPlugin.configure({
    options: { remarkPlugins: [remarkGfm, remarkInlineMarks, remarkBreakSemantics], rules: { ...wikiLinkMdRules, ...mermaidMdRules, ...tableMdRules, ...rawMarkdownMdRules } },
  }),
] as const;



// Native review publication updates the surrounding UI independently of
// Slate's own text updates. Stable props let unchanged editor content bail out.
const DocumentContent = memo(PlateContent);
const DocumentContents = memo(TocSidebar);
const DocumentPresence = memo(PlatePresence);
const emptyImage: ImageApi = {};
const emptyWikiLink: WikiLinkApi = {};

export function PlateMarkdownEditor({ model, review, mode, options, onReady, onComment, onCompositionChange, peers, image = emptyImage, wikiLink = emptyWikiLink }: {
  model: DocumentModel;
  review: NativeReviewState;
  peers: DocumentSelection[];
  mode: EditorMode;
  options: PlateAdapterOptions;
  onReady: (adapter: PlateDocumentAdapter) => void | (() => void);
  onComment: () => void;
  onCompositionChange: (composing: boolean) => void;
  image?: ImageApi;
  wikiLink?: WikiLinkApi;
}) {
  const current = useRef({ options, onReady, onComment, onCompositionChange }); current.current = { options, onReady, onComment, onCompositionChange };
  const compositionEnd = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(compositionEnd.current), []);
  const highlights = useMemo(() => ({ activeId: review.activeId, hoverId: review.hoverId, draftTarget: review.draftTarget,
    setActiveId: review.setActiveId, setHoverId: review.setHoverId }), [review.activeId, review.hoverId, review.draftTarget, review.setActiveId, review.setHoverId]);
  const initialValue = useMemo(() => projectPlate(model.view(), true, (point) => model.locate(point)), [model]);
  // Keep each edit's React work bounded, while Slate's chunk wrappers let the
  // browser skip layout for distant text. Identity stays in native blocks.
  const editor = usePlateEditor({ plugins: plateMarkdownPlugins as never, chunking: { chunkSize: 50 }, value: initialValue, nodeId: nativeNodeIdOptions }, [model]);
  const decorate = useReviewDecoration(model, editor, review.draftTarget);
  const inputHandlers = useMemo(() => ({
    onPointerDown() { documentAdapter(editor)?.endIntent(); },
    onDOMBeforeInput() {
      // Slate can defer selectionchange while finishing a render. Without
      // this read, it may treat ordinary typing as a temporary target-range
      // edit and restore the caret to the previous paragraph after one key.
      if (!editor.api.isComposing()) documentAdapter(editor)?.captureInputSelection();
    },
    onKeyDown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.altKey && /^Digit[0-6]$/.test(event.code)) {
        event.preventDefault(); documentAdapter(editor)?.captureInputSelection();
        applyBlockType(editor, event.code === 'Digit0' ? KEYS.p : `h${event.code.slice(-1)}`); return true;
      }
      if (/^(Arrow|Home$|End$|PageUp$|PageDown$)/.test(event.key)) documentAdapter(editor)?.endIntent();
      if ((event.metaKey || event.ctrlKey) && !event.altKey && (event.key.toLowerCase() === 'z' || !event.metaKey && event.key.toLowerCase() === 'y')) {
        event.preventDefault(); documentAdapter(editor)?.history(event.shiftKey || event.key.toLowerCase() === 'y'); return true;
      }
      if ((event.metaKey || event.ctrlKey) && event.altKey && event.key.toLowerCase() === 'm') {
        event.preventDefault(); current.current.onComment(); return true;
      }
    },
    onCompositionStart() {
      clearTimeout(compositionEnd.current); current.current.onCompositionChange(true);
      documentAdapter(editor)?.captureInputSelection();
      documentAdapter(editor)?.setComposing(true);
      if (editor.selection && RangeApi.isExpanded(editor.selection)) {
        // Finish the selected-text deletion before the browser starts changing
        // the composition node. Otherwise Firefox can remove DOM nodes that
        // React still needs to reconcile, especially across table boundaries.
        flushSync(() => { editor.tf.deleteFragment(); editor.api.onChange(); });
      }
    },
    onCompositionEnd() {
      // Final input can arrive after compositionend. Commit it before merging.
      compositionEnd.current = setTimeout(() => { documentAdapter(editor)?.setComposing(false); current.current.onCompositionChange(false); }, 0);
    },
  }), [editor]);
  useLayoutEffect(() => {
    const adapter = new PlateDocumentAdapter(editor, model, {
      changed: (batch) => current.current.options.changed(batch),
      proposed: (id) => current.current.options.proposed?.(id),
      error: (error) => current.current.options.error(error),
      selection: (points) => current.current.options.selection?.(points),
      readOnly: () => current.current.options.readOnly?.() ?? false,
      mode: () => current.current.options.mode?.() ?? 'editing',
      author: () => current.current.options.author?.() ?? 'user',
    });
    const cleanup = current.current.onReady(adapter);
    return () => { cleanup?.(); adapter.dispose(); };
  }, [editor, model]);
  useLayoutEffect(() => { documentAdapter(editor)?.endIntent(); }, [editor, mode]);
  return <ReviewHighlightsContext.Provider value={highlights}><WikiLinkProvider value={wikiLink}><ImageProvider value={image}>
    <Plate editor={editor} readOnly={mode === 'viewing'}>
      <SourceDraftRecovery editor={editor} />
      {mode !== 'viewing' && <FloatingFormatToolbar onComment={onComment} review={review} />}
      <div className="relative flex h-full min-h-0">
        <PlateContainer className="relative min-w-0 flex-1 overflow-auto">
          <DocumentContents topOffset={48} />
          <DocumentPresence model={model} peers={peers} />
          <DocumentContent aria-label="Plate markdown editor"
            role="textbox"
            decorate={decorate}
            className="min-h-full w-full pt-12 pb-8 pl-[max(2rem,calc((100%-68ch)/2))] pr-[max(1rem,calc((100%-68ch)/2))] text-[15px] leading-7 text-ink outline-none [&_[data-slate-placeholder=true]]:text-faint"
            placeholder="Write markdown…" spellCheck={false}
            {...inputHandlers} />
        </PlateContainer>
      </div>
    </Plate>
  </ImageProvider></WikiLinkProvider></ReviewHighlightsContext.Provider>;
}
function FloatingFormatToolbar({
  onComment, review,
}: {
  onComment: () => void;
  review: NativeReviewState;
}) {
  const editor = useEditorRef();
  const focusedEditorId = useEventEditorValue('focus');
  const commenting = Boolean(review.draftTarget);
  const state = useFloatingToolbarState({
    editorId: editor.id,
    focusedEditorId,
    hideToolbar: commenting,
    floatingOptions: {
      placement: 'top',
      middleware: [offset(8), flip({ padding: 8 }), shift({ padding: 8 })],
    },
  });
  // Inline void chips have native text even though Slate's string is empty.
  const selectedNativeText = useEditorSelector((editor) => editor.selection && editor.api.isExpanded()
    ? editor.api.string() || (selectedText(editor.children, editor.selection).length ? 'selected' : '') : '', []);
  const { hidden, props, ref } = useFloatingToolbar({ ...state, selectionText: selectedNativeText });
  useEffect(() => {
    if (!commenting) return;
    // The selection remains the comment's target. Keep its formatting popup
    // closed after submission until the user starts a new text selection.
    state.setWaitForCollapsedSelection(true); state.setOpen(false);
  }, [commenting, state.setWaitForCollapsedSelection, state.setOpen]);
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
      <MarkButton label="Inline code" nodeType={KEYS.code}>
        <Code size={15} />
      </MarkButton>
      <LinkButton />
      <MoreMarksButton />
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
      <CommentButton onComment={onComment} />
      <NativeReviewContext.Provider value={review}><NativeSuggestionActions /></NativeReviewContext.Provider>
    </div>
  );
}

function CommentButton({ onComment }: { onComment: () => void }) {
  return (
    <button
      aria-label="Comment"
      className="inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body"
      data-testid="comment-button"
      // Preserve the text selection while the comment composer opens.
      onMouseDown={(event) => event.preventDefault()}
      onClick={onComment}
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
  const editor = useEditorRef();
  return (
    <button
      aria-label={label}
      aria-pressed={state.pressed}
      className={cn(
        'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body',
        state.pressed && 'bg-well text-ink'
      )}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => turnIntoList(editor, KEYS.listTodo)}
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
  const editor = useEditorRef();
  return (
    <button
      aria-label={label}
      aria-pressed={state.pressed}
      className={cn(
        'inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body',
        state.pressed && 'bg-well text-ink'
      )}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => turnIntoList(editor, nodeType)}
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

// Less-common inline marks (super/subscript) live behind a "…" overflow so the
// toolbar's primary marks stay uncluttered. Mirrors TurnIntoButton's dropdown.
function MoreMarksButton() {
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label="More formatting"
          className="inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body"
          title="More"
          type="button"
        >
          <MoreHorizontal size={15} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          className="z-50 min-w-40 rounded-md border border-line bg-raised p-1 shadow-lg"
          sideOffset={6}
        >
          <MarkMenuItem label="Superscript" nodeType={KEYS.sup}>
            <Superscript size={15} />
          </MarkMenuItem>
          <MarkMenuItem label="Subscript" nodeType={KEYS.sub}>
            <Subscript size={15} />
          </MarkMenuItem>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function MarkMenuItem({
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
    <DropdownMenu.CheckboxItem
      checked={state.pressed}
      className={cn(
        'flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-sm text-body outline-none select-none data-highlighted:bg-well',
        state.pressed && 'text-accent-ink'
      )}
      onSelect={() => props.onClick()}
    >
      <span className="shrink-0 text-muted">{children}</span>
      {label}
      <DropdownMenu.ItemIndicator className="ms-auto">
        <Check size={14} />
      </DropdownMenu.ItemIndicator>
    </DropdownMenu.CheckboxItem>
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

// User conversions are interpreted by the native engine, including code
// wrapping, attribute inheritance, review decisions, and source identity.
export function applyBlockType(editor: SlateEditor, type: string) {
  const ids = [...editor.api.blocks<TElement>({ mode: 'lowest' })].map(([node]) => String(node.id));
  documentAdapter(editor)?.convertBlocks(ids, { kind: type });
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

// Own the editor's drag manager and scope its events to this editor. The tree
// has a backend scoped to its own root. React DnD's implicit global singleton
// would reuse that backend and never receive drag events from the editor.
function EditorDndProvider({ children }: { children?: ReactNode }) {
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  const manager = useMemo(() => root ? createDragDropManager(HTML5Backend, undefined, { rootElement: root }) : undefined, [root]);
  return <div className="contents" ref={setRoot}>{manager && <DndProvider manager={manager}>{children}</DndProvider>}</div>;
}

const HANDLE_SIZE = 24;

function DraggableBlock(props: PlateElementProps) {
  const { children, element } = props;
  const editor = useEditorRef();
  // Disable the drag preview (transparent image) — otherwise Chrome renders its
  // default globe icon for the empty preview element. The dragged block fades
  // (opacity-50) and the drop-line shows the target, which is feedback enough.
  const { isDragging, nodeRef, handleRef } = useDraggable({ element, drag: { canDrag: supportsBlockDrag }, preview: { disable: true } });
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
          aria-label={supportsBlockDrag ? 'Drag to move block' : 'Block actions'}
          className={cn('flex size-6 items-center justify-center rounded text-faint transition-colors hover:bg-well hover:text-muted', supportsBlockDrag && 'cursor-grab active:cursor-grabbing')}
          data-plate-prevent-deselect
          data-quarry-block-handle
          onClick={(event) => {
            const box = event.currentTarget.getBoundingClientRect();
            setMenuRect({ left: box.left, top: box.bottom + 4 });
          }}
          ref={handleRef}
          title={supportsBlockDrag ? 'Drag to move · click for actions' : 'Block actions'}
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
  // Review ownership belongs to the original characters. Wiki source spelling
  // and marks describe visible content; they contain no native identity.
  const rest = Object.fromEntries(Object.entries(node).filter(([key]) => key !== 'id' &&
    (!key.startsWith('quarry') || key === 'quarrySource' || key === 'quarryMarks' || key === 'quarryLeaves')));
  if (TextApi.isText(node)) return { ...rest, text: node.text };
  if (node.type === 'quarry_proposal') return { text: node.children.map(inlineText).join('') };
  return { ...rest, type: node.type, children: node.children.map(cloneWithoutIds) };
}

export function turnIntoList(editor: SlateEditor, listStyleType: string, checked?: boolean, start?: number) {
  const entries = [...editor.api.blocks<TElement>({ mode: 'lowest' })];
  const togglesOff = checked === undefined && start === undefined && entries.every(([node]) => node.listStyleType === listStyleType);
  documentAdapter(editor)?.convertBlocks(entries.map(([node]) => String(node.id)), {
    kind: KEYS.p, ...(togglesOff ? {} : { list: { style: listStyleType, checked, start } }),
  });
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
  const adapter = documentAdapter(editor);
  if (adapter) adapter.deleteBlock(String(element.id)); else editor.tf.removeNodes({ at });
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
