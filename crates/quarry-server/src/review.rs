use crate::{ApiError, gateway};
use quarry_collab_codec::{
    ReviewMeta, ReviewMetaEntry, ReviewSuggestionKind as CodecReviewSuggestionKind, review_markers,
    review_meta_with_inline_comment_bodies, split_markdown_blocks,
};
use quarry_core::QuarryError;
use quarry_storage::QuarryStore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use utoipa::ToSchema;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DocumentReviewQuery {
    #[serde(default, rename = "includeResolved", alias = "include_resolved")]
    include_resolved: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub(crate) enum DryRunValue {
    #[serde(rename = "1")]
    One,
    #[serde(rename = "true")]
    True,
    #[serde(rename = "yes")]
    Yes,
    #[serde(rename = "0")]
    Zero,
    #[serde(rename = "false")]
    False,
    #[serde(rename = "no")]
    No,
}

impl DocumentReviewQuery {
    pub(crate) fn include_resolved(&self) -> Result<bool, ApiError> {
        parse_agent_bool_query(self.include_resolved.as_deref(), "includeResolved")
    }
}

fn parse_agent_bool_query(value: Option<&str>, name: &str) -> Result<bool, ApiError> {
    let Some(value) = value else {
        return Ok(false);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(QuarryError::InvalidPath(format!("invalid {name} value")).into()),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentBlockRef {
    pub ordinal: usize,
    #[serde(
        rename = "contentHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub content_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentSnapshotBlock {
    #[serde(rename = "ref")]
    pub block_ref: AgentBlockRef,
    pub markdown: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentDocumentSnapshot {
    #[serde(rename = "documentId")]
    pub document_id: String,
    #[serde(rename = "baseToken")]
    pub base_token: String,
    pub blocks: Vec<AgentSnapshotBlock>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentReviewResponse {
    #[serde(rename = "documentId")]
    pub document_id: String,
    #[serde(rename = "baseToken")]
    pub base_token: String,
    pub comments: Vec<AgentReviewComment>,
    pub suggestions: Vec<AgentReviewSuggestion>,
    /// diff3 conflict review items (Phase 4): unresolved whole-file merge
    /// conflicts, present only for documents with canonical block rows.
    pub conflicts: Vec<AgentReviewConflict>,
}

/// A `kind = conflict` review item: a diff3 merge kept the canonical side and
/// recorded the losing incoming hunk here. Resolves and deletes through
/// `POST .../transactions` with `comment.resolve` / `comment.delete`;
/// resolution never mutates the document.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentReviewConflict {
    pub id: String,
    pub status: String,
    pub by: String,
    pub at: String,
    /// The surviving block the conflict region attaches after; `null` means
    /// the document start.
    #[serde(rename = "afterBlockId")]
    pub after_block_id: Option<String>,
    /// The base (shadow) context the merge diffed against.
    #[serde(rename = "baseMarkdown")]
    pub base_markdown: String,
    /// The losing incoming hunk (empty = the write deleted this region).
    #[serde(rename = "incomingMarkdown")]
    pub incoming_markdown: String,
    /// The canonical side that was retained (empty = canonical had deleted
    /// the region).
    #[serde(rename = "canonicalMarkdown")]
    pub canonical_markdown: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentReviewComment {
    pub id: String,
    pub status: String,
    pub by: String,
    pub at: String,
    #[serde(rename = "editedAt")]
    pub edited_at: Option<String>,
    #[serde(rename = "ref")]
    pub block_ref: AgentBlockRef,
    pub quote: String,
    pub body: String,
    pub replies: Vec<AgentReviewReply>,
    /// Row-anchored position; present only when the document has canonical
    /// block rows (the Phase 2 review projection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<gateway::BlockReviewAnchor>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentReviewReply {
    pub id: String,
    pub status: String,
    pub by: String,
    pub at: String,
    #[serde(rename = "editedAt")]
    pub edited_at: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentReviewSuggestion {
    pub id: String,
    pub status: String,
    pub kind: AgentSuggestionKind,
    pub by: String,
    pub at: String,
    #[serde(rename = "ref")]
    pub block_ref: AgentBlockRef,
    pub quote: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub preview: AgentSuggestionPreview,
    pub replies: Vec<AgentReviewReply>,
    /// Row-anchored position; present only when the document has canonical
    /// block rows (the Phase 2 review projection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<gateway::BlockReviewAnchor>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentSuggestionPreview {
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentSuggestionKind {
    BlockDelete,
    Insert,
    Delete,
    Remove,
    Replace,
    Substitution,
}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/review",
    params(("secret" = String, Path), ("includeResolved" = Option<DryRunValue>, Query)),
    responses((status = 200, body = AgentReviewResponse), (status = 404, body = crate::ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn tmp_document_review_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/review",
    params(("library" = String, Path), ("path" = String, Path), ("includeResolved" = Option<DryRunValue>, Query)),
    responses((status = 200, body = AgentReviewResponse), (status = 404, body = crate::ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_review_openapi() {}

// `ReviewMeta` / `ReviewMetaEntry` and the endmatter readers are imported from
// the `quarry_collab_codec` facade, single-sourced with the slate conversion
// that needs them.

pub(crate) async fn agent_document_snapshot(
    store: &QuarryStore,
    library: &str,
    path: &str,
) -> Result<AgentDocumentSnapshot, ApiError> {
    let document = store.get_document(library, path).await?;
    let markdown = document_markdown(&document)?;
    let base_token = document.version.id;
    let blocks = snapshot_blocks(&markdown);
    Ok(AgentDocumentSnapshot {
        document_id: document.id.to_string(),
        base_token: base_token.to_string(),
        blocks,
    })
}

pub(crate) async fn agent_document_review(
    store: &QuarryStore,
    library: &str,
    path: &str,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    let document = store.get_document(library, path).await?;
    agent_document_review_from_document(store, document, include_resolved).await
}

pub(crate) async fn agent_tmp_document_review(
    store: &QuarryStore,
    path: &str,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    let document = store.get_tmp_document(path).await?;
    agent_document_review_from_document(store, document, include_resolved).await
}

async fn agent_document_review_from_document(
    store: &QuarryStore,
    document: quarry_core::Document,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    // Documents with canonical block rows project review items from
    // `block_review_items` (the Phase 2 rows-backed projection); documents
    // without rows keep the legacy CriticMarkup/endmatter projection.
    let rows = store.load_block_tree(&document.id).await?;
    if !rows.is_empty() {
        let items = store.list_block_review_items(&document.id).await?;
        return Ok(gateway::review_response_from_rows(
            document.id.to_string(),
            document.version.id.to_string(),
            &rows,
            &items,
            include_resolved,
        ));
    }
    let markdown = document_markdown(&document)?;
    Ok(agent_review_response_from_markdown(
        document.id.to_string(),
        document.version.id.to_string(),
        &markdown,
        include_resolved,
    ))
}

fn agent_review_response_from_markdown(
    document_id: String,
    base_token: String,
    markdown: &str,
    include_resolved: bool,
) -> AgentReviewResponse {
    let blocks = snapshot_blocks(markdown);
    let (_, meta) = review_meta_with_inline_comment_bodies(markdown);
    let markers = agent_review_markers(&blocks);
    let comments = agent_review_comments(&markers.comments, &meta, include_resolved);
    let suggestions = agent_review_suggestions(&markers.suggestions, &meta, include_resolved);
    AgentReviewResponse {
        document_id,
        base_token,
        comments,
        suggestions,
        // Conflict items exist only for documents with block rows (the
        // Phase 4 reconciler); the legacy projection has none.
        conflicts: Vec::new(),
    }
}

fn document_markdown(document: &quarry_core::Document) -> Result<String, ApiError> {
    if !is_markdown_content_type(&document.version.content_type) {
        return Err(QuarryError::InvalidPath(
            "agent document APIs require markdown content".to_string(),
        )
        .into());
    }
    std::str::from_utf8(&document.content)
        .map(str::to_string)
        .map_err(|_| {
            QuarryError::InvalidPath("agent document APIs require UTF-8 markdown".to_string())
                .into()
        })
}

fn is_markdown_content_type(content_type: &str) -> bool {
    matches!(
        content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "text/markdown" | "text/x-markdown" | "application/markdown" | "application/x-markdown"
    )
}

fn snapshot_blocks(markdown: &str) -> Vec<AgentSnapshotBlock> {
    split_markdown_blocks(markdown)
        .into_iter()
        .enumerate()
        .map(|(ordinal, markdown)| AgentSnapshotBlock {
            block_ref: AgentBlockRef {
                ordinal,
                content_hash: Some(block_hash(&markdown)),
            },
            markdown,
        })
        .collect()
}

#[derive(Clone, Debug)]
struct ReviewCommentMarker {
    id: String,
    block_ref: AgentBlockRef,
    quote: String,
    body: String,
}

#[derive(Clone, Debug)]
struct ReviewSuggestionMarker {
    id: String,
    block_ref: AgentBlockRef,
    kind: AgentSuggestionKind,
    quote: String,
    content: String,
    preview: AgentSuggestionPreview,
}

struct AgentReviewMarkers {
    comments: Vec<ReviewCommentMarker>,
    suggestions: Vec<ReviewSuggestionMarker>,
}

fn agent_review_comments(
    markers: &[ReviewCommentMarker],
    meta: &ReviewMeta,
    include_resolved: bool,
) -> Vec<AgentReviewComment> {
    let mut replies = agent_review_replies_by_parent(meta, include_resolved);
    markers
        .iter()
        .filter_map(|marker| {
            let entry = meta.comments.get(&marker.id)?;
            if entry.re.is_some() || !include_review_entry(entry, include_resolved) {
                return None;
            }
            Some(AgentReviewComment {
                id: marker.id.clone(),
                status: review_entry_status(entry),
                by: entry.by.clone(),
                at: entry.at.clone(),
                edited_at: entry.edited_at.clone(),
                block_ref: marker.block_ref.clone(),
                quote: marker.quote.clone(),
                body: entry.body.clone().unwrap_or_else(|| marker.body.clone()),
                replies: replies.remove(&marker.id).unwrap_or_default(),
                anchor: None,
            })
        })
        .collect()
}

fn agent_review_replies_by_parent(
    meta: &ReviewMeta,
    include_resolved: bool,
) -> HashMap<String, Vec<AgentReviewReply>> {
    let mut replies = HashMap::new();
    for (id, entry) in &meta.comments {
        let Some(parent_id) = entry.re.as_deref() else {
            continue;
        };
        if !include_review_entry(entry, include_resolved) {
            continue;
        }
        replies
            .entry(parent_id.to_string())
            .or_insert_with(Vec::new)
            .push(AgentReviewReply {
                id: id.clone(),
                status: review_entry_status(entry),
                by: entry.by.clone(),
                at: entry.at.clone(),
                edited_at: entry.edited_at.clone(),
                body: entry.body.clone().unwrap_or_default(),
            });
    }
    replies
}

fn agent_review_suggestions(
    markers: &[ReviewSuggestionMarker],
    meta: &ReviewMeta,
    include_resolved: bool,
) -> Vec<AgentReviewSuggestion> {
    let mut replies = agent_review_replies_by_parent(meta, include_resolved);
    markers
        .iter()
        .filter_map(|marker| {
            let entry = meta.suggestions.get(&marker.id)?;
            if review_entry_is_resolved(entry) {
                return None;
            }
            Some(AgentReviewSuggestion {
                id: marker.id.clone(),
                status: review_entry_status(entry),
                kind: marker.kind.clone(),
                by: entry.by.clone(),
                at: entry.at.clone(),
                block_ref: marker.block_ref.clone(),
                quote: marker.quote.clone(),
                content: marker.content.clone(),
                body: entry.body.clone(),
                preview: marker.preview.clone(),
                replies: replies.remove(&marker.id).unwrap_or_default(),
                anchor: None,
            })
        })
        .collect()
}

fn agent_review_markers(blocks: &[AgentSnapshotBlock]) -> AgentReviewMarkers {
    let mut seen_comments = HashSet::new();
    let mut seen_suggestions = HashSet::new();
    let mut comments = Vec::new();
    let mut suggestions = Vec::new();
    for block in blocks {
        let markers = review_markers(&block.markdown);
        for marker in markers.comments {
            if seen_comments.insert(marker.id.clone()) {
                comments.push(ReviewCommentMarker {
                    id: marker.id,
                    block_ref: block.block_ref.clone(),
                    quote: marker.quote,
                    body: marker.body,
                });
            }
        }
        for marker in markers.suggestions {
            if seen_suggestions.insert(marker.id.clone()) {
                suggestions.push(ReviewSuggestionMarker {
                    id: marker.id,
                    block_ref: block.block_ref.clone(),
                    kind: agent_suggestion_kind(marker.kind),
                    quote: marker.quote,
                    content: marker.content,
                    preview: AgentSuggestionPreview {
                        before: marker.before,
                        after: marker.after,
                    },
                });
            }
        }
    }
    AgentReviewMarkers {
        comments,
        suggestions,
    }
}

fn agent_suggestion_kind(kind: CodecReviewSuggestionKind) -> AgentSuggestionKind {
    match kind {
        CodecReviewSuggestionKind::Insert => AgentSuggestionKind::Insert,
        CodecReviewSuggestionKind::Delete => AgentSuggestionKind::Delete,
        CodecReviewSuggestionKind::Substitution => AgentSuggestionKind::Substitution,
    }
}

fn include_review_entry(entry: &ReviewMetaEntry, include_resolved: bool) -> bool {
    include_resolved || !review_entry_is_resolved(entry)
}

fn review_entry_is_resolved(entry: &ReviewMetaEntry) -> bool {
    entry
        .status
        .as_deref()
        .map(str::trim)
        .is_some_and(|status| status.eq_ignore_ascii_case("resolved"))
}

fn review_entry_status(entry: &ReviewMetaEntry) -> String {
    match entry.status.as_deref().map(str::trim) {
        Some(status) if status.eq_ignore_ascii_case("resolved") => "resolved".to_string(),
        Some(status) if !status.is_empty() => status.to_string(),
        _ => "open".to_string(),
    }
}

fn block_hash(markdown: &str) -> String {
    blake3::hash(markdown.as_bytes()).to_hex().to_string()
}
