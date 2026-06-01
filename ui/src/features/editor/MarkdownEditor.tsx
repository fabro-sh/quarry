import { lazy, Suspense } from 'react';
import type { EditorMode } from './PlateMarkdownEditor';

export type { EditorMode };

const PlateMarkdownEditor = lazy(() =>
  import('./PlateMarkdownEditor').then((module) => ({ default: module.PlateMarkdownEditor }))
);

interface MarkdownEditorProps {
  content: string;
  mode: EditorMode;
  onChange: (content: string) => void;
}

export function MarkdownEditor({ content, mode, onChange }: MarkdownEditorProps) {
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-surface" aria-label="Editor">
      <div className="min-h-0 flex-1 overflow-auto">
        <Suspense fallback={<div className="px-8 py-7 text-sm text-muted">Loading editor…</div>}>
          <PlateMarkdownEditor content={content} mode={mode} onChange={onChange} />
        </Suspense>
      </div>
    </section>
  );
}
