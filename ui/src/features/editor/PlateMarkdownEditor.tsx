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
import { useEffect, useRef } from 'react';
import { ParagraphPlugin, Plate, PlateContent, usePlateEditor, type PlateEditor } from 'platejs/react';

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
    <div className="rounded-md border border-[#d9d6cc] bg-white">
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
        <PlateContent
          aria-label="Plate markdown editor"
          className="min-h-[420px] px-4 py-3 text-[15px] leading-7 text-[#1e211f] outline-none focus:ring-2 focus:ring-[#256f64]/25 [&_[data-slate-placeholder=true]]:text-[#62645e]"
          disabled={disabled}
          placeholder="Write markdown..."
          spellCheck={false}
        />
      </Plate>
    </div>
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
