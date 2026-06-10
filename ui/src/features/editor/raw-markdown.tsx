import { createSlatePlugin, type TElement, type TText } from 'platejs';
import { PlateElement, type PlateElementProps } from 'platejs/react';

// Blocks the canonical row model cannot represent (wikilink-bearing
// paragraphs degraded at checkpoint, unsupported constructs from imports)
// are stored as `raw_markdown` rows carrying their Markdown source in a
// `markdown` attr, and arrive in the session doc as `raw_markdown` elements
// with the same attr. The browser renders that source read-only in a styled
// block: the element is an atomic VOID, so the caret cannot enter it and
// typing cannot corrupt the source — editing these blocks belongs to the
// whole-file surfaces (Git, FUSE, CLI) and agents. Before this renderer
// they displayed as empty space: invisible data.

export const RAW_MARKDOWN_KEY = 'raw_markdown';

export interface TRawMarkdownElement extends TElement {
  type: typeof RAW_MARKDOWN_KEY;
  markdown?: string;
  children: [TText];
}

export function RawMarkdownBlock(props: PlateElementProps<TRawMarkdownElement>) {
  const source = props.element.markdown ?? '';
  return (
    <PlateElement {...props} className="my-1">
      <div contentEditable={false}>
        <pre
          className="overflow-x-auto whitespace-pre-wrap rounded-sm border border-line bg-well/60 px-3 py-2 font-mono text-[13px] leading-6 text-body"
          data-testid="raw-markdown-block"
          title="Raw Markdown block (shown as source)"
        >
          {source}
        </pre>
      </div>
      {props.children}
    </PlateElement>
  );
}

export const RawMarkdownPlugin = createSlatePlugin({
  key: RAW_MARKDOWN_KEY,
  node: { isElement: true, isVoid: true },
}).withComponent(RawMarkdownBlock);

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
