//! Semantic mutation gateway — Phases 2 and 3 of the session-scoped
//! collaboration rewrite: a mode-independent apply engine with a
//! per-document mode switch (rows-authoritative vs session-authoritative
//! dispatch; see `apply_rows_transaction` / `apply_session_transaction` and
//! the `session` module).
//!
//! `POST /v1/libraries/{library}/documents/{path}/transactions` is the public
//! mutation contract for agents (and later Git/FUSE/CLI via Phase 4's
//! reconciler). A transaction is an envelope
//! `{client_tx_id, base_clock?, actor{kind,id,label}, ops[]}`; ops are
//! validated and applied to the canonical block rows in memory, then committed
//! atomically as ONE new document version, ONE legacy history row, and ONE
//! `block_transactions` history record (see
//! [`quarry_storage::BlockMutationCommit`]). The normalized Markdown export
//! is published through the existing `document_versions` path in the same SQL
//! transaction, so legacy readers, links, and the document event stream keep
//! working; the same `doc.changed` events as other writes fire after commit.
//!
//! ## Clock semantics
//!
//! The document clock is the head `document_versions` id. ETag-shaped tokens
//! (`"…"`, `W/"…"`) are tolerated by unquoting. A missing or head-matching
//! `base_clock` acks `committed`; a clock naming any OLDER version of the
//! document applies against CURRENT rows (every referenced block/anchor must
//! still validate) and acks `committed_rebased`; an unknown or garbage clock
//! fails with retryable `STALE_BASE`. There are no generic 409s.
//!
//! ## Typed errors
//!
//! Failures return `{code, retryable, message}`:
//!
//! | code | status | retryable |
//! |------|--------|-----------|
//! | `STALE_BASE` | 412 | yes |
//! | `BLOCK_MOVE_CONFLICT` | 412 | yes |
//! | `BLOCK_DELETED` | 404 | no |
//! | `ANCHOR_NOT_FOUND` | 404 | no |
//! | `SUGGESTION_INVALIDATED` | 422 | no |
//! | `SUGGESTION_ALREADY_RESOLVED` | 422 | no |
//! | `UNSUPPORTED_MARKDOWN` | 422 | no |
//! | `UNSUPPORTED_BLOCK_DOCUMENT` | 422 | no |
//! | `INVALID_TRANSACTION` | 400 | no |
//!
//! `retryable: true` means "refetch `/blocks` and resubmit with a fresh
//! clock"; `retryable: false` means the op as stated can never succeed.
//!
//! ## Vocabulary decisions (binding, from the Phase 0 findings)
//!
//! - `replace_block_content` computes the minimal common-prefix/suffix UTF-16
//!   diff between old and new text. Review anchors entirely inside the
//!   preserved prefix keep their offsets; anchors in the preserved suffix
//!   shift by the length delta; anchors overlapping the changed middle die —
//!   open comments orphan, open suggestions invalidate, and dead anchors
//!   collapse to `start == end` at the change site (Gate A rule). A pure
//!   insertion at an anchor's start boundary is excluded from the anchor
//!   (never grows leftward); at its end boundary it is also excluded (never
//!   grows rightward); strictly interior inserts grow the anchor. Mark/link
//!   ranges adjust the same way except overlap clamps to the preserved
//!   portions instead of dying (an interior insert grows a formatting run —
//!   the Gate A formatting-inheritance rule).
//! - `set_block_type` changes `block_type` while preserving `block_id`, text,
//!   marks, links, children, and anchors (design delta 3). If `attrs` is
//!   provided it replaces the block's attrs wholesale (the caller normalizes
//!   them for the new type); otherwise attrs are kept unchanged.
//! - `raw_markdown` blocks carry their source in `attrs.markdown` and have no
//!   flat text, so text/mark/link/anchor ops against them — and
//!   `set_block_type` to or from `raw_markdown` — are `INVALID_TRANSACTION`.
//!   Edit raw blocks with `set_block_attrs` or replace them wholesale;
//!   `insert_block` and `set_block_attrs` require raw_markdown attrs to
//!   carry a non-empty string `markdown` key (attrs replace wholesale, so a
//!   missing key would silently erase the block's content).
//! - `set_link` replaces every link range that intersects `[start, end)`;
//!   `url: null` just removes them. Partial overlaps are not trimmed.
//! - `comment.edit` updates the body and `updated_at` of an open comment root
//!   or reply. It never rewrites anchors, quotes, authors, creation
//!   timestamps, replies, or document text. Suggestion/conflict ids are
//!   `ANCHOR_NOT_FOUND`; non-open comments are `INVALID_TRANSACTION`.
//! - `suggestion.accept` applies the stored replacement to the anchored range
//!   through the same minimal-diff rules, resolves the suggestion, and
//!   re-anchors it on the replacement text. `suggestion.reject` resolves
//!   without changing text; rejecting an invalidated/orphaned suggestion is
//!   allowed (it dismisses the dead item), while accepting one fails with
//!   `SUGGESTION_INVALIDATED`.
//! - `conflict.add` (Phase 4) persists a diff3 conflict artifact as a
//!   `kind = conflict` review item without mutating the document: the
//!   attachment point rides in the item's `block_id` (`""` = document start,
//!   from `after_block_id`), the losing incoming hunk in `body`, the base
//!   context in `context_before`, and the retained canonical side in
//!   `quote`. Conflict items resolve and delete with `comment.resolve` /
//!   `comment.delete` (resolution never mutates the document); replies stay
//!   comment-only. Deleting a conflict's attachment block orphans the item
//!   like any other anchored item.
//! - Deleting the last block re-mints the canonical empty-paragraph row (the
//!   editor's empty-document shape); its id is listed in `changed_block_ids`.
//! - Ops apply sequentially: each op's offsets, positions, and block
//!   references are interpreted against the document as left by the previous
//!   ops in the same transaction, not against the pre-transaction state.
//! - `changed_block_ids` lists every block an op directly targeted: content,
//!   attrs, or type changes, the moved block of `move_block`, every deleted
//!   block (including descendants), inserted blocks, and the block rewritten
//!   by `suggestion.accept`. Siblings displaced by an insert/move/delete
//!   (position renumbering) are NOT listed; review-metadata-only ops touch
//!   no blocks. The list is sorted and deduplicated.
//!
//! ## Idempotency
//!
//! `client_tx_id` is unique per document (`block_transactions`). A duplicate
//! returns the ORIGINAL ack without re-applying — the ack's status and
//! `changed_block_ids` are stored alongside the request ops in the history
//! record's `ops` JSON (`{ops, actor, ack}`). Request bodies are not hashed:
//! a reused `client_tx_id` with different ops still replays the original ack.
//!
//! ## Reads and the review projection
//!
//! `GET …/{path}/blocks` returns the canonical rows plus the current clock.
//! For a BlockDocument whose projection is missing (legacy write cleared it,
//! or it was never imported), the read materializes rows from the head
//! Markdown via the Phase 1 import path — publishing the one-time normalized
//! version — so the returned `block_id`s are durable and addressable.
//! `POST /transactions` against a projection-less document materializes rows
//! in memory and persists them with the ops as one version.
//!
//! `GET …/{path}/review` projects from `block_review_items` whenever the
//! document has block rows, preserving the legacy response shape: `ref` holds
//! the anchored block's depth-first ordinal (0 when the block is gone),
//! `contentHash` is omitted, and each item additionally carries
//! `anchor: {blockId, startOffset, endOffset}`. Resolved items are filtered
//! unless `includeResolved`; orphaned and invalidated items always show.
//! Comments and replies include `editedAt` when `updated_at != created_at`,
//! otherwise `null`.
//! `conflict`-kind rows (Phase 4) project as `conflicts[]` with
//! `afterBlockId` (`null` = document start) and the base/incoming/canonical
//! Markdown payloads. Documents without rows keep the legacy
//! CriticMarkup/endmatter projection untouched (`conflicts` empty).
//!
//! ## Whole-file writes (Phase 4)
//!
//! Markdown PUTs, Git sync/import, FUSE flushes, and CLI puts reconcile via
//! diff3 against the canonical rows and dispatch through
//! [`execute_block_transaction`] — see the `markdown_write` module. They are
//! ordinary transactions here: rows-mode or session-mode per the switch,
//! identity-preserving, conflicts as `conflict.add` ops.
//!
//! ## Known limitation (recorded in the README limitations)
//!
//! Session-mode commit failure can land merged content without its
//! review-item side effects when the caller ignores the typed retryable
//! error — see the HONEST WINDOW note in [`apply_session_transaction`].

