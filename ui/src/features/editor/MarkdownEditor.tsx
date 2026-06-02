import { lazy, Suspense } from 'react';
import type { EditorMode } from './PlateMarkdownEditor';
import type { ImageApi } from './image-element';
import type { WikiLinkApi } from './wiki-link-element';

export type { EditorMode };
export type { WikiLinkApi };
export type { ImageApi };

const PlateMarkdownEditor = lazy(() =>
  import('./PlateMarkdownEditor').then((module) => ({ default: module.PlateMarkdownEditor }))
);

interface MarkdownEditorProps {
  content: string;
  mode: EditorMode;
  wikiLink?: WikiLinkApi;
  image?: ImageApi;
  onChange: (content: string) => void;
}

export function MarkdownEditor({ content, mode, wikiLink, image, onChange }: MarkdownEditorProps) {
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-surface" aria-label="Editor">
      <div className="min-h-0 flex-1 overflow-auto">
        <Suspense fallback={<div className="px-8 py-7 text-sm text-muted">Loading editor…</div>}>
          <PlateMarkdownEditor content={content} mode={mode} wikiLink={wikiLink} image={image} onChange={onChange} />
        </Suspense>
      </div>
    </section>
  );
}
