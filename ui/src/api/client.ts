import type {
  ConflictRecord,
  DocumentListEntry,
  DocumentVersion,
  DocumentVersionContent,
  Library,
  LinkCollection,
  SearchResponse,
  SearchSuggestion,
  VersionDiff,
  WriteOutcome,
} from './generated/types';

export interface LoadedDocument {
  documentId: string;
  path: string;
  content: string;
  contentType: string;
  etag: string;
}

export interface SavedDocument {
  outcome: WriteOutcome;
  etag: string;
}

export interface GitPeer {
  id: string;
  library_id: string;
  kind: string;
  config: Record<string, unknown>;
}

export interface GitSyncResult {
  imported_paths: string[];
  exported_paths: string[];
  conflict_paths: string[];
  conflicts: ConflictRecord[];
  commit_id: string | null;
}

export interface GitImportResult {
  imported_paths: string[];
  transaction_id: string;
}

export interface GitExportResult {
  exported_paths: string[];
  commit_id: string | null;
}

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly payload: unknown = null
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

export class ApiPreconditionError extends ApiError {
  constructor(message: string, payload: unknown = null) {
    super(message, 412, payload);
    this.name = 'ApiPreconditionError';
  }
}

export const listLibraries = () => jsonRequest<Library[]>('/v1/libraries');

export const createLibrary = (slug: string) =>
  jsonRequest<Library>('/v1/libraries', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ slug }),
  });

export const listDocuments = (library: string) =>
  jsonRequest<DocumentListEntry[]>(`/v1/libraries/${segment(library)}/documents`);

export async function getDocument(library: string, path: string): Promise<LoadedDocument> {
  const response = await fetch(documentHref(library, path));
  await assertOk(response);
  const contentType = response.headers.get('content-type') ?? 'application/octet-stream';
  return {
    documentId: response.headers.get('x-quarry-document-id') ?? '',
    path,
    content: isTextContentType(contentType) ? await response.text() : '',
    contentType,
    etag: response.headers.get('etag') ?? '',
  };
}

export function putDocument(
  library: string,
  path: string,
  content: string,
  etag: string,
  contentType = 'text/markdown'
) {
  return writeDocument(library, path, content, {
    'If-Match': etag,
    'content-type': contentType,
  });
}

export function createDocument(
  library: string,
  path: string,
  content = '',
  contentType = 'text/markdown'
) {
  return writeDocument(library, path, content, {
    'If-None-Match': '*',
    'content-type': contentType,
  });
}

// Create a binary document (e.g. a dropped image) from raw bytes. Uses
// If-None-Match:* so an identical asset already at the path stays put — callers
// treat the resulting 412 (ApiPreconditionError) as success.
export async function putBinaryDocument(
  library: string,
  path: string,
  blob: Blob,
  contentType: string
): Promise<void> {
  const response = await fetch(documentHref(library, path), {
    method: 'PUT',
    headers: { 'If-None-Match': '*', 'content-type': contentType },
    body: blob,
  });
  await assertOk(response);
}

export async function moveDocument(library: string, fromPath: string, toPath: string) {
  return jsonRequest(`/v1/libraries/${segment(library)}/documents/${pathSegments(fromPath)}/move`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ to_path: toPath }),
  });
}

export async function deleteDocument(library: string, path: string) {
  return jsonRequest(`/v1/libraries/${segment(library)}/documents/${pathSegments(path)}`, {
    method: 'DELETE',
  });
}

export const listConflicts = (library: string) =>
  jsonRequest<ConflictRecord[]>(`/v1/libraries/${segment(library)}/conflicts`);

