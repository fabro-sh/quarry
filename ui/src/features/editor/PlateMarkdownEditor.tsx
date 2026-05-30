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
import { LinkPlugin } from '@platejs/link/react';
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
import { flip, offset, shift, useFloatingToolbar, useFloatingToolbarState } from '@platejs/floating';
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
  Italic,
  List,
  ListOrdered,
  ListTodo,
  Pilcrow,
  Quote,
  SquareCode,
  Strikethrough,
  Subscript,
  Superscript,
  Trash2,
  Type,
  Underline,
} from 'lucide-react';
import { ElementApi, KEYS, NodeApi, PathApi, type Descendant, type TElement, type TListElement } from 'platejs';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import remarkGfm from 'remark-gfm';
import {
  ParagraphPlugin,
  Plate,
  PlateContent,
  PlateElement,
  useEditorRef,
  useEventEditorValue,
  useEditorSelector,
  useMarkToolbarButton,
  useMarkToolbarButtonState,
  usePlateEditor,
  useReadOnly,
  useSelectionFragmentProp,
  type PlateEditor,
  type PlateElementProps,
  type RenderNodeWrapper,
} from 'platejs/react';

import { cn } from '../../lib/utils';
import { markdownToPlateValue, plateValueToMarkdown, type PlateValue } from './markdown-codec';
import { remarkInlineMarks } from './remark-inline-marks';

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
  LinkPlugin,
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
  MarkdownPlugin.configure({ options: { remarkPlugins: [remarkGfm, remarkInlineMarks] } }),
] as const;

export function PlateMarkdownEditor({
  content,
  disabled,
  onChange,
}: {
  content: string;
  disabled?: boolean;
  onChange: (content: string) => void;
}) {
  const initialValueRef = useRef<PlateValue | null>(null);
  if (!initialValueRef.current) {
    initialValueRef.current = markdownToPlateValue(content);
  }
  const lastContentRef = useRef(content);
  const lastSerializedRef = useRef(plateValueToMarkdown(initialValueRef.current));
  const editor = usePlateEditor(
    {
      plugins: plateMarkdownPlugins as never,
      value: initialValueRef.current as never,
    },
    []
  );

  useEffect(() => {
    if (content === lastContentRef.current) return;
    const nextValue = markdownToPlateValue(content);
    resetPlateEditor(editor, nextValue);
    lastContentRef.current = content;
    lastSerializedRef.current = plateValueToMarkdown(nextValue);
  }, [content, editor]);

  return (
    <Plate
      editor={editor}
      readOnly={disabled}
      onValueChange={({ editor, value }) => {
        if (editor.meta.resetting) {
          editor.meta.resetting = undefined;
          return;
        }
        const nextMarkdown = plateValueToMarkdown(value as PlateValue);
        if (nextMarkdown === lastSerializedRef.current) return;
        lastContentRef.current = nextMarkdown;
        lastSerializedRef.current = nextMarkdown;
        onChange(nextMarkdown);
      }}
    >
      {disabled ? null : <FloatingFormatToolbar />}
      <PlateContent
        aria-label="Plate markdown editor"
        className="min-h-full w-full px-[max(2rem,calc((100%-68ch)/2))] pt-16 pb-8 text-[15px] leading-7 text-ink outline-none [&_[data-slate-placeholder=true]]:text-faint"
        disabled={disabled}
        placeholder="Write markdown…"
        spellCheck={false}
      />
    </Plate>
  );
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
    </div>
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
                applyBlockType(editor, item.value);
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

function CodeBlockElement(props: PlateElementProps) {
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
