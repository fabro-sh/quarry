import { lazy, Suspense } from 'react';
import { cn } from '../../lib/utils';
import type { DocumentEditorConfig, EditorMode, ImageApi, WikiLinkApi } from './editor-types';
export type { DocumentEditorConfig, EditorMode, ImageApi, WikiLinkApi } from './editor-types';
const DocumentMarkdownEditor = lazy(() => import('./DocumentMarkdownEditor').then((module) => ({ default: module.DocumentMarkdownEditor })));

interface MarkdownEditorProps {
  readonly author?: string;
  readonly className?: string;
  readonly document: DocumentEditorConfig;
  readonly documentUrl: string;
  readonly image?: ImageApi;
  readonly mode: EditorMode;
  readonly wikiLink?: WikiLinkApi;
}
export function MarkdownEditor({ author = 'user', className, document, documentUrl, mode, wikiLink, image }: MarkdownEditorProps) {
  return <section className={cn('flex min-h-0 flex-1 flex-col bg-surface', className)} aria-label="Editor">
    <div className="min-h-0 flex-1 overflow-auto">
      <Suspense fallback={<p role="status" className="p-8">Loading editor…</p>}>
        <DocumentMarkdownEditor key={document.documentId} documentUrl={documentUrl} config={document} author={author} mode={mode} image={image} wikiLink={wikiLink} reviewInPanel />
      </Suspense>
    </div>
  </section>;
}
