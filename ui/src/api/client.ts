import type {
  AgentReviewResponse,
  BlockTransactionAck,
  BlockTransactionErrorCode,
  BlockTransactionRequest,
  BlockTreeResponse,
  ConflictRecord,
  CollabInviteToken,
  DocumentListEntry,
  DocumentHistoryEntry,
  DocumentVersion,
  DocumentVersionContent,
  Library,
  LinkCollection,
  SearchResponse,
  SearchSuggestion,
  VersionDiff,
  WriteOutcome,
} from './generated/types';
import {
  type DocumentRef,
  documentRefPath,
  documentRefUrl,
  pathSegments,
  segment,
} from './document-ref';

export interface LoadedDocument {
  documentId: string;
  path: string;
  content: string;
  contentType: string;
  etag: string;
  expiresAt?: string;
}

export interface SavedDocument {
  outcome: WriteOutcome;
  etag: string;
}

export interface DocumentMutationOptions {
  originId?: string;
  transactionActor?: string;
  transactionMessage?: string;
  transactionProvenance?: Record<string, unknown>;
}

export interface CreateTmpDocumentRequest {
  content?: string;
  metadata?: Record<string, unknown>;
  expiresAt?: string;
}

export interface Capabilities {
  tmp_documents: boolean;
  lib_documents: boolean;
}

export interface PromoteTmpDocumentRequest {
  library: string;
  path: string;
  ifMatch?: string;
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

// Library presence entries carry extra `library`/`path` fields on the wire;
// the UI displays only this scope-agnostic shape, so one response type serves
// both scopes.
export interface AgentPresenceDisplayEntry {
  documentId: string;
  agentId: string;
  status: string;
  by?: string;
  updatedAt: string;
}

export interface AgentPresenceListResponse {
  presence: AgentPresenceDisplayEntry[];
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

/**
 * A typed `{code, retryable, message}` failure from the block transaction
 * gateway. `retryable: true` means "refetch blocks and resubmit with a fresh
 * clock"; `retryable: false` means the ops as stated can never succeed.
 */
export class BlockTransactionError extends ApiError {
  constructor(
    message: string,
    status: number,
    public readonly code: BlockTransactionErrorCode,
    public readonly retryable: boolean,
    payload: unknown = null
  ) {
    super(message, status, payload);
    this.name = 'BlockTransactionError';
  }
}

export const legacyCapabilities: Capabilities = {
  tmp_documents: true,
  lib_documents: true,
};

export async function getCapabilities() {
  try {
    return await jsonRequest<Capabilities>('/v1/capabilities');
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) return legacyCapabilities;
    throw error;
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

export async function getDocument(ref: DocumentRef): Promise<LoadedDocument> {
  const response = await fetch(documentRefUrl(ref));
  await assertOk(response);
  const contentType = response.headers.get('content-type') ?? 'application/octet-stream';
  return {
    documentId: response.headers.get('x-quarry-document-id') ?? '',
    path: documentRefPath(ref),
    content: isTextContentType(contentType) ? await response.text() : '',
    contentType,
    etag: response.headers.get('etag') ?? '',
    expiresAt: response.headers.get('x-quarry-expires-at') ?? undefined,
  };
}

export async function createTmpDocument(
  request: CreateTmpDocumentRequest = {}
): Promise<SavedDocument> {
  const response = await fetch('/v1/tmp/documents', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      content: request.content,
      content_type: 'text/markdown',
      metadata: request.metadata,
      expires_at: request.expiresAt,
    }),
  });
  await assertOk(response);
  return {
    outcome: (await response.json()) as WriteOutcome,
    etag: response.headers.get('etag') ?? '',
  };
}

export function putDocument(
  ref: DocumentRef,
  content: string,
  etag: string,
  contentType = 'text/markdown',
  options: DocumentMutationOptions = {}
): Promise<SavedDocument> {
  const headers = mutationHeaders(options, {
    'If-Match': etag,
    'content-type': contentType,
  });
  return writeDocument(ref, content, headers);
}

