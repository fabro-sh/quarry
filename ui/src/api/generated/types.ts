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
  inline_content: number[] | null;
  metadata: Record<string, unknown>;
  content_type: string;
  byte_size: number;
  created_at: string;
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
  resolution_status: 'resolved' | 'unresolved' | 'ambiguous';
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
  resolution_status: 'resolved' | 'unresolved' | 'ambiguous';
}

export interface GraphResponse {
  nodes: GraphNode[];
  edges: GraphEdge[];
  truncated: boolean;
}
