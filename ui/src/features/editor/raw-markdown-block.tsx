import { PlateElement, type PlateElementProps } from 'platejs/react';

import { BaseRawMarkdownPlugin, type TRawMarkdownElement } from './raw-markdown';

// The browser renders a raw_markdown block's source read-only in a styled
// block. Before this renderer they displayed as empty space: invisible data.
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

export const RawMarkdownPlugin = BaseRawMarkdownPlugin.withComponent(RawMarkdownBlock);
