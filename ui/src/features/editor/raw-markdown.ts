import { createSlatePlugin, type TElement, type TText } from 'platejs';

// Imported constructs without a rich block type retain their exact source in
// a native `raw_markdown` block. Its Slate element is void so ordinary typing
// cannot enter the source. The source form submits an explicit native update.
//
// This headless half normalizes private clipboard fragments; the browser
// renderer lives in raw-markdown-block.tsx.

export const RAW_MARKDOWN_KEY = 'raw_markdown';

export interface TRawMarkdownElement extends TElement {
  type: typeof RAW_MARKDOWN_KEY;
  markdown?: string;
  children: [TText];
}

export const BaseRawMarkdownPlugin = createSlatePlugin({
  key: RAW_MARKDOWN_KEY,
  node: { isElement: true, isVoid: true },
});

// Private clipboard serialization emits the source verbatim. Durable saves,
// downloads and diffs use the native document's Markdown projection.
export const rawMarkdownMdRules = {
  [RAW_MARKDOWN_KEY]: {
    serialize: (node: TRawMarkdownElement) => ({
      type: 'html' as const,
      value: node.markdown ?? '',
    }),
  },
};
