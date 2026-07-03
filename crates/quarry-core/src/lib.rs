use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::error::Error as StdError;
use std::str::FromStr;
use thiserror::Error;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

pub const INLINE_CONTENT_THRESHOLD: usize = 64 * 1024;
pub const GIT_BINARY_WARN_THRESHOLD: usize = 5 * 1024 * 1024;

pub type Result<T> = std::result::Result<T, QuarryError>;

#[derive(Debug, Error)]
pub enum QuarryError {
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("gone: {0}")]
    Gone(String),
    #[error("precondition failed: {0}")]
    PreconditionFailed(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(String),
    #[error("is a directory: {0}")]
    IsADirectory(String),
    #[error("read-only: {0}")]
    ReadOnly(String),
    #[error("invariant violation: {0}")]
    Invariant(String),
    #[error("storage busy: {0}")]
    Busy(String),
    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("unsupported markdown: {0}")]
    UnsupportedMarkdown(#[from] quarry_collab_codec::Unsupported),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("storage error: {source}")]
    StorageSource {
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
    #[error("git error: {0}")]
    Git(String),
    #[error("git error: {source}")]
    GitSource {
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("invalid {kind} {value}")]
pub struct ParseEnumError {
    kind: &'static str,
    value: String,
}

impl ParseEnumError {
    fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct Library {
    pub id: String,
    pub slug: String,
    pub created_at: String,
    pub settings: JsonValue,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DocumentSource {
    Rest,
    Git,
    Fuse,
    Cli,
    System,
}

impl DocumentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rest => "rest",
            Self::Git => "git",
            Self::Fuse => "fuse",
            Self::Cli => "cli",
            Self::System => "system",
        }
    }
}

impl FromStr for DocumentSource {
    type Err = ParseEnumError;

    fn from_str(source: &str) -> std::result::Result<Self, Self::Err> {
        match source {
            "rest" => Ok(Self::Rest),
            "git" => Ok(Self::Git),
            "fuse" => Ok(Self::Fuse),
            "cli" => Ok(Self::Cli),
            "system" => Ok(Self::System),
            other => Err(ParseEnumError::new("document source", other)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TransactionState {
    Open,
    Committed,
    RolledBack,
}

impl TransactionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Committed => "committed",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl FromStr for TransactionState {
    type Err = ParseEnumError;

    fn from_str(state: &str) -> std::result::Result<Self, Self::Err> {
        match state {
            "open" => Ok(Self::Open),
            "committed" => Ok(Self::Committed),
            "rolled_back" => Ok(Self::RolledBack),
            other => Err(ParseEnumError::new("transaction state", other)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Put,
    Delete,
    Move,
    Metadata,
}

impl ChangeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Delete => "delete",
            Self::Move => "move",
            Self::Metadata => "metadata",
        }
    }
}

impl FromStr for ChangeType {
    type Err = ParseEnumError;

    fn from_str(change_type: &str) -> std::result::Result<Self, Self::Err> {
        match change_type {
            "put" => Ok(Self::Put),
            "delete" => Ok(Self::Delete),
            "move" => Ok(Self::Move),
            "metadata" => Ok(Self::Metadata),
            other => Err(ParseEnumError::new("change type", other)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConflictStatus {
    Open,
    Resolved,
}

impl ConflictStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }
}

impl FromStr for ConflictStatus {
    type Err = ParseEnumError;

    fn from_str(status: &str) -> std::result::Result<Self, Self::Err> {
        match status {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            other => Err(ParseEnumError::new("conflict status", other)),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum WritePrecondition {
    None,
    IfMatch(String),
    IfNoneMatch,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct DocumentVersion {
    pub id: String,
    pub document_id: String,
    pub tx_id: String,
    #[serde(default)]
    pub transaction_source: Option<DocumentSource>,
    #[serde(default)]
    pub transaction_actor: Option<String>,
    #[serde(default)]
    pub transaction_message: Option<String>,
    #[serde(default)]
    pub transaction_provenance: Option<JsonValue>,
    pub content_hash: Option<String>,
    #[serde(skip)]
    pub inline_content: Option<Vec<u8>>,
    pub metadata: JsonValue,
    pub content_type: String,
    pub byte_size: u64,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct DocumentHistoryEntry {
    pub id: String,
    pub document_id: String,
    pub latest_version_id: String,
    pub earliest_version_id: String,
    pub raw_version_count: u64,
    #[serde(default)]
    pub source: Option<DocumentSource>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub provenance: Option<JsonValue>,
    #[serde(default)]
    pub checkpoint_reason: Option<String>,
    pub content_type: String,
    pub byte_size: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct Document {
    pub id: String,
    pub library_id: Option<String>,
    pub path: String,
    pub version: DocumentVersion,
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = Binary))]
    pub content: Vec<u8>,
    pub metadata: JsonValue,
    #[serde(default)]
    pub expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct DocumentListEntry {
    pub id: String,
    #[serde(default)]
    pub library_id: Option<String>,
    pub path: String,
    pub head_version_id: String,
    pub content_type: String,
    pub byte_size: u64,
    pub content_hash: Option<String>,
    pub metadata: JsonValue,
    #[serde(default)]
    pub expires_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct CollabInviteToken {
    pub id: String,
    pub document_id: String,
    pub role: String,
    pub by_hint: Option<String>,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct WriteOutcome {
    pub document: DocumentListEntry,
    pub version: DocumentVersion,
    pub transaction: TransactionRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct TransactionRecord {
    pub id: String,
    pub library_id: String,
    pub state: TransactionState,
    pub actor: Option<String>,
    pub source: DocumentSource,
    pub message: Option<String>,
    pub provenance: JsonValue,
    pub created_at: String,
    pub committed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct ConflictRecord {
    pub id: String,
    pub library_id: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_path: Option<String>,
    pub ours_version_id: Option<String>,
    pub theirs_version_id: Option<String>,
    pub status: ConflictStatus,
    pub discovered_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct GitPeer {
    pub id: String,
    pub library_id: String,
    pub kind: String,
    pub config: JsonValue,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct SyncStateEntry {
    pub peer_id: String,
    pub path: String,
    pub last_synced_doc_version_id: Option<String>,
    pub last_synced_git_oid: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct GcReport {
    pub reachable: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct SearchResult {
    pub document_id: String,
    pub path: String,
    pub title: String,
    pub content_type: String,
    pub score: f64,
    pub snippet: Option<String>,
    pub matched_fields: Vec<String>,
    pub head_version_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct SearchSuggestion {
    pub path: String,
    pub title: String,
    pub match_type: String,
    pub head_version_id: String,
    pub matched_text: Option<String>,
    pub target_anchor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct ReindexReport {
    pub ok: bool,
    pub indexed_documents: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct DocumentLink {
    pub src_doc_id: String,
    pub src_version_id: String,
    pub src_path: String,
    pub target_kind: String,
    pub target_text: String,
    pub target_doc_id: Option<String>,
    pub target_path: Option<String>,
    pub target_anchor: Option<String>,
    pub alias: Option<String>,
    pub start_offset: usize,
    pub end_offset: usize,
    pub resolved: bool,
    pub resolution_status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct LinkCollection {
    pub path: String,
    pub links: Vec<DocumentLink>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct GraphNode {
    pub id: String,
    pub path: String,
    pub title: String,
    pub content_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct GraphEdge {
    pub id: String,
    pub source: String,
    pub source_path: String,
    pub target: Option<String>,
    pub target_path: Option<String>,
    pub target_kind: String,
    pub target_text: String,
    pub resolved: bool,
    pub resolution_status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct GraphResponse {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct DocumentVersionContent {
    pub version: DocumentVersion,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
pub struct VersionDiff {
    pub base_version_id: String,
    pub against_version_id: String,
    pub unified_diff: String,
}

pub fn now_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Renders document metadata as YAML frontmatter: `content_type` stays out
/// (it is transport metadata, not a document attribute), keys sort for a
/// stable export order, and an empty mapping renders nothing.
///
/// This is the single canonical rendering shared by block-document export
/// (`quarry-storage`) and the Git adapter (`quarry-git`); the two must stay
/// byte-identical for diff3 reconciliation to see clean bases.
pub fn render_markdown_frontmatter(metadata: &JsonValue) -> Result<String> {
    let mut frontmatter = std::collections::BTreeMap::new();
    if let JsonValue::Object(object) = metadata {
        for (key, value) in object {
            if key != "content_type" {
                frontmatter.insert(key.clone(), value.clone());
            }
        }
    }
    if frontmatter.is_empty() {
        return Ok(String::new());
    }
    let yaml = serde_yaml::to_string(&frontmatter)?;
    Ok(format!("---\n{}\n---\n", yaml.trim_end()))
}

pub fn normalize_path(path: &str) -> Result<String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Err(QuarryError::InvalidPath(path.to_string()));
    }
    if trimmed == ".quarry" || trimmed.starts_with(".quarry/") {
        return Err(QuarryError::InvalidPath(format!(
            "{path} is reserved for Quarry metadata"
        )));
    }
    if trimmed.contains('\\') {
        return Err(QuarryError::InvalidPath(path.to_string()));
    }

    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(QuarryError::InvalidPath(path.to_string()));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

pub fn parent_dirs(path: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    while !parts.is_empty() {
        dirs.push(parts.join("/"));
        parts.pop();
    }
    dirs.reverse();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_enums_parse_from_their_storage_strings() {
        assert_eq!("rest".parse::<DocumentSource>(), Ok(DocumentSource::Rest));
        assert_eq!("git".parse::<DocumentSource>(), Ok(DocumentSource::Git));
        assert_eq!("fuse".parse::<DocumentSource>(), Ok(DocumentSource::Fuse));
        assert_eq!("cli".parse::<DocumentSource>(), Ok(DocumentSource::Cli));
        assert_eq!(
            "system".parse::<DocumentSource>(),
            Ok(DocumentSource::System)
        );

        assert_eq!(
            "committed".parse::<TransactionState>(),
            Ok(TransactionState::Committed)
        );
        assert_eq!("move".parse::<ChangeType>(), Ok(ChangeType::Move));
        assert_eq!(
            "resolved".parse::<ConflictStatus>(),
            Ok(ConflictStatus::Resolved)
        );
    }

    #[test]
    fn core_enums_reject_unknown_storage_strings() {
        assert!("unknown".parse::<DocumentSource>().is_err());
        assert!("unknown".parse::<TransactionState>().is_err());
        assert!("unknown".parse::<ChangeType>().is_err());
        assert!("unknown".parse::<ConflictStatus>().is_err());
    }

    #[test]
    fn document_version_omits_inline_content_from_json() {
        // inline_content is a storage detail (small content inlined as a BLOB). It
        // must never be serialized into API responses, where it ballooned write
        // payloads ~20x as a per-byte integer array. The Rust field stays so
        // storage can populate it; serde just drops it from the wire.
        let version = DocumentVersion {
            id: "v1".into(),
            document_id: "d1".into(),
            tx_id: "t1".into(),
            transaction_source: None,
            transaction_actor: None,
            transaction_message: None,
            transaction_provenance: None,
            content_hash: Some("abc123".into()),
            inline_content: Some(b"# Title\n".to_vec()),
            metadata: JsonValue::Null,
            content_type: "text/markdown".into(),
            byte_size: 8,
            created_at: "2026-06-05T00:00:00Z".into(),
        };

        let json = serde_json::to_value(&version).expect("serialize");
        assert!(
            json.get("inline_content").is_none(),
            "inline_content must not appear in serialized JSON"
        );
        // Identifying metadata still travels so clients can reference the content.
        assert_eq!(json["content_hash"], "abc123");
        assert_eq!(json["byte_size"], 8);
    }
}