use crate::{
    json_response, json_with_etag, AgentBlockRef, AgentReviewComment, AgentReviewReply,
    AgentReviewResponse, AgentReviewSuggestion, AgentSuggestionKind, AgentSuggestionPreview,
    ApiError, AppState,
};
use axum::http::StatusCode;
use axum::response::Response;
use quarry_collab_codec::{
    block_rows_to_markdown, is_utf16_boundary, utf16_len, Attrs, BlockRow, LinkRange, MarkRun,
    KNOWN_BLOCK_TYPES,
};
use quarry_core::{
    now_timestamp, render_markdown_frontmatter, DocumentSource, QuarryError, WritePrecondition,
};
use quarry_storage::{
    document_kind, BlockMutationCommit, BlockMutationOutcome, BlockMutationState, BlockReviewItem,
    BlockReviewKind, BlockReviewState, BlockTransactionRecord, DocumentKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::{BTreeSet, HashMap};
use utoipa::ToSchema;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Typed errors
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GatewayErrorCode {
    StaleBase,
    BlockDeleted,
    AnchorNotFound,
    BlockMoveConflict,
    SuggestionInvalidated,
    SuggestionAlreadyResolved,
    UnsupportedMarkdown,
    InvalidTransaction,
    UnknownBlockType,
    UnsupportedBlockDocument,
}

impl GatewayErrorCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StaleBase => "STALE_BASE",
            Self::BlockDeleted => "BLOCK_DELETED",
            Self::AnchorNotFound => "ANCHOR_NOT_FOUND",
            Self::BlockMoveConflict => "BLOCK_MOVE_CONFLICT",
            Self::SuggestionInvalidated => "SUGGESTION_INVALIDATED",
            Self::SuggestionAlreadyResolved => "SUGGESTION_ALREADY_RESOLVED",
            Self::UnsupportedMarkdown => "UNSUPPORTED_MARKDOWN",
            Self::InvalidTransaction => "INVALID_TRANSACTION",
            Self::UnknownBlockType => "UNKNOWN_BLOCK_TYPE",
            Self::UnsupportedBlockDocument => "UNSUPPORTED_BLOCK_DOCUMENT",
        }
    }

    fn retryable(self) -> bool {
        matches!(self, Self::StaleBase | Self::BlockMoveConflict)
    }

    fn status(self) -> StatusCode {
        match self {
            Self::StaleBase | Self::BlockMoveConflict => StatusCode::PRECONDITION_FAILED,
            Self::BlockDeleted | Self::AnchorNotFound => StatusCode::NOT_FOUND,
            Self::InvalidTransaction | Self::UnknownBlockType => StatusCode::BAD_REQUEST,
            Self::SuggestionInvalidated
            | Self::SuggestionAlreadyResolved
            | Self::UnsupportedMarkdown
            | Self::UnsupportedBlockDocument => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GatewayError {
    code: GatewayErrorCode,
    message: String,
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
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(GatewayErrorCode::InvalidTransaction, message)
    }

    fn block_deleted(block_id: &str) -> Self {
        Self::new(
            GatewayErrorCode::BlockDeleted,
            format!("block {block_id} does not exist in this document"),
        )
    }

    fn into_response(self) -> Response {
        let status = self.code.status();
        let payload = BlockTransactionError {
            code: self.code.as_str().to_string(),
            retryable: self.code.retryable(),
            message: self.message,
        };
        json_response(status, &payload).unwrap_or_else(|error| {
            // Serializing three plain fields cannot fail; keep the ApiError
            // fallback rather than panicking in a response path.
            axum::response::IntoResponse::into_response(error)
        })
    }
}

/// A gateway call fails either with a typed `{code, retryable, message}`
/// payload or with an ordinary [`ApiError`] (not found, busy, internal).
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
            other => Self::Api(other.into()),
        }
    }
}

pub(crate) fn gateway_reply(
    result: Result<Response, GatewayFailure>,
) -> Result<Response, ApiError> {
    match result {
        Ok(response) => Ok(response),
        Err(GatewayFailure::Typed(error)) => Ok(error.into_response()),
        Err(GatewayFailure::Api(error)) => Err(error),
    }
}

// ---------------------------------------------------------------------------
// Wire payloads
// ---------------------------------------------------------------------------

/// Typed error payload returned by the gateway routes.
#[derive(Debug, Serialize, ToSchema)]
pub struct BlockTransactionError {
    pub code: String,
    pub retryable: bool,
    pub message: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionRequest {
    pub client_tx_id: String,
    #[serde(default)]
    pub base_clock: Option<String>,
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
    /// `committed` (clock matched the head or was omitted) or
    /// `committed_rebased` (stale-but-valid clock; ops validated against the
    /// current rows).
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
}

#[derive(Debug)]
struct ParsedTransaction {
    client_tx_id: String,
    base_clock: Option<String>,
    actor: BlockTransactionActor,
    ops: Vec<BlockOp>,
    ops_json: JsonValue,
}

fn parse_transaction(payload: JsonValue) -> Result<ParsedTransaction, GatewayError> {
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
        let message = match marks_hint(op_name, op, &error.to_string()) {
            Some(hint) => format!("{error}; {hint}"),
            None => error.to_string(),
        };
        GatewayError::invalid(format!(
            "invalid op at index {index} ({op_name}): {message}"
        ))
    })
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
// Minimal text diff and range adjustment
// ---------------------------------------------------------------------------

/// The minimal common-prefix/suffix diff between two texts, in UTF-16 code
/// units. The changed span is `[prefix, old_mid_end)` in the old text and
/// `[prefix, new_mid_end)` in the new text; a suffix offset `o` maps to
/// `o - old_mid_end + new_mid_end`.
#[derive(Clone, Copy, Debug)]
struct TextDiff {
    prefix: u32,
    old_mid_end: u32,
    new_mid_end: u32,
}

impl TextDiff {
    fn is_pure_insertion(&self) -> bool {
        self.prefix == self.old_mid_end
    }

    fn shift_suffix(&self, offset: u32) -> u32 {
        offset - self.old_mid_end + self.new_mid_end
    }
}