export function createDocument(
  library: string,
  path: string,
  content = '',
  contentType = 'text/markdown',
  options: DocumentMutationOptions = {}
) {
  return writeDocument({ scope: 'library', library, path }, content, mutationHeaders(options, {
    'If-None-Match': '*',
    'content-type': contentType,
  }));
}

// Create a binary document (e.g. a dropped image) from raw bytes. Uses
// If-None-Match:* so an identical asset already at the path stays put — callers
// treat the resulting 412 (ApiPreconditionError) as success.
export async function putBinaryDocument(
  library: string,
  path: string,
  blob: Blob,
  contentType: string,
  options: DocumentMutationOptions = {}
): Promise<void> {
  const response = await fetch(documentHref(library, path), {
    method: 'PUT',
    headers: mutationHeaders(options, { 'If-None-Match': '*', 'content-type': contentType }),
    body: blob,
  });
  await assertOk(response);
}

export async function moveDocument(
  library: string,
  fromPath: string,
  toPath: string,
  options: DocumentMutationOptions = {}
) {
  return jsonRequest(`/v1/libraries/${segment(library)}/documents/${pathSegments(fromPath)}/move`, {
    method: 'POST',
    headers: mutationHeaders(options, { 'content-type': 'application/json' }),
    body: JSON.stringify({ to_path: toPath }),
  });
}

export async function deleteDocument(ref: DocumentRef, options: DocumentMutationOptions = {}) {
  return jsonRequest(documentRefUrl(ref), {
    method: 'DELETE',
    headers: mutationHeaders(options),
  });
}

export const listConflicts = (library: string) =>
  jsonRequest<ConflictRecord[]>(`/v1/libraries/${segment(library)}/conflicts`);

export const resolveConflict = (library: string, conflict: string) =>
  jsonRequest<ConflictRecord>(
    `/v1/libraries/${segment(library)}/conflicts/${segment(conflict)}/resolve`,
    { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' }
  );

export const createCollabInvite = (
  library: string,
  path: string,
  request: { byHint?: string; role?: 'editor' | 'viewer' } = {}
) =>
  jsonRequest<CollabInviteToken>(`/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/share`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ byHint: request.byHint, role: request.role ?? 'editor' }),
  });

export async function fetchAgentPrompt(
  params:
    | { scope: 'tmp'; secret: string }
    | { scope: 'library'; library: string; path: string; token: string }
): Promise<string> {
  const url =
    params.scope === 'tmp'
      ? `/v1/tmp/documents/${segment(params.secret)}/agent-prompt`
      : `/v1/libraries/${segment(params.library)}/documents/${pathSegments(params.path)}/agent-prompt?token=${segment(params.token)}`;
  const response = await fetch(url);
  await assertOk(response);
  return response.text();
}

export const listAgentPresence = (ref: DocumentRef) =>
  jsonRequest<AgentPresenceListResponse>(documentRefUrl(ref, '/presence'));

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

export const versions = (ref: DocumentRef) =>
  jsonRequest<DocumentHistoryEntry[]>(documentRefUrl(ref, '/versions'));

export const rawVersions = (library: string, path: string) =>
  jsonRequest<DocumentVersion[]>(
    `/v1/libraries/${segment(library)}/documents/${pathSegments(path)}/versions/raw`
  );

export const documentVersion = (ref: DocumentRef, version: string) =>
  jsonRequest<DocumentVersionContent>(documentRefUrl(ref, `/versions/${segment(version)}`));

export const setDocumentTtl = (ref: DocumentRef, expiresAt: string | null) =>
  jsonRequest<{ expires_at: string | null }>(documentRefUrl(ref, '/ttl'), {
    method: 'PATCH',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ expires_at: expiresAt }),
  });

export const promoteTmpDocument = (secret: string, request: PromoteTmpDocumentRequest) =>
  jsonRequest<DocumentListEntry>(`/v1/tmp/documents/${segment(secret)}/promote`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      library: request.library,
      path: request.path,
      if_match: request.ifMatch,
    }),
  });

