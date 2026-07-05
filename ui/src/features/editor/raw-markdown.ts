import { createSlatePlugin, type TElement, type TText } from 'platejs';

// Blocks the canonical row model cannot represent (wikilink-bearing
// paragraphs degraded at checkpoint, unsupported constructs from imports)
// are stored as `raw_markdown` rows carrying their Markdown source in a
// `markdown` attr, and arrive in the session doc as `raw_markdown` elements
// with the same attr. The element is an atomic VOID, so the caret cannot
// enter it and typing cannot corrupt the source — editing these blocks
// belongs to the whole-file surfaces (Git, FUSE, CLI) and agents.
//
// This headless half runs inside the mirror-serializer worker; the browser
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

// Serialize the block back to its verbatim source for the local Markdown
// mirror (downloads, diffs). mdast `html` nodes emit their value verbatim —
// remark-stringify's raw escape hatch — which is exactly what an opaque
// Markdown block needs. (Persistence never takes this path: checkpoints
// project the session doc server-side, where the `markdown` attr is
// authoritative.)
export const rawMarkdownMdRules = {
  [RAW_MARKDOWN_KEY]: {
    serialize: (node: TRawMarkdownElement) => ({
      type: 'html' as const,
      value: node.markdown ?? '',
    }),
  },
};