fn utf16_text_diff(old: &str, new: &str) -> TextDiff {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let max_common = old_chars.len().min(new_chars.len());
    let mut prefix_chars = 0;
    while prefix_chars < max_common && old_chars[prefix_chars] == new_chars[prefix_chars] {
        prefix_chars += 1;
    }
    let mut suffix_chars = 0;
    while suffix_chars < max_common - prefix_chars
        && old_chars[old_chars.len() - 1 - suffix_chars]
            == new_chars[new_chars.len() - 1 - suffix_chars]
    {
        suffix_chars += 1;
    }
    let units = |chars: &[char]| chars.iter().map(|ch| ch.len_utf16() as u32).sum::<u32>();
    let prefix = units(&old_chars[..prefix_chars]);
    let old_suffix = units(&old_chars[old_chars.len() - suffix_chars..]);
    let new_suffix = units(&new_chars[new_chars.len() - suffix_chars..]);
    TextDiff {
        prefix,
        old_mid_end: utf16_len(old) - old_suffix,
        new_mid_end: utf16_len(new) - new_suffix,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnchorFate {
    Keep(u32, u32),
    /// The anchor overlapped the changed middle: it collapses to the change
    /// site; open comments orphan, open suggestions invalidate.
    Dead(u32),
}

fn adjust_anchor(diff: TextDiff, start: u32, end: u32) -> AnchorFate {
    // A collapsed anchor (start == end, a dead orphaned/invalidated/resolved
    // marker) is a single point: both ends move together so they can never
    // cross. Without this, a pure insertion exactly at the point would shift
    // the start (start-boundary inserts are excluded) but keep the end
    // (end-boundary inserts are excluded too), inverting the range.
    if start == end {
        let point = if end <= diff.prefix {
            end
        } else if start >= diff.old_mid_end {
            diff.shift_suffix(start)
        } else {
            diff.prefix
        };
        return AnchorFate::Keep(point, point);
    }
    if diff.is_pure_insertion() {
        // Inserts exactly at the start boundary are excluded (the anchor
        // never grows leftward); at the end boundary likewise. Interior
        // inserts grow the anchor.
        let start = if start < diff.prefix {
            start
        } else {
            diff.shift_suffix(start)
        };
        let end = if end <= diff.prefix {
            end
        } else {
            diff.shift_suffix(end)
        };
        return AnchorFate::Keep(start, end);
    }
    if end <= diff.prefix {
        AnchorFate::Keep(start, end)
    } else if start >= diff.old_mid_end {
        AnchorFate::Keep(diff.shift_suffix(start), diff.shift_suffix(end))
    } else {
        AnchorFate::Dead(diff.prefix)
    }
}

/// Mark/link ranges clamp to the preserved prefix/suffix instead of dying;
/// `None` means the range vanished entirely inside the changed middle.
fn adjust_range(diff: TextDiff, start: u32, end: u32) -> Option<(u32, u32)> {
    if diff.is_pure_insertion() {
        return match adjust_anchor(diff, start, end) {
            AnchorFate::Keep(start, end) => Some((start, end)),
            AnchorFate::Dead(_) => unreachable!("pure insertions never kill ranges"),
        };
    }
    let new_start = if start <= diff.prefix {
        start
    } else if start >= diff.old_mid_end {
        diff.shift_suffix(start)
    } else {
        diff.new_mid_end
    };
    let new_end = if end <= diff.prefix {
        end
    } else if end >= diff.old_mid_end {
        diff.shift_suffix(end)
    } else {
        diff.prefix
    };
    (new_start < new_end).then_some((new_start, new_end))
}

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

#[derive(Clone, Debug)]
struct ModelBlock {
    parent: Option<String>,
    block_type: String,
    attrs: Attrs,
    text: String,
    marks: Vec<MarkRun>,
    links: Vec<LinkRange>,
}

#[derive(Clone, Debug, Default)]
struct DocModel {
    blocks: HashMap<String, ModelBlock>,
    children: HashMap<Option<String>, Vec<String>>,
}

impl DocModel {
    fn from_rows(rows: &[BlockRow]) -> Self {
        let mut model = Self::default();
        // `load_block_tree` returns depth-first order with siblings already
        // position-sorted, so pushing in encounter order preserves sibling
        // order under every parent.
        for row in rows {
            model.blocks.insert(
                row.block_id.clone(),
                ModelBlock {
                    parent: row.parent_block_id.clone(),
                    block_type: row.block_type.clone(),
                    attrs: row.attrs.clone(),
                    text: row.text.clone(),
                    marks: row.marks.clone(),
                    links: row.links.clone(),
                },
            );
            model
                .children
                .entry(row.parent_block_id.clone())
                .or_default()
                .push(row.block_id.clone());
        }
        model
    }

    fn to_rows(&self) -> Vec<BlockRow> {
        let mut rows = Vec::with_capacity(self.blocks.len());
        self.collect_rows(None, &mut rows);
        rows
    }

    fn collect_rows(&self, parent: Option<&str>, out: &mut Vec<BlockRow>) {
        let Some(children) = self.children.get(&parent.map(str::to_string)) else {
            return;
        };
        for (position, block_id) in children.iter().enumerate() {
            let block = &self.blocks[block_id];
            out.push(BlockRow {
                block_id: block_id.clone(),
                parent_block_id: parent.map(str::to_string),
                position: position as u32,
                block_type: block.block_type.clone(),
                attrs: block.attrs.clone(),
                text: block.text.clone(),
                marks: block.marks.clone(),
                links: block.links.clone(),
            });
            self.collect_rows(Some(block_id), out);
        }
    }

    fn has_children(&self, block_id: &str) -> bool {
        self.children
            .get(&Some(block_id.to_string()))
            .is_some_and(|children| !children.is_empty())
    }

    fn is_or_descends_from(&self, root: &str, candidate: &str) -> bool {
        let mut current = Some(candidate.to_string());
        while let Some(id) = current {
            if id == root {
                return true;
            }
            current = self.blocks.get(&id).and_then(|block| block.parent.clone());
        }
        false
    }

    fn detach(&mut self, block_id: &str) {
        let parent = self.blocks[block_id].parent.clone();
        if let Some(siblings) = self.children.get_mut(&parent) {
            siblings.retain(|id| id != block_id);
        }
    }

    fn attach(&mut self, block_id: &str, parent: Option<String>, position: u32) {
        let siblings = self.children.entry(parent.clone()).or_default();
        let index = (position as usize).min(siblings.len());
        siblings.insert(index, block_id.to_string());
        if let Some(block) = self.blocks.get_mut(block_id) {
            block.parent = parent;
        }
    }

    /// Removes a block and its whole subtree, returning every removed id in
    /// depth-first order.
    fn remove_subtree(&mut self, block_id: &str) -> Vec<String> {
        self.detach(block_id);
        let mut removed = Vec::new();
        let mut stack = vec![block_id.to_string()];
        while let Some(id) = stack.pop() {
            if let Some(children) = self.children.remove(&Some(id.clone())) {
                stack.extend(children);
            }
            self.blocks.remove(&id);
            removed.push(id);
        }
        removed
    }
}

// ---------------------------------------------------------------------------
// Op application
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ApplyResult {
    rows: Vec<BlockRow>,
    review_items: Vec<BlockReviewItem>,
    changed_block_ids: Vec<String>,
}

struct ApplyContext {
    model: DocModel,
    items: Vec<BlockReviewItem>,
    changed: BTreeSet<String>,
    document_id: String,
    author: String,
    now: String,
    minted: DeterministicIds,
}

/// Ids the apply engine mints (inserted blocks without a caller id, review
/// items, the re-minted empty paragraph) derive deterministically from the
/// transaction's `client_tx_id` plus a counter: re-running the SAME
/// transaction's ops mints the SAME ids. This makes a session-mode retry
/// after a failed commit unable to silently duplicate inserted content —
/// the deterministic id collides with the first application and the op
/// fails with a typed error instead.
struct DeterministicIds {
    seed: String,
    next: u32,
}

impl DeterministicIds {
    fn new(client_tx_id: &str) -> Self {
        Self {
            seed: client_tx_id.to_string(),
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

fn apply_ops(
    state: &BlockMutationState,
    ops: &[BlockOp],
    actor: &BlockTransactionActor,
    client_tx_id: &str,
) -> Result<ApplyResult, GatewayError> {
    let mut ctx = ApplyContext {
        model: DocModel::from_rows(&state.rows),
        items: state.review_items.clone(),
        changed: BTreeSet::new(),
        document_id: state.document_id.clone(),
        author: actor.display(),
        now: now_timestamp(),
        minted: DeterministicIds::new(client_tx_id),
    };
    for op in ops {
        apply_op(&mut ctx, op)?;
    }
    // Deleting the last block leaves the canonical empty-document shape: one
    // empty paragraph row (Phase 1's "zero rows means no projection" rule).
    if ctx
        .model
        .children
        .get(&None)
        .is_none_or(|top| top.is_empty())
    {
        let block_id = ctx.minted.mint();
        ctx.model.blocks.insert(
            block_id.clone(),
            ModelBlock {
                parent: None,
                block_type: "p".to_string(),
                attrs: Attrs::new(),
                text: String::new(),
                marks: Vec::new(),
                links: Vec::new(),
            },
        );
        ctx.model.attach(&block_id, None, 0);
        ctx.changed.insert(block_id);
    }
    Ok(ApplyResult {
        rows: ctx.model.to_rows(),
        review_items: ctx.items,
        changed_block_ids: ctx.changed.into_iter().collect(),
    })
}

fn apply_op(ctx: &mut ApplyContext, op: &BlockOp) -> Result<(), GatewayError> {
    match op {
        BlockOp::InsertBlock {
            block_id,
            parent_block_id,
            position,
            block_type,
            attrs,
            text,
            marks,
            links,
        } => {
            let block_id = match block_id {
                Some(id) if id.trim().is_empty() => {
                    return Err(GatewayError::invalid("block_id must not be empty"));
                }
                Some(id) => id.clone(),
                None => ctx.minted.mint(),
            };
            // Caller-supplied AND minted ids collide here: minted ids are
            // deterministic per (client_tx_id, op), so a retried transaction
            // whose first application already reached the doc fails typed
            // instead of silently duplicating the inserted block.
            if ctx.model.blocks.contains_key(&block_id) {
                return Err(GatewayError::invalid(format!(
                    "block {block_id} already exists in this document"
                )));
            }
            if let Some(parent) = parent_block_id {
                if !ctx.model.blocks.contains_key(parent) {
                    return Err(GatewayError::block_deleted(parent));
                }
            }
            validate_block_type(block_type)?;
            validate_attrs(attrs)?;
            let attrs = normalize_list_attrs(attrs)?;
            if block_type == "raw_markdown" {
                if !text.is_empty() || !marks.is_empty() || !links.is_empty() {
                    return Err(GatewayError::invalid(
                        "raw_markdown blocks carry no flat text, marks, or links",
                    ));
                }
                validate_raw_markdown_attrs(&attrs)?;
            }
            validate_inline_ranges(text, marks, links)?;
            ctx.model.blocks.insert(
                block_id.clone(),
                ModelBlock {
                    parent: parent_block_id.clone(),
                    block_type: block_type.clone(),
                    attrs,
                    text: text.clone(),
                    marks: marks.clone(),
                    links: links.clone(),
                },
            );
            ctx.model
                .attach(&block_id, parent_block_id.clone(), *position);
            ctx.changed.insert(block_id);
            Ok(())
        }
        BlockOp::DeleteBlock { block_id } => {
            if !ctx.model.blocks.contains_key(block_id) {
                return Err(GatewayError::block_deleted(block_id));
            }
            let removed = ctx.model.remove_subtree(block_id);
            for item in &mut ctx.items {
                if removed.contains(&item.block_id) && item.state == BlockReviewState::Open {
                    item.state = match item.kind {
                        BlockReviewKind::Suggestion => BlockReviewState::Invalidated,
                        _ => BlockReviewState::Orphaned,
                    };
                    item.updated_at = ctx.now.clone();
                }
            }
            ctx.changed.extend(removed);
            Ok(())
        }
        BlockOp::MoveBlock {
            block_id,
            parent_block_id,
            position,
        } => {
            if !ctx.model.blocks.contains_key(block_id) {
                return Err(GatewayError::block_deleted(block_id));
            }
            if let Some(parent) = parent_block_id {
                if !ctx.model.blocks.contains_key(parent) {
                    return Err(GatewayError::new(
                        GatewayErrorCode::BlockMoveConflict,
                        format!("move target parent {parent} does not exist"),
                    ));
                }
                if ctx.model.is_or_descends_from(block_id, parent) {
                    return Err(GatewayError::new(
                        GatewayErrorCode::BlockMoveConflict,
                        format!("moving {block_id} under {parent} would create a cycle"),
                    ));
                }
            }
            ctx.model.detach(block_id);
            ctx.model
                .attach(block_id, parent_block_id.clone(), *position);
            ctx.changed.insert(block_id.clone());
            Ok(())
        }
        BlockOp::ReplaceBlockContent {
            block_id,
            text,
            marks,
            links,
        } => {
            replace_block_text(ctx, block_id, text.clone(), None)?;
            let block = ctx
                .model
                .blocks
                .get_mut(block_id)
                .expect("replace_block_text verified existence");
            if let Some(marks) = marks {
                validate_inline_ranges(text, marks, &[])?;
                block.marks = marks.clone();
            }
            if let Some(links) = links {
                validate_inline_ranges(text, &[], links)?;
                block.links = links.clone();
            }
            Ok(())
        }
        BlockOp::SetBlockAttrs { block_id, attrs } => {
            validate_attrs(attrs)?;
            let attrs = normalize_list_attrs(attrs)?;
            let block = require_block_mut(&mut ctx.model, block_id)?;
            if block.block_type == "raw_markdown" {
                // Attrs replace wholesale: dropping or blanking the markdown
                // attribute would silently erase the block's content.
                validate_raw_markdown_attrs(&attrs)?;
            }
            block.attrs = attrs;
            ctx.changed.insert(block_id.clone());
            Ok(())
        }
        BlockOp::SetBlockType {
            block_id,
            block_type,
            attrs,
        } => {
            validate_block_type(block_type)?;
            let attrs = attrs
                .as_ref()
                .map(|attrs| {
                    validate_attrs(attrs)?;
                    normalize_list_attrs(attrs)
                })
                .transpose()?;
            let block = require_block_mut(&mut ctx.model, block_id)?;
            if block.block_type == "raw_markdown" || block_type == "raw_markdown" {
                return Err(GatewayError::invalid(
                    "set_block_type cannot convert to or from raw_markdown; \
                     replace the block instead",
                ));
            }
            block.block_type = block_type.clone();
            if let Some(attrs) = attrs {
                block.attrs = attrs;
            }
            ctx.changed.insert(block_id.clone());
            Ok(())
        }
        BlockOp::AddMark {
            block_id,
            start,
            end,
            marks,
        } => {
            if marks.is_empty() {
                return Err(GatewayError::invalid("add_mark requires at least one mark"));
            }
            validate_attrs(marks)?;
            let block = require_inline_block_mut(ctx, block_id)?;
            validate_span(&block.text, *start, *end)?;
            block.marks = rewrite_marks(&block.marks, &block.text, *start, *end, |attrs| {
                for (key, value) in marks {
                    attrs.insert(key.clone(), value.clone());
                }
            });
            ctx.changed.insert(block_id.clone());
            Ok(())
        }
        BlockOp::RemoveMark {
            block_id,
            start,
            end,
            marks,
        } => {
            if marks.is_empty() {
                return Err(GatewayError::invalid(
                    "remove_mark requires at least one mark key",
                ));
            }
            let block = require_inline_block_mut(ctx, block_id)?;
            validate_span(&block.text, *start, *end)?;
            block.marks = rewrite_marks(&block.marks, &block.text, *start, *end, |attrs| {
                for key in marks {
                    attrs.shift_remove(key);
                }
            });
            ctx.changed.insert(block_id.clone());
            Ok(())
        }
        BlockOp::SetLink {
            block_id,
            start,
            end,
            url,
        } => {
            let block = require_inline_block_mut(ctx, block_id)?;
            validate_span(&block.text, *start, *end)?;
            block
                .links
                .retain(|link| link.end <= *start || link.start >= *end);
            if let Some(url) = url {
                block.links.push(LinkRange {
                    start: *start,
                    end: *end,
                    url: url.clone(),
                });
                block.links.sort_by_key(|link| link.start);
            }
            ctx.changed.insert(block_id.clone());
            Ok(())
        }
        BlockOp::CommentAdd {
            block_id,
            start,
            end,
            body,
            quote,
        } => {
            add_review_item(
                ctx,
                block_id,
                *start,
                *end,
                BlockReviewKind::Comment,
                Some(body.clone()),
                None,
                quote.clone(),
                None,
            )?;
            Ok(())
        }
        BlockOp::CommentReply { item_id, body } => {
            let target = require_reply_target(ctx, item_id)?;
            let root_id = target.parent_item_id.clone().unwrap_or(target.id.clone());
            let root = ctx
                .items
                .iter()
                .find(|item| item.id == root_id)
                .ok_or_else(|| anchor_not_found(&root_id))?
                .clone();
            require_open_reply_root(&root, &root_id)?;
            let reply_id = ctx.minted.mint();
            require_unused_item_id(ctx, &reply_id)?;
            ctx.items.push(BlockReviewItem {
                id: reply_id,
                document_id: ctx.document_id.clone(),
                block_id: root.block_id,
                kind: BlockReviewKind::Comment,
                start_offset: root.start_offset,
                end_offset: root.end_offset,
                body: Some(body.clone()),
                replacement: None,
                author: Some(ctx.author.clone()),
                state: root.state,
                quote: root.quote,
                context_before: None,
                context_after: None,
                parent_item_id: Some(root_id),
                created_at: ctx.now.clone(),
                updated_at: ctx.now.clone(),
            });
            Ok(())
        }
        BlockOp::CommentEdit { item_id, body } => {
            let target = require_open_comment_for_edit(ctx, item_id)?;
            let target_id = target.id.clone();
            for item in &mut ctx.items {
                if item.id == target_id {
                    item.body = Some(body.clone());
                    item.updated_at = ctx.now.clone();
                    break;
                }
            }
            Ok(())
        }
        BlockOp::CommentResolve { item_id } => {
            let _ = require_resolvable(ctx, item_id)?;
            let now = ctx.now.clone();
            for item in &mut ctx.items {
                if item.id == *item_id {
                    item.state = BlockReviewState::Resolved;
                    item.updated_at = now.clone();
                }
            }
            Ok(())
        }
        BlockOp::CommentDelete { item_id } => {
            let _ = require_resolvable(ctx, item_id)?;
            ctx.items.retain(|item| {
                item.id != *item_id && item.parent_item_id.as_deref() != Some(item_id)
            });
            Ok(())
        }
        BlockOp::SuggestionAdd {
            block_id,
            start,
            end,
            replacement,
            body,
            quote,
        } => {
            add_review_item(
                ctx,
                block_id,
                *start,
                *end,
                BlockReviewKind::Suggestion,
                body.clone(),
                Some(replacement.clone()),
                quote.clone(),
                None,
            )?;
            Ok(())
        }
        BlockOp::SuggestionAccept { item_id } => {
            let suggestion = require_suggestion(ctx, item_id)?;
            match suggestion.state {
                BlockReviewState::Open => {}
                BlockReviewState::Resolved => {
                    return Err(GatewayError::new(
                        GatewayErrorCode::SuggestionAlreadyResolved,
                        format!("suggestion {item_id} is already resolved"),
                    ));
                }
                BlockReviewState::Invalidated | BlockReviewState::Orphaned => {
                    return Err(suggestion_invalidated(item_id));
                }
            }
            let block_id = suggestion.block_id.clone();
            let replacement = suggestion.replacement.clone().unwrap_or_default();
            let (start, end) = (suggestion.start_offset, suggestion.end_offset);
            let Some(block) = ctx.model.blocks.get(&block_id) else {
                return Err(suggestion_invalidated(item_id));
            };
            let new_text = format!(
                "{}{}{}",
                utf16_slice(&block.text, 0, start),
                replacement,
                utf16_slice(&block.text, end, utf16_len(&block.text)),
            );
            replace_block_text(ctx, &block_id, new_text, Some(item_id))?;
            let replacement_len = utf16_len(&replacement);
            let now = ctx.now.clone();
            for item in &mut ctx.items {
                if item.id == *item_id {
                    item.state = BlockReviewState::Resolved;
                    item.start_offset = start;
                    item.end_offset = start + replacement_len;
                    item.updated_at = now.clone();
                }
            }
            ctx.items
                .retain(|item| item.parent_item_id.as_deref() != Some(item_id));
            Ok(())
        }
        BlockOp::SuggestionReject { item_id } => {
            let suggestion = require_suggestion(ctx, item_id)?;
            if suggestion.state == BlockReviewState::Resolved {
                return Err(GatewayError::new(
                    GatewayErrorCode::SuggestionAlreadyResolved,
                    format!("suggestion {item_id} is already resolved"),
                ));
            }
            let now = ctx.now.clone();
            for item in &mut ctx.items {
                if item.id == *item_id {
                    item.state = BlockReviewState::Resolved;
                    item.updated_at = now.clone();
                }
            }
            ctx.items
                .retain(|item| item.parent_item_id.as_deref() != Some(item_id));
            Ok(())
        }
        BlockOp::ConflictAdd {
            after_block_id,
            base_markdown,
            incoming_markdown,
            canonical_markdown,
        } => {
            if let Some(after) = after_block_id {
                if !ctx.model.blocks.contains_key(after) {
                    return Err(GatewayError::block_deleted(after));
                }
            }
            let id = ctx.minted.mint();
            require_unused_item_id(ctx, &id)?;
            ctx.items.push(BlockReviewItem {
                id,
                document_id: ctx.document_id.clone(),
                // `block_id` holds the attachment point; "" = document start.
                block_id: after_block_id.clone().unwrap_or_default(),
                kind: BlockReviewKind::Conflict,
                start_offset: 0,
                end_offset: 0,
                body: Some(incoming_markdown.clone()),
                replacement: None,
                author: Some(ctx.author.clone()),
                state: BlockReviewState::Open,
                quote: Some(canonical_markdown.clone()),
                context_before: Some(base_markdown.clone()),
                context_after: None,
                parent_item_id: None,
                created_at: ctx.now.clone(),
                updated_at: ctx.now.clone(),
            });
            Ok(())
        }
    }
}

/// Replaces a block's full text via the minimal prefix/suffix diff, adjusting
/// the block's mark/link ranges and every review anchor on the block.
/// `protected_item` (the accepting suggestion) is skipped — its anchor is
/// re-set explicitly by the caller.
fn replace_block_text(
    ctx: &mut ApplyContext,
    block_id: &str,
    new_text: String,
    protected_item: Option<&str>,
) -> Result<(), GatewayError> {
    if ctx.model.has_children(block_id) {
        return Err(GatewayError::invalid(format!(
            "block {block_id} is a container and carries no inline text"
        )));
    }
    {
        let block = require_block_mut(&mut ctx.model, block_id)?;
        if block.block_type == "raw_markdown" {
            return Err(GatewayError::invalid(format!(
                "block {block_id} is raw_markdown and carries no flat text; \
                 use set_block_attrs to edit its markdown attribute"
            )));
        }
    }
    let block = ctx
        .model
        .blocks
        .get_mut(block_id)
        .expect("checked just above");
    let diff = utf16_text_diff(&block.text, &new_text);
    block.text = new_text;
    block.marks = block
        .marks
        .iter()
        .filter_map(|run| {
            adjust_range(diff, run.start, run.end).map(|(start, end)| MarkRun {
                start,
                end,
                marks: run.marks.clone(),
            })
        })
        .collect();
    block.links = block
        .links
        .iter()
        .filter_map(|link| {
            adjust_range(diff, link.start, link.end).map(|(start, end)| LinkRange {
                start,
                end,
                url: link.url.clone(),
            })
        })
        .collect();
    for item in &mut ctx.items {
        if item.block_id != block_id || protected_item == Some(item.id.as_str()) {
            continue;
        }
        match adjust_anchor(diff, item.start_offset, item.end_offset) {
            AnchorFate::Keep(start, end) => {
                item.start_offset = start;
                item.end_offset = end;
            }
            AnchorFate::Dead(at) => {
                item.start_offset = at;
                item.end_offset = at;
                if item.state == BlockReviewState::Open {
                    item.state = match item.kind {
                        BlockReviewKind::Suggestion => BlockReviewState::Invalidated,
                        _ => BlockReviewState::Orphaned,
                    };
                }
                item.updated_at = ctx.now.clone();
            }
        }
    }
    ctx.changed.insert(block_id.to_string());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn add_review_item(
    ctx: &mut ApplyContext,
    block_id: &str,
    start: u32,
    end: u32,
    kind: BlockReviewKind,
    body: Option<String>,
    replacement: Option<String>,
    quote: Option<String>,
    parent_item_id: Option<String>,
) -> Result<String, GatewayError> {
    let Some(block) = ctx.model.blocks.get(block_id) else {
        return Err(GatewayError::block_deleted(block_id));
    };
    if block.block_type == "raw_markdown" || ctx.model.has_children(block_id) {
        return Err(GatewayError::invalid(format!(
            "block {block_id} carries no inline text to anchor a review item"
        )));
    }
    validate_span(&block.text, start, end)?;
    let quote = quote.unwrap_or_else(|| utf16_slice(&block.text, start, end));
    let id = ctx.minted.mint();
    require_unused_item_id(ctx, &id)?;
    ctx.items.push(BlockReviewItem {
        id: id.clone(),
        document_id: ctx.document_id.clone(),
        block_id: block_id.to_string(),
        kind,
        start_offset: start,
        end_offset: end,
        body,
        replacement,
        author: Some(ctx.author.clone()),
        state: BlockReviewState::Open,
        quote: Some(quote),
        context_before: None,
        context_after: None,
        parent_item_id,
        created_at: ctx.now.clone(),
        updated_at: ctx.now.clone(),
    });
    Ok(id)
}

/// Minted review-item ids are deterministic per transaction (see
/// [`DeterministicIds`]); a retried transaction whose first application
/// already reached the doc collides here instead of duplicating the item.
fn require_unused_item_id(ctx: &ApplyContext, id: &str) -> Result<(), GatewayError> {
    if ctx.items.iter().any(|item| item.id == id) {
        return Err(GatewayError::invalid(format!(
            "review item {id} already exists in this document"
        )));
    }
    Ok(())
}

fn require_block_mut<'a>(
    model: &'a mut DocModel,
    block_id: &str,
) -> Result<&'a mut ModelBlock, GatewayError> {
    model
        .blocks
        .get_mut(block_id)
        .ok_or_else(|| GatewayError::block_deleted(block_id))
}

/// A block that can carry inline content: exists, is not a container, and is
/// not `raw_markdown`.
fn require_inline_block_mut<'a>(
    ctx: &'a mut ApplyContext,
    block_id: &str,
) -> Result<&'a mut ModelBlock, GatewayError> {
    if ctx.model.has_children(block_id) {
        return Err(GatewayError::invalid(format!(
            "block {block_id} is a container and carries no inline text"
        )));
    }
    let block = require_block_mut(&mut ctx.model, block_id)?;
    if block.block_type == "raw_markdown" {
        return Err(GatewayError::invalid(format!(
            "block {block_id} is raw_markdown and carries no flat text"
        )));
    }
    Ok(block)
}

fn require_comment<'a>(
    ctx: &'a ApplyContext,
    item_id: &str,
) -> Result<&'a BlockReviewItem, GatewayError> {
    ctx.items
        .iter()
        .find(|item| item.id == item_id && item.kind == BlockReviewKind::Comment)
        .ok_or_else(|| anchor_not_found(item_id))
}