export const diffVersion = (ref: DocumentRef, version: string, against?: string) =>
  jsonRequest<VersionDiff>(
    documentRefUrl(
      ref,
      `/versions/${segment(version)}/diff${against ? `?against=${encodeURIComponent(against)}` : ''}`
    )
  );

export async function restoreVersion(
  ref: DocumentRef,
  version: string,
  options: DocumentMutationOptions = {}
): Promise<SavedDocument> {
  const response = await fetch(documentRefUrl(ref, `/versions/${segment(version)}/restore`), {
    method: 'POST',
    headers: mutationHeaders(options, { 'content-type': 'application/json' }),
    body: '{}',
  });
  await assertOk(response);
  return {
    outcome: (await response.json()) as WriteOutcome,
    etag: response.headers.get('etag') ?? '',
  };
}

// Canonical block rows plus the current document clock. Reading a markdown
// document that has no stored projection materializes one server-side, so the
// returned block ids are durable and addressable by transactions.
export const getDocumentBlocks = (ref: DocumentRef) =>
  jsonRequest<BlockTreeResponse>(documentRefUrl(ref, '/blocks'));

// The rows-backed review projection: comments and suggestions with their
// row anchors and states (open/resolved/orphaned/invalidated), plus diff3
// conflict review items. Resolved items are included so the Comments panel
// can show the document's full review record.
export const getDocumentReview = (ref: DocumentRef) =>
  jsonRequest<AgentReviewResponse>(documentRefUrl(ref, '/review?includeResolved=1'));

// Submits one semantic block transaction. Non-2xx responses with the gateway's
// typed `{code, retryable, message}` body throw BlockTransactionError; other
// failures fall back to the generic ApiError mapping.
export async function postBlockTransaction(
  ref: DocumentRef,
  request: BlockTransactionRequest
): Promise<BlockTransactionAck> {
  const response = await fetch(documentRefUrl(ref, '/transactions'), {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(request),
  });
  return readBlockTransactionResponse(response);
}

async function readBlockTransactionResponse(response: Response): Promise<BlockTransactionAck> {
  if (response.ok) return (await response.json()) as BlockTransactionAck;
  const payload = await readErrorPayload(response);
  if (isBlockTransactionFailure(payload)) {
    throw new BlockTransactionError(
      payload.message,
      response.status,
      payload.code,
      payload.retryable,
      payload
    );
  }
  const message =
    payload && typeof payload === 'object' && 'error' in payload
      ? String(payload.error)
      : response.statusText;
  throw new ApiError(message, response.status, payload);
}

function isBlockTransactionFailure(
  payload: unknown
): payload is { code: BlockTransactionErrorCode; retryable: boolean; message: string } {
  return (
    typeof payload === 'object' &&
    payload !== null &&
    'code' in payload &&
    typeof payload.code === 'string' &&
    'retryable' in payload &&
    typeof payload.retryable === 'boolean' &&
    'message' in payload &&
    typeof payload.message === 'string'
  );
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
  ref: DocumentRef,
  content: string,
  headers: Record<string, string>
): Promise<SavedDocument> {
  const response = await fetch(documentRefUrl(ref), {
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

function mutationHeaders(
  options: DocumentMutationOptions = {},
  headers: Record<string, string> = {}
) {
  const next = { ...headers };
  if (options.originId) next['X-Quarry-Origin-Id'] = options.originId;
  if (options.transactionActor) {
    // fetch rejects non-Latin-1 header values; the server percent-decodes
    // only this header — message and provenance must stay unencoded (Latin-1).
    next['X-Quarry-Transaction-Actor'] = encodeURIComponent(options.transactionActor);
  }
  if (options.transactionMessage) next['X-Quarry-Transaction-Message'] = options.transactionMessage;
  if (options.transactionProvenance) {
    next['X-Quarry-Transaction-Provenance'] = JSON.stringify(options.transactionProvenance);
  }
  return next;
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
  return documentRefUrl({ scope: 'library', library, path });
}

export function tmpDocumentHref(secret: string) {
  return documentRefUrl({ scope: 'tmp', secret });
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

