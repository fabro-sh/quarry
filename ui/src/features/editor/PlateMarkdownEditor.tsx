import {
  BlockquotePlugin,
  BoldPlugin,
  CodePlugin,
  H1Plugin,
  H2Plugin,
  H3Plugin,
  ItalicPlugin,
  StrikethroughPlugin,
} from '@platejs/basic-nodes/react';
import { CodeBlockPlugin, CodeLinePlugin, CodeSyntaxPlugin } from '@platejs/code-block/react';
import { LinkPlugin } from '@platejs/link/react';
import { ListPlugin } from '@platejs/list/react';
import { MarkdownPlugin } from '@platejs/markdown';
import { Bold, Code, Italic, Strikethrough } from 'lucide-react';
import { KEYS } from 'platejs';
import { useEffect, useRef, type ReactNode } from 'react';
import {
  ParagraphPlugin,
  Plate,
  PlateContent,
  useMarkToolbarButton,
  useMarkToolbarButtonState,
  usePlateEditor,
  type PlateEditor,
} from 'platejs/react';

import { cn } from '../../lib/utils';
import { markdownToPlateValue, plateValueToMarkdown, type PlateValue } from './markdown-codec';

const plateMarkdownPlugins = [
  ParagraphPlugin,
  H1Plugin,
  H2Plugin,
  H3Plugin,
  BlockquotePlugin,
  CodeBlockPlugin,
  CodeLinePlugin,
  CodeSyntaxPlugin,
  BoldPlugin,
  ItalicPlugin,
  CodePlugin,
  StrikethroughPlugin,
  ListPlugin,
  LinkPlugin,
  MarkdownPlugin,
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
      {disabled ? null : (
        <div className="sticky top-0 z-10 flex items-center gap-0.5 border-b border-line bg-surface px-3 py-1.5">
          <MarkButton label="Bold" nodeType={KEYS.bold}>
            <Bold size={15} />
          </MarkButton>
          <MarkButton label="Italic" nodeType={KEYS.italic}>
            <Italic size={15} />
          </MarkButton>
          <MarkButton label="Strikethrough" nodeType={KEYS.strikethrough}>
            <Strikethrough size={15} />
          </MarkButton>
          <MarkButton label="Inline code" nodeType={KEYS.code}>
            <Code size={15} />
          </MarkButton>
        </div>
      )}
      <PlateContent
        aria-label="Plate markdown editor"
        className="mx-auto min-h-[420px] w-full max-w-[68ch] px-8 py-8 text-[15px] leading-7 text-ink outline-none [&_[data-slate-placeholder=true]]:text-faint"
        disabled={disabled}
        placeholder="Write markdown…"
        spellCheck={false}
      />
    </Plate>
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
