#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use anyhow::Context as _;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use quarry_storage::QuarryStore;
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::{capture_debug_logs, document_test_app, json_request, response_json};

async fn block_test_app() -> (tempfile::TempDir, axum::Router, QuarryStore) {
    let (root, app, store) = document_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug": "blocks"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    (root, app, store)
}

async fn put_block_markdown(app: &axum::Router, path: &str, body: &str) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/libraries/blocks/documents/{path}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn get_block_tree(app: &axum::Router, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/blocks/documents/{path}/blocks"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn post_block_transaction(
    app: &axum::Router,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/blocks/documents/{path}/transactions"),
            body,
        ))
        .await
        .unwrap();
    let status = response.status();
    (status, response_json(response).await)
}

async fn commit_block_transaction(app: &axum::Router, path: &str, body: Value) -> Value {
    let (status, ack) = post_block_transaction(app, path, body).await;
    assert_eq!(status, StatusCode::OK, "transaction failed: {ack}");
    ack
}

fn block_tx(client_tx_id: &str, ops: Value) -> Value {
    serde_json::json!({
        "client_tx_id": client_tx_id,
        "actor": {"kind": "agent", "id": "agent-1", "label": "Agent One"},
        "ops": ops
    })
}

async fn get_document_markdown(app: &axum::Router, path: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/blocks/documents/{path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

async fn get_block_review(app: &axum::Router, path: &str, include_resolved: bool) -> Value {
    let query = if include_resolved {
        "?includeResolved=1"
    } else {
        ""
    };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/blocks/documents/{path}/review{query}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

fn assert_typed_error(status: StatusCode, body: &Value, code: &str, retryable: bool) {
    assert_eq!(body["code"], code, "unexpected error body: {body}");
    assert_eq!(body["retryable"], retryable);
    assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));
    let expected = match code {
        "STALE_BASE" | "BLOCK_MOVE_CONFLICT" => StatusCode::PRECONDITION_FAILED,
        "BLOCK_DELETED" | "ANCHOR_NOT_FOUND" => StatusCode::NOT_FOUND,
        "INVALID_TRANSACTION" => StatusCode::BAD_REQUEST,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    assert_eq!(status, expected);
}

async fn raw_version_count(app: &axum::Router, path: &str) -> usize {
    raw_versions(app, path).await.as_array().unwrap().len()
}

async fn raw_versions(app: &axum::Router, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/blocks/documents/{path}/versions/raw"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

#[tokio::test]
async fn markdown_put_rejects_raw_downgrade_without_opt_in() -> anyhow::Result<()> {
    let (_root, app, store) = block_test_app().await;
    put_block_markdown(&app, "guide", "# Guide\n\nBody.\n").await;

    let request = Request::builder()
        .method(Method::PUT)
        .uri("/v1/libraries/blocks/documents/guide")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("raw body"))
        .context("build raw downgrade request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("send raw downgrade request")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .context("raw downgrade error should be a string")?
            .contains("Markdown block document")
    );

    let document = store
        .get_document("blocks", "guide")
        .await
        .context("load document after rejected raw downgrade")?;
    assert_eq!(document.version.content_type, "text/markdown");
    assert_eq!(
        String::from_utf8(document.content).context("decode preserved markdown content")?,
        "# Guide\n\nBody.\n"
    );

    let request = Request::builder()
        .method(Method::PUT)
        .uri("/v1/libraries/blocks/documents/guide")
        .header(header::CONTENT_TYPE, "text/plain")
        .header("x-quarry-allow-document-kind-change", "true")
        .body(Body::from("raw body"))
        .context("build explicit raw downgrade request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("send explicit raw downgrade request")?;
    assert_eq!(response.status(), StatusCode::OK);
    let document = store
        .get_document("blocks", "guide")
        .await
        .context("load document after explicit raw downgrade")?;
    assert_eq!(document.version.content_type, "text/plain");
    assert_eq!(document.content, b"raw body".to_vec());
    assert_eq!(
        store
            .load_block_tree(&document.id)
            .await
            .context("load block tree after explicit raw downgrade")?,
        Vec::<quarry_collab_codec::BlockRow>::new()
    );
    Ok(())
}

/// Phase 7: a version restore on a BlockDocument is a whole-file write
/// through the reconciler (the two-way degenerate merge), not a legacy byte
/// put -- the block projection survives (ids stable, anchors live) and the
/// content equals the restored version exactly.
#[tokio::test]
async fn version_restore_merges_through_the_gateway_preserving_ids_and_anchors()
-> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "undo.md", "# Title\n\nAlpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "undo.md").await;
    let restore_to = tree["document_clock"]
        .as_str()
        .context("block tree should include document clock")?
        .to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .context("block tree should include block array")?
        .iter()
        .map(|block| {
            block["block_id"]
                .as_str()
                .context("block should include block_id")
                .map(ToString::to_string)
        })
        .collect::<anyhow::Result<_>>()?;
    let original = get_document_markdown(&app, "undo.md").await;

    // A live anchor on the Title block and a later content edit to Alpha.
    commit_block_transaction(
        &app,
        "undo.md",
        block_tx(
            "tx-anchor-title",
            serde_json::json!([{
                "op": "comment.add", "block_id": ids[0], "start": 0, "end": 5, "body": "survive the restore"
            }]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "undo.md",
        block_tx(
            "tx-edit-alpha",
            serde_json::json!([{
                "op": "replace_block_content", "block_id": ids[1], "text": "Alpha, edited."
            }]),
        ),
    )
    .await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/blocks/documents/undo.md/versions/{restore_to}/restore"),
            serde_json::json!({}),
        ))
        .await
        .context("restore document version")?;
    assert_eq!(response.status(), StatusCode::OK);

    // The content is the restored version, as a NEW head.
    assert_eq!(get_document_markdown(&app, "undo.md").await, original);
    let restored = get_block_tree(&app, "undo.md").await;
    assert_ne!(restored["document_clock"], serde_json::json!(restore_to));
    let restored_ids: Vec<String> = restored["blocks"]
        .as_array()
        .context("restored block tree should include block array")?
        .iter()
        .map(|block| {
            block["block_id"]
                .as_str()
                .context("restored block should include block_id")
                .map(ToString::to_string)
        })
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(
        restored_ids, ids,
        "the restore merges through the reconciler instead of clearing the projection"
    );
    let review = get_block_review(&app, "undo.md", false).await;
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(review["comments"][0]["anchor"]["blockId"], ids[0].as_str());
    assert_eq!(review["conflicts"], serde_json::json!([]));
    Ok(())
}

#[tokio::test]
async fn conflict_items_persist_project_and_resolve_without_mutating_the_document()
-> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf.md", "Alpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "conf.md").await;
    let alpha_id = tree["blocks"][0]["block_id"]
        .as_str()
        .context("block tree should include the alpha block id")?
        .to_string();
    let markdown_before = get_document_markdown(&app, "conf.md").await;

    let ack = commit_block_transaction(
        &app,
        "conf.md",
        block_tx(
            "tx-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "after_block_id": alpha_id,
                "base_markdown": "Bravo, base.\n",
                "incoming_markdown": "Bravo, incoming edit.\n",
                "canonical_markdown": "Bravo.\n"
            }]),
        ),
    )
    .await;
    // The op never mutates the document: no changed blocks, content intact
    // (modulo the one-version commit).
    assert_eq!(ack["changed_block_ids"], serde_json::json!([]));
    assert_eq!(
        get_document_markdown(&app, "conf.md").await,
        markdown_before
    );

    let review = get_block_review(&app, "conf.md", false).await;
    let conflicts = review["conflicts"]
        .as_array()
        .context("review should include conflicts array")?;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["status"], "open");
    assert_eq!(
        conflicts[0]["afterBlockId"].as_str(),
        Some(alpha_id.as_str())
    );
    assert_eq!(conflicts[0]["baseMarkdown"], "Bravo, base.\n");
    assert_eq!(conflicts[0]["incomingMarkdown"], "Bravo, incoming edit.\n");
    assert_eq!(conflicts[0]["canonicalMarkdown"], "Bravo.\n");
    let conflict_id = conflicts[0]["id"]
        .as_str()
        .context("conflict should include id")?
        .to_string();

    // Conflicts resolve with the comment vocabulary; resolution never
    // mutates the document.
    commit_block_transaction(
        &app,
        "conf.md",
        block_tx(
            "tx-resolve-conflict",
            serde_json::json!([{ "op": "comment.resolve", "item_id": conflict_id }]),
        ),
    )
    .await;
    let open_review = get_block_review(&app, "conf.md", false).await;
    assert_eq!(
        open_review["conflicts"]
            .as_array()
            .context("open review should include conflicts array")?
            .len(),
        0
    );
    let full_review = get_block_review(&app, "conf.md", true).await;
    assert_eq!(full_review["conflicts"][0]["status"], "resolved");
    assert_eq!(
        get_document_markdown(&app, "conf.md").await,
        markdown_before
    );
    Ok(())
}

