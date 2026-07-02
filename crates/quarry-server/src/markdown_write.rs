//! Whole-file Markdown writes — Phase 4 of the session-scoped collab
//! rewrite: the ONE reconciliation implementation behind Git, FUSE, the CLI,
//! and the REST Markdown `PUT`.
//!
//! A write is `diff3(base, incoming file, current canonical rows)` via
//! [`quarry_collab_codec::reconcile`], translated into gateway ops and
//! submitted through [`gateway::execute_block_transaction`] — so it takes the
//! per-document mutex and rides the mode switch: rows mode commits straight
//! to SQL; an active browser session receives the merge as a collaborator
//! edit and checkpoints before the ack (no errno, no LWW overwrite). Adapters
//! differ only in base bookkeeping:
//!
//! - **Git** stores per-peer shadow bases (`block_shadow_bases`, surface
//!   `git`, scope = peer id) at export/import and passes them here.
//! - **FUSE** captures the base per open handle at `open()` (in-memory; a
//!   handle's base advances to whatever it last wrote).
//! - **CLI** and missing-base cases use [`BlockWriteBase::CurrentCanonical`]
//!   — the two-way degenerate merge that can never conflict.
//! - **REST `PUT`** resolves `If-Match` to that version's stored content as
//!   the base (falling back to two-way without one).
//!
//! True conflicts never fail the write: each [`ReconcileConflict`] becomes a
//! `conflict.add` op in the SAME transaction, so artifacts commit atomically
//! with the merge and surface via `GET /review`. The only write failures are
//! content errors (CriticMarkup → typed `UNSUPPORTED_MARKDOWN`, invalid
//! frontmatter YAML, non-UTF-8 bytes) and ordinary storage failures — never
//! reconciliation outcomes.
//!
//! Byte-identical writes (vs the head content) short-circuit without a
//! commit, so repeated `git import`/no-op saves do not churn versions.
//!
//! ## Placement translation (final-index → sequential)
//!
//! The codec emits placements at FINAL merged top-level indices with
//! detach-all-moves-first semantics (reconcile rule 8); the gateway applies
//! ops sequentially. [`sequential_ops`] bridges them exactly by simulating:
//! it computes the final top-level order, then walks it against a working
//! copy of the current order, emitting `move_block`/`insert_block` ops whose
//! positions are correct at their application time. Move attribution may
//! differ from the codec's (moving a displaced sibling instead of the
//! designated mover) — the merged order, ids, and anchors are identical
//! either way, pinned by `sequential_ops_*` tests.

use crate::gateway::{
    self, BlockOp, BlockTransactionActor, GatewayError, GatewayErrorCode, GatewayFailure,
    TransactionContext, TransactionPlan, TransactionReply, TransactionSettings,
};
use crate::log_redaction;
use crate::AppState;
use axum::http::StatusCode;
use quarry_collab_codec::{
    block_rows_to_markdown, reconcile, BlockRow, ReconcileBase, ReconcileOp,
};
use quarry_core::{DocumentSource, QuarryError, WriteOutcome, WritePrecondition};
use quarry_storage::{
    document_kind, merge_json, split_markdown_frontmatter, BlockMarkdownWrite,
    BlockMarkdownWriteOutcome, BlockMarkdownWriter, BlockReviewKind, BlockWriteBase, DocumentKind,
    DocumentScopeRef,
};
use serde_json::Value as JsonValue;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// The Markdown `PUT` body for a BlockDocument: `If-Match` selects the base
/// version (its stored content), `If-None-Match` is a create, no
/// precondition degenerates to two-way. An `If-Match` naming an unknown
/// version keeps the legacy 412 (client confusion, not a merge input);
/// a KNOWN stale version merges instead of failing.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn put_block_document(
    state: &AppState,
    library: &str,
    path: &str,
    body: Vec<u8>,
    metadata: JsonValue,
    precondition: WritePrecondition,
    origin_id: Option<String>,
    transaction: quarry_storage::TransactionMetadata,
) -> Result<axum::response::Response, GatewayFailure> {
    put_scoped_block_document(
        state,
        DocumentScopeRef::library(library),
        path,
        body,
        metadata,
        precondition,
        origin_id,
        transaction,
    )
    .await
}