fn require_reply_target<'a>(
    ctx: &'a ApplyContext,
    item_id: &str,
) -> Result<&'a BlockReviewItem, GatewayError> {
    let item = ctx
        .items
        .iter()
        .find(|item| {
            item.id == item_id
                && matches!(
                    item.kind,
                    BlockReviewKind::Comment | BlockReviewKind::Suggestion
                )
        })
        .ok_or_else(|| anchor_not_found(item_id))?;
    if item.state != BlockReviewState::Open {
        return Err(GatewayError::invalid(format!(
            "review item {item_id} is not open"
        )));
    }
    Ok(item)
}

fn require_open_reply_root(root: &BlockReviewItem, root_id: &str) -> Result<(), GatewayError> {
    if !matches!(
        root.kind,
        BlockReviewKind::Comment | BlockReviewKind::Suggestion
    ) {
        return Err(anchor_not_found(root_id));
    }
    if root.state != BlockReviewState::Open {
        return Err(GatewayError::invalid(format!(
            "review thread {root_id} is not open"
        )));
    }
    Ok(())
}

fn require_open_comment_for_edit<'a>(
    ctx: &'a ApplyContext,
    item_id: &str,
) -> Result<&'a BlockReviewItem, GatewayError> {
    let item = require_comment(ctx, item_id)?;
    if item.state != BlockReviewState::Open {
        return Err(GatewayError::invalid(format!(
            "comment {item_id} is not open"
        )));
    }
    if let Some(parent_id) = item.parent_item_id.as_deref() {
        let parent = ctx
            .items
            .iter()
            .find(|parent| {
                parent.id == parent_id
                    && matches!(
                        parent.kind,
                        BlockReviewKind::Comment | BlockReviewKind::Suggestion
                    )
            })
            .ok_or_else(|| anchor_not_found(parent_id))?;
        require_open_reply_root(parent, parent_id).map_err(|_| {
            GatewayError::invalid(format!("comment {item_id} belongs to a non-open thread"))
        })?;
    }
    Ok(item)
}

