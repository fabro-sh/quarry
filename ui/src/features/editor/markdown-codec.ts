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
  BaseSubscriptPlugin,
  BaseSuperscriptPlugin,
  BaseUnderlinePlugin,
} from '@platejs/basic-nodes';
import { BaseCodeBlockPlugin, BaseCodeLinePlugin, BaseCodeSyntaxPlugin } from '@platejs/code-block';
import { BaseLinkPlugin } from '@platejs/link';
import { BaseImagePlugin } from '@platejs/media';
import { BaseListPlugin } from '@platejs/list';
import { MarkdownPlugin } from '@platejs/markdown';
import { BaseParagraphPlugin, createSlateEditor, ElementApi, KEYS, type Descendant } from 'platejs';
import remarkGfm from 'remark-gfm';

import { remarkInlineMarks } from './remark-inline-marks';
import { stripPlaceholders } from './image';
import { applyMermaid, BaseMermaidPlugin, mermaidMdRules } from './mermaid';
import {
  BaseTableCellHeaderPlugin,
  BaseTableCellPlugin,
  BaseTablePlugin,
  BaseTableRowPlugin,
  tableMdRules,
} from './table';
import { applyWikiLinks, BaseWikiLinkPlugin, wikiLinkMdRules } from './wiki-link';

export type PlateValue = Array<Record<string, unknown>>;

/** The shared Base* plugins for Markdown (de)serialization, without the MarkdownPlugin. */
export const baseMarkdownPlugins = [
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
  BaseUnderlinePlugin,
  BaseSubscriptPlugin,
  BaseSuperscriptPlugin,
  BaseListPlugin,
  BaseLinkPlugin,
  BaseWikiLinkPlugin,
  BaseImagePlugin,
  BaseMermaidPlugin,
  BaseTablePlugin,
  BaseTableRowPlugin,
  BaseTableCellPlugin,
  BaseTableCellHeaderPlugin,
];

export function markdownToPlateValue(markdown: string): PlateValue {
  return applyMermaid(applyWikiLinks(editor().api.markdown.deserialize(markdown) as never)) as PlateValue;
}

export function plateValueToMarkdown(value: PlateValue): string {
  const cleaned = stripTrailingEmptyParagraphs(stripPlaceholders(applyWikiLinks(value as never)));
  return editor().api.markdown.serialize({ value: cleaned as never });
}

// The live editor keeps a trailing empty paragraph after the last block (via
// TrailingBlockPlugin) so there's always a line to type on — even below an atomic
// void block like a Mermaid diagram or image. That paragraph is editor
// scaffolding, not content: drop it on serialize so the markdown round-trips
// cleanly and a freshly-loaded document isn't spuriously marked dirty.
export function stripTrailingEmptyParagraphs(value: Descendant[]): Descendant[] {
  let end = value.length;
  while (end > 0 && isEmptyParagraph(value[end - 1])) end -= 1;
  return value.slice(0, end);
}

function isEmptyParagraph(node: Descendant): boolean {
  return (
    ElementApi.isElement(node) &&
    node.type === KEYS.p &&
    node.children.every((child) => 'text' in child && child.text === '')
  );
}

function editor() {
  return createSlateEditor({
    plugins: [
      ...baseMarkdownPlugins,
      MarkdownPlugin.configure({
        options: {
          remarkPlugins: [remarkGfm, remarkInlineMarks],
          rules: { ...wikiLinkMdRules, ...mermaidMdRules, ...tableMdRules },
        },
      }),
    ],
  });
}
