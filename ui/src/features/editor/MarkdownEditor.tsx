import { lazy, Suspense } from 'react';
import { cn } from '../../lib/utils';
import type { CollabEditorConfig, EditorMode } from './PlateMarkdownEditor';
import type { ImageApi } from './image-element';
import type { WikiLinkApi } from './wiki-link-element';

export type { EditorMode };
export type { CollabEditorConfig };
export type { WikiLinkApi };
export type { ImageApi };

const PlateMarkdownEditor = lazy(() =>
  import('./PlateMarkdownEditor').then((module) => ({ default: module.PlateMarkdownEditor }))
);

interface MarkdownEditorProps {
  readonly author?: string;
  readonly className?: string;
  readonly collab?: CollabEditorConfig;
  readonly content: string;
  readonly image?: ImageApi;
  readonly mode: EditorMode;
  readonly onChange: (content: string) => void;
  readonly wikiLink?: WikiLinkApi;
}

export function MarkdownEditor({
  author = 'user',
  className,
  collab,
  content,
  mode,
  wikiLink,
  image,
  onChange,
}: MarkdownEditorProps) {
  return (
    <section
      className={cn('flex min-h-0 flex-1 flex-col bg-surface', className)}
      aria-label="Editor"
    >
      <div className="min-h-0 flex-1 overflow-auto">
        <Suspense
          fallback={
            <div className="pt-12 pl-[max(2rem,calc((100%-68ch)/2))] text-sm text-muted">
              Loading editor...
            </div>
          }
        >
          <PlateMarkdownEditor
            author={author}
            collab={collab}
            content={content}
            mode={mode}
            wikiLink={wikiLink}
            image={image}
            onChange={onChange}
          />
        </Suspense>
      </div>
    </section>
  );
}