/// Conflict review items resolve and delete with the comment vocabulary
/// (`comment.resolve` / `comment.delete`) — resolution never mutates the
/// document. Replies stay comment-only.
fn require_resolvable<'a>(
    ctx: &'a ApplyContext,
    item_id: &str,
) -> Result<&'a BlockReviewItem, GatewayError> {
    ctx.items
        .iter()
        .find(|item| {
            item.id == item_id
                && matches!(
                    item.kind,
                    BlockReviewKind::Comment | BlockReviewKind::Conflict
                )
        })
        .ok_or_else(|| anchor_not_found(item_id))
}

fn require_suggestion<'a>(
    ctx: &'a ApplyContext,
    item_id: &str,
) -> Result<&'a BlockReviewItem, GatewayError> {
    ctx.items
        .iter()
        .find(|item| item.id == item_id && item.kind == BlockReviewKind::Suggestion)
        .ok_or_else(|| anchor_not_found(item_id))
}

fn anchor_not_found(item_id: &str) -> GatewayError {
    GatewayError::new(
        GatewayErrorCode::AnchorNotFound,
        format!("review item {item_id} does not exist"),
    )
}

fn suggestion_invalidated(item_id: &str) -> GatewayError {
    GatewayError::new(
        GatewayErrorCode::SuggestionInvalidated,
        format!("suggestion {item_id} was invalidated by a content change"),
    )
}

