export interface Library {
  id: string;
  slug: string;
  created_at: string;
  settings: Record<string, unknown>;
}

export interface DocumentListEntry {
  id: string;
  path: string;
  head_version_id: string;
  content_type: string;
  byte_size: number;
  content_hash: string | null;
  metadata: Record<string, unknown>;
  updated_at: string;
}

export interface DocumentVersion {
  id: string;
  document_id: string;
  tx_id: string;
  transaction_source: 'rest' | 'git' | 'fuse' | 'cli' | 'system' | null;
  transaction_actor: string | null;
  transaction_message: string | null;
  transaction_provenance: Record<string, unknown> | null;
  content_hash: string | null;
  inline_content?: number[] | null;
  metadata: Record<string, unknown>;
  content_type: string;
  byte_size: number;
  created_at: string;
}

export interface DocumentHistoryEntry {
  id: string;
  document_id: string;
  latest_version_id: string;
  earliest_version_id: string;
  raw_version_count: number;
  source: 'rest' | 'git' | 'fuse' | 'cli' | 'system' | null;
  actor: string | null;
  message: string | null;
  provenance: Record<string, unknown> | null;
  checkpoint_reason: string | null;
  content_type: string;
  byte_size: number;
  created_at: string;
  updated_at: string;
}

export interface DocumentVersionContent {
  version: DocumentVersion;
  content: string;
}

export interface VersionDiff {
  base_version_id: string;
  against_version_id: string;
  unified_diff: string;
}

export interface WriteOutcome {
  document: DocumentListEntry;
  version: DocumentVersion;
  transaction: TransactionRecord;
}

export interface CollabInviteToken {
  id: string;
  document_id: string;
  role: string;
  by_hint: string | null;
  created_at: string;
  revoked_at: string | null;
}

export interface TransactionRecord {
  id: string;
  library_id: string;
  state: 'open' | 'committed' | 'rolled_back';
  actor: string | null;
  source: 'rest' | 'git' | 'fuse' | 'cli' | 'system';
  message: string | null;
  provenance: Record<string, unknown>;
  created_at: string;
  committed_at: string | null;
}

export interface ConflictRecord {
  id: string;
  library_id: string;
  path: string;
  conflict_path?: string | null;
  ours_version_id: string | null;
  theirs_version_id: string | null;
  status: 'open' | 'resolved';
  discovered_at: string;
  resolved_at: string | null;
}

export interface SearchResult {
  document_id: string;
  path: string;
  title: string;
  content_type: string;
  score: number;
  snippet: string | null;
  matched_fields: string[];
  head_version_id: string;
}

export interface SearchResponse {
  results: SearchResult[];
  cursor: string | null;
}

export interface SearchSuggestion {
  path: string;
  title: string;
  match_type: string;
  head_version_id: string;
  matched_text: string | null;
  target_anchor: string | null;
}

export interface DocumentLink {
  src_doc_id: string;
  src_version_id: string;
  src_path: string;
  target_kind: string;
  target_text: string;
  target_doc_id: string | null;
  target_path: string | null;
  target_anchor: string | null;
  alias: string | null;
  start_offset: number;
  end_offset: number;
  resolved: boolean;
  resolution_status: 'resolved' | 'unresolved' | 'ambiguous' | 'external';
}

export interface LinkCollection {
  path: string;
  links: DocumentLink[];
}

export interface GraphNode {
  id: string;
  path: string;
  title: string;
  content_type: string;
}

export interface GraphEdge {
  id: string;
  source: string;
  source_path: string;
  target: string | null;
  target_path: string | null;
  target_kind: string;
  target_text: string;
  resolved: boolean;
  resolution_status: 'resolved' | 'unresolved' | 'ambiguous' | 'external';
}

export interface GraphResponse {
  nodes: GraphNode[];
  edges: GraphEdge[];
  truncated: boolean;
}

export interface AgentBlockRef {
  ordinal: number;
  contentHash?: string | null;
}

export interface AgentSnapshotBlock {
  ref: AgentBlockRef;
  markdown: string;
}

export interface AgentDocumentSnapshot {
  documentId: string;
  baseToken: string;
  blocks: AgentSnapshotBlock[];
}

export type AgentSuggestionKind =
  | 'block_delete'
  | 'insert'
  | 'delete'
  | 'remove'
  | 'replace'
  | 'substitution';

export interface AgentSuggestionPreview {
  before: string;
  after: string;
}

export interface AgentReviewReply {
  id: string;
  status: string;
  by: string;
  at: string;
  editedAt: string | null;
  body: string;
}

export interface AgentReviewComment {
  id: string;
  status: string;
  by: string;
  at: string;
  editedAt: string | null;
  ref: AgentBlockRef;
  quote: string;
  body: string;
  replies: AgentReviewReply[];
  /** Row-anchored position; present when the document has canonical block rows. */
  anchor?: BlockReviewAnchor | null;
}

export interface AgentReviewSuggestion {
  id: string;
  status: string;
  kind: AgentSuggestionKind;
  by: string;
  at: string;
  ref: AgentBlockRef;
  quote: string;
  content: string;
  body?: string | null;
  preview: AgentSuggestionPreview;
  replies: AgentReviewReply[];
  /** Row-anchored position; present when the document has canonical block rows. */
  anchor?: BlockReviewAnchor | null;
}

