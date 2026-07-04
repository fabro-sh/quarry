#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

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
async fn conflict_items_persist_project_and_resolve_without_mutating_the_document() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf.md", "Alpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "conf.md").await;
    let alpha_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();
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
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["status"], "open");
    assert_eq!(
        conflicts[0]["afterBlockId"].as_str(),
        Some(alpha_id.as_str())
    );
    assert_eq!(conflicts[0]["baseMarkdown"], "Bravo, base.\n");
    assert_eq!(conflicts[0]["incomingMarkdown"], "Bravo, incoming edit.\n");
    assert_eq!(conflicts[0]["canonicalMarkdown"], "Bravo.\n");
    let conflict_id = conflicts[0]["id"].as_str().unwrap().to_string();

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
    assert_eq!(open_review["conflicts"].as_array().unwrap().len(), 0);
    let full_review = get_block_review(&app, "conf.md", true).await;
    assert_eq!(full_review["conflicts"][0]["status"], "resolved");
    assert_eq!(
        get_document_markdown(&app, "conf.md").await,
        markdown_before
    );
}

#[tokio::test]
async fn comment_edit_on_conflict_id_returns_anchor_not_found() {
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
    let conflict_id = review["conflicts"][0]["id"].as_str().unwrap().to_string();

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
}

#[tokio::test]
async fn document_start_conflicts_anchor_null_and_delete_dismisses_them() {
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
    let conflict_id = conflict["id"].as_str().unwrap().to_string();

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
    assert_eq!(review["conflicts"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn conflict_add_requires_an_existing_attachment_block() {
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
}

#[tokio::test]
async fn markdown_put_merges_against_the_if_match_base_preserving_ids_and_anchors() {
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
    let base_clock = tree["document_clock"].as_str().unwrap().to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
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
        .unwrap();
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
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(merged_ids, ids, "sibling block ids survive the file write");
    let review = get_block_review(&app, "merge.md", false).await;
    assert_eq!(review["conflicts"].as_array().unwrap().len(), 0);
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(review["comments"][0]["anchor"]["blockId"], ids[0].as_str());
}

#[tokio::test]
async fn markdown_put_overlapping_edits_become_conflict_review_items() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "clash.md", "# Title\n\nAlpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "clash.md").await;
    let base_clock = tree["document_clock"].as_str().unwrap().to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
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
        .unwrap();
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
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["afterBlockId"].as_str(), Some(ids[1].as_str()));
    assert_eq!(conflicts[0]["incomingMarkdown"], "Bravo, external.\n");
    assert_eq!(conflicts[0]["baseMarkdown"], "Bravo.\n");
    assert_eq!(conflicts[0]["canonicalMarkdown"], "Bravo, canonical.\n");
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
async fn markdown_put_with_conflict_markers_flags_a_review_item() {
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
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["status"], "open");
    assert_eq!(conflicts[0]["incomingMarkdown"], CONFLICT_MARKER_SOUP);
    assert!(conflicts[0]["afterBlockId"].is_null());
}

#[tokio::test]
async fn first_import_with_conflict_markers_flags_a_review_item() {
    let (_root, app, _store) = block_test_app().await;

    put_block_markdown(
        &app,
        "soup-new.md",
        &format!("# Notes\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;

    let review = get_block_review(&app, "soup-new.md", false).await;
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["incomingMarkdown"], CONFLICT_MARKER_SOUP);
}

#[tokio::test]
async fn unchanged_conflict_markers_do_not_stack_flags() {
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
    assert_eq!(review["conflicts"].as_array().unwrap().len(), 1);
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