fn validate_block_type(block_type: &str) -> Result<(), GatewayError> {
    if KNOWN_BLOCK_TYPES.contains(&block_type) {
        return Ok(());
    }
    Err(GatewayError::new(
        GatewayErrorCode::UnknownBlockType,
        format!(
            "unknown block_type \"{block_type}\"; valid types: {}. There is no list \
             block type — a list item is a \"p\" block with attrs \
             {{\"indent\": 1, \"listStyleType\": \"disc\" | \"decimal\" | \"todo\"}}",
            KNOWN_BLOCK_TYPES.join(", ")
        ),
    ))
}

/// The `id` attribute is the block identity on exported Slate elements; ops
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

/// Completes and validates the list shape on block attrs. The browser
/// editor's list plugin requires `indent` next to `listStyleType` and
/// silently strips the list shape when it is missing — a commit would
/// succeed and the data would vanish downstream — so the gateway defaults
/// `indent` to 1 and rejects list styles the editor cannot represent.
fn normalize_list_attrs(attrs: &Attrs) -> Result<Attrs, GatewayError> {
    let mut attrs = attrs.clone();
    let Some(style) = attrs.get("listStyleType") else {
        return Ok(attrs);
    };
    if !style
        .as_str()
        .is_some_and(|style| LIST_STYLE_TYPES.contains(&style))
    {
        return Err(GatewayError::invalid(format!(
            "unknown listStyleType {style}; valid values: \"disc\", \"decimal\", \"todo\""
        )));
    }
    attrs.entry("indent".to_string()).or_insert(json!(1));
    Ok(attrs)
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
fn rewrite_marks(
    marks: &[MarkRun],
    text: &str,
    start: u32,
    end: u32,
    change: impl Fn(&mut Attrs),
) -> Vec<MarkRun> {
    let len = utf16_len(text);
    let mut boundaries = BTreeSet::from([0, len, start, end]);
    for run in marks {
        boundaries.insert(run.start);
        boundaries.insert(run.end);
    }
    let boundaries: Vec<u32> = boundaries.into_iter().collect();
    let mut result: Vec<MarkRun> = Vec::new();
    for window in boundaries.windows(2) {
        let (segment_start, segment_end) = (window[0], window[1]);
        let mut attrs = marks
            .iter()
            .find(|run| run.start <= segment_start && segment_end <= run.end)
            .map(|run| run.marks.clone())
            .unwrap_or_default();
        if start <= segment_start && segment_end <= end {
            change(&mut attrs);
        }
        if attrs.is_empty() {
            continue;
        }
        if let Some(last) = result.last_mut() {
            if last.end == segment_start && last.marks == attrs {
                last.end = segment_end;
                continue;
            }
        }
        result.push(MarkRun {
            start: segment_start,
            end: segment_end,
            marks: attrs,
        });
    }
    result
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

const COMMIT_RETRY_LIMIT: usize = 3;

pub(crate) async fn document_blocks(
    state: &AppState,
    library: &str,
    path: &str,
) -> Result<Response, ApiError> {
    gateway_reply(document_blocks_inner(state, library, path).await)
}

async fn document_blocks_inner(
    state: &AppState,
    library: &str,
    path: &str,
) -> Result<Response, GatewayFailure> {
    let document = state.store.get_document(library, path).await?;
    require_block_document(&document.path, &document.version.content_type)?;
    let mut document_clock = document.version.id.clone();
    let mut rows = state.store.load_block_tree(&document.id).await?;
    if rows.is_empty() {
        // Materialize the projection from the head Markdown so the returned
        // block ids are durable and addressable by later transactions. This
        // publishes the one-time normalized version (Phase 1 import path).
        let markdown = String::from_utf8(document.content.clone()).map_err(|_| {
            GatewayFailure::Api(
                QuarryError::InvalidInput(format!(
                    "document {} is not valid UTF-8 Markdown",
                    document.path
                ))
                .into(),
            )
        })?;
        let outcome = state
            .store
            .import_block_document(
                library,
                path,
                &markdown,
                document.version.metadata.clone(),
                &document.version.content_type,
                DocumentSource::Rest,
                WritePrecondition::IfMatch(document.version.id.clone()),
            )
            .await?;
        document_clock = outcome.version.id;
        rows = state.store.load_block_tree(&document.id).await?;
    }
    let payload = BlockTreeResponse {
        document_id: document.id,
        document_clock: document_clock.clone(),
        blocks: rows.into_iter().map(block_payload).collect(),
    };
    Ok(json_with_etag(StatusCode::OK, &payload, &document_clock)?)
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
    gateway_reply(document_block_transactions_inner(state, library, path, payload).await)
}

async fn document_block_transactions_inner(
    state: &AppState,
    library: &str,
    path: &str,
    payload: JsonValue,
) -> Result<Response, GatewayFailure> {
    let request = parse_transaction(payload)?;
    let ctx = TransactionContext {
        client_tx_id: request.client_tx_id,
        base_clock: request.base_clock,
        actor: request.actor,
    };
    let (ops, ops_json) = (request.ops, request.ops_json);
    let mut plan = move |_snapshot: &BlockMutationState| {
        Ok(TransactionPlan {
            ops: ops.clone(),
            ops_json: ops_json.clone(),
        })
    };
    let reply = execute_block_transaction(
        state,
        library,
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
                document_clock: committed.outcome.version.id.clone(),
                transaction_id: committed.transaction_id,
                changed_block_ids: committed.changed_block_ids,
            };
            Ok(json_with_etag(StatusCode::OK, &ack, &ack.document_clock)?)
        }
        TransactionReply::Replayed(record) => replay_response(&record),
    }
}

/// The mode switch: resolve the document, take its per-document mutex
/// (serializing against session seed/checkpoint/discard), and dispatch on
/// whether a live session exists. A transaction racing a transition waits
/// here; it is never rejected because a session exists.
pub(crate) async fn execute_block_transaction(
    state: &AppState,
    library: &str,
    path: &str,
    ctx: &TransactionContext,
    settings: &TransactionSettings,
    plan: PlanProvider<'_>,
) -> Result<TransactionReply, GatewayFailure> {
    let document = state.store.head_document(library, path).await?;
    let guard = state.sessions.lock_document(&document.id).await;
    match guard.session() {
        Some(session) => {
            apply_session_transaction(state, &session, library, path, ctx, settings, plan).await
        }
        None => apply_rows_transaction(state, library, path, ctx, settings, plan).await,
    }
}

