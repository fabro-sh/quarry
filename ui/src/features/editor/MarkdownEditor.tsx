import { lazy, Suspense } from 'react';

const PlateMarkdownEditor = lazy(() =>
  import('./PlateMarkdownEditor').then((module) => ({ default: module.PlateMarkdownEditor }))
);

interface MarkdownEditorProps {
  content: string;
  disabled?: boolean;
  status: string;
  onChange: (content: string) => void;
}

export function MarkdownEditor({ content, disabled, status, onChange }: MarkdownEditorProps) {
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-surface" aria-label="Editor">
      <div className="min-h-0 flex-1 overflow-auto">
        <Suspense fallback={<div className="px-8 py-7 text-sm text-muted">Loading editor…</div>}>
          <PlateMarkdownEditor content={content} disabled={disabled} onChange={onChange} />
        </Suspense>
      </div>
      <div
        aria-label="Save status"
        className="flex h-8 shrink-0 items-center border-t border-line px-4 text-xs text-muted"
      >
        {status}
      </div>
    </section>
  );
}
