import { Download, FileArchive, FileText, Image as ImageIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import { isTextContentType } from '../api/client';
import { type CollabSaveState } from '../features/collab/save-state';
import {
  MarkdownEditor,
  type CollabEditorConfig,
  type EditorMode,
  type ImageApi,
  type WikiLinkApi,
} from '../features/editor/MarkdownEditor';
import { cn } from '../lib/utils';

export interface DocumentBodyProps {
  readonly author: string;
  readonly byteSize?: number;
  readonly className?: string;
  readonly collabBaseUrl?: string;
  readonly collabEnabled: boolean;
  readonly collabRoomName?: string;
  readonly collabSessionId: string;
  readonly collabToken?: string;
  readonly content: string;
  readonly contentHash?: string | null;
  readonly contentType: string;
  readonly documentId: string;
  readonly href: string;
  readonly image?: ImageApi;
  readonly mode: EditorMode;
  readonly onChange: (content: string) => void;
  readonly onSaveStateChange: (state: CollabSaveState) => void;
  readonly path: string;
  readonly wikiLink: WikiLinkApi;
}

export function DocumentBody({
  author,
  byteSize,
  className,
  collabBaseUrl,
  collabEnabled,
  collabRoomName,
  collabSessionId,
  collabToken,
  content,
  contentHash,
  contentType,
  documentId,
  href,
  image,
  mode,
  onChange,
  onSaveStateChange,
  path,
  wikiLink,
}: DocumentBodyProps): ReactNode {
  if (isMarkdownDocument(path, contentType)) {
    const collab: CollabEditorConfig | undefined = collabEnabled && documentId
      ? {
          documentId,
          baseUrl: collabBaseUrl,
          onSaveStateChange,
          roomName: collabRoomName,
          sessionId: collabSessionId,
          token: collabToken,
        }
      : undefined;
    return (
      <MarkdownEditor
        author={author}
        className={className}
        collab={collab}
        content={content}
        image={image}
        mode={mode}
        onChange={onChange}
        wikiLink={wikiLink}
      />
    );
  }

  if (isTextContentType(contentType)) {
    return (
      <TextSourcePreview
        className={className}
        content={content}
        contentType={contentType}
        path={path}
      />
    );
  }
  if (isImageContentType(contentType)) {
    return (
      <ImagePreview
        byteSize={byteSize}
        className={className}
        contentType={contentType}
        href={href}
        path={path}
      />
    );
  }
  return (
    <BinaryPreview
      byteSize={byteSize}
      className={className}
      contentHash={contentHash}
      contentType={contentType}
      href={href}
      path={path}
    />
  );
}

interface PreviewProps {
  readonly className?: string;
  readonly contentType: string;
  readonly path: string;
}

interface TextSourcePreviewProps extends PreviewProps {
  readonly content: string;
}

function TextSourcePreview({
  className,
  content,
  contentType,
  path,
}: TextSourcePreviewProps): ReactNode {
  return (
    <section
      aria-label="Text document preview"
      className={cn('flex min-h-0 flex-1 flex-col bg-surface', className)}
    >
      <div className="flex h-11 shrink-0 items-center gap-3 border-b border-line px-3 text-sm text-body">
        <FileText className="shrink-0 text-accent" size={15} />
        <span className="min-w-0 flex-1 truncate">{path}</span>
        <span className="shrink-0 text-xs text-muted">{contentType}</span>
        <span className="shrink-0 text-xs text-muted">Read-only</span>
      </div>
      <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap px-8 py-6 font-mono text-[13px] leading-6 text-body">
        {content}
      </pre>
    </section>
  );
}

interface ImagePreviewProps extends PreviewProps {
  readonly byteSize?: number;
  readonly href: string;
}

function ImagePreview({
  byteSize,
  className,
  contentType,
  href,
  path,
}: ImagePreviewProps): ReactNode {
  return (
    <section
      aria-label="Image preview"
      className={cn('flex min-h-0 flex-1 flex-col bg-surface', className)}
    >
      <div className="flex h-11 shrink-0 items-center gap-3 border-b border-line px-3 text-sm text-body">
        <ImageIcon className="shrink-0 text-accent" size={15} />
        <span className="min-w-0 flex-1 truncate">{path}</span>
        <span className="shrink-0 text-xs text-muted">{contentType}</span>
        {typeof byteSize === 'number' ? (
          <span className="shrink-0 text-xs tabular-nums text-muted">{formatBytes(byteSize)}</span>
        ) : null}
      </div>
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto p-6">
        <img
          alt={`${path} preview`}
          className="max-h-full max-w-full rounded-sm object-contain outline-1 -outline-offset-1 outline-black/10"
          src={href}
        />
      </div>
    </section>
  );
}

interface BinaryPreviewProps extends PreviewProps {
  readonly byteSize?: number;
  readonly contentHash?: string | null;
  readonly href: string;
}

function BinaryPreview({
  byteSize,
  className,
  contentHash,
  contentType,
  href,
  path,
}: BinaryPreviewProps): ReactNode {
  return (
    <section
      aria-label="Binary document preview"
      className={cn(
        'flex min-h-0 flex-1 items-center justify-center bg-surface p-6',
        className
      )}
    >
      <div className="w-full max-w-xl rounded-md border border-line bg-raised p-5">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-accent-tint text-accent">
            <FileArchive size={20} />
          </div>
          <div className="min-w-0 flex-1">
            <h2 className="truncate text-sm font-semibold text-ink">{path}</h2>
            <p className="mt-1 text-sm text-muted">This binary document is available for download.</p>
          </div>
          <a className={secondaryButton} download={documentBasename(path)} href={href}>
            <Download size={15} />
            Download
          </a>
        </div>
        <dl className="mt-5 grid grid-cols-[120px_1fr] gap-x-3 gap-y-2 text-sm">
          <dt className="text-muted">Path</dt>
          <dd className="min-w-0 truncate font-mono text-body">{path}</dd>
          <dt className="text-muted">Content type</dt>
          <dd className="min-w-0 truncate font-mono text-body">{contentType}</dd>
          <dt className="text-muted">Size</dt>
          <dd className="tabular-nums text-body">
            {typeof byteSize === 'number' ? formatBytes(byteSize) : 'Unknown'}
          </dd>
          {contentHash ? (
            <>
              <dt className="text-muted">Hash</dt>
              <dd className="min-w-0 truncate font-mono text-body">{contentHash}</dd>
            </>
          ) : null}
        </dl>
      </div>
    </section>
  );
}

function isMarkdownDocument(path: string, contentType: string): boolean {
  const mediaType = contentType.split(';', 1)[0]?.trim().toLowerCase();
  return (
    mediaType === 'text/markdown' ||
    mediaType === 'text/x-markdown' ||
    /\.(md|markdown)$/i.test(path)
  );
}

function isImageContentType(contentType: string): boolean {
  return contentType.split(';', 1)[0]?.trim().toLowerCase().startsWith('image/') ?? false;
}

function documentBasename(path: string): string {
  return path.split('/').filter(Boolean).at(-1) ?? path;
}

function formatBytes(bytes: number): string {
  if (bytes === 1) return '1 byte';
  if (bytes < 1024) return `${bytes} bytes`;
  const units = ['KB', 'MB', 'GB', 'TB'] as const;
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const formatted = Number.isInteger(value) ? String(value) : value.toFixed(1);
  return `${formatted} ${units[unitIndex]}`;
}

const secondaryButton =
  'inline-flex h-8 items-center gap-1.5 rounded-md border border-line-strong bg-raised px-3 text-sm text-body transition-colors hover:bg-well';