/// Rows-authoritative mode: ops apply directly to block rows in one SQL
/// transaction. The caller holds the per-document mutex.
async fn apply_rows_transaction(
    state: &AppState,
    library: &str,
    path: &str,
    ctx: &TransactionContext,
    settings: &TransactionSettings,
    plan: PlanProvider<'_>,
) -> Result<TransactionReply, GatewayFailure> {
    for _attempt in 0..COMMIT_RETRY_LIMIT {
        let snapshot = state
            .store
            .block_mutation_state(library, path, &ctx.client_tx_id)
            .await?;
        if let Some(record) = &snapshot.replay {
            return Ok(TransactionReply::Replayed(record.clone()));
        }
        require_block_document(&snapshot.path, &snapshot.content_type)?;
        let status = transaction_status(&ctx.base_clock, &snapshot)?;
        let planned = plan(&snapshot)?;
        let applied = apply_ops(&snapshot, &planned.ops, &ctx.actor, &ctx.client_tx_id)?;
        let metadata = settings
            .metadata
            .clone()
            .unwrap_or_else(|| snapshot.metadata.clone());
        let commit = BlockMutationCommit {
            document_id: snapshot.document_id.clone(),
            expected_head_version_id: snapshot.head_version_id.clone(),
            client_tx_id: ctx.client_tx_id.clone(),
            actor_kind: ctx.actor.kind.clone(),
            actor_id: ctx.actor.id.clone(),
            transaction_actor: settings
                .transaction
                .actor
                .clone()
                .or_else(|| Some(ctx.actor.display())),
            transaction_message: settings.transaction.message.clone(),
            transaction_provenance: Some(settings.transaction.provenance.clone())
                .filter(|provenance| !provenance.is_null() && provenance != &json!({})),
            origin_id: settings.origin_id.clone(),
            source: settings.source.clone(),
            recorded_ops: recorded_ops(
                &planned.ops_json,
                &ctx.actor,
                status,
                &applied.changed_block_ids,
            ),
            metadata: metadata.clone(),
            content_type: snapshot.content_type.clone(),
            rows: applied.rows.clone(),
            review_items: applied.review_items,
            normalized_markdown: normalized_markdown(&applied.rows, &metadata)?,
        };
        match state.store.commit_block_mutation(library, commit).await {
            Ok(BlockMutationOutcome::Applied { outcome, record }) => {
                return Ok(TransactionReply::Committed(CommittedTransaction {
                    status,
                    outcome,
                    transaction_id: record.id,
                    changed_block_ids: applied.changed_block_ids,
                }));
            }
            Ok(BlockMutationOutcome::Replayed(record)) => {
                return Ok(TransactionReply::Replayed(record))
            }
            // Another write moved the head between load and commit: reload
            // the state and recompute against the new rows.
            Err(QuarryError::PreconditionFailed(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(GatewayFailure::Api(
        QuarryError::Busy("document head kept moving during the block transaction".to_string())
            .into(),
    ))
}

/// Session-authoritative mode (the Gate B mechanics): the transaction
/// applies into the live Yjs doc as another collaborator and forces a
/// checkpoint before acking, so an acked write is durable in rows.
///
/// Under the per-document mutex and the doc's write lock:
///
/// 1. flush pending browser typing as a coalesced `browser_session`
///    checkpoint (so the ops validate against — and history attributes —
///    exactly what the agent's transaction saw);
/// 2. run the same pure apply engine as rows mode against the freshly
///    checkpointed rows;
/// 3. reconcile the live doc in place to the applied result (untouched
///    blocks keep their element identity, so peer cursors survive; review
///    ops become mark/meta edits);
/// 4. commit the applied rows/items as the transaction's version and ack.
///
/// Browser updates queue on the doc write lock for the duration and merge
/// right after — they land in the NEXT checkpoint. No op needs the
/// reseed/state-replacement fallback: every gateway op is expressible as an
/// in-place doc reconciliation.
#[allow(clippy::too_many_arguments)]
async fn apply_session_transaction(
    state: &AppState,
    session: &crate::session::LiveSession,
    library: &str,
    path: &str,
    ctx: &TransactionContext,
    settings: &TransactionSettings,
    plan: PlanProvider<'_>,
) -> Result<TransactionReply, GatewayFailure> {
    let mut awareness = session.awareness().clone().write_owned().await;
    // 1. Force-checkpoint pending typing (no-op when clean). Head races are
    //    handled inside commit_doc_state (it retries and surfaces Busy).
    if session.is_dirty() {
        session
            .commit_doc_state(&state.store, &mut awareness)
            .await
            .map_err(GatewayFailure::from)?;
    }
    let snapshot = state
        .store
        .block_mutation_state(library, path, &ctx.client_tx_id)
        .await?;
    if let Some(record) = &snapshot.replay {
        return Ok(TransactionReply::Replayed(record.clone()));
    }
    require_block_document(&snapshot.path, &snapshot.content_type)?;
    let status = transaction_status(&ctx.base_clock, &snapshot)?;

    // 2. Apply against the session's authoritative state. After the
    //    flush, stored rows equal the session projection; review items
    //    come from the session (the store snapshot agrees, but the
    //    session is the source of truth while it lives).
    let mut session_snapshot = snapshot.clone();
    session_snapshot.review_items = session.items_snapshot();
    let planned = plan(&session_snapshot)?;
    let applied = apply_ops(
        &session_snapshot,
        &planned.ops,
        &ctx.actor,
        &ctx.client_tx_id,
    )?;

    // 3. Write the change into the live doc as a collaborator.
    let pre = crate::session::doc_image(&session_snapshot.rows, &session_snapshot.review_items)
        .map_err(GatewayFailure::from)?;
    let desired = crate::session::doc_image(&applied.rows, &applied.review_items)
        .map_err(GatewayFailure::from)?;
    session
        .apply_desired_state(&awareness, &pre, &desired, &applied.review_items)
        .map_err(GatewayFailure::from)?;

    // 4. Checkpoint-before-ack: commit the applied state as this
    //    transaction's version.
    let metadata = settings
        .metadata
        .clone()
        .unwrap_or_else(|| session_snapshot.metadata.clone());
    let commit = BlockMutationCommit {
        document_id: session_snapshot.document_id.clone(),
        expected_head_version_id: session_snapshot.head_version_id.clone(),
        client_tx_id: ctx.client_tx_id.clone(),
        actor_kind: ctx.actor.kind.clone(),
        actor_id: ctx.actor.id.clone(),
        transaction_actor: settings
            .transaction
            .actor
            .clone()
            .or_else(|| Some(ctx.actor.display())),
        transaction_message: settings.transaction.message.clone(),
        transaction_provenance: Some(settings.transaction.provenance.clone())
            .filter(|provenance| !provenance.is_null() && provenance != &json!({})),
        // In-session browsers classify this as a benign refresh
        // (`session-events.ts`), exactly like checkpoint commits — for
        // whole-file writes too, since the session doc already carries the
        // merged state when the event fires.
        origin_id: Some(format!("agent-injected:tx:{}", ctx.client_tx_id)),
        source: settings.source.clone(),
        recorded_ops: recorded_ops(
            &planned.ops_json,
            &ctx.actor,
            status,
            &applied.changed_block_ids,
        ),
        metadata: metadata.clone(),
        content_type: session_snapshot.content_type.clone(),
        rows: applied.rows.clone(),
        review_items: applied.review_items.clone(),
        normalized_markdown: normalized_markdown(&applied.rows, &metadata)?,
    };
    match state.store.commit_block_mutation(library, commit).await {
        Ok(BlockMutationOutcome::Applied { outcome, record }) => {
            session.mark_committed(&mut awareness, &outcome, &applied.review_items);
            Ok(TransactionReply::Committed(CommittedTransaction {
                status,
                outcome,
                transaction_id: record.id,
                changed_block_ids: applied.changed_block_ids,
            }))
        }
        Ok(BlockMutationOutcome::Replayed(record)) => Ok(TransactionReply::Replayed(record)),
        // An external legacy write raced the session between the step-1
        // flush and this commit. The live doc already carries this
        // transaction's edits but nothing was committed (so there is no
        // replay record yet). Surface Busy: a client retry re-runs the
        // ops against the flushed state — idempotent for replaces, and
        // safe for inserts because minted ids are deterministic per
        // (client_tx_id, op): a re-insert collides with the first
        // application's block and fails typed instead of duplicating.
        //
        // HONEST WINDOW (any commit failure below, not just this arm): the
        // live doc was mutated in step 3 BEFORE the commit, so on failure
        // the merged CONTENT survives in the session and lands via the next
        // checkpoint — but the transaction's review-item side effects do
        // not ride in the doc and are dropped with the failed commit. In
        // particular, Phase 4 conflict artifacts (`conflict.add` is not
        // doc-represented) can be lost while the merged content still
        // arrives: content-without-its-conflicts if the caller does not
        // retry. The remaining writers that bypass this mutex are
        // staged-transaction commits and direct store callers (every
        // Markdown PUT / Git / FUSE / CLI / metadata write / version
        // restore now routes through the gateway).
        //
        // Phase 5 assessed the two candidate fixes and re-scoped the window
        // to a recorded limitation (Phase 7) instead of fixing it here:
        // committing BEFORE the doc mutation trades this window for a worse
        // one (a commit whose doc apply then fails is silently REVERTED by
        // the next checkpoint — an acked transaction lost, instead of a
        // retryable error), and the real fix — persisting non-doc side
        // effects in a pending queue keyed by client_tx_id, replayed on the
        // next successful commit — is new durable machinery that none of
        // the in-tree callers need: every gateway caller (markdown_write,
        // Git, FUSE, CLI, REST agents) surfaces or retries the typed
        // retryable Busy error.
        Err(QuarryError::PreconditionFailed(detail)) => {
            Err(GatewayFailure::Api(QuarryError::Busy(detail).into()))
        }
        Err(error) => Err(error.into()),
    }
}

fn recorded_ops(
    ops_json: &JsonValue,
    actor: &BlockTransactionActor,
    status: &str,
    changed_block_ids: &[String],
) -> JsonValue {
    json!({
        "ops": ops_json,
        "actor": actor,
        "ack": {
            "status": status,
            "changed_block_ids": changed_block_ids,
        },
    })
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
        return Err(stale_base(token));
    };
    if clock == snapshot.head_version_id {
        Ok("committed")
    } else if snapshot.version_ids.contains(&clock) {
        Ok("committed_rebased")
    } else {
        Err(stale_base(token))
    }
}

fn stale_base(token: &str) -> GatewayError {
    GatewayError::new(
        GatewayErrorCode::StaleBase,
        format!("base_clock {token} does not name a known version of this document"),
    )
}

/// Answers a duplicate `client_tx_id` from the stored history record: the
/// ORIGINAL ack (status and changed ids ride in the record's `ops.ack`).
fn replay_response(record: &BlockTransactionRecord) -> Result<Response, GatewayFailure> {
    let document_clock = record.resulting_version_id.clone().ok_or_else(|| {
        GatewayFailure::Api(
            QuarryError::Storage(format!(
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
        ordinal: ordinals.get(item.block_id.as_str()).copied().unwrap_or(0),
        content_hash: None,
    };
    let anchor = |item: &BlockReviewItem| {
        Some(BlockReviewAnchor {
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
            let replacement = item.replacement.clone().unwrap_or_default();
            AgentReviewSuggestion {
                id: item.id.clone(),
                status: item.state.as_str().to_string(),
                kind: if replacement.is_empty() {
                    AgentSuggestionKind::Delete
                } else {
                    AgentSuggestionKind::Replace
                },
                by: by(item),
                at: item.created_at.clone(),
                block_ref: block_ref(item),
                quote: quote(item),
                content: replacement.clone(),
                preview: AgentSuggestionPreview {
                    before: anchored_text(item).unwrap_or_else(|| quote(item)),
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
            projection_missing: false,
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

    /// The silent failure that shipped: `listStyleType` without `indent`
    /// committed, then the browser editor's list plugin stripped the list
    /// shape. The gateway must complete the pair so the commit and the
    /// surviving data agree.
    #[test]
    fn list_style_type_without_indent_gets_indent_defaulted_to_1() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Existing.")]);
        let ops = [op(json!({
            "op": "insert_block",
            "position": 1,
            "block_type": "p",
            "attrs": {"listStyleType": "disc"},
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
    }

    #[test]
    fn set_block_attrs_also_defaults_list_indent() {
        let state = state_with_rows(vec![paragraph("b1", 0, "Item one.")]);
        let ops = [op(json!({
            "op": "set_block_attrs",
            "block_id": "b1",
            "attrs": {"listStyleType": "decimal"}
        }))];

        let outcome = apply_ops(&state, &ops, &actor(), "tx-attrs").unwrap();

        let item = outcome.rows.first().expect("row");
        assert_eq!(item.attrs.get("indent"), Some(&json!(1)));
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
    }

    /// A session-mode retry after a failed commit re-runs the same ops; the
    /// engine must mint the SAME ids so re-application cannot silently
    /// duplicate inserted content (it collides and fails typed instead).
    #[test]
    fn minted_ids_are_deterministic_per_client_tx_id_so_retries_cannot_duplicate() {
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
    fn diff_of_pure_insertion_is_collapsed_at_the_insertion_point() {
        let diff = utf16_text_diff("Hello world", "Hello brave world");
        assert_eq!(diff.prefix, 6);
        assert_eq!(diff.old_mid_end, 6);
        assert_eq!(diff.new_mid_end, 12);
        assert!(diff.is_pure_insertion());
    }

    #[test]
    fn diff_measures_utf16_units_for_surrogate_pairs() {
        // 😀 is one char but two UTF-16 code units.
        let diff = utf16_text_diff("a😀b", "a😀XYb");
        assert_eq!(diff.prefix, 3);
        assert_eq!(diff.old_mid_end, 3);
        assert_eq!(diff.new_mid_end, 5);
    }

    #[test]
    fn anchor_in_preserved_prefix_keeps_offsets() {
        let diff = utf16_text_diff("keep THIS tail", "keep THAT tail");
        assert_eq!(adjust_anchor(diff, 0, 4), AnchorFate::Keep(0, 4));
    }

    #[test]
    fn anchor_in_preserved_suffix_shifts_by_the_delta() {
        let diff = utf16_text_diff("short middle tail", "shorter-now middle tail");
        // "tail" sits at [13, 17) in the old text and shifts right by 6.
        assert_eq!(adjust_anchor(diff, 13, 17), AnchorFate::Keep(19, 23));
    }

    #[test]
    fn anchor_overlapping_the_changed_middle_dies_at_the_change_site() {
        let diff = utf16_text_diff("aaa MIDDLE zzz", "aaa OTHER zzz");
        assert_eq!(adjust_anchor(diff, 2, 8), AnchorFate::Dead(diff.prefix));
    }

    #[test]
    fn insertion_at_anchor_start_boundary_is_excluded() {
        // Insert at offset 4 == anchor start: the anchor never grows leftward,
        // so it shifts right past the inserted text.
        let diff = utf16_text_diff("pre ANCHOR", "pre XXANCHOR");
        assert_eq!(adjust_anchor(diff, 4, 10), AnchorFate::Keep(6, 12));
    }

    #[test]
    fn insertion_at_anchor_end_boundary_is_excluded() {
        // Insert at offset 6 == anchor end: the anchor never grows rightward.
        let diff = utf16_text_diff("ANCHOR post", "ANCHORXX post");
        assert_eq!(adjust_anchor(diff, 0, 6), AnchorFate::Keep(0, 6));
    }

    #[test]
    fn interior_insertion_grows_the_anchor() {
        let diff = utf16_text_diff("ANCHOR", "ANCXXHOR");
        assert_eq!(adjust_anchor(diff, 0, 6), AnchorFate::Keep(0, 8));
    }

    #[test]
    fn range_spanning_the_whole_middle_stretches_over_the_replacement() {
        let diff = utf16_text_diff("ab MIDDLE yz", "ab LONGER-MIDDLE yz");
        assert_eq!(adjust_range(diff, 0, 12), Some((0, 19)));
    }

    #[test]
    fn range_partially_overlapping_the_middle_clamps_to_the_preserved_prefix() {
        let diff = utf16_text_diff("aaa MIDDLE zzz", "aaa OTHER zzz");
        // Range [0, 8) keeps its prefix part [0, 4).
        assert_eq!(adjust_range(diff, 0, 8), Some((0, diff.prefix)));
    }

    #[test]
    fn range_entirely_inside_the_middle_vanishes() {
        let diff = utf16_text_diff("aaa MIDDLE zzz", "aaa OTHER zzz");
        assert_eq!(adjust_range(diff, 5, 9), None);
    }

    #[test]
    fn rewrite_marks_adds_a_run_and_coalesces_neighbours() {
        let existing = vec![MarkRun {
            start: 0,
            end: 4,
            marks: bold(),
        }];
        let result = rewrite_marks(&existing, "abcdefgh", 4, 8, |attrs| {
            attrs.insert("bold".to_string(), json!(true));
        });
        assert_eq!(result.len(), 1);
        assert_eq!((result[0].start, result[0].end), (0, 8));
        assert_eq!(result[0].marks, bold());
    }

    #[test]
    fn rewrite_marks_removes_a_key_from_the_span_only() {
        let existing = vec![MarkRun {
            start: 0,
            end: 8,
            marks: bold(),
        }];
        let result = rewrite_marks(&existing, "abcdefgh", 2, 4, |attrs| {
            attrs.shift_remove("bold");
        });
        let shape: Vec<(u32, u32)> = result.iter().map(|run| (run.start, run.end)).collect();
        assert_eq!(shape, vec![(0, 2), (4, 8)]);
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
    fn deleting_the_last_block_remints_the_empty_paragraph_shape() {
        let state = state_with_rows(vec![paragraph("only", 0, "Text")]);
        let applied = apply_ops(
            &state,
            &[op(json!({"op": "delete_block", "block_id": "only"}))],
            &actor(),
            "test-tx",
        )
        .unwrap();
        assert_eq!(applied.rows.len(), 1);
        assert_eq!(applied.rows[0].block_type, "p");
        assert_eq!(applied.rows[0].text, "");
        assert_ne!(applied.rows[0].block_id, "only");
        // Both the deleted block and the minted replacement are "touched".
        assert_eq!(applied.changed_block_ids.len(), 2);
        assert!(applied.changed_block_ids.contains(&"only".to_string()));
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
    fn collapsed_anchor_stays_a_point_under_insertion_at_its_offset() {
        // "prefix " is 7 units; the insertion lands exactly at offset 7 where
        // a dead anchor collapsed. The point must move as one (never invert).
        let diff = utf16_text_diff("prefix tail", "prefix X tail");
        assert!(diff.is_pure_insertion());
        assert_eq!(adjust_anchor(diff, 7, 7), AnchorFate::Keep(7, 7));
        // A collapsed point strictly after the insertion shifts as one.
        assert_eq!(adjust_anchor(diff, 9, 9), AnchorFate::Keep(11, 11));
    }

    #[test]
    fn collapsed_anchor_inside_a_replaced_middle_moves_to_the_change_site() {
        let diff = utf16_text_diff("aaa MIDDLE zzz", "aaa OTHER zzz");
        assert_eq!(
            adjust_anchor(diff, 6, 6),
            AnchorFate::Keep(diff.prefix, diff.prefix)
        );
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
