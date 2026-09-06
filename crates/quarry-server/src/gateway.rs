//! Public agent operations and validation for the Automerge document authority.
#[path = "document_operations.rs"]
mod native;

use crate::{
    AgentBlockRef, AgentReviewComment, AgentReviewReply, AgentReviewResponse,
    AgentReviewSuggestion, AgentSuggestionKind, AgentSuggestionPreview, ApiError, ApiErrorCode,
    ApiErrorDetails, ApiErrorTarget, AppState, json_with_etag,
};
use axum::http::StatusCode;
use axum::response::Response;
use quarry_core::{DocumentSource, QuarryError, now_timestamp, render_markdown_frontmatter};
use quarry_markdown::{
    Attrs, BlockRow, LinkRange, MarkRun, block_rows_to_markdown, is_known_block_type,
    is_utf16_boundary, known_block_types, markdown_to_block_rows, utf16_len,
};
use quarry_storage::{
    BlockMutationOutcome, BlockMutationState, BlockReviewItem, BlockReviewKind, BlockReviewState,
    BlockTransactionRecord, DocumentKind, DocumentScopeRef, document_kind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use utoipa::ToSchema;
use uuid::Uuid;

/// Operations agents may send through the public semantic transaction API.
/// `conflict.add` is deliberately excluded: whole-document reconcilers use it
/// internally to persist merge-conflict review items.
pub(crate) const PUBLIC_TRANSACTION_OPERATIONS: [&str; 22] = [
    "insert_block",
    "insert_markdown",
    "delete_block",
    "move_block",
    "replace_block_content",
    "set_block_type",
    "set_block_attrs",
    "add_mark",
    "remove_mark",
    "set_link",
    "comment.add",
    "comment.reply",
    "comment.edit",
    "comment.resolve",
    "comment.delete",
    "suggestion.add",
    "suggestion.add_block_delete",
    "suggestion.add_markdown",
    "suggestion.accept",
    "suggestion.reject",
    "conflict.keep_canonical",
    "conflict.accept_incoming",
];

// ---------------------------------------------------------------------------
// Typed errors
// ---------------------------------------------------------------------------

pub(crate) type GatewayErrorCode = ApiErrorCode;

#[derive(Clone, Debug)]
pub(crate) struct GatewayError {
    code: GatewayErrorCode,
    message: String,
    details: Option<Box<ApiErrorDetails>>,
}

impl GatewayError {
    pub(crate) fn code(&self) -> GatewayErrorCode {
        self.code
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn new(code: GatewayErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    fn with_operation(mut self, op_index: usize, op: impl Into<String>) -> Self {
        let details = self.details_mut();
        details.op_index = Some(op_index);
        details.op = Some(op.into());
        self
    }

    fn with_target(mut self, kind: &str, id: impl Into<String>) -> Self {
        self.details_mut().target = Some(ApiErrorTarget {
            kind: kind.to_string(),
            id: id.into(),
        });
        self
    }

    fn with_validation(
        mut self,
        field: &str,
        value: impl Into<String>,
        allowed_values: Vec<String>,
    ) -> Self {
        let details = self.details_mut();
        details.field = Some(field.to_string());
        details.value = Some(value.into());
        details.allowed_values = allowed_values;
        self
    }

    fn with_field(mut self, field: impl Into<String>) -> Self {
        self.details_mut().field = Some(field.into());
        self
    }

    fn with_current_value(mut self, current_value: impl Into<String>) -> Self {
        self.details_mut().current_value = Some(current_value.into());
        self
    }

    fn details_mut(&mut self) -> &mut ApiErrorDetails {
        self.details
            .get_or_insert_with(|| Box::new(ApiErrorDetails::default()))
    }

    fn has_target(&self) -> bool {
        self.details
            .as_deref()
            .and_then(|details| details.target.as_ref())
            .is_some()
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(GatewayErrorCode::InvalidTransaction, message)
    }

    fn block_deleted(block_id: &str) -> Self {
        Self::new(
            GatewayErrorCode::BlockDeleted,
            format!("block {block_id} does not exist in this document"),
        )
        .with_target("block", block_id)
    }

    fn into_api_error(self) -> ApiError {
        let error = ApiError::new(self.code, self.message);
        match self.details {
            Some(details) => error.with_details(*details),
            None => error,
        }
    }
}

/// Gateway operations retain semantic errors internally so retry paths can
/// branch on codes before every failure projects to [`ApiError`].
pub(crate) enum GatewayFailure {
    Typed(GatewayError),
    Api(ApiError),
}

impl From<GatewayError> for GatewayFailure {
    fn from(error: GatewayError) -> Self {
        Self::Typed(error)
    }
}

impl From<ApiError> for GatewayFailure {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<QuarryError> for GatewayFailure {
    fn from(error: QuarryError) -> Self {
        match error {
            QuarryError::UnsupportedMarkdown(unsupported) => Self::Typed(GatewayError::new(
                GatewayErrorCode::UnsupportedMarkdown,
                unsupported.to_string(),
            )),
            QuarryError::PayloadTooLarge(message) => Self::Typed(GatewayError::new(
                GatewayErrorCode::PayloadTooLarge,
                message,
            )),
            other => Self::Api(other.into()),
        }
    }
}

pub(crate) fn gateway_reply(
    result: Result<Response, GatewayFailure>,
) -> Result<Response, ApiError> {
    match result {
        Ok(response) => Ok(response),
        Err(GatewayFailure::Typed(error)) => Err(error.into_api_error()),
        Err(GatewayFailure::Api(error)) => Err(error),
    }
}

// ---------------------------------------------------------------------------
// Wire payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionRequest {
    pub client_tx_id: String,
    /// The document_clock of the read used to construct these operations.
    pub base_clock: String,
    pub actor: BlockTransactionActor,
    /// Semantic operations; see the module docs for the vocabulary.
    #[schema(value_type = Vec<Object>)]
    pub ops: Vec<JsonValue>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionActor {
    pub kind: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

impl BlockTransactionActor {
    fn display(&self) -> String {
        self.label
            .clone()
            .or_else(|| self.id.clone())
            .unwrap_or_else(|| self.kind.clone())
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlockTransactionAck {
    /// `committed` (clock matched the head) or `committed_rebased` (operations
    /// resolved against a known older version and merged into the current head).
    pub status: String,
    /// The new document clock: the head version id after the commit.
    pub document_clock: String,
    /// The recorded `block_transactions` history row id.
    pub transaction_id: String,
    pub changed_block_ids: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlockTreeResponse {
    pub document_id: String,
    /// The current document clock (head version id) the rows correspond to.
    pub document_clock: String,
    pub blocks: Vec<BlockNodePayload>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlockNodePayload {
    pub block_id: String,
    pub parent_block_id: Option<String>,
    pub position: u32,
    pub block_type: String,
    #[schema(value_type = Object)]
    pub attrs: Attrs,
    /// Flat block text; all offsets into it are UTF-16 code units.
    pub text: String,
    pub marks: Vec<BlockMarkRunPayload>,
    pub links: Vec<BlockLinkRangePayload>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlockMarkRunPayload {
    pub start: u32,
    pub end: u32,
    #[schema(value_type = Object)]
    pub marks: Attrs,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BlockLinkRangePayload {
    pub start: u32,
    pub end: u32,
    pub url: String,
}

/// Row-anchored review position, attached to review items projected from
/// block rows. Offsets are UTF-16 code units; `end_offset` is exclusive.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BlockReviewAnchor {
    #[serde(rename = "blockId")]
    pub block_id: String,
    #[serde(rename = "startOffset")]
    pub start_offset: u32,
    #[serde(rename = "endOffset")]
    pub end_offset: u32,
}

// ---------------------------------------------------------------------------
// Op vocabulary
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum BlockOp {
    InsertBlock {
        #[serde(default)]
        block_id: Option<String>,
        #[serde(default)]
        parent_block_id: Option<String>,
        position: u32,
        block_type: String,
        #[serde(default)]
        attrs: Attrs,
        #[serde(default)]
        text: String,
        #[serde(default)]
        marks: Vec<MarkRun>,
        #[serde(default)]
        links: Vec<LinkRange>,
    },
    /// Parses and inserts a complete Markdown fragment between top-level
    /// blocks. `after_block_id = None` means document start.
    InsertMarkdown {
        #[serde(default)]
        after_block_id: Option<String>,
        markdown: String,
    },
    DeleteBlock {
        block_id: String,
    },
    MoveBlock {
        block_id: String,
        #[serde(default)]
        parent_block_id: Option<String>,
        position: u32,
    },
    ReplaceBlockContent {
        block_id: String,
        text: String,
        #[serde(default)]
        marks: Option<Vec<MarkRun>>,
        #[serde(default)]
        links: Option<Vec<LinkRange>>,
    },
    SetBlockAttrs {
        block_id: String,
        attrs: Attrs,
    },
    SetBlockType {
        block_id: String,
        block_type: String,
        #[serde(default)]
        attrs: Option<Attrs>,
    },
    AddMark {
        block_id: String,
        start: u32,
        end: u32,
        marks: Attrs,
    },
    RemoveMark {
        block_id: String,
        start: u32,
        end: u32,
        marks: Vec<String>,
    },
    SetLink {
        block_id: String,
        start: u32,
        end: u32,
        #[serde(default)]
        url: Option<String>,
    },
    #[serde(rename = "comment.add")]
    CommentAdd {
        block_id: String,
        start: u32,
        end: u32,
        body: String,
        #[serde(default)]
        quote: Option<String>,
    },
    #[serde(rename = "comment.reply")]
    CommentReply {
        item_id: String,
        body: String,
    },
    #[serde(rename = "comment.edit")]
    CommentEdit {
        item_id: String,
        body: String,
    },
    #[serde(rename = "comment.resolve")]
    CommentResolve {
        item_id: String,
    },
    #[serde(rename = "comment.delete")]
    CommentDelete {
        item_id: String,
    },
    #[serde(rename = "suggestion.add")]
    SuggestionAdd {
        block_id: String,
        start: u32,
        end: u32,
        replacement: String,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        quote: Option<String>,
    },
    #[serde(rename = "suggestion.add_block_delete")]
    SuggestionAddBlockDelete {
        block_id: String,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        quote: Option<String>,
    },
    #[serde(rename = "suggestion.add_markdown")]
    SuggestionAddMarkdown {
        #[serde(default)]
        after_block_id: Option<String>,
        markdown: String,
        #[serde(default)]
        body: Option<String>,
    },
    #[serde(rename = "suggestion.accept")]
    SuggestionAccept {
        item_id: String,
    },
    #[serde(rename = "suggestion.reject")]
    SuggestionReject {
        item_id: String,
    },
    /// Conflict-as-data (Phase 4): a diff3 conflict artifact persisted as a
    /// `kind = conflict` review item. `after_block_id` is the surviving block
    /// the conflict region attaches after (`None` = document start);
    /// `incoming_markdown` is the losing hunk, `base_markdown` the base
    /// context, `canonical_markdown` the retained canonical side. The
    /// document itself is never mutated by this op.
    #[serde(rename = "conflict.add")]
    ConflictAdd {
        #[serde(default)]
        after_block_id: Option<String>,
        #[serde(default)]
        base_markdown: String,
        incoming_markdown: String,
        #[serde(default)]
        canonical_markdown: String,
    },
    /// Resolves an open conflict while retaining the canonical hunk already
    /// present in the document.
    #[serde(rename = "conflict.keep_canonical")]
    ConflictKeepCanonical {
        item_id: String,
    },
    /// Atomically replaces the still-matching canonical hunk with the saved
    /// incoming side and resolves the conflict.
    #[serde(rename = "conflict.accept_incoming")]
    ConflictAcceptIncoming {
        item_id: String,
    },
}

impl BlockOp {
    fn name(&self) -> &'static str {
        match self {
            Self::InsertBlock { .. } => "insert_block",
            Self::InsertMarkdown { .. } => "insert_markdown",
            Self::DeleteBlock { .. } => "delete_block",
            Self::MoveBlock { .. } => "move_block",
            Self::ReplaceBlockContent { .. } => "replace_block_content",
            Self::SetBlockAttrs { .. } => "set_block_attrs",
            Self::SetBlockType { .. } => "set_block_type",
            Self::AddMark { .. } => "add_mark",
            Self::RemoveMark { .. } => "remove_mark",
            Self::SetLink { .. } => "set_link",
            Self::CommentAdd { .. } => "comment.add",
            Self::CommentReply { .. } => "comment.reply",
            Self::CommentEdit { .. } => "comment.edit",
            Self::CommentResolve { .. } => "comment.resolve",
            Self::CommentDelete { .. } => "comment.delete",
            Self::SuggestionAdd { .. } => "suggestion.add",
            Self::SuggestionAddBlockDelete { .. } => "suggestion.add_block_delete",
            Self::SuggestionAddMarkdown { .. } => "suggestion.add_markdown",
            Self::SuggestionAccept { .. } => "suggestion.accept",
            Self::SuggestionReject { .. } => "suggestion.reject",
            Self::ConflictAdd { .. } => "conflict.add",
            Self::ConflictKeepCanonical { .. } => "conflict.keep_canonical",
            Self::ConflictAcceptIncoming { .. } => "conflict.accept_incoming",
        }
    }

    fn primary_target(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::InsertBlock {
                block_id: Some(block_id),
                ..
            }
            | Self::DeleteBlock { block_id }
            | Self::MoveBlock { block_id, .. }
            | Self::ReplaceBlockContent { block_id, .. }
            | Self::SetBlockAttrs { block_id, .. }
            | Self::SetBlockType { block_id, .. }
            | Self::AddMark { block_id, .. }
            | Self::RemoveMark { block_id, .. }
            | Self::SetLink { block_id, .. }
            | Self::CommentAdd { block_id, .. }
            | Self::SuggestionAdd { block_id, .. }
            | Self::SuggestionAddBlockDelete { block_id, .. } => Some(("block", block_id)),
            Self::InsertMarkdown {
                after_block_id: Some(block_id),
                ..
            }
            | Self::SuggestionAddMarkdown {
                after_block_id: Some(block_id),
                ..
            }
            | Self::ConflictAdd {
                after_block_id: Some(block_id),
                ..
            } => Some(("block", block_id)),
            Self::CommentReply { item_id, .. }
            | Self::CommentEdit { item_id, .. }
            | Self::CommentResolve { item_id }
            | Self::CommentDelete { item_id }
            | Self::SuggestionAccept { item_id }
            | Self::SuggestionReject { item_id } => Some(("review_item", item_id)),
            Self::ConflictKeepCanonical { item_id } | Self::ConflictAcceptIncoming { item_id } => {
                Some(("conflict", item_id))
            }
            Self::InsertBlock { block_id: None, .. }
            | Self::InsertMarkdown {
                after_block_id: None,
                ..
            }
            | Self::SuggestionAddMarkdown {
                after_block_id: None,
                ..
            }
            | Self::ConflictAdd {
                after_block_id: None,
                ..
            } => None,
        }
    }
}

#[derive(Debug)]
struct ParsedTransaction {
    client_tx_id: String,
    base_clock: String,
    actor: BlockTransactionActor,
    ops: Vec<BlockOp>,
    ops_json: JsonValue,
}

fn parse_transaction(payload: JsonValue) -> Result<ParsedTransaction, GatewayError> {
    if payload.is_object()
        && payload
            .get("base_clock")
            .and_then(JsonValue::as_str)
            .is_none_or(|clock| clock.trim().is_empty())
    {
        return Err(GatewayError::invalid(
            "base_clock must be the nonempty document_clock from the read used to construct these operations",
        ).with_field("base_clock"));
    }
    let request: BlockTransactionRequest = serde_json::from_value(payload)
        .map_err(|error| GatewayError::invalid(format!("invalid transaction envelope: {error}")))?;
    if request.client_tx_id.trim().is_empty() {
        return Err(GatewayError::invalid("client_tx_id must not be empty"));
    }
    if request.actor.kind.trim().is_empty() {
        return Err(GatewayError::invalid("actor.kind must not be empty"));
    }
    if request.ops.is_empty() {
        return Err(GatewayError::invalid(
            "a transaction must contain at least one op",
        ));
    }
    let ops = request
        .ops
        .iter()
        .enumerate()
        .map(|(index, op)| parse_op(index, op))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ParsedTransaction {
        client_tx_id: request.client_tx_id,
        base_clock: request.base_clock,
        actor: request.actor,
        ops_json: JsonValue::Array(request.ops),
        ops,
    })
}

/// Deserializes one transaction op, qualifying any failure with the op name
/// and — for marks — the offending element and the expected shape. Serde's
/// bare "missing field `marks`" reads as the op-level field being absent even
/// when the real failure is a malformed run inside `marks[0]` (the
/// internally-tagged op enum buffers its content, so no JSON path survives
/// into serde errors).
fn parse_op(index: usize, op: &JsonValue) -> Result<BlockOp, GatewayError> {
    serde_json::from_value::<BlockOp>(op.clone()).map_err(|error| {
        let op_name = op.get("op").and_then(JsonValue::as_str).unwrap_or("?");
        let serde_message = error.to_string();
        let message = match marks_hint(op_name, op, &serde_message) {
            Some(hint) => format!("{error}; {hint}"),
            None => serde_message.clone(),
        };
        let failure = GatewayError::invalid(format!(
            "invalid op at index {index} ({op_name}): {message}"
        ))
        .with_operation(index, op_name);
        match invalid_op_field(op_name, &serde_message) {
            Some(field) => failure.with_field(field),
            None => failure,
        }
    })
}

fn invalid_op_field(op_name: &str, message: &str) -> Option<String> {
    if message.contains("`marks`") {
        return Some("marks".to_string());
    }
    if message.contains("unknown variant") {
        return Some("op".to_string());
    }
    let field = message.split_once("field `")?.1.split_once('`')?.0;
    (!field.is_empty())
        .then(|| field.to_string())
        .or_else(|| (op_name == "?").then(|| "op".to_string()))
}

/// The marks vocabulary is the most commonly guessed-wrong shape, so a parse
/// failure on an op whose `marks` field is malformed (or whose serde error
/// names `marks`) spells out the expected shape for that op.
fn marks_hint(op_name: &str, op: &JsonValue, message: &str) -> Option<String> {
    let marks = op.get("marks");
    let message_names_marks = message.contains("`marks`");
    match op_name {
        "add_mark" => (message_names_marks || marks.is_some_and(|m| !m.is_object())).then(|| {
            r#"in add_mark, `marks` is an object keyed by mark name, e.g. {"bold": true}"#
                .to_string()
        }),
        "remove_mark" => (message_names_marks || marks.is_some_and(|m| !m.is_array())).then(|| {
            r#"in remove_mark, `marks` is a list of mark names, e.g. ["bold"]"#.to_string()
        }),
        _ => {
            const RUN_SHAPE: &str = r#"a mark run is {"start": 0, "end": 5, "marks": {"bold": true}} — `marks` is an object keyed by mark name, never a list or a `type` field"#;
            if let Some(run_index) = first_bad_mark_run(op) {
                Some(format!(
                    "`marks[{run_index}]` is not a valid mark run; {RUN_SHAPE}"
                ))
            } else {
                message_names_marks.then(|| RUN_SHAPE.to_string())
            }
        }
    }
}

/// Index of the first element of `op.marks` that is not shaped
/// `{start, end, marks: <object>}`.
fn first_bad_mark_run(op: &JsonValue) -> Option<usize> {
    let runs = op.get("marks")?.as_array()?;
    runs.iter().position(|run| {
        !(run.get("start").is_some_and(JsonValue::is_u64)
            && run.get("end").is_some_and(JsonValue::is_u64)
            && run.get("marks").is_some_and(JsonValue::is_object))
    })
}

/// Unquotes an ETag-shaped clock token. `None` means the token is garbage —
/// the caller answers `STALE_BASE`.
fn unquote_clock(token: &str) -> Option<String> {
    let token = token.trim();
    let token = token.strip_prefix("W/").unwrap_or(token);
    let token = if token.starts_with('"') || token.ends_with('"') {
        token.strip_prefix('"')?.strip_suffix('"')?
    } else {
        token
    };
    if token.is_empty() || token.contains('"') {
        return None;
    }
    Some(token.to_string())
}

// ---------------------------------------------------------------------------
// Anchor adjustment over text-diff hunks (the diff helper is shared with the
// session splice through the quarry_markdown facade).
// ---------------------------------------------------------------------------

fn utf16_byte_offset(text: &str, target: u32) -> usize {
    let mut seen = 0u32;
    for (byte_index, ch) in text.char_indices() {
        if seen >= target {
            return byte_index;
        }
        seen += ch.len_utf16() as u32;
    }
    text.len()
}

fn utf16_slice(text: &str, start: u32, end: u32) -> String {
    text[utf16_byte_offset(text, start)..utf16_byte_offset(text, end)].to_string()
}

// ---------------------------------------------------------------------------
// In-memory document model
// ---------------------------------------------------------------------------

fn normalize_conflict_fragment(markdown: &str) -> Result<(usize, String), GatewayError> {
    if markdown.trim().is_empty() {
        return Ok((0, String::new()));
    }
    let mut fragment_id = 0u32;
    let rows = parse_markdown_fragment(markdown, || {
        let id = format!("conflict-fragment-{fragment_id}");
        fragment_id += 1;
        id
    })?;
    let root_count = rows
        .iter()
        .filter(|row| row.parent_block_id.is_none())
        .count();
    let normalized = block_rows_to_markdown(&rows).map_err(|unsupported| {
        GatewayError::new(
            GatewayErrorCode::UnsupportedMarkdown,
            unsupported.to_string(),
        )
    })?;
    Ok((root_count, normalized))
}

fn parse_markdown_fragment(
    markdown: &str,
    mint_block_id: impl FnMut() -> String,
) -> Result<Vec<BlockRow>, GatewayError> {
    let rows = markdown_to_block_rows(markdown, mint_block_id).map_err(|unsupported| {
        GatewayError::new(
            GatewayErrorCode::UnsupportedMarkdown,
            unsupported.to_string(),
        )
    })?;
    if rows.is_empty() {
        return Err(GatewayError::invalid(
            "markdown fragment must contain at least one block",
        ));
    }
    block_rows_to_markdown(&rows).map_err(|unsupported| {
        GatewayError::new(
            GatewayErrorCode::UnsupportedMarkdown,
            unsupported.to_string(),
        )
    })?;
    Ok(rows)
}

/// Ids the apply engine mints (inserted blocks without a caller id, review
/// items, the re-minted empty paragraph) derive deterministically from the
/// document id, the transaction's `client_tx_id`, and a counter: re-running
/// the SAME transaction's ops on the SAME document mints the SAME ids. This
/// makes a session-mode retry after a failed commit unable to silently
/// duplicate inserted content, while the same client transaction id remains
/// safely reusable on another document.
struct DeterministicIds {
    seed: String,
    next: u32,
}

impl DeterministicIds {
    fn new(document_id: &str, client_tx_id: &str) -> Self {
        Self {
            seed: format!("{document_id}\0{client_tx_id}"),
            next: 0,
        }
    }

    fn mint(&mut self) -> String {
        let counter = self.next;
        self.next += 1;
        let hash = blake3::hash(format!("{}\0{counter}", self.seed).as_bytes());
        Uuid::from_slice(&hash.as_bytes()[..16])
            .expect("16 hash bytes always form a uuid")
            .to_string()
    }
}

fn conflict_hunk_changed(item_id: &str) -> GatewayError {
    GatewayError::new(
        GatewayErrorCode::Conflict,
        format!(
            "conflict {item_id} no longer matches the canonical document; re-read /blocks and /review"
        ),
    )
    .with_target("conflict", item_id)
}

fn validate_block_type(block_type: &str) -> Result<(), GatewayError> {
    if is_known_block_type(block_type) {
        return Ok(());
    }
    let allowed_values: Vec<String> = known_block_types().map(str::to_string).collect();
    let valid_types = allowed_values.join(", ");
    Err(GatewayError::new(
        GatewayErrorCode::UnknownBlockType,
        format!(
            "unknown block_type \"{block_type}\"; valid types: {}. There is no list \
             block type — a list item is a \"p\" block with attrs \
             {{\"indent\": 1, \"listStyleType\": \"disc\" | \"decimal\" | \"todo\"}}",
            valid_types
        ),
    )
    .with_validation("block_type", block_type, allowed_values))
}

/// The `id` attribute is the block identity on exported syntax nodes; ops
/// never smuggle it through attrs.
fn validate_attrs(attrs: &Attrs) -> Result<(), GatewayError> {
    if attrs.contains_key("id") {
        return Err(GatewayError::invalid(
            "attrs must not contain the reserved key \"id\"",
        ));
    }
    Ok(())
}

const LIST_STYLE_TYPES: [&str; 3] = ["disc", "decimal", "todo"];

/// Validates list attributes before translating them into native commands.
/// Lists use paragraphs with a positive indent. Only numbered lists use a
/// start number. Only task lists use a checked state.
fn normalize_list_attrs(block_type: &str, attrs: &Attrs) -> Result<Attrs, GatewayError> {
    let mut attrs = attrs.clone();
    let Some(style_value) = attrs.get("listStyleType") else {
        attrs.shift_remove("listStart");
        attrs.shift_remove("checked");
        return Ok(attrs);
    };
    let Some(style) = style_value
        .as_str()
        .filter(|style| LIST_STYLE_TYPES.contains(style))
        .map(str::to_string)
    else {
        return Err(GatewayError::invalid(format!(
            "unknown listStyleType {style_value}; valid values: \"disc\", \"decimal\", \"todo\""
        ))
        .with_validation(
            "attrs.listStyleType",
            display_json_value(style_value),
            LIST_STYLE_TYPES.map(str::to_string).to_vec(),
        ));
    };
    if block_type != "p" {
        return Err(GatewayError::invalid(format!(
            "listStyleType is valid only on p blocks, not {block_type}"
        ))
        .with_validation("block_type", block_type, vec!["p".to_string()]));
    }

    match attrs.get("indent") {
        None => {
            attrs.insert("indent".to_string(), json!(1));
        }
        Some(indent) if indent.as_u64().is_some_and(|indent| indent >= 1) => {}
        Some(indent) => {
            return Err(GatewayError::invalid(format!(
                "list indent must be a positive integer, got {indent}"
            ))
            .with_validation("attrs.indent", display_json_value(indent), Vec::new()));
        }
    }

    match style.as_str() {
        "disc" => {
            attrs.shift_remove("listStart");
            attrs.shift_remove("checked");
        }
        "decimal" => {
            attrs.shift_remove("checked");
            if let Some(list_start) = attrs.get("listStart")
                && list_start.as_u64().is_none()
            {
                return Err(GatewayError::invalid(format!(
                    "decimal listStart must be a non-negative integer, got {list_start}"
                ))
                .with_validation(
                    "attrs.listStart",
                    display_json_value(list_start),
                    Vec::new(),
                ));
            }
        }
        "todo" => {
            attrs.shift_remove("listStart");
            match attrs.get("checked") {
                None => {
                    attrs.insert("checked".to_string(), json!(false));
                }
                Some(checked) if checked.is_boolean() => {}
                Some(checked) => {
                    return Err(GatewayError::invalid(format!(
                        "todo checked must be a boolean, got {checked}"
                    ))
                    .with_validation(
                        "attrs.checked",
                        display_json_value(checked),
                        ["false", "true"].map(str::to_string).to_vec(),
                    ));
                }
            }
        }
        _ => unreachable!("list style was checked against LIST_STYLE_TYPES"),
    }
    Ok(attrs)
}

fn display_json_value(value: &JsonValue) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

/// A raw_markdown block's whole content lives in its `markdown` attribute;
/// the Markdown writer emits an empty block if it is missing or blank.
fn validate_raw_markdown_attrs(attrs: &Attrs) -> Result<(), GatewayError> {
    let markdown = attrs.get("markdown").and_then(JsonValue::as_str);
    if markdown.is_none_or(str::is_empty) {
        return Err(GatewayError::invalid(
            "raw_markdown blocks require a non-empty string markdown attribute",
        ));
    }
    Ok(())
}

fn validate_span(text: &str, start: u32, end: u32) -> Result<(), GatewayError> {
    if start >= end {
        return Err(GatewayError::invalid(format!(
            "span [{start}, {end}) must be non-empty"
        )));
    }
    if end > utf16_len(text) {
        return Err(GatewayError::invalid(format!(
            "span [{start}, {end}) is past the block text (UTF-16 length {})",
            utf16_len(text)
        )));
    }
    if !is_utf16_boundary(text, start) || !is_utf16_boundary(text, end) {
        return Err(GatewayError::invalid(format!(
            "span [{start}, {end}) splits a surrogate pair"
        )));
    }
    Ok(())
}

fn validate_inline_ranges(
    text: &str,
    marks: &[MarkRun],
    links: &[LinkRange],
) -> Result<(), GatewayError> {
    let mut previous_end = 0u32;
    for run in marks {
        validate_span(text, run.start, run.end)?;
        if run.marks.is_empty() {
            return Err(GatewayError::invalid("mark runs must carry marks"));
        }
        validate_attrs(&run.marks)?;
        if run.start < previous_end {
            return Err(GatewayError::invalid(
                "mark runs must be ordered and disjoint",
            ));
        }
        previous_end = run.end;
    }
    let mut previous_end = 0u32;
    for link in links {
        validate_span(text, link.start, link.end)?;
        if link.start < previous_end {
            return Err(GatewayError::invalid(
                "link ranges must be ordered and disjoint",
            ));
        }
        previous_end = link.end;
    }
    Ok(())
}

/// Rebuilds a block's mark runs with `change` applied to every segment of
/// `[start, end)`, preserving formatting outside the span and coalescing
/// adjacent equal runs.
pub(crate) async fn document_blocks(
    state: &AppState,
    library: &str,
    path: &str,
) -> Result<Response, ApiError> {
    gateway_reply(document_blocks_for_scope(state, &DocumentScopeRef::library(library), path).await)
}

pub(crate) async fn tmp_document_blocks(
    state: &AppState,
    path: &str,
) -> Result<Response, ApiError> {
    gateway_reply(document_blocks_for_scope(state, &DocumentScopeRef::Tmp, path).await)
}

async fn document_blocks_for_scope(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
) -> Result<Response, GatewayFailure> {
    let saved = state
        .store
        .durable_document_for_scope(scope, path)
        .await?
        .ok_or_else(|| {
            GatewayError::new(
                GatewayErrorCode::UnsupportedBlockDocument,
                "Document has no native state",
            )
        })?;
    let document = quarry_document::Document::load(&saved.bytes)
        .map_err(crate::document_engine::document_error)?;
    let rows = quarry_storage::document_projection(&document)?;
    let payload = BlockTreeResponse {
        document_id: saved.document_id,
        document_clock: saved.version_id.clone(),
        blocks: rows.into_iter().map(block_payload).collect(),
    };
    Ok(json_with_etag(StatusCode::OK, &payload, &saved.version_id)?)
}

fn block_payload(row: BlockRow) -> BlockNodePayload {
    BlockNodePayload {
        block_id: row.block_id,
        parent_block_id: row.parent_block_id,
        position: row.position,
        block_type: row.block_type,
        attrs: row.attrs,
        text: row.text,
        marks: row
            .marks
            .into_iter()
            .map(|run| BlockMarkRunPayload {
                start: run.start,
                end: run.end,
                marks: run.marks,
            })
            .collect(),
        links: row
            .links
            .into_iter()
            .map(|link| BlockLinkRangePayload {
                start: link.start,
                end: link.end,
                url: link.url,
            })
            .collect(),
    }
}

/// Per-call write options for [`execute_block_transaction`]: the REST route
/// uses the defaults; whole-file reconciled writes (Phase 4) carry their
/// surface's source, an origin id for event classification, and the merged
/// frontmatter metadata.
pub(crate) struct TransactionSettings {
    pub source: DocumentSource,
    pub origin_id: Option<String>,
    /// Replaces the document metadata at commit (whole-file writes carry
    /// incoming frontmatter); `None` keeps the snapshot metadata.
    pub metadata: Option<JsonValue>,
    /// Legacy `x-quarry-transaction-*` attribution carried by direct PUTs;
    /// actor falls back to the transaction actor's display name.
    pub transaction: quarry_storage::TransactionMetadata,
}

impl Default for TransactionSettings {
    fn default() -> Self {
        Self {
            source: DocumentSource::Rest,
            origin_id: None,
            metadata: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        }
    }
}

/// The provenance recorded on a gateway commit: an explicit
/// `x-quarry-transaction-provenance` value passes through, an explicit
/// null/empty object engages the store's per-commit default, and an absent
/// header keeps the legacy `auto_commit` marker.
fn commit_provenance(settings: &TransactionSettings) -> Option<JsonValue> {
    match &settings.transaction.provenance {
        None => Some(json!({ "mode": "auto_commit" })),
        Some(provenance) if provenance.is_null() || *provenance == json!({}) => None,
        Some(provenance) => Some(provenance.clone()),
    }
}

/// The transaction envelope minus the ops (those come from the plan
/// provider, recomputed per snapshot).
pub(crate) struct TransactionContext {
    pub client_tx_id: String,
    pub base_clock: Option<String>,
    pub actor: BlockTransactionActor,
}

/// One application's ops, computed against a specific snapshot. The provider
/// is re-invoked when a commit retry reloads the snapshot, so plans derived
/// from the snapshot (the diff3 reconcile) never go stale.
pub(crate) struct TransactionPlan {
    pub ops: Vec<BlockOp>,
    /// Reconciliation has already translated these offsets to this snapshot.
    /// Fixed agent requests still address the caller's base clock.
    pub uses_current_snapshot: bool,
    /// Recorded verbatim in `block_transactions.ops` history.
    pub ops_json: JsonValue,
}

type PlanProvider<'a> =
    &'a mut (dyn FnMut(&BlockMutationState) -> Result<TransactionPlan, GatewayFailure> + Send);

pub(crate) struct CommittedTransaction {
    pub status: &'static str,
    pub outcome: Box<quarry_core::WriteOutcome>,
    pub transaction_id: String,
    pub changed_block_ids: Vec<String>,
    pub created_conflict_ids: Vec<String>,
}

pub(crate) enum TransactionReply {
    Committed(CommittedTransaction),
    Replayed(BlockTransactionRecord),
}

pub(crate) async fn document_block_transactions(
    state: &AppState,
    library: &str,
    path: &str,
    payload: JsonValue,
) -> Result<Response, ApiError> {
    gateway_reply(
        document_block_transactions_for_scope(
            state,
            &DocumentScopeRef::library(library),
            path,
            payload,
        )
        .await,
    )
}

pub(crate) async fn tmp_document_block_transactions(
    state: &AppState,
    path: &str,
    payload: JsonValue,
) -> Result<Response, ApiError> {
    gateway_reply(
        document_block_transactions_for_scope(state, &DocumentScopeRef::Tmp, path, payload).await,
    )
}

async fn document_block_transactions_for_scope(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    payload: JsonValue,
) -> Result<Response, GatewayFailure> {
    let request = parse_transaction(payload)?;
    let ctx = TransactionContext {
        client_tx_id: request.client_tx_id,
        base_clock: Some(request.base_clock),
        actor: request.actor,
    };
    let (ops, ops_json) = (request.ops, request.ops_json);
    let mut plan = move |_snapshot: &BlockMutationState| {
        Ok(TransactionPlan {
            uses_current_snapshot: false,
            ops: ops.clone(),
            ops_json: ops_json.clone(),
        })
    };
    let reply = execute_block_transaction(
        state,
        scope,
        path,
        &ctx,
        &TransactionSettings::default(),
        &mut plan,
    )
    .await?;
    transaction_reply_response(reply)
}

fn transaction_reply_response(reply: TransactionReply) -> Result<Response, GatewayFailure> {
    match reply {
        TransactionReply::Committed(committed) => {
            let ack = BlockTransactionAck {
                status: committed.status.to_string(),
                document_clock: committed.outcome.version.id.to_string(),
                transaction_id: committed.transaction_id,
                changed_block_ids: committed.changed_block_ids,
            };
            Ok(json_with_etag(StatusCode::OK, &ack, &ack.document_clock)?)
        }
        TransactionReply::Replayed(record) => replay_response(&record),
    }
}

/// Serialize writers by document identity, translate public operations into
/// native commands, and atomically publish their state and projections.
pub(crate) async fn execute_block_transaction(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    ctx: &TransactionContext,
    settings: &TransactionSettings,
    plan: PlanProvider<'_>,
) -> Result<TransactionReply, GatewayFailure> {
    state
        .store
        .run_global_operation(async {
            let document = state.store.head_document_for_scope(scope, path).await?;
            let _guard = state.documents.lock(&document.id).await;
            Box::pin(native::apply_transaction(
                state, scope, path, ctx, settings, plan,
            ))
            .await
        })
        .await
}

fn block_mutation_reply(
    outcome: BlockMutationOutcome,
    status: &'static str,
    changed_block_ids: Vec<String>,
    created_conflict_ids: Vec<String>,
) -> TransactionReply {
    match outcome {
        BlockMutationOutcome::Applied { outcome, record } => {
            TransactionReply::Committed(CommittedTransaction {
                status,
                outcome,
                transaction_id: record.id,
                changed_block_ids,
                created_conflict_ids,
            })
        }
        BlockMutationOutcome::Replayed(record) => TransactionReply::Replayed(record),
    }
}

fn normalized_markdown(rows: &[BlockRow], metadata: &JsonValue) -> Result<String, GatewayFailure> {
    let body = block_rows_to_markdown(rows).map_err(|unsupported| {
        GatewayError::new(
            GatewayErrorCode::UnsupportedMarkdown,
            unsupported.to_string(),
        )
    })?;
    Ok(format!(
        "{}{}",
        render_markdown_frontmatter(metadata).map_err(GatewayFailure::from)?,
        body
    ))
}

pub(crate) fn require_block_document(path: &str, content_type: &str) -> Result<(), GatewayError> {
    if document_kind(path, content_type) == DocumentKind::RawDocument {
        return Err(GatewayError::new(
            GatewayErrorCode::UnsupportedBlockDocument,
            format!("{path} ({content_type}) is a raw document outside the block model"),
        ));
    }
    Ok(())
}

fn transaction_status(
    base_clock: &Option<String>,
    snapshot: &BlockMutationState,
) -> Result<&'static str, GatewayError> {
    let Some(token) = base_clock else {
        return Ok("committed");
    };
    let Some(clock) = unquote_clock(token) else {
        return Err(stale_base(token, &snapshot.head_version_id));
    };
    if clock == snapshot.head_version_id {
        Ok("committed")
    } else if snapshot.version_ids.contains(&clock) {
        Ok("committed_rebased")
    } else {
        Err(stale_base(token, &snapshot.head_version_id))
    }
}

fn stale_base(token: &str, current_clock: &str) -> GatewayError {
    GatewayError::new(
        GatewayErrorCode::StaleBase,
        format!("base_clock {token} does not name a known version of this document"),
    )
    .with_validation("base_clock", token, Vec::new())
    .with_current_value(current_clock)
}

/// Answers a duplicate `client_tx_id` from the stored history record: the
/// ORIGINAL ack (status and changed ids ride in the record's `ops.ack`).
fn replay_response(record: &BlockTransactionRecord) -> Result<Response, GatewayFailure> {
    let document_clock = record.resulting_version_id.clone().ok_or_else(|| {
        GatewayFailure::Api(
            QuarryError::Invariant(format!(
                "block transaction {} has no resulting version to replay",
                record.id
            ))
            .into(),
        )
    })?;
    let ack_meta = &record.ops["ack"];
    let ack = BlockTransactionAck {
        status: ack_meta["status"]
            .as_str()
            .unwrap_or("committed")
            .to_string(),
        document_clock: document_clock.clone(),
        transaction_id: record.id.clone(),
        changed_block_ids: ack_meta["changed_block_ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    };
    Ok(json_with_etag(StatusCode::OK, &ack, &document_clock)?)
}

// ---------------------------------------------------------------------------
// Rows-backed review projection
// ---------------------------------------------------------------------------

pub(crate) fn review_response_from_rows(
    document_id: String,
    base_token: String,
    rows: &[BlockRow],
    items: &[BlockReviewItem],
    include_resolved: bool,
) -> AgentReviewResponse {
    let ordinals: HashMap<&str, usize> = rows
        .iter()
        .enumerate()
        .map(|(ordinal, row)| (row.block_id.as_str(), ordinal))
        .collect();
    let texts: HashMap<&str, &str> = rows
        .iter()
        .map(|row| (row.block_id.as_str(), row.text.as_str()))
        .collect();
    let block_ref = |item: &BlockReviewItem| AgentBlockRef {
        block_id: (!item.block_id.is_empty()).then(|| item.block_id.clone()),
    };
    let anchor = |item: &BlockReviewItem| {
        ordinals
            .contains_key(item.block_id.as_str())
            .then(|| BlockReviewAnchor {
                block_id: item.block_id.clone(),
                start_offset: item.start_offset,
                end_offset: item.end_offset,
            })
    };
    let anchored_text = |item: &BlockReviewItem| {
        texts
            .get(item.block_id.as_str())
            .filter(|text| item.end_offset <= utf16_len(text))
            .map(|text| utf16_slice(text, item.start_offset, item.end_offset))
    };
    let quote = |item: &BlockReviewItem| {
        item.quote
            .clone()
            .or_else(|| anchored_text(item))
            .unwrap_or_default()
    };
    let by = |item: &BlockReviewItem| item.author.clone().unwrap_or_else(|| "unknown".to_string());
    let edited_at = |item: &BlockReviewItem| {
        (item.updated_at != item.created_at).then(|| item.updated_at.clone())
    };
    let mut replies_by_parent: HashMap<String, Vec<AgentReviewReply>> = HashMap::new();
    for reply in items
        .iter()
        .filter(|item| item.kind == BlockReviewKind::Comment)
        .filter(|item| include_resolved || item.state != BlockReviewState::Resolved)
    {
        let Some(parent_id) = reply.parent_item_id.as_deref() else {
            continue;
        };
        replies_by_parent
            .entry(parent_id.to_string())
            .or_default()
            .push(AgentReviewReply {
                target: None,
                id: reply.id.clone(),
                status: reply.state.as_str().to_string(),
                by: by(reply),
                at: reply.created_at.clone(),
                edited_at: edited_at(reply),
                body: reply.body.clone().unwrap_or_default(),
            });
    }

    let comments = items
        .iter()
        .filter(|item| item.kind == BlockReviewKind::Comment && item.parent_item_id.is_none())
        .filter(|item| include_resolved || item.state != BlockReviewState::Resolved)
        .map(|item| AgentReviewComment {
            target: None,
            id: item.id.clone(),
            status: item.state.as_str().to_string(),
            by: by(item),
            at: item.created_at.clone(),
            edited_at: edited_at(item),
            block_ref: block_ref(item),
            quote: quote(item),
            body: item.body.clone().unwrap_or_default(),
            replies: replies_by_parent.remove(&item.id).unwrap_or_default(),
            anchor: anchor(item),
        })
        .collect();

    let suggestions = items
        .iter()
        .filter(|item| item.kind == BlockReviewKind::Suggestion)
        .filter(|item| include_resolved || item.state != BlockReviewState::Resolved)
        .map(|item| {
            let block_delete = item.is_block_delete_suggestion();
            let markdown_insert = item.is_markdown_insert_suggestion();
            let replacement = item.replacement.clone().unwrap_or_default();
            AgentReviewSuggestion {
                action: None,
                acceptance_error: None,
                target: None,
                id: item.id.clone(),
                status: item.state.as_str().to_string(),
                kind: if markdown_insert {
                    AgentSuggestionKind::MarkdownInsert
                } else if block_delete {
                    AgentSuggestionKind::BlockDelete
                } else if replacement.is_empty() {
                    AgentSuggestionKind::Delete
                } else {
                    AgentSuggestionKind::Replace
                },
                by: by(item),
                at: item.created_at.clone(),
                block_ref: block_ref(item),
                quote: quote(item),
                content: replacement.clone(),
                body: item.body.clone(),
                preview: AgentSuggestionPreview {
                    before: if markdown_insert {
                        String::new()
                    } else if block_delete {
                        texts
                            .get(item.block_id.as_str())
                            .filter(|text| !text.is_empty())
                            .map(|text| (*text).to_string())
                            .unwrap_or_else(|| quote(item))
                    } else {
                        anchored_text(item).unwrap_or_else(|| quote(item))
                    },
                    after: replacement,
                },
                replies: replies_by_parent.remove(&item.id).unwrap_or_default(),
                anchor: anchor(item),
            }
        })
        .collect();

    // Conflict items store the attachment point in `block_id` ("" = document
    // start), the incoming hunk in `body`, the base context in
    // `context_before`, and the retained canonical side in `quote`.
    let conflicts = items
        .iter()
        .filter(|item| item.kind == BlockReviewKind::Conflict)
        .filter(|item| include_resolved || item.state != BlockReviewState::Resolved)
        .map(|item| crate::AgentReviewConflict {
            replies: replies_by_parent.remove(&item.id).unwrap_or_default(),
            id: item.id.clone(),
            status: item.state.as_str().to_string(),
            by: by(item),
            at: item.created_at.clone(),
            after_block_id: Some(item.block_id.clone()).filter(|id| !id.is_empty()),
            base_markdown: item.context_before.clone().unwrap_or_default(),
            incoming_markdown: item.body.clone().unwrap_or_default(),
            canonical_markdown: item.quote.clone().unwrap_or_default(),
        })
        .collect();

    AgentReviewResponse {
        document_id,
        base_token,
        comments,
        suggestions,
        conflicts,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "tests use unwrap for transaction fixtures"
    )]

    use super::native::apply_test_ops as apply_ops;
    use super::*;

    fn paragraph(block_id: &str, position: u32, text: &str) -> BlockRow {
        BlockRow {
            block_id: block_id.to_string(),
            parent_block_id: None,
            position,
            block_type: "p".to_string(),
            attrs: Attrs::new(),
            text: text.to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        }
    }

    fn state_with_rows(rows: Vec<BlockRow>) -> BlockMutationState {
        BlockMutationState {
            document_id: "doc-1".to_string(),
            path: "doc.md".to_string(),
            head_version_id: "v1".to_string(),
            content_type: "text/markdown".to_string(),
            metadata: serde_json::json!({}),
            rows,
            review_items: Vec::new(),
            version_ids: std::collections::HashSet::from(["v1".to_string()]),
            replay: None,
        }
    }

    fn actor() -> BlockTransactionActor {
        BlockTransactionActor {
            kind: "agent".to_string(),
            id: Some("agent-1".to_string()),
            label: Some("Agent One".to_string()),
        }
    }

    fn op(value: JsonValue) -> BlockOp {
        serde_json::from_value(value).expect("test op must parse")
    }

    /// The exact failure an agent hit in the wild: a mark run written as
    /// `{"type": "strong", ...}` fails INSIDE `marks[0]`, but serde's bare
    /// "missing field `marks`" read as if the op-level `marks` field were
    /// absent — the agent retried the identical payload. The error must name
    /// the op, the offending run, and the expected run shape.
    #[test]
    fn op_parse_errors_name_the_op_the_bad_run_and_the_mark_shape() {
        let error = parse_op(
            7,
            &json!({
                "op": "insert_block",
                "position": 1,
                "block_type": "p",
                "text": "Linux consideration: both work.",
                "marks": [{"type": "strong", "start": 0, "end": 20}]
            }),
        )
        .unwrap_err();

        let message = error.message();
        assert!(
            message.starts_with("invalid op at index 7 (insert_block): missing field `marks`"),
            "got: {message}"
        );
        assert!(
            message.contains("`marks[0]` is not a valid mark run"),
            "got: {message}"
        );
        assert!(
            message.contains(r#"a mark run is {"start": 0, "end": 5, "marks": {"bold": true}}"#),
            "got: {message}"
        );
    }

    /// `remove_mark` takes a LIST of names where `add_mark` takes an object;
    /// each hint must state its own op's shape, not the run shape.
    #[test]
    fn remove_mark_parse_error_hints_the_list_shape() {
        let error = parse_op(
            0,
            &json!({
                "op": "remove_mark",
                "block_id": "b1",
                "start": 0,
                "end": 4,
                "marks": {"bold": true}
            }),
        )
        .unwrap_err();

        let message = error.message();
        assert!(
            message.starts_with("invalid op at index 0 (remove_mark):"),
            "got: {message}"
        );
        assert!(
            message.contains(r#"`marks` is a list of mark names, e.g. ["bold"]"#),
            "got: {message}"
        );
    }

    #[test]
    fn add_mark_parse_error_hints_the_object_shape() {
        let error = parse_op(
            0,
            &json!({
                "op": "add_mark",
                "block_id": "b1",
                "start": 0,
                "end": 4,
                "marks": ["bold"]
            }),
        )
        .unwrap_err();

        let message = error.message();
        assert!(
            message.contains(r#"`marks` is an object keyed by mark name, e.g. {"bold": true}"#),
            "got: {message}"
        );
    }

    /// Failures with no `marks` involvement stay hint-free.
    #[test]
    fn op_parse_error_without_marks_has_no_hint() {
        let error = parse_op(2, &json!({"op": "made_up_op"})).unwrap_err();

        let message = error.message();
        assert!(
            message.starts_with("invalid op at index 2 (made_up_op): unknown variant"),
            "got: {message}"
        );
        assert!(!message.contains("mark run"), "got: {message}");
    }

    /// The wild `ul` guess: an unknown block type must fail typed at the API
    /// boundary — naming the valid vocabulary and the list-item recipe — not
    /// surface later as an UNSUPPORTED_MARKDOWN rendering error.
    #[test]
    fn unknown_block_type_fails_typed_with_vocabulary_and_list_recipe() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Existing.")]);
        let ops = [op(
            json!({"op": "insert_block", "position": 1, "block_type": "ul", "text": "item"}),
        )];

        let error = apply_ops(&state, &ops, &actor(), "tx-ul").unwrap_err();

        assert_eq!(error.code(), GatewayErrorCode::UnknownBlockType);
        let message = error.message();
        assert!(
            message.starts_with("unknown block_type \"ul\"; valid types: p, h1, h2"),
            "got: {message}"
        );
        assert!(
            message.contains(r#"a list item is a "p" block with attrs"#),
            "got: {message}"
        );
    }

    /// The shape an agent sent during a real structural edit: sequential
    /// `listStart` and `checked` values on an unordered list are meaningless
    /// and may be removed by the editor after the ack. The committed row must
    /// already be the browser-stable shape.
    #[test]
    fn unordered_list_attrs_are_canonicalized_before_commit() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Existing.")]);
        let ops = [op(json!({
            "op": "insert_block",
            "position": 1,
            "block_type": "p",
            "attrs": {"listStyleType": "disc", "listStart": 2, "checked": true},
            "text": "CPU: AMD Ryzen 5 7600"
        }))];

        let outcome = apply_ops(&state, &ops, &actor(), "tx-list").unwrap();

        let item = outcome
            .rows
            .iter()
            .find(|row| row.text == "CPU: AMD Ryzen 5 7600")
            .expect("inserted row");
        assert_eq!(item.attrs.get("listStyleType"), Some(&json!("disc")));
        assert_eq!(item.attrs.get("indent"), Some(&json!(1)));
        assert!(!item.attrs.contains_key("listStart"));
        assert!(!item.attrs.contains_key("checked"));
    }

    #[test]
    fn todo_list_attrs_default_checked_and_drop_ordering() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Item one.")]);
        let ops = [op(json!({
            "op": "set_block_attrs",
            "block_id": "b1",
            "attrs": {"listStyleType": "todo", "listStart": 9}
        }))];

        let outcome = apply_ops(&state, &ops, &actor(), "tx-attrs").unwrap();

        let item = outcome.rows.first().expect("row");
        assert_eq!(item.attrs.get("indent"), Some(&json!(1)));
        assert_eq!(item.attrs.get("checked"), Some(&json!(false)));
        assert!(!item.attrs.contains_key("listStart"));
    }

    #[test]
    fn decimal_list_attrs_keep_ordering_and_drop_todo_state() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Item one.")]);
        let ops = [op(json!({
            "op": "set_block_attrs",
            "block_id": "b1",
            "attrs": {"indent": 2, "listStyleType": "decimal", "listStart": 7, "checked": true}
        }))];

        let outcome = apply_ops(&state, &ops, &actor(), "tx-decimal").unwrap();

        let item = outcome.rows.first().expect("row");
        assert_eq!(item.attrs.get("indent"), Some(&json!(2)));
        assert_eq!(item.attrs.get("listStart"), Some(&json!(7)));
        assert!(!item.attrs.contains_key("checked"));
    }

    /// An unrepresentable list style would be stripped by the editor just
    /// like a missing indent; reject it loudly instead.
    #[test]
    fn unknown_list_style_type_is_rejected() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Existing.")]);
        let ops = [op(json!({
            "op": "insert_block",
            "position": 1,
            "block_type": "p",
            "attrs": {"listStyleType": "circle"},
            "text": "item"
        }))];

        let error = apply_ops(&state, &ops, &actor(), "tx-style").unwrap_err();

        assert_eq!(error.code(), GatewayErrorCode::InvalidTransaction);
        assert!(
            error.message().contains(
                r#"unknown listStyleType "circle"; valid values: "disc", "decimal", "todo""#
            ),
            "got: {}",
            error.message()
        );
        let details = error.details.as_deref().expect("structured details");
        assert_eq!(details.op_index, Some(0));
        assert_eq!(details.field.as_deref(), Some("attrs.listStyleType"));
        assert_eq!(details.value.as_deref(), Some("circle"));
        assert_eq!(details.allowed_values, ["disc", "decimal", "todo"]);
    }

    #[test]
    fn malformed_list_attrs_are_rejected_before_commit() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Existing.")]);
        let ops = [op(json!({
            "op": "insert_block",
            "position": 1,
            "block_type": "p",
            "attrs": {"indent": 0, "listStyleType": "todo", "checked": "yes"},
            "text": "item"
        }))];

        let error = apply_ops(&state, &ops, &actor(), "tx-bad-list").unwrap_err();

        assert_eq!(error.code(), GatewayErrorCode::InvalidTransaction);
        let details = error.details.as_deref().expect("structured details");
        assert_eq!(details.field.as_deref(), Some("attrs.indent"));
        assert_eq!(details.value.as_deref(), Some("0"));
    }

    #[test]
    fn list_attrs_are_rejected_on_non_paragraph_blocks() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Existing.")]);
        let ops = [op(json!({
            "op": "insert_block",
            "position": 1,
            "block_type": "h2",
            "attrs": {"indent": 1, "listStyleType": "disc"},
            "text": "Not a list item"
        }))];

        let error = apply_ops(&state, &ops, &actor(), "tx-heading-list").unwrap_err();

        assert_eq!(error.code(), GatewayErrorCode::InvalidTransaction);
        let details = error.details.as_deref().expect("structured details");
        assert_eq!(details.field.as_deref(), Some("block_type"));
        assert_eq!(details.value.as_deref(), Some("h2"));
        assert_eq!(details.allowed_values, ["p"]);
    }

    #[test]
    fn changing_a_list_paragraph_type_drops_only_its_list_shape() {
        let mut item = paragraph("b1", 0, "Former list item.");
        item.attrs = serde_json::from_value(json!({
            "indent": 2,
            "listStyleType": "disc",
            "listStart": 2,
            "listRestart": 2,
            "listRestartPolite": 2,
            "checked": true,
            "custom": "keep"
        }))
        .expect("attrs");
        let state = state_with_rows(vec![item]);
        let ops = [op(json!({
            "op": "set_block_type",
            "block_id": "b1",
            "block_type": "h2"
        }))];

        let outcome = apply_ops(&state, &ops, &actor(), "tx-list-heading").unwrap();

        let heading = outcome.rows.first().expect("row");
        let expected_attrs: Attrs =
            serde_json::from_value(json!({"custom": "keep"})).expect("attrs");
        assert_eq!(heading.block_type, "h2");
        assert_eq!(heading.attrs, expected_attrs);
    }

    /// A session-mode retry after a failed commit re-runs the same ops; the
    /// engine must mint the SAME ids so re-application cannot silently
    /// duplicate inserted content (it collides and fails typed instead).
    /// Another document may reuse the client transaction id safely.
    #[test]
    fn minted_ids_are_deterministic_per_document_and_client_transaction() {
        let rows = vec![BlockRow {
            block_id: "b1".to_string(),
            parent_block_id: None,
            position: 0,
            block_type: "p".to_string(),
            attrs: Attrs::new(),
            text: "Existing.".to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        }];
        let ops = [
            op(json!({"op": "insert_block", "position": 1, "block_type": "p", "text": "New."})),
            op(json!({"op": "comment.add", "block_id": "b1", "start": 0, "end": 8, "body": "hi"})),
        ];
        let state = state_with_rows(rows);

        let first = apply_ops(&state, &ops, &actor(), "tx-deterministic").unwrap();
        let second = apply_ops(&state, &ops, &actor(), "tx-deterministic").unwrap();
        assert_eq!(first.rows, second.rows);
        // Timestamps come from the wall clock at apply time; everything else
        // must be identical.
        let timeless = |items: &[BlockReviewItem]| -> Vec<BlockReviewItem> {
            items
                .iter()
                .cloned()
                .map(|mut item| {
                    item.created_at = String::new();
                    item.updated_at = String::new();
                    item
                })
                .collect()
        };
        assert_eq!(
            timeless(&first.review_items),
            timeless(&second.review_items)
        );

        // The same client transaction id belongs to an independent
        // idempotency namespace on another document.
        let mut other_document = state.clone();
        other_document.document_id = "doc-2".to_string();
        let in_other_document =
            apply_ops(&other_document, &ops, &actor(), "tx-deterministic").unwrap();
        assert_ne!(first.changed_block_ids, in_other_document.changed_block_ids);
        assert_ne!(
            first.review_items[0].id,
            in_other_document.review_items[0].id
        );

        // A DIFFERENT transaction mints different ids.
        let other = apply_ops(&state, &ops, &actor(), "tx-other").unwrap();
        assert_ne!(first.changed_block_ids, other.changed_block_ids);

        // Re-applying the same transaction against its own post-state (the
        // failed-commit retry shape) collides instead of duplicating.
        let mut post = state_with_rows(first.rows.clone());
        post.review_items = first.review_items.clone();
        let inserted_id = first
            .rows
            .iter()
            .find(|row| row.text == "New.")
            .unwrap()
            .block_id
            .clone();
        let retry_ops = [op(json!({
            "op": "insert_block",
            "position": 1,
            "block_type": "p",
            "text": "New."
        }))];
        let error = apply_ops(&post, &retry_ops, &actor(), "tx-deterministic").unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
        assert!(error.message.contains(&inserted_id));

        // Review items collide the same way: a comment-only transaction
        // retried against its own post-state re-mints the same item id.
        let comment_ops = [op(json!({
            "op": "comment.add", "block_id": "b1", "start": 0, "end": 8, "body": "hi"
        }))];
        let commented = apply_ops(&state, &comment_ops, &actor(), "tx-comment-only").unwrap();
        let mut commented_post = state_with_rows(commented.rows.clone());
        commented_post.review_items = commented.review_items.clone();
        let error =
            apply_ops(&commented_post, &comment_ops, &actor(), "tx-comment-only").unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
        assert!(error.message.contains(&commented.review_items[0].id));
    }

    #[test]
    fn replace_block_content_keeps_an_interior_comment_when_both_ends_change() {
        let state = state_with_rows(vec![paragraph("p1", 0, "AAA middle ZZZ")]);
        let commented = apply_ops(
            &state,
            &[op(json!({
                "op": "comment.add", "block_id": "p1", "start": 4, "end": 10, "body": "keep me"
            }))],
            &actor(),
            "tx-comment",
        )
        .unwrap();
        let mut with_comment = state_with_rows(commented.rows.clone());
        with_comment.review_items = commented.review_items.clone();

        let applied = apply_ops(
            &with_comment,
            &[op(json!({
                "op": "replace_block_content", "block_id": "p1", "text": "BBBB middle YY"
            }))],
            &actor(),
            "tx-replace",
        )
        .unwrap();

        let item = &applied.review_items[0];
        assert_eq!(item.state, BlockReviewState::Open);
        assert_eq!((item.start_offset, item.end_offset), (5, 11));
    }

    fn bold() -> Attrs {
        let mut attrs = Attrs::new();
        attrs.insert("bold".to_string(), json!(true));
        attrs
    }

    #[test]
    fn moving_a_block_under_its_own_descendant_is_a_move_conflict() {
        let parent = BlockRow {
            block_id: "outer".to_string(),
            parent_block_id: None,
            position: 0,
            block_type: "code_block".to_string(),
            attrs: Attrs::new(),
            text: String::new(),
            marks: Vec::new(),
            links: Vec::new(),
        };
        let child = BlockRow {
            block_id: "inner".to_string(),
            parent_block_id: Some("outer".to_string()),
            position: 0,
            block_type: "code_line".to_string(),
            attrs: Attrs::new(),
            text: "line".to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        };
        let state = state_with_rows(vec![parent, child]);
        let error = apply_ops(
            &state,
            &[op(json!({
                "op": "move_block",
                "block_id": "outer",
                "parent_block_id": "inner",
                "position": 0
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::BlockMoveConflict);
        assert!(error.code.retryable());
    }

    #[test]
    fn deleting_the_last_block_leaves_an_empty_native_document() {
        let state = state_with_rows(vec![paragraph("p1", 0, "Last")]);
        let result = apply_ops(
            &state,
            &[op(json!({"op":"delete_block", "block_id":"p1"}))],
            &actor(),
            "delete-last",
        )
        .unwrap();
        assert!(result.rows.is_empty());
        assert!(result.review_items.is_empty());
    }

    #[test]
    fn inserting_a_duplicate_block_id_is_an_invalid_transaction() {
        let state = state_with_rows(vec![paragraph("p1", 0, "Text")]);
        let error = apply_ops(
            &state,
            &[op(json!({
                "op": "insert_block",
                "block_id": "p1",
                "position": 1,
                "block_type": "p",
                "text": "again"
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
    }

    #[test]
    fn capability_registry_rejects_flat_content_on_void_blocks() {
        let state = state_with_rows(vec![paragraph("p1", 0, "Text")]);
        let error = apply_ops(
            &state,
            &[op(json!({
                "op": "insert_block",
                "position": 1,
                "block_type": "hr",
                "text": "silently lost"
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap_err();

        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
        assert!(error.message.contains("hr cannot contain inline text"));
    }

    #[test]
    fn capability_registry_rejects_text_edits_on_void_blocks() {
        let mut horizontal_rule = paragraph("hr1", 0, "");
        horizontal_rule.block_type = "hr".to_string();
        let state = state_with_rows(vec![horizontal_rule]);
        let error = apply_ops(
            &state,
            &[op(json!({
                "op": "replace_block_content",
                "block_id": "hr1",
                "text": "silently lost"
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap_err();

        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
        assert!(error.message.contains("hr cannot contain inline text"));
    }

    #[test]
    fn capability_registry_prevents_type_changes_that_would_drop_text() {
        let state = state_with_rows(vec![paragraph("p1", 0, "Keep this")]);
        let error = apply_ops(
            &state,
            &[op(json!({
                "op": "set_block_type",
                "block_id": "p1",
                "block_type": "hr"
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap_err();

        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
        assert!(error.message.contains("hr cannot contain inline text"));
    }

    #[test]
    fn replace_block_content_keeps_marks_outside_the_change_and_shifts_suffix_marks() {
        let mut row = paragraph("p1", 0, "bold plain tail");
        row.marks = vec![
            MarkRun {
                start: 0,
                end: 4,
                marks: bold(),
            },
            MarkRun {
                start: 11,
                end: 15,
                marks: bold(),
            },
        ];
        let state = state_with_rows(vec![row]);
        let applied = apply_ops(
            &state,
            &[op(json!({
                "op": "replace_block_content",
                "block_id": "p1",
                "text": "bold replaced-middle tail"
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap();
        let shape: Vec<(u32, u32)> = applied.rows[0]
            .marks
            .iter()
            .map(|run| (run.start, run.end))
            .collect();
        assert_eq!(shape, vec![(0, 4), (21, 25)]);
    }

    #[test]
    fn unknown_op_kind_is_an_invalid_transaction() {
        let error = parse_transaction(json!({
            "client_tx_id": "tx-1",
            "base_clock": "v1",
            "actor": {"kind": "agent"},
            "ops": [{"op": "explode_block", "block_id": "p1"}]
        }))
        .unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
    }

    #[test]
    fn empty_ops_array_is_an_invalid_transaction() {
        let error = parse_transaction(json!({
            "client_tx_id": "tx-1",
            "base_clock": "v1",
            "actor": {"kind": "agent"},
            "ops": []
        }))
        .unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
    }

    #[test]
    fn clock_tokens_unquote_etag_shapes() {
        assert_eq!(unquote_clock("\"v1\""), Some("v1".to_string()));
        assert_eq!(unquote_clock("W/\"v1\""), Some("v1".to_string()));
        assert_eq!(unquote_clock("v1"), Some("v1".to_string()));
        assert_eq!(unquote_clock("\"unbalanced"), None);
        assert_eq!(unquote_clock(""), None);
    }

    #[test]
    fn set_block_attrs_on_raw_markdown_requires_the_markdown_attribute() {
        let mut raw = paragraph("raw", 0, "");
        raw.block_type = "raw_markdown".to_string();
        raw.attrs
            .insert("markdown".to_string(), json!("<div>x</div>"));
        let state = state_with_rows(vec![raw]);
        let error = apply_ops(
            &state,
            &[op(json!({
                "op": "set_block_attrs",
                "block_id": "raw",
                "attrs": {"note": "markdown key missing"}
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
    }

    #[test]
    fn insert_block_raw_markdown_requires_the_markdown_attribute() {
        let state = state_with_rows(vec![paragraph("p1", 0, "Text")]);
        let error = apply_ops(
            &state,
            &[op(json!({
                "op": "insert_block",
                "position": 1,
                "block_type": "raw_markdown",
                "attrs": {"markdown": ""}
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap_err();
        assert_eq!(error.code, GatewayErrorCode::InvalidTransaction);
    }

    #[test]
    fn insert_block_raw_markdown_with_markdown_attr_commits() {
        let state = state_with_rows(vec![paragraph("p1", 0, "Text")]);
        let applied = apply_ops(
            &state,
            &[op(json!({
                "op": "insert_block",
                "position": 1,
                "block_type": "raw_markdown",
                "attrs": {"markdown": "<div>kept</div>"}
            }))],
            &actor(),
            "test-tx",
        )
        .unwrap();
        assert_eq!(applied.rows[1].block_type, "raw_markdown");
        assert_eq!(applied.rows[1].attrs["markdown"], json!("<div>kept</div>"));
    }
}