/**
 * A `kind = conflict` review item: a diff3 whole-file merge kept the
 * canonical side and recorded the losing incoming hunk here. Resolve or
 * delete through `POST .../transactions` with `comment.resolve` /
 * `comment.delete`; resolution never mutates the document.
 */
export interface AgentReviewConflict {
  id: string;
  status: string;
  by: string;
  at: string;
  /** The surviving block the conflict attaches after; null = document start. */
  afterBlockId: string | null;
  baseMarkdown: string;
  incomingMarkdown: string;
  canonicalMarkdown: string;
}

export interface AgentReviewResponse {
  documentId: string;
  baseToken: string;
  comments: AgentReviewComment[];
  suggestions: AgentReviewSuggestion[];
  /** diff3 conflict review items; present for documents with block rows. */
  conflicts: AgentReviewConflict[];
}

// ---------------------------------------------------------------------------
// Block API (Phase 2 semantic mutation gateway)
//
// HAND-MAINTAINED: `bun run generate:api` only rewrites openapi.json; the
// types below (including the BlockTransactionOp union, which the OpenAPI
// schema models as an untyped object array) mirror the server payloads in
// crates/quarry-server/src/gateway.rs and must be kept in sync by hand.
// ---------------------------------------------------------------------------

/** UTF-16 offsets into a block's flat text; `end` is exclusive. */
export interface BlockMarkRun {
  start: number;
  end: number;
  marks: Record<string, unknown>;
}

export interface BlockLinkRange {
  start: number;
  end: number;
  url: string;
}

export interface BlockNode {
  block_id: string;
  parent_block_id: string | null;
  position: number;
  block_type: string;
  attrs: Record<string, unknown>;
  text: string;
  marks: BlockMarkRun[];
  links: BlockLinkRange[];
}

export interface BlockTreeResponse {
  document_id: string;
  /** The document clock (head version id) the rows correspond to. */
  document_clock: string;
  blocks: BlockNode[];
}

export interface BlockReviewAnchor {
  blockId: string;
  startOffset: number;
  endOffset: number;
}

export interface BlockTransactionActor {
  kind: string;
  id?: string | null;
  label?: string | null;
}

export type BlockTransactionOp =
  | {
      op: 'insert_block';
      block_id?: string;
      parent_block_id?: string | null;
      position: number;
      block_type: string;
      attrs?: Record<string, unknown>;
      text?: string;
      marks?: BlockMarkRun[];
      links?: BlockLinkRange[];
    }
  | { op: 'delete_block'; block_id: string }
  | { op: 'move_block'; block_id: string; parent_block_id?: string | null; position: number }
  | {
      op: 'replace_block_content';
      block_id: string;
      text: string;
      marks?: BlockMarkRun[];
      links?: BlockLinkRange[];
    }
  | { op: 'set_block_attrs'; block_id: string; attrs: Record<string, unknown> }
  | { op: 'set_block_type'; block_id: string; block_type: string; attrs?: Record<string, unknown> }
  | { op: 'add_mark'; block_id: string; start: number; end: number; marks: Record<string, unknown> }
  | { op: 'remove_mark'; block_id: string; start: number; end: number; marks: string[] }
  | { op: 'set_link'; block_id: string; start: number; end: number; url: string | null }
  | { op: 'comment.add'; block_id: string; start: number; end: number; body: string; quote?: string }
  | { op: 'comment.reply'; item_id: string; body: string }
  | { op: 'comment.edit'; item_id: string; body: string }
  | { op: 'comment.resolve'; item_id: string }
  | { op: 'comment.delete'; item_id: string }
  | {
      op: 'suggestion.add';
      block_id: string;
      start: number;
      end: number;
      replacement: string;
      body?: string;
      quote?: string;
    }
  | { op: 'suggestion.add_block_delete'; block_id: string; body?: string; quote?: string }
  | { op: 'suggestion.accept'; item_id: string }
  | { op: 'suggestion.reject'; item_id: string };

export interface BlockTransactionRequest {
  client_tx_id: string;
  /** Document clock the ops were computed against; ETag-quoted tokens are tolerated. */
  base_clock?: string;
  actor: BlockTransactionActor;
  ops: BlockTransactionOp[];
}

export interface BlockTransactionAck {
  status: 'committed' | 'committed_rebased';
  document_clock: string;
  transaction_id: string;
  changed_block_ids: string[];
}

export type ApiErrorCode =
  | 'INVALID_REQUEST'
  | 'NOT_FOUND'
  | 'GONE'
  | 'PRECONDITION_FAILED'
  | 'CONFLICT'
  | 'METHOD_NOT_ALLOWED'
  | 'UNSUPPORTED_MEDIA_TYPE'
  | 'PAYLOAD_TOO_LARGE'
  | 'UNPROCESSABLE_ENTITY'
  | 'SERVICE_BUSY'
  | 'INTERNAL_ERROR'
  | 'STALE_BASE'
  | 'BLOCK_DELETED'
  | 'ANCHOR_NOT_FOUND'
  | 'BLOCK_MOVE_CONFLICT'
  | 'SUGGESTION_INVALIDATED'
  | 'SUGGESTION_ALREADY_RESOLVED'
  | 'UNSUPPORTED_MARKDOWN'
  | 'INVALID_TRANSACTION'
  | 'UNKNOWN_BLOCK_TYPE'
  | 'UNSUPPORTED_BLOCK_DOCUMENT';

export interface ApiErrorResponse {
  code: ApiErrorCode;
  retryable: boolean;
  message: string;
}
