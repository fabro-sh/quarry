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

use common::{document_test_app, json_request, response_json};

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