export const resolveConflict = (library: string, conflict: string) =>
  jsonRequest<ConflictRecord>(
    `/v1/libraries/${segment(library)}/conflicts/${segment(conflict)}/resolve`,
    { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' }
  );

export const searchDocuments = (library: string, query: string) =>
  jsonRequest<SearchResponse>(
    `/v1/libraries/${segment(library)}/search?q=${encodeURIComponent(query)}&limit=50`
  );

export const suggestDocuments = (library: string, query: string) =>
  jsonRequest<SearchSuggestion[]>(
    `/v1/libraries/${segment(library)}/search/suggest?q=${encodeURIComponent(query)}&limit=20`
  );

export const outgoingLinks = (library: string, path: string) =>
  jsonRequest<LinkCollection>(
    `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/outgoing-links`
  );

export const backlinks = (library: string, path: string) =>
  jsonRequest<LinkCollection>(
    `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/backlinks`
  );

export const versions = (library: string, path: string) =>
  jsonRequest<DocumentVersion[]>(
    `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/versions`
  );

export const documentVersion = (library: string, path: string, version: string) =>
  jsonRequest<DocumentVersionContent>(
    `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/versions/${segment(version)}`
  );

export const diffVersion = (library: string, path: string, version: string, against?: string) =>
  jsonRequest<VersionDiff>(
    `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/versions/${segment(
      version
    )}/diff${against ? `?against=${encodeURIComponent(against)}` : ''}`
  );

export async function restoreVersion(
  library: string,
  path: string,
  version: string
): Promise<SavedDocument> {
  const response = await fetch(
    `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/versions/${segment(
      version
    )}/restore`,
    { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' }
  );
  await assertOk(response);
  return {
    outcome: (await response.json()) as WriteOutcome,
    etag: response.headers.get('etag') ?? '',
  };
}

export const listGitPeers = (library: string) =>
  jsonRequest<GitPeer[]>(`/v1/libraries/${segment(library)}/git/peers`);

export const createGitPeer = (
  library: string,
  request: { repo: string; branch?: string; remote?: string }
) =>
  jsonRequest<GitPeer>(`/v1/libraries/${segment(library)}/git/peers`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(request),
  });

export const gitImport = (library: string, repo: string) =>
  jsonRequest<GitImportResult>(`/v1/libraries/${segment(library)}/git/import`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ repo }),
  });

export const gitExport = (
  library: string,
  request: { repo: string; branch?: string; force_large?: boolean; frontmatter_markdown?: boolean }
) =>
  jsonRequest<GitExportResult>(`/v1/libraries/${segment(library)}/git/export`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(request),
  });

export const gitPull = (library: string, peer: string) => gitPeerOperation(library, peer, 'pull');
export const gitPush = (library: string, peer: string) => gitPeerOperation(library, peer, 'push');
export const gitSync = (library: string, peer: string) => gitPeerOperation(library, peer, 'sync');

function gitPeerOperation(library: string, peer: string, operation: 'pull' | 'push' | 'sync') {
  return jsonRequest<GitSyncResult>(
    `/v1/libraries/${segment(library)}/git/peers/${segment(peer)}/${operation}`,
    { method: 'POST' }
  );
}

async function writeDocument(
  library: string,
  path: string,
  content: string,
  headers: Record<string, string>
): Promise<SavedDocument> {
  const response = await fetch(documentHref(library, path), {
    method: 'PUT',
    headers,
    body: content,
  });
  await assertOk(response);
  return {
    outcome: (await response.json()) as WriteOutcome,
    etag: response.headers.get('etag') ?? '',
  };
}

async function jsonRequest<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  await assertOk(response);
  return (await response.json()) as T;
}

async function assertOk(response: Response) {
  if (response.ok) return;
  const payload = await readErrorPayload(response);
  const message =
    payload && typeof payload === 'object' && 'error' in payload
      ? String((payload as { error: unknown }).error)
      : response.statusText;
  if (response.status === 412) {
    throw new ApiPreconditionError(message, payload);
  }
  throw new ApiError(message, response.status, payload);
}

async function readErrorPayload(response: Response) {
  const contentType = response.headers.get('content-type') ?? '';
  if (!contentType.includes('application/json')) return null;
  try {
    return await response.json();
  } catch {
    return null;
  }
}

export function documentHref(library: string, path: string) {
  return `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}`;
}

export function isTextContentType(contentType: string) {
  const normalized = contentType.split(';', 1)[0]?.trim().toLowerCase() ?? '';
  if (
    normalized.startsWith('image/') ||
    normalized.startsWith('audio/') ||
    normalized.startsWith('video/')
  ) {
    return false;
  }
  return (
    normalized.startsWith('text/') ||
    normalized === 'application/json' ||
    normalized === 'application/ld+json' ||
    normalized === 'application/xml' ||
    normalized === 'application/yaml' ||
    normalized === 'application/x-yaml' ||
    normalized === 'application/toml' ||
    normalized.endsWith('+json') ||
    normalized.endsWith('+xml') ||
    normalized.endsWith('+yaml')
  );
}

function segment(value: string) {
  return encodeURIComponent(value);
}

function pathSegments(path: string) {
  return path.split('/').map(segment).join('/');
}
