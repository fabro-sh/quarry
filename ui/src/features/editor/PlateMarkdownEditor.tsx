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
  UnderlinePlugin,
} from '@platejs/basic-nodes/react';
import { CodeBlockPlugin, CodeLinePlugin, CodeSyntaxPlugin } from '@platejs/code-block/react';
import { insertEmptyCodeBlock, toggleCodeBlock } from '@platejs/code-block';
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
  Code,
  Copy,
  Heading1,
  Heading2,
  Heading3,
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
  Underline,
} from 'lucide-react';
import { KEYS, NodeApi, type TElement, type TListElement } from 'platejs';
import { useEffect, useRef, useState, type ReactNode } from 'react';
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
import { remarkUnderline } from './remark-underline';

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
  ListPlugin.configure({ render: { belowNodes: BlockList } }),
  LinkPlugin,
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
  MarkdownPlugin.configure({ options: { remarkPlugins: [remarkGfm, remarkUnderline] } }),
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
                if (item.value === KEYS.codeBlock) {
                  if (!inCodeBlock) toggleCodeBlock(editor);
                } else if (inCodeBlock) {
                  // Unwrap to paragraphs first, then apply the target block type.
                  toggleCodeBlock(editor);
                  if (item.value !== KEYS.p) setBlockType(editor, item.value);
                } else {
                  setBlockType(editor, item.value);
                }
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
  return (
    <ListTag className="relative m-0 p-0" start={listStart} style={{ listStyleType }}>
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
