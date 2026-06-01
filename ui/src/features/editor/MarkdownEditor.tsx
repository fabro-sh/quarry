import { lazy, Suspense } from 'react';
import type { EditorMode } from './PlateMarkdownEditor';
import type { WikiLinkApi } from './wiki-link-element';

export type { EditorMode };
export type { WikiLinkApi };

const PlateMarkdownEditor = lazy(() =>
  import('./PlateMarkdownEditor').then((module) => ({ default: module.PlateMarkdownEditor }))
);

interface MarkdownEditorProps {
  content: string;
  mode: EditorMode;
  wikiLink?: WikiLinkApi;
  onChange: (content: string) => void;
}

export function MarkdownEditor({ content, mode, wikiLink, onChange }: MarkdownEditorProps) {
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-surface" aria-label="Editor">
      <div className="min-h-0 flex-1 overflow-auto">
        <Suspense fallback={<div className="px-8 py-7 text-sm text-muted">Loading editor…</div>}>
          <PlateMarkdownEditor content={content} mode={mode} wikiLink={wikiLink} onChange={onChange} />
        </Suspense>
      </div>
    </section>
  );
}
