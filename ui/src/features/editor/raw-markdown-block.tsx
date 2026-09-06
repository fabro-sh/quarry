import { PlateElement, useReadOnly, type PlateElementProps } from 'platejs/react';
import { useSourceDraft } from './source-draft-editor';

import { BaseRawMarkdownPlugin, type TRawMarkdownElement } from './raw-markdown';

export function RawMarkdownBlock(props: PlateElementProps<TRawMarkdownElement>) {
  const source = props.element.markdown ?? '';
  const readOnly = useReadOnly();
  const edit = useSourceDraft(props.editor, String(props.element.id), 'markdown', source);
  return (
    <PlateElement {...props} className="my-1">
      <div contentEditable={false}>
        {edit.draft ? <form onSubmit={(event) => { event.preventDefault(); edit.save(); }}>
          <textarea aria-label="Markdown source" className="min-h-32 w-full rounded border border-line bg-well px-3 py-2 font-mono text-sm" readOnly={readOnly} value={edit.draft.value} onChange={(event) => edit.update(event.target.value)} autoFocus />
          {edit.draft.error && <p role="alert" className="text-sm text-danger">{edit.draft.error}</p>}
          <div className="flex gap-3 text-sm"><button disabled={readOnly} type="submit">Save source</button><button type="button" onClick={edit.cancel}>Cancel</button></div>
        </form> : <><pre
          className="overflow-x-auto whitespace-pre-wrap rounded-sm border border-line bg-well/60 px-3 py-2 font-mono text-[13px] leading-6 text-body"
          data-testid="raw-markdown-block"
          title="Raw Markdown block (shown as source)"
        >
          {source}
        </pre>
        {!readOnly && <button type="button" className="py-1 text-xs text-muted hover:text-body" onClick={edit.begin}>Edit source</button>}
        </>}
      </div>
      {props.children}
    </PlateElement>
  );
}

export const RawMarkdownPlugin = BaseRawMarkdownPlugin.withComponent(RawMarkdownBlock);
