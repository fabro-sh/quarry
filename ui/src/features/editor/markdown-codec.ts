import {
  BaseBlockquotePlugin,
  BaseBoldPlugin,
  BaseCodePlugin,
  BaseH1Plugin,
  BaseH2Plugin,
  BaseH3Plugin,
  BaseH4Plugin,
  BaseH5Plugin,
  BaseH6Plugin,
  BaseHorizontalRulePlugin,
  BaseItalicPlugin,
  BaseStrikethroughPlugin,
} from '@platejs/basic-nodes';
import { BaseCodeBlockPlugin, BaseCodeLinePlugin, BaseCodeSyntaxPlugin } from '@platejs/code-block';
import { BaseLinkPlugin } from '@platejs/link';
import { BaseListPlugin } from '@platejs/list';
import { MarkdownPlugin } from '@platejs/markdown';
import { BaseParagraphPlugin, createSlateEditor } from 'platejs';
import remarkGfm from 'remark-gfm';

export type PlateValue = Array<Record<string, unknown>>;

export function markdownToPlateValue(markdown: string): PlateValue {
  return editor().api.markdown.deserialize(markdown) as PlateValue;
}

export function plateValueToMarkdown(value: PlateValue): string {
  return editor().api.markdown.serialize({ value: value as never });
}

function editor() {
  return createSlateEditor({
    plugins: [
      BaseParagraphPlugin,
      BaseH1Plugin,
      BaseH2Plugin,
      BaseH3Plugin,
      BaseH4Plugin,
      BaseH5Plugin,
      BaseH6Plugin,
      BaseBlockquotePlugin,
      BaseHorizontalRulePlugin,
      BaseCodeBlockPlugin,
      BaseCodeLinePlugin,
      BaseCodeSyntaxPlugin,
      BaseBoldPlugin,
      BaseItalicPlugin,
      BaseCodePlugin,
      BaseStrikethroughPlugin,
      BaseListPlugin,
      BaseLinkPlugin,
      MarkdownPlugin.configure({ options: { remarkPlugins: [remarkGfm] } }),
    ],
  } as never) as ReturnType<typeof createSlateEditor> & {
    api: {
      markdown: {
        deserialize: (markdown: string) => unknown;
        serialize: (options: { value: never }) => string;
      };
    };
  };
}