pub(crate) async fn put_tmp_block_document(
    state: &AppState,
    path: &str,
    body: Vec<u8>,
    metadata: JsonValue,
    precondition: WritePrecondition,
    origin_id: Option<String>,
    transaction: quarry_storage::TransactionMetadata,
) -> Result<axum::response::Response, GatewayFailure> {
    put_scoped_block_document(
        state,
        DocumentScopeRef::Tmp,
        path,
        body,
        metadata,
        precondition,
        origin_id,
        transaction,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn put_scoped_block_document(
    state: &AppState,
    scope: DocumentScopeRef,
    path: &str,
    body: Vec<u8>,
    metadata: JsonValue,
    precondition: WritePrecondition,
    origin_id: Option<String>,
    transaction: quarry_storage::TransactionMetadata,
) -> Result<axum::response::Response, GatewayFailure> {
    let markdown = String::from_utf8(body).map_err(|_| {
        GatewayFailure::Api(
            QuarryError::InvalidInput(format!("markdown PUT body for {path} must be valid UTF-8"))
                .into(),
        )
    })?;
    let base = match precondition {
        WritePrecondition::IfNoneMatch => {
            let head = state.store.head_document_for_scope(&scope, path).await;
            if head.is_ok() {
                return Err(GatewayFailure::Api(
                    QuarryError::PreconditionFailed(format!("{path} already exists")).into(),
                ));
            }
            if let Err(error) = head {
                if !matches!(error, QuarryError::NotFound(_)) {
                    return Err(error.into());
                }
            }
            BlockWriteBase::CurrentCanonical
        }
        WritePrecondition::IfMatch(version_id) => {
            let version = state
                .store
                .document_version_for_scope(&scope, path, &version_id)
                .await;
            match version {
                Ok(version) => BlockWriteBase::Markdown {
                    markdown: version.content,
                    version_id: Some(version_id),
                },
                Err(QuarryError::NotFound(_)) => {
                    return Err(GatewayFailure::Api(
                        QuarryError::PreconditionFailed(format!(
                            "If-Match {version_id} does not name a known version of {path}"
                        ))
                        .into(),
                    ))
                }
                Err(error) => return Err(error.into()),
            }
        }
        WritePrecondition::None => BlockWriteBase::CurrentCanonical,
    };
    let result = write_markdown_with(
        state,
        BlockMarkdownWrite {
            scope,
            path: path.to_string(),
            markdown,
            metadata,
            base,
            source: DocumentSource::Rest,
            surface: "rest".to_string(),
            actor_label: None,
        },
        origin_id,
        transaction,
    )
    .await?;
    Ok(crate::json_with_etag(
        StatusCode::OK,
        &result.outcome,
        &result.outcome.version.id,
    )?)
}

/// A version restore on a BlockDocument: restore IS a whole-file write of
/// the stored version's content through the reconciler. The two-way base
/// (current canonical) makes the merge degenerate — the result equals the
/// restored content exactly — while unchanged blocks keep their `block_id`s
/// and live review anchors, and the write rides the mode switch (an active
/// browser session receives the restore as a collaborator edit instead of
/// having its projection cleared underneath it). RawDocument restores keep
/// the legacy byte path in storage.
pub(crate) async fn restore_block_document_version(
    state: &AppState,
    library: &str,
    path: &str,
    version: &quarry_core::DocumentVersionContent,
    origin_id: Option<String>,
    actor: Option<String>,
) -> Result<axum::response::Response, GatewayFailure> {
    let version_id = &version.version.id;
    let result = write_markdown_with(
        state,
        BlockMarkdownWrite {
            scope: DocumentScopeRef::library(library),
            path: path.to_string(),
            markdown: version.content.clone(),
            metadata: version.version.metadata.clone(),
            base: BlockWriteBase::CurrentCanonical,
            source: DocumentSource::Rest,
            surface: "rest".to_string(),
            actor_label: Some(format!("Restore version {version_id}")),
        },
        origin_id,
        quarry_storage::TransactionMetadata {
            actor,
            message: Some(format!("Restore version {version_id}")),
            provenance: Some(serde_json::json!({
                "mode": "auto_commit",
                "history": {"kind": "checkpoint", "reason": "restore"}
            })),
        },
    )
    .await?;
    Ok(crate::json_with_etag(
        StatusCode::OK,
        &result.outcome,
        &result.outcome.version.id,
    )?)
}

/// A metadata patch on a BlockDocument (metadata IS the frontmatter): a
/// zero-op gateway transaction with a metadata override. The block rows and
/// review items are untouched (the body did not change — rows stay valid by
/// construction); only the rendered frontmatter in the normalized content
/// moves. Dispatching through [`gateway::execute_block_transaction`] takes
/// the per-document mutex and composes with a live session for free: the
/// session doc carries body content only, so session mode flushes pending
/// typing, applies the empty op set (a no-op on the doc), and commits the
/// flushed rows under the new metadata — typing and frontmatter both land.
///
/// A projection-less BlockDocument (legacy write cleared its rows)
/// materializes here exactly like `GET /blocks` does, publishing the
/// one-time normalized content alongside the new metadata.
pub(crate) async fn patch_block_document_metadata(
    state: &AppState,
    library: &str,
    path: &str,
    patch: JsonValue,
    precondition: WritePrecondition,
) -> Result<axum::response::Response, GatewayFailure> {
    let document = state.store.get_document(library, path).await?;
    // The legacy patch enforced preconditions against the head via
    // put_document; mirror that strictness (a metadata patch is not a
    // content merge, so If-Match is a true precondition here, not a base
    // selector).
    match &precondition {
        WritePrecondition::IfMatch(version) if *version != document.version.id => {
            return Err(GatewayFailure::Api(
                QuarryError::PreconditionFailed(format!(
                    "If-Match {version} does not match the head of {path}"
                ))
                .into(),
            ));
        }
        WritePrecondition::IfNoneMatch => {
            return Err(GatewayFailure::Api(
                QuarryError::PreconditionFailed(format!("{path} already exists")).into(),
            ));
        }
        _ => {}
    }
    // Read-merge-write against the head we just loaded; a concurrent
    // metadata write between this read and the commit loses, exactly like
    // the legacy read-merge-put path.
    let mut metadata = document.metadata;
    merge_json(&mut metadata, patch);
    let ctx = TransactionContext {
        client_tx_id: Uuid::new_v4().to_string(),
        base_clock: None,
        actor: BlockTransactionActor {
            kind: "rest".to_string(),
            id: None,
            label: Some("Metadata patch".to_string()),
        },
    };
    let settings = TransactionSettings {
        source: DocumentSource::Rest,
        origin_id: None,
        metadata: Some(metadata),
        transaction: quarry_storage::TransactionMetadata::default(),
    };
    let mut plan = |_snapshot: &quarry_storage::BlockMutationState| {
        Ok(TransactionPlan {
            ops: Vec::new(),
            ops_json: JsonValue::Array(Vec::new()),
        })
    };
    let reply = gateway::execute_block_transaction(
        state,
        &DocumentScopeRef::library(library),
        path,
        &ctx,
        &settings,
        &mut plan,
    )
    .await?;
    let committed = match reply {
        TransactionReply::Committed(committed) => committed,
        TransactionReply::Replayed(record) => {
            return Err(GatewayFailure::Api(
                QuarryError::Storage(format!(
                    "fresh client_tx_id {} unexpectedly replayed",
                    record.client_tx_id
                ))
                .into(),
            ))
        }
    };
    Ok(crate::json_with_etag(
        StatusCode::OK,
        &*committed.outcome,
        &committed.outcome.version.id,
    )?)
}

/// The reconciled whole-file write. See the module docs; returns gateway
/// failures so the REST route keeps its typed error payloads.
pub(crate) async fn write_markdown_reconciled(
    state: &AppState,
    write: BlockMarkdownWrite,
) -> Result<BlockMarkdownWriteOutcome, GatewayFailure> {
    write_markdown_with(
        state,
        write,
        None,
        quarry_storage::TransactionMetadata::default(),
    )
    .await
}

async fn write_markdown_with(
    state: &AppState,
    write: BlockMarkdownWrite,
    origin_id: Option<String>,
    transaction: quarry_storage::TransactionMetadata,
) -> Result<BlockMarkdownWriteOutcome, GatewayFailure> {
    let content_type = write
        .metadata
        .get("content_type")
        .and_then(JsonValue::as_str)
        .unwrap_or("text/markdown")
        .to_string();
    gateway::require_block_document(&write.path, &content_type)?;
    let marker_hunk = first_conflict_marker_hunk(&write.markdown);

    // First import: the document does not exist yet — every block takes a
    // fresh id through the Phase 1 import path.
    let document = match state
        .store
        .get_document_for_scope(&write.scope, &write.path)
        .await
    {
        Ok(document) => {
            log_block_write_started(&write, Some(&document.id));
            document
        }
        Err(QuarryError::NotFound(_)) => {
            log_block_write_started(&write, None);
            let outcome = state
                .store
                .import_block_document_for_scope(
                    &write.scope,
                    &write.path,
                    &write.markdown,
                    write.metadata.clone(),
                    &content_type,
                    write.source.clone(),
                    WritePrecondition::IfNoneMatch,
                    origin_id,
                    transaction,
                )
                .await?;
            let conflicts = match &marker_hunk {
                Some(hunk) => flag_imported_conflict_markers(state, &write, hunk).await,
                None => 0,
            };
            let canonical_body = canonical_body(state, &outcome.document.id).await?;
            return Ok(BlockMarkdownWriteOutcome {
                outcome,
                changed: true,
                canonical_body,
                conflicts,
            });
        }
        Err(error) => return Err(error.into()),
    };

    // Byte-identical no-op: nothing to merge, nothing to commit. This check
    // runs OUTSIDE the document mutex (taken later by the dispatch), which
    // is benign: if a racing write changes the content between this read
    // and the lock, answering "no change against the version we observed"
    // is a legal serialization — identical to this write committing first
    // and the racing write winning afterwards.
    if document.content == write.markdown.as_bytes() {
        let entry = state
            .store
            .head_document_for_scope(&write.scope, &write.path)
            .await?;
        let transaction = state.store.get_transaction(&document.version.tx_id).await?;
        let canonical_body = canonical_body(state, &document.id).await?;
        return Ok(BlockMarkdownWriteOutcome {
            outcome: WriteOutcome {
                document: entry,
                version: document.version,
                transaction,
            },
            changed: false,
            canonical_body,
            conflicts: 0,
        });
    }

    let (incoming_frontmatter, incoming_body) =
        split_frontmatter_owned(&write.markdown).map_err(GatewayFailure::from)?;
    let mut merged_metadata = incoming_frontmatter;
    merge_json(&mut merged_metadata, write.metadata.clone());

    let base_body = match &write.base {
        BlockWriteBase::CurrentCanonical => None,
        BlockWriteBase::Markdown { markdown, .. } => Some(
            split_frontmatter_owned(markdown)
                .map_err(GatewayFailure::from)?
                .1,
        ),
    };
    let base_version = match &write.base {
        BlockWriteBase::Markdown {
            version_id: Some(version_id),
            ..
        } => Some(version_id.clone()),
        _ => None,
    };

    let actor = BlockTransactionActor {
        kind: write.surface.clone(),
        id: None,
        label: write.actor_label.clone(),
    };
    let settings = TransactionSettings {
        source: write.source.clone(),
        origin_id,
        metadata: Some(merged_metadata),
        transaction,
    };

    let mut conflicts = 0usize;
    let mut op_count = 0usize;
    let mut degraded = false;
    let mut plan = |snapshot: &quarry_storage::BlockMutationState| {
        let base = match &base_body {
            Some(body) => ReconcileBase::Markdown(body),
            None => ReconcileBase::CurrentCanonical,
        };
        let reconciled = reconcile(base, &incoming_body, &snapshot.rows, || {
            Uuid::new_v4().to_string()
        })
        .map_err(|unsupported| {
            GatewayError::new(
                GatewayErrorCode::UnsupportedMarkdown,
                unsupported.to_string(),
            )
        })?;
        conflicts = reconciled.conflicts.len();
        degraded = reconciled.degraded;
        let top_ids: Vec<String> = snapshot
            .rows
            .iter()
            .filter(|row| row.parent_block_id.is_none())
            .map(|row| row.block_id.clone())
            .collect();
        let mut ops = sequential_ops(&top_ids, &reconciled.ops);
        op_count = ops.len();
        ops.extend(
            reconciled
                .conflicts
                .into_iter()
                .map(|conflict| BlockOp::ConflictAdd {
                    after_block_id: conflict.after_block_id,
                    base_markdown: conflict.base_markdown,
                    incoming_markdown: conflict.incoming_markdown,
                    canonical_markdown: conflict.canonical_markdown,
                }),
        );
        // Incoming conflict-marker soup (a half-resolved git merge) commits
        // as content — writes never fail — but flags a review item in the
        // same transaction. Skipping hunks a conflict item already carries
        // keeps repeated saves from stacking flags and honors dismissals.
        if let Some(hunk) = &marker_hunk {
            let already_flagged = snapshot.review_items.iter().any(|item| {
                item.kind == BlockReviewKind::Conflict
                    && item.body.as_deref() == Some(hunk.as_str())
            });
            if !already_flagged {
                conflicts += 1;
                ops.push(conflict_marker_flag_op(hunk));
            }
        }
        let ops_json = serde_json::to_value(&ops)
            .map_err(|error| GatewayFailure::Api(QuarryError::Json(error).into()))?;
        Ok(TransactionPlan { ops, ops_json })
    };

    // The shadow base's version engages the gateway's rebase ack when it
    // still names a known version; an unknown/garbage clock must NOT fail a
    // file write, so retry once clockless.
    let mut ctx = TransactionContext {
        client_tx_id: Uuid::new_v4().to_string(),
        base_clock: base_version,
        actor,
    };
    let reply = match gateway::execute_block_transaction(
        state,
        &write.scope,
        &write.path,
        &ctx,
        &settings,
        &mut plan,
    )
    .await
    {
        Err(GatewayFailure::Typed(error))
            if error.code() == GatewayErrorCode::StaleBase && ctx.base_clock.is_some() =>
        {
            ctx.base_clock = None;
            gateway::execute_block_transaction(
                state,
                &write.scope,
                &write.path,
                &ctx,
                &settings,
                &mut plan,
            )
            .await?
        }
        other => other?,
    };
    let committed = match reply {
        TransactionReply::Committed(committed) => committed,
        // Unreachable with a fresh UUID client_tx_id; surface honestly.
        TransactionReply::Replayed(record) => {
            return Err(GatewayFailure::Api(
                QuarryError::Storage(format!(
                    "fresh client_tx_id {} unexpectedly replayed",
                    record.client_tx_id
                ))
                .into(),
            ))
        }
    };
    if degraded {
        tracing::warn!(
            event = "document.block_write.lcs_degraded",
            path = %loggable_path(&write),
            surface = %write.surface,
            "reconcile exceeded the LCS budget and fell back to bounded \
             positional pairing; move detection was lost for this write"
        );
    }
    tracing::info!(
        event = "document.block_write.reconciled",
        path = %loggable_path(&write),
        surface = %write.surface,
        result = %reconcile_result(op_count, conflicts),
        op_count,
        conflict_count = conflicts,
        "whole-file write reconciled"
    );
    let canonical_body = canonical_body(state, &committed.outcome.document.id).await?;
    Ok(BlockMarkdownWriteOutcome {
        outcome: *committed.outcome,
        changed: true,
        canonical_body,
        conflicts,
    })
}

/// The outcome vocabulary of the per-reconcile log: `conflicts` when review
/// items were recorded, `merged` when content ops applied cleanly, `clean`
/// when the incoming file normalized to the canonical state.
fn reconcile_result(op_count: usize, conflicts: usize) -> &'static str {
    if conflicts > 0 {
        "conflicts"
    } else if op_count > 0 {
        "merged"
    } else {
        "clean"
    }
}

/// Tmp paths are capability secrets; never log them raw.
fn loggable_path(write: &BlockMarkdownWrite) -> String {
    match &write.scope {
        DocumentScopeRef::Library { .. } => write.path.clone(),
        DocumentScopeRef::Tmp => {
            log_redaction::redact_tmp_document_identifier(&write.path).into_owned()
        }
    }
}

async fn canonical_body(state: &AppState, document_id: &str) -> Result<String, GatewayFailure> {
    let rows = state.store.load_block_tree(document_id).await?;
    block_rows_to_markdown(&rows).map_err(|unsupported| {
        GatewayFailure::Api(QuarryError::UnsupportedMarkdown(unsupported).into())
    })
}

fn log_block_write_started(write: &BlockMarkdownWrite, document_id: Option<&str>) {
    match &write.scope {
        DocumentScopeRef::Library { .. } => {
            tracing::debug!(
                event = "document.block_write.started",
                scope = %write.scope.event_library_id(),
                path = %write.path,
                document_id = %document_id.unwrap_or(""),
                surface = %write.surface,
                content_bytes = write.markdown.len(),
                "reconciled markdown write started"
            );
        }
        DocumentScopeRef::Tmp => {
            tracing::debug!(
                event = "document.block_write.started",
                scope = %"tmp",
                path = %log_redaction::redact_tmp_document_identifier(&write.path),
                document_id = %document_id.unwrap_or(""),
                surface = %write.surface,
                content_bytes = write.markdown.len(),
                "reconciled markdown write started"
            );
        }
    }
}

fn split_frontmatter_owned(markdown: &str) -> Result<(JsonValue, String), QuarryError> {
    let (frontmatter, body) = split_markdown_frontmatter(markdown)?;
    Ok((frontmatter, body.to_string()))
}

/// The first complete git conflict hunk (`<<<<<<<` … `=======` … `>>>>>>>`)
/// in the incoming markdown — the evidence excerpt for the review flag.
/// Requiring the full ordered triple keeps ordinary content (a setext
/// heading's `=======` underline, a quoted marker line) from
/// false-positiving on a single marker line.
fn first_conflict_marker_hunk(markdown: &str) -> Option<String> {
    let lines: Vec<&str> = markdown.lines().collect();
    let start = lines
        .iter()
        .position(|line| is_conflict_marker(line, "<<<<<<<"))?;
    let separator = start
        + 1
        + lines[start + 1..]
            .iter()
            .position(|line| *line == "=======")?;
    let end = separator
        + 1
        + lines[separator + 1..]
            .iter()
            .position(|line| is_conflict_marker(line, ">>>>>>>"))?;
    Some(format!("{}\n", lines[start..=end].join("\n")))
}

fn is_conflict_marker(line: &str, marker: &str) -> bool {
    line == marker
        || line
            .strip_prefix(marker)
            .is_some_and(|rest| rest.starts_with(' '))
}

/// The review flag for detected marker soup: a document-start conflict item
/// carrying the hunk as the incoming side. No base/canonical sides — this is
/// a warning about the incoming content, not a diff3 artifact.
fn conflict_marker_flag_op(hunk: &str) -> BlockOp {
    BlockOp::ConflictAdd {
        after_block_id: None,
        base_markdown: String::new(),
        incoming_markdown: hunk.to_string(),
        canonical_markdown: String::new(),
    }
}

/// Flags marker soup on a first import: the import path commits without a
/// gateway plan, so the flag rides a follow-up single-op transaction.
/// Failing to flag warns but never fails the write that already landed.
async fn flag_imported_conflict_markers(
    state: &AppState,
    write: &BlockMarkdownWrite,
    hunk: &str,
) -> usize {
    let ctx = TransactionContext {
        client_tx_id: Uuid::new_v4().to_string(),
        base_clock: None,
        actor: BlockTransactionActor {
            kind: write.surface.clone(),
            id: None,
            label: write.actor_label.clone(),
        },
    };
    let settings = TransactionSettings {
        source: write.source.clone(),
        origin_id: None,
        metadata: None,
        transaction: quarry_storage::TransactionMetadata::default(),
    };
    let mut plan = |_snapshot: &quarry_storage::BlockMutationState| {
        let ops = vec![conflict_marker_flag_op(hunk)];
        let ops_json = serde_json::to_value(&ops)
            .map_err(|error| GatewayFailure::Api(QuarryError::Json(error).into()))?;
        Ok(TransactionPlan { ops, ops_json })
    };
    let result = gateway::execute_block_transaction(
        state,
        &write.scope,
        &write.path,
        &ctx,
        &settings,
        &mut plan,
    )
    .await;
    match result {
        Ok(_) => 1,
        Err(failure) => {
            tracing::warn!(
                event = "document.block_write.marker_flag_failed",
                path = %loggable_path(write),
                error = %failure_to_quarry(failure),
                "conflict markers detected but the review flag failed to commit"
            );
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Final-index → sequential placement translation.
// ---------------------------------------------------------------------------

/// Translates codec [`ReconcileOp`]s (rule-8 final-index placements) into
/// gateway [`BlockOp`]s applied sequentially. `top_ids` is the snapshot's
/// current top-level order.
fn sequential_ops(top_ids: &[String], ops: &[ReconcileOp]) -> Vec<BlockOp> {
    let mut out: Vec<BlockOp> = Vec::new();
    let mut working: Vec<String> = top_ids.to_vec();
    let mut moved: Vec<&str> = Vec::new();
    let mut placements: Vec<(&ReconcileOp, usize)> = Vec::new();

    for op in ops {
        match op {
            ReconcileOp::ReplaceBlockContent {
                block_id,
                text,
                marks,
                links,
            } => out.push(BlockOp::ReplaceBlockContent {
                block_id: block_id.clone(),
                text: text.clone(),
                marks: Some(marks.clone()),
                links: Some(links.clone()),
            }),
            ReconcileOp::SetBlockType {
                block_id,
                block_type,
                attrs,
            } => out.push(BlockOp::SetBlockType {
                block_id: block_id.clone(),
                block_type: block_type.clone(),
                attrs: attrs.clone(),
            }),
            ReconcileOp::SetBlockAttrs { block_id, attrs } => out.push(BlockOp::SetBlockAttrs {
                block_id: block_id.clone(),
                attrs: attrs.clone(),
            }),
            ReconcileOp::DeleteBlock { block_id } => {
                working.retain(|id| id != block_id);
                out.push(BlockOp::DeleteBlock {
                    block_id: block_id.clone(),
                });
            }
            ReconcileOp::MoveBlock { block_id, position } => {
                moved.push(block_id);
                placements.push((op, *position));
            }
            ReconcileOp::InsertBlock { position, .. } => placements.push((op, *position)),
        }
    }

    // The final top-level order per rule-8 semantics: detach every move
    // target, then place moves and inserts at their final indices ascending.
    let mut final_order: Vec<String> = working
        .iter()
        .filter(|id| !moved.contains(&id.as_str()))
        .cloned()
        .collect();
    placements.sort_by_key(|(_, position)| *position);
    for (op, position) in &placements {
        let id = match op {
            ReconcileOp::MoveBlock { block_id, .. } => block_id.clone(),
            ReconcileOp::InsertBlock { rows, .. } => rows[0].block_id.clone(),
            _ => unreachable!("placements hold only moves and inserts"),
        };
        final_order.insert((*position).min(final_order.len()), id);
    }
    let inserted_rows: std::collections::HashMap<&str, &[BlockRow]> = placements
        .iter()
        .filter_map(|(op, _)| match op {
            ReconcileOp::InsertBlock { rows, .. } => {
                Some((rows[0].block_id.as_str(), rows.as_slice()))
            }
            _ => None,
        })
        .collect();

    // Walk the final order against the working copy, emitting sequential
    // ops. Whenever working[k] != final[k], `final[k]` is either a fresh
    // insert or a block that sits later in the working list (the prefix
    // below k is already final); inserting/moving it to k is exact.
    for (k, want) in final_order.iter().enumerate() {
        if working.get(k) == Some(want) {
            continue;
        }
        if let Some(rows) = inserted_rows.get(want.as_str()) {
            out.extend(insert_subtree_ops(rows, k as u32));
            working.insert(k, want.clone());
        } else {
            let from = working
                .iter()
                .position(|id| id == want)
                .expect("a non-inserted final block exists in the working order");
            let id = working.remove(from);
            working.insert(k, id);
            out.push(BlockOp::MoveBlock {
                block_id: want.clone(),
                parent_block_id: None,
                position: k as u32,
            });
        }
    }
    debug_assert_eq!(working, final_order);
    out
}

/// One inserted top-level subtree as gateway insert ops: the top row at the
/// sequential top-level `position`, descendants under their parents at their
/// sibling positions (depth-first, parents before children).
fn insert_subtree_ops(rows: &[BlockRow], position: u32) -> Vec<BlockOp> {
    rows.iter()
        .map(|row| BlockOp::InsertBlock {
            block_id: Some(row.block_id.clone()),
            parent_block_id: row.parent_block_id.clone(),
            position: if row.parent_block_id.is_none() {
                position
            } else {
                row.position
            },
            block_type: row.block_type.clone(),
            attrs: row.attrs.clone(),
            text: row.text.clone(),
            marks: row.marks.clone(),
            links: row.links.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The store-installed writer (Git/FUSE/CLI surface).
// ---------------------------------------------------------------------------

/// [`BlockMarkdownWriter`] over the live server state: the single
/// reconciliation implementation, session-mode aware. Installed into the
/// store by [`crate::app_state`].
pub(crate) struct GatewayMarkdownWriter {
    state: AppState,
}

impl GatewayMarkdownWriter {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl BlockMarkdownWriter for GatewayMarkdownWriter {
    fn write_markdown(
        &self,
        write: BlockMarkdownWrite,
    ) -> Pin<Box<dyn Future<Output = Result<BlockMarkdownWriteOutcome, QuarryError>> + Send + '_>>
    {
        Box::pin(async move {
            debug_assert_eq!(
                document_kind(&write.path, "text/markdown"),
                DocumentKind::BlockDocument,
                "adapters route only BlockDocuments through the writer"
            );
            write_markdown_reconciled(&self.state, write)
                .await
                .map_err(failure_to_quarry)
        })
    }
}

/// Maps gateway failures onto the adapters' `QuarryError` surface. Typed
/// content errors keep their identity (`UNSUPPORTED_MARKDOWN` stays the
/// typed unsupported-markdown error); everything else maps by status.
fn failure_to_quarry(failure: GatewayFailure) -> QuarryError {
    match failure {
        GatewayFailure::Typed(error) => match error.code() {
            GatewayErrorCode::UnsupportedMarkdown => QuarryError::UnsupportedMarkdown(
                quarry_collab_codec::Unsupported::new(error.message().to_string()),
            ),
            code => QuarryError::InvalidInput(format!("{}: {}", code.as_str(), error.message())),
        },
        GatewayFailure::Api(error) => match error.status() {
            StatusCode::NOT_FOUND => QuarryError::NotFound(error.message().to_string()),
            StatusCode::PRECONDITION_FAILED => {
                QuarryError::PreconditionFailed(error.message().to_string())
            }
            StatusCode::CONFLICT => QuarryError::Conflict(error.message().to_string()),
            StatusCode::SERVICE_UNAVAILABLE => QuarryError::Busy(error.message().to_string()),
            StatusCode::BAD_REQUEST => QuarryError::InvalidInput(error.message().to_string()),
            _ => QuarryError::Storage(error.message().to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_collab_codec::Attrs;

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|id| id.to_string()).collect()
    }

    fn insert_rows(id: &str, text: &str) -> Vec<BlockRow> {
        vec![BlockRow {
            block_id: id.to_string(),
            parent_block_id: None,
            position: 0,
            block_type: "p".to_string(),
            attrs: Attrs::new(),
            text: text.to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        }]
    }

    /// Applies gateway ops SEQUENTIALLY to a top-level id list — the gateway's
    /// own application semantics, used to verify the translation.
    fn apply_sequential(top_ids: &[String], ops: &[BlockOp]) -> Vec<String> {
        let mut order = top_ids.to_vec();
        for op in ops {
            match op {
                BlockOp::DeleteBlock { block_id } => order.retain(|id| id != block_id),
                BlockOp::MoveBlock {
                    block_id, position, ..
                } => {
                    order.retain(|id| id != block_id);
                    order.insert((*position as usize).min(order.len()), block_id.clone());
                }
                BlockOp::InsertBlock {
                    block_id: Some(block_id),
                    parent_block_id: None,
                    position,
                    ..
                } => order.insert((*position as usize).min(order.len()), block_id.clone()),
                _ => {}
            }
        }
        order
    }

    /// The case final-index placements get wrong under sequential
    /// application: an insert before a not-yet-detached moved block. The
    /// simulation must produce `[A, X, B, M]`, not `[X, A, B, M]`.
    #[test]
    fn sequential_ops_handles_an_insert_before_a_later_moved_block() {
        let top = ids(&["M", "A", "B"]);
        let ops = vec![
            ReconcileOp::InsertBlock {
                position: 1,
                rows: insert_rows("X", "inserted"),
            },
            ReconcileOp::MoveBlock {
                block_id: "M".to_string(),
                position: 3,
            },
        ];

        let translated = sequential_ops(&top, &ops);
        assert_eq!(
            apply_sequential(&top, &translated),
            ids(&["A", "X", "B", "M"])
        );
    }

    /// Move attribution may differ from the codec's designated mover; the
    /// resulting order and ids are identical (every move preserves identity).
    #[test]
    fn sequential_ops_reproduces_codec_moves_exactly() {
        let top = ids(&["t", "a", "b", "c"]);
        let ops = vec![ReconcileOp::MoveBlock {
            block_id: "b".to_string(),
            position: 3,
        }];

        let translated = sequential_ops(&top, &ops);
        assert_eq!(
            apply_sequential(&top, &translated),
            ids(&["t", "a", "c", "b"])
        );
    }

    #[test]
    fn sequential_ops_orders_deletes_before_placements_and_expands_subtrees() {
        let top = ids(&["a", "b"]);
        let mut rows = insert_rows("code", "");
        rows[0].block_type = "code_block".to_string();
        rows.push(BlockRow {
            block_id: "line".to_string(),
            parent_block_id: Some("code".to_string()),
            position: 0,
            block_type: "code_line".to_string(),
            attrs: Attrs::new(),
            text: "let x = 1;".to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        });
        let ops = vec![
            ReconcileOp::DeleteBlock {
                block_id: "b".to_string(),
            },
            ReconcileOp::InsertBlock { position: 1, rows },
        ];

        let translated = sequential_ops(&top, &ops);
        assert_eq!(apply_sequential(&top, &translated), ids(&["a", "code"]));
        let child = translated
            .iter()
            .find_map(|op| match op {
                BlockOp::InsertBlock {
                    block_id: Some(id),
                    parent_block_id: Some(parent),
                    position,
                    ..
                } => Some((id.clone(), parent.clone(), *position)),
                _ => None,
            })
            .expect("child insert present");
        assert_eq!(child, ("line".to_string(), "code".to_string(), 0));
    }

    #[test]
    fn finds_the_first_complete_conflict_hunk() {
        let markdown =
            "Intro.\n\n<<<<<<< HEAD\nOurs line.\n=======\nTheirs line.\n>>>>>>> feature\n\nOutro.\n";

        assert_eq!(
            first_conflict_marker_hunk(markdown).as_deref(),
            Some("<<<<<<< HEAD\nOurs line.\n=======\nTheirs line.\n>>>>>>> feature\n")
        );
    }

    #[test]
    fn detects_markers_in_crlf_content() {
        let markdown = "<<<<<<< HEAD\r\nOurs.\r\n=======\r\nTheirs.\r\n>>>>>>> main\r\n";

        assert_eq!(
            first_conflict_marker_hunk(markdown).as_deref(),
            Some("<<<<<<< HEAD\nOurs.\n=======\nTheirs.\n>>>>>>> main\n")
        );
    }

    // A paragraph followed by `=======` is a setext heading — ordinary
    // markdown, not a merge artifact.
    #[test]
    fn ignores_a_setext_heading_underline_without_the_other_markers() {
        assert_eq!(first_conflict_marker_hunk("Title\n=======\nBody.\n"), None);
    }

    #[test]
    fn ignores_an_unclosed_conflict_hunk() {
        assert_eq!(
            first_conflict_marker_hunk("<<<<<<< HEAD\nOurs.\n=======\nTheirs.\n"),
            None
        );
    }

    #[test]
    fn ignores_markers_out_of_order() {
        assert_eq!(
            first_conflict_marker_hunk(">>>>>>> feature\n=======\n<<<<<<< HEAD\n"),
            None
        );
    }
}
