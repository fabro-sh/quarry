use crate::{ApiError, gateway};
use quarry_core::QuarryError;
use quarry_storage::QuarryStore;
use serde::{Deserialize, Serialize};
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
    #[serde(rename = "blockId")]
    pub block_id: Option<String>,
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
/// recorded the losing incoming hunk here. `conflict.keep_canonical` resolves
/// without a content change; `conflict.accept_incoming` verifies and replaces
/// the retained hunk atomically.
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<AgentReviewReply>,
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
    /// Native targets can span blocks. Discussion status is independent of
    /// whether target text is attached, hidden, deleted, or unknown at import.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<quarry_document::ResolvedTarget>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<quarry_document::ResolvedTarget>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentReviewSuggestion {
    pub id: String,
    pub status: String,
    pub acceptance_error: Option<String>,
    pub kind: AgentSuggestionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<quarry_document::ProposalAction>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<quarry_document::ResolvedTarget>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentSuggestionPreview {
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentSuggestionKind {
    BlockMove,
    BlockUpdate,
    Format,
    BlockDelete,
    MarkdownInsert,
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
// the `quarry_markdown` facade, single-sourced with the Markdown syntax conversion
// that needs them.

pub(crate) async fn agent_document_snapshot(
    store: &QuarryStore,
    library: &str,
    path: &str,
) -> Result<AgentDocumentSnapshot, ApiError> {
    let saved = store
        .durable_document_for_scope(&quarry_storage::DocumentScopeRef::library(library), path)
        .await?
        .ok_or_else(|| QuarryError::InvalidPath("Only Markdown documents support review".into()))?;
    let document = quarry_document::Document::load(&saved.bytes)
        .map_err(crate::document_engine::document_error)?;
    let rows = quarry_storage::document_projection(&document)?;
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        let root = index;
        index += 1;
        while index < rows.len() && rows[index].parent_block_id.is_some() {
            index += 1;
        }
        blocks.push(AgentSnapshotBlock {
            block_ref: AgentBlockRef {
                block_id: Some(rows[root].block_id.clone()),
            },
            markdown: quarry_markdown::block_rows_to_markdown(&rows[root..index])
                .map_err(|e| QuarryError::InvalidInput(e.to_string()))?,
        });
    }
    Ok(AgentDocumentSnapshot {
        document_id: saved.document_id,
        base_token: saved.version_id,
        blocks,
    })
}

pub(crate) async fn agent_document_review(
    store: &QuarryStore,
    library: &str,
    path: &str,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    if let Some(saved) = store
        .durable_document_for_scope(&quarry_storage::DocumentScopeRef::library(library), path)
        .await?
    {
        return native_review_response(&saved, include_resolved);
    }
    Err(QuarryError::InvalidPath("Only Markdown documents support review".into()).into())
}

pub(crate) async fn agent_tmp_document_review(
    store: &QuarryStore,
    path: &str,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    if let Some(saved) = store
        .durable_document_for_scope(&quarry_storage::DocumentScopeRef::Tmp, path)
        .await?
    {
        return native_review_response(&saved, include_resolved);
    }
    Err(QuarryError::Invariant("Markdown document has no native state".into()).into())
}

fn native_review_response(
    saved: &quarry_storage::DurableDocumentState,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    let document = quarry_document::Document::load(&saved.bytes)
        .map_err(crate::document_engine::document_error)?;
    let rows = quarry_storage::document_projection(&document)?;
    let items = quarry_storage::document_review_projection(&document)?;
    let mut response = gateway::review_response_from_rows(
        saved.document_id.clone(),
        saved.version_id.clone(),
        &rows,
        &items,
        include_resolved,
    );
    let status = |state: quarry_document::DiscussionState| {
        match state {
            quarry_document::DiscussionState::Open => "open",
            quarry_document::DiscussionState::Resolved => "resolved",
        }
        .to_string()
    };
    let reply = |reply: &mut AgentReviewReply| -> Result<(), ApiError> {
        let comment = document
            .comment(&reply.id)
            .map_err(crate::document_engine::document_error)?;
        reply.status = status(comment.state);
        reply.target = Some(
            document
                .comment_target(&reply.id)
                .map_err(crate::document_engine::document_error)?,
        );
        Ok(())
    };
    for comment in &mut response.comments {
        comment.status = status(
            document
                .comment(&comment.id)
                .map_err(crate::document_engine::document_error)?
                .state,
        );
        comment.target = Some(
            document
                .comment_target(&comment.id)
                .map_err(crate::document_engine::document_error)?,
        );
        for child in &mut comment.replies {
            reply(child)?;
        }
    }
    for proposal in &mut response.suggestions {
        let action = document
            .proposal(&proposal.id)
            .map_err(crate::document_engine::document_error)?
            .action;
        if matches!(
            action,
            quarry_document::ProposalAction::UpdateBlock { .. }
                | quarry_document::ProposalAction::ConvertBlock { .. }
        ) {
            proposal.kind = AgentSuggestionKind::BlockUpdate;
        }
        if matches!(action, quarry_document::ProposalAction::Format { .. }) {
            proposal.kind = AgentSuggestionKind::Format;
        }
        if matches!(action, quarry_document::ProposalAction::MoveBlock { .. }) {
            proposal.kind = AgentSuggestionKind::BlockMove;
        }
        proposal.action = Some(action);
        if let quarry_document::ProposalAction::Text { original_quote, .. } = document
            .proposal(&proposal.id)
            .map_err(crate::document_engine::document_error)?
            .action
        {
            proposal.kind = if original_quote.is_empty() {
                AgentSuggestionKind::Insert
            } else if proposal.content.is_empty() {
                AgentSuggestionKind::Delete
            } else {
                AgentSuggestionKind::Replace
            };
        }
        proposal.acceptance_error = document
            .validate_proposal_acceptance(&proposal.id)
            .err()
            .map(|e| e.to_string());
        proposal.target = Some(
            document
                .proposal_target(&proposal.id)
                .map_err(crate::document_engine::document_error)?,
        );
        for child in &mut proposal.replies {
            reply(child)?;
        }
    }
    for conflict in &mut response.conflicts {
        for child in &mut conflict.replies {
            reply(child)?;
        }
    }
    Ok(response)
}