#[tokio::test]
async fn comment_edit_on_conflict_id_returns_anchor_not_found() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-edit.md", "Alpha.\n").await;

    commit_block_transaction(
        &app,
        "conf-edit.md",
        block_tx(
            "tx-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "base_markdown": "Alpha.\n",
                "incoming_markdown": "Incoming.\n",
                "canonical_markdown": "Alpha.\n"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-edit.md", false).await;
    let conflict_id = review["conflicts"][0]["id"]
        .as_str()
        .context("conflict should include id")?
        .to_string();

    let (status, body) = post_block_transaction(
        &app,
        "conf-edit.md",
        block_tx(
            "tx-edit-conflict",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": conflict_id,
                "body": "not a comment"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "ANCHOR_NOT_FOUND", false);
    Ok(())
}

/// Replies stay comment-only: `comment.reply` on a conflict item is
/// `ANCHOR_NOT_FOUND` (conflicts resolve/delete with the comment vocabulary
/// but cannot host threads).
#[tokio::test]
async fn comment_reply_on_a_conflict_item_is_anchor_not_found() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-reply.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "conf-reply.md").await;
    commit_block_transaction(
        &app,
        "conf-reply.md",
        block_tx(
            "tx-conflict-for-reply",
            serde_json::json!([{
                "op": "conflict.add",
                "incoming_markdown": "Hunk.\n"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-reply.md", false).await;
    let conflict_id = review["conflicts"][0]["id"]
        .as_str()
        .context("conflict should include id")?
        .to_string();

    let (status, body) = post_block_transaction(
        &app,
        "conf-reply.md",
        block_tx(
            "tx-reply-to-conflict",
            serde_json::json!([{
                "op": "comment.reply", "item_id": conflict_id, "body": "no threads here"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "ANCHOR_NOT_FOUND", false);
    Ok(())
}

#[tokio::test]
async fn document_start_conflicts_anchor_null_and_delete_dismisses_them() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-start.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "conf-start.md").await;

    commit_block_transaction(
        &app,
        "conf-start.md",
        block_tx(
            "tx-start-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "base_markdown": "Old heading.\n",
                "incoming_markdown": "New heading.\n",
                "canonical_markdown": ""
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-start.md", false).await;
    let conflict = &review["conflicts"][0];
    assert!(conflict["afterBlockId"].is_null());
    assert_eq!(conflict["canonicalMarkdown"], "");
    let conflict_id = conflict["id"]
        .as_str()
        .context("conflict should include id")?
        .to_string();

    // comment.delete removes the conflict row outright.
    commit_block_transaction(
        &app,
        "conf-start.md",
        block_tx(
            "tx-delete-conflict",
            serde_json::json!([{ "op": "comment.delete", "item_id": conflict_id }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-start.md", true).await;
    assert_eq!(
        review["conflicts"]
            .as_array()
            .context("review should include conflicts array")?
            .len(),
        0
    );
    Ok(())
}

#[tokio::test]
async fn conflict_add_requires_an_existing_attachment_block() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-missing.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "conf-missing.md").await;

    let (status, body) = post_block_transaction(
        &app,
        "conf-missing.md",
        block_tx(
            "tx-bad-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "after_block_id": "no-such-block",
                "incoming_markdown": "Hunk.\n"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "BLOCK_DELETED", false);
    Ok(())
}

#[tokio::test]
async fn markdown_put_merges_against_the_if_match_base_preserving_ids_and_anchors()
-> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    // The separator keeps the two edited regions apart: edits to ADJACENT
    // blocks (no stable block between them) are conflict-absorbed by design.
    put_block_markdown(
        &app,
        "merge.md",
        "# Title\n\nAlpha.\n\nSeparator.\n\nBravo.\n",
    )
    .await;
    let tree = get_block_tree(&app, "merge.md").await;
    let base_clock = tree["document_clock"]
        .as_str()
        .context("block tree should include document clock")?
        .to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .context("block tree should include block array")?
        .iter()
        .map(|block| {
            block["block_id"]
                .as_str()
                .context("block should include block_id")
                .map(ToString::to_string)
        })
        .collect::<anyhow::Result<_>>()?;
    let base_export = get_document_markdown(&app, "merge.md").await;

    // A live anchor on the Title block (untouched by either side).
    commit_block_transaction(
        &app,
        "merge.md",
        block_tx(
            "tx-anchor-title",
            serde_json::json!([{
                "op": "comment.add", "block_id": ids[0], "start": 0, "end": 5, "body": "keep me"
            }]),
        ),
    )
    .await;
    // Canonical edit to Alpha (a browser/agent write after the export).
    commit_block_transaction(
        &app,
        "merge.md",
        block_tx(
            "tx-canonical-alpha",
            serde_json::json!([{
                "op": "replace_block_content", "block_id": ids[1], "text": "Alpha, canonical."
            }]),
        ),
    )
    .await;

    // The external writer edits Bravo against the OLD export and PUTs with
    // the old clock.
    let incoming = base_export.replace("Bravo.", "Bravo, external.");
    assert_ne!(incoming, base_export);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/merge.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{base_clock}\""))
                .body(Body::from(incoming))
                .unwrap(),
        )
        .await
        .context("put stale markdown merge")?;
    assert_eq!(response.status(), StatusCode::OK);

    // Both sides landed; nothing conflicted.
    let merged = get_document_markdown(&app, "merge.md").await;
    assert_eq!(
        merged,
        "# Title\n\nAlpha, canonical.\n\nSeparator.\n\nBravo, external.\n"
    );
    let tree = get_block_tree(&app, "merge.md").await;
    let merged_ids: Vec<String> = tree["blocks"]
        .as_array()
        .context("merged block tree should include block array")?
        .iter()
        .map(|block| {
            block["block_id"]
                .as_str()
                .context("merged block should include block_id")
                .map(ToString::to_string)
        })
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(merged_ids, ids, "sibling block ids survive the file write");
    let review = get_block_review(&app, "merge.md", false).await;
    assert_eq!(
        review["conflicts"]
            .as_array()
            .context("review should include conflicts array")?
            .len(),
        0
    );
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(review["comments"][0]["anchor"]["blockId"], ids[0].as_str());
    Ok(())
}

#[tokio::test]
async fn markdown_put_overlapping_edits_become_conflict_review_items() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "clash.md", "# Title\n\nAlpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "clash.md").await;
    let base_clock = tree["document_clock"]
        .as_str()
        .context("block tree should include document clock")?
        .to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .context("block tree should include block array")?
        .iter()
        .map(|block| {
            block["block_id"]
                .as_str()
                .context("block should include block_id")
                .map(ToString::to_string)
        })
        .collect::<anyhow::Result<_>>()?;
    let base_export = get_document_markdown(&app, "clash.md").await;

    commit_block_transaction(
        &app,
        "clash.md",
        block_tx(
            "tx-canonical-bravo",
            serde_json::json!([{
                "op": "replace_block_content", "block_id": ids[2], "text": "Bravo, canonical."
            }]),
        ),
    )
    .await;

    let incoming = base_export.replace("Bravo.", "Bravo, external.");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/clash.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{base_clock}\""))
                .body(Body::from(incoming))
                .unwrap(),
        )
        .await
        .context("put overlapping stale markdown merge")?;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "conflicts never fail the write"
    );

    // Canonical side retained…
    assert_eq!(
        get_document_markdown(&app, "clash.md").await,
        "# Title\n\nAlpha.\n\nBravo, canonical.\n"
    );
    // …and the losing hunk rides in a conflict review item.
    let review = get_block_review(&app, "clash.md", false).await;
    let conflicts = review["conflicts"]
        .as_array()
        .context("review should include conflicts array")?;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["afterBlockId"].as_str(), Some(ids[1].as_str()));
    assert_eq!(conflicts[0]["incomingMarkdown"], "Bravo, external.\n");
    assert_eq!(conflicts[0]["baseMarkdown"], "Bravo.\n");
    assert_eq!(conflicts[0]["canonicalMarkdown"], "Bravo, canonical.\n");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn whole_file_writes_log_a_reconcile_outcome() {
    let (logs, _guard) = capture_debug_logs();
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "obs.md", "Alpha.\n").await;
    logs.clear();

    put_block_markdown(&app, "obs.md", "Alpha changed.\n").await;

    let output = logs.output();
    assert!(
        output.contains("document.block_write.reconciled"),
        "reconciled writes should log their outcome:\n{output}"
    );
    assert!(
        output.contains("result=merged"),
        "the outcome log should classify the merge:\n{output}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_reconciles_warn_about_lcs_degradation() {
    let (logs, _guard) = capture_debug_logs();
    let (_root, app, _store) = block_test_app().await;
    // 1030² changed-middle cells exceed the 2^20 LCS budget; every block
    // differs so prefix/suffix trimming cannot shrink the matrix.
    let base: String = (0..1030)
        .map(|index| format!("Base {index}.\n\n"))
        .collect();
    put_block_markdown(&app, "big.md", &base).await;
    logs.clear();

    let incoming: String = (0..1030)
        .map(|index| format!("Incoming {index}.\n\n"))
        .collect();
    put_block_markdown(&app, "big.md", &incoming).await;

    let output = logs.output();
    assert!(
        output.contains("document.block_write.lcs_degraded"),
        "degraded reconciles should warn:\n{output}"
    );
}

const CONFLICT_MARKER_SOUP: &str =
    "<<<<<<< HEAD\nOurs line.\n=======\nTheirs line.\n>>>>>>> feature\n";

#[tokio::test]
async fn markdown_put_with_conflict_markers_flags_a_review_item() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "soup.md", "Alpha.\n").await;

    put_block_markdown(
        &app,
        "soup.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;

    let markdown = get_document_markdown(&app, "soup.md").await;
    assert_ne!(markdown, "Alpha.\n", "the soup write still committed");
    let review = get_block_review(&app, "soup.md", false).await;
    let conflicts = review["conflicts"]
        .as_array()
        .context("review should include conflicts array")?;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["status"], "open");
    assert_eq!(conflicts[0]["incomingMarkdown"], CONFLICT_MARKER_SOUP);
    assert!(conflicts[0]["afterBlockId"].is_null());
    Ok(())
}

#[tokio::test]
async fn first_import_with_conflict_markers_flags_a_review_item() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;

    put_block_markdown(
        &app,
        "soup-new.md",
        &format!("# Notes\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;

    let review = get_block_review(&app, "soup-new.md", false).await;
    let conflicts = review["conflicts"]
        .as_array()
        .context("review should include conflicts array")?;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["incomingMarkdown"], CONFLICT_MARKER_SOUP);
    Ok(())
}

#[tokio::test]
async fn unchanged_conflict_markers_do_not_stack_flags() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "soup-again.md", "Alpha.\n").await;
    put_block_markdown(
        &app,
        "soup-again.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;

    put_block_markdown(
        &app,
        "soup-again.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}\nMore prose.\n"),
    )
    .await;

    let review = get_block_review(&app, "soup-again.md", true).await;
    assert_eq!(
        review["conflicts"]
            .as_array()
            .context("review should include conflicts array")?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn dismissed_conflict_marker_flags_stay_dismissed() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "soup-dismissed.md", "Alpha.\n").await;
    put_block_markdown(
        &app,
        "soup-dismissed.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;
    let review = get_block_review(&app, "soup-dismissed.md", false).await;
    let conflict_id = review["conflicts"][0]["id"].as_str().unwrap().to_string();
    commit_block_transaction(
        &app,
        "soup-dismissed.md",
        block_tx(
            "tx-dismiss-soup",
            serde_json::json!([{ "op": "comment.resolve", "item_id": conflict_id }]),
        ),
    )
    .await;

    put_block_markdown(
        &app,
        "soup-dismissed.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}\nMore prose.\n"),
    )
    .await;

    let open_review = get_block_review(&app, "soup-dismissed.md", false).await;
    assert_eq!(open_review["conflicts"].as_array().unwrap().len(), 0);
    let full_review = get_block_review(&app, "soup-dismissed.md", true).await;
    assert_eq!(full_review["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(full_review["conflicts"][0]["status"], "resolved");
}

#[tokio::test]
async fn byte_identical_markdown_put_commits_no_new_version() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "noop.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "noop.md").await; // materialize + normalize
    let content = get_document_markdown(&app, "noop.md").await;
    let versions_before = raw_version_count(&app, "noop.md").await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/noop.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from(content.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let outcome = response_json(response).await;
    assert!(outcome["version"]["id"].is_string());
    assert_eq!(raw_version_count(&app, "noop.md").await, versions_before);
    assert_eq!(get_document_markdown(&app, "noop.md").await, content);
}

#[tokio::test]
async fn markdown_put_with_critic_markup_fails_typed_unsupported() {
    let (_root, app, _store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/critic.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("Some {++inserted++} text.\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_typed_error(status, &body, "UNSUPPORTED_MARKDOWN", false);
}
