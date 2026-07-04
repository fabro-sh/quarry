#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use quarry_storage::{QuarryStore, StoreEvent, StoreEventKind};
use serde_json::Value;
use tokio::time::{Duration, timeout};
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

async fn raw_version_count(app: &axum::Router, path: &str) -> usize {
    raw_versions(app, path).await.as_array().unwrap().len()
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

async fn next_document_put_event(
    events: &mut tokio::sync::broadcast::Receiver<StoreEvent>,
) -> StoreEvent {
    timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind() == StoreEventKind::DocumentPut {
                break event;
            }
        }
    })
    .await
    .unwrap()
}

fn assert_json_uuid(value: &Value) {
    let id = value.as_str().expect("id should be a string");
    uuid::Uuid::parse_str(id).expect("id should parse as a UUID");
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
async fn blocks_route_materializes_rows_with_stable_ids() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "# Title\n\nBody one.\n").await;

    let first = get_block_tree(&app, "doc.md").await;
    let blocks = first["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["block_type"], "h1");
    assert_eq!(blocks[0]["text"], "Title");
    assert_eq!(blocks[1]["block_type"], "p");
    assert_eq!(blocks[1]["text"], "Body one.");
    assert_json_uuid(&first["document_clock"]);

    // A second read returns the same persisted ids and clock: the lazy
    // materialization happened exactly once.
    let second = get_block_tree(&app, "doc.md").await;
    assert_eq!(second, first);
}

#[tokio::test]
async fn block_routes_reject_raw_documents_with_a_typed_error() {
    let (_root, app, _store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/image.png")
                .header(header::CONTENT_TYPE, "image/png")
                .body(Body::from(vec![0x89u8, 0x50, 0x4e, 0x47]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/blocks/documents/image.png/blocks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_typed_error(status, &body, "UNSUPPORTED_BLOCK_DOCUMENT", false);

    let (status, body) = post_block_transaction(
        &app,
        "image.png",
        block_tx(
            "tx-raw",
            serde_json::json!([{ "op": "delete_block", "block_id": "x" }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "UNSUPPORTED_BLOCK_DOCUMENT", false);
}

#[tokio::test]
async fn block_transaction_insert_block_commits_one_version_and_emits_events() {
    let (_root, app, store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "First.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let versions_before = raw_version_count(&app, "doc.md").await;
    let mut events = store.subscribe_events();

    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-insert",
            serde_json::json!([{
                "op": "insert_block",
                "position": 1,
                "block_type": "p",
                "text": "Second."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["status"], "committed");
    assert_eq!(ack["changed_block_ids"].as_array().unwrap().len(), 1);
    assert_json_uuid(&ack["transaction_id"]);
    let clock = ack["document_clock"].as_str().unwrap();
    assert_ne!(clock, tree["document_clock"].as_str().unwrap());

    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "First.\n\nSecond.\n"
    );
    assert_eq!(raw_version_count(&app, "doc.md").await, versions_before + 1);

    let event = next_document_put_event(&mut events).await;
    assert_eq!(event.version_id(), Some(clock));
    assert_eq!(event.path(), Some("doc.md"));
}

#[tokio::test]
async fn block_transaction_replace_block_content_preserves_block_identity() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Original text.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-replace",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Rewritten text."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["changed_block_ids"], serde_json::json!([block_id]));

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], block_id.as_str());
    assert_eq!(after["blocks"][0]["text"], "Rewritten text.");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Rewritten text.\n"
    );
}

#[tokio::test]
async fn block_transaction_move_block_is_placement_only() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Alpha.\n\nBeta.\n\nGamma.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let gamma = tree["blocks"][2]["block_id"].as_str().unwrap().to_string();
    let alpha = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-move",
            serde_json::json!([{
                "op": "move_block",
                "block_id": gamma,
                "position": 0
            }]),
        ),
    )
    .await;

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], gamma.as_str());
    assert_eq!(after["blocks"][0]["text"], "Gamma.");
    assert_eq!(after["blocks"][1]["block_id"], alpha.as_str());
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Gamma.\n\nAlpha.\n\nBeta.\n"
    );
}

#[tokio::test]
async fn block_transaction_set_block_type_preserves_identity_text_and_anchors() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Heading soon.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 0,
                "end": 7,
                "body": "anchored before the type change"
            }]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-type",
            serde_json::json!([{
                "op": "set_block_type",
                "block_id": block_id,
                "block_type": "h2"
            }]),
        ),
    )
    .await;

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], block_id.as_str());
    assert_eq!(after["blocks"][0]["block_type"], "h2");
    assert_eq!(after["blocks"][0]["text"], "Heading soon.");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "## Heading soon.\n"
    );
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["comments"][0]["anchor"]["blockId"],
        block_id.as_str()
    );
    assert_eq!(review["comments"][0]["anchor"]["startOffset"], 0);
    assert_eq!(review["comments"][0]["anchor"]["endOffset"], 7);
    assert_eq!(review["comments"][0]["status"], "open");
}

#[tokio::test]
async fn block_transaction_set_block_attrs_edits_raw_markdown_blocks() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "<div>\nopaque\n</div>\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    assert_eq!(tree["blocks"][0]["block_type"], "raw_markdown");
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-attrs",
            serde_json::json!([{
                "op": "set_block_attrs",
                "block_id": block_id,
                "attrs": {"markdown": "<section>\nreplaced\n</section>"}
            }]),
        ),
    )
    .await;

    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "<section>\nreplaced\n</section>\n"
    );
    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], block_id.as_str());
}

#[tokio::test]
async fn block_transaction_marks_and_links_render_in_markdown() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Bold and linked words.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-format",
            serde_json::json!([
                {"op": "add_mark", "block_id": block_id, "start": 0, "end": 4, "marks": {"bold": true}},
                {"op": "set_link", "block_id": block_id, "start": 9, "end": 15, "url": "https://example.com"}
            ]),
        ),
    )
    .await;
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "**Bold** and [linked](https://example.com) words.\n"
    );

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-unformat",
            serde_json::json!([
                {"op": "remove_mark", "block_id": block_id, "start": 0, "end": 4, "marks": ["bold"]},
                {"op": "set_link", "block_id": block_id, "start": 9, "end": 15, "url": null}
            ]),
        ),
    )
    .await;
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Bold and linked words.\n"
    );
}

#[tokio::test]
async fn block_transaction_comment_lifecycle_projects_from_rows() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "why this?"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let comment = &review["comments"][0];
    assert_eq!(comment["status"], "open");
    assert_eq!(comment["body"], "why this?");
    assert_eq!(comment["quote"], "this");
    assert_eq!(comment["by"], "Agent One");
    assert_eq!(comment["ref"]["ordinal"], 0);
    assert_eq!(comment["anchor"]["blockId"], block_id.as_str());
    let comment_id = comment["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": comment_id,
                "body": "because reasons"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["comments"][0]["replies"][0]["body"],
        "because reasons"
    );

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-resolve",
            serde_json::json!([{ "op": "comment.resolve", "item_id": comment_id }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert!(review["comments"].as_array().unwrap().is_empty());
    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["comments"][0]["status"], "resolved");

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-delete-comment",
            serde_json::json!([{ "op": "comment.delete", "item_id": comment_id }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", true).await;
    assert!(review["comments"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn block_transaction_comment_edit_updates_body_and_edited_at() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "original note"
            }]),
        ),
    )
    .await;
    let before = get_block_review(&app, "doc.md", false).await;
    let comment = &before["comments"][0];
    assert!(comment["editedAt"].is_null());
    let comment_id = comment["id"].as_str().unwrap().to_string();
    let created_at = comment["at"].as_str().unwrap().to_string();
    let anchor = comment["anchor"].clone();
    let quote = comment["quote"].clone();

    tokio::time::sleep(Duration::from_millis(2)).await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-comment",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": comment_id,
                "body": "edited note"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let edited = &review["comments"][0];
    assert_eq!(edited["body"], "edited note");
    assert_eq!(edited["at"], created_at);
    assert_ne!(edited["editedAt"], Value::Null);
    assert_ne!(edited["editedAt"], edited["at"]);
    assert_eq!(edited["anchor"], anchor);
    assert_eq!(edited["quote"], quote);
}

#[tokio::test]
async fn block_transaction_comment_edit_updates_reply_without_changing_root() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "root note"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let root_id = review["comments"][0]["id"].as_str().unwrap().to_string();
    let root_at = review["comments"][0]["at"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": root_id,
                "body": "reply note"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let reply_id = review["comments"][0]["replies"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reply_at = review["comments"][0]["replies"][0]["at"]
        .as_str()
        .unwrap()
        .to_string();

    tokio::time::sleep(Duration::from_millis(2)).await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-reply",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": reply_id,
                "body": "edited reply"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let root = &review["comments"][0];
    let reply = &root["replies"][0];
    assert_eq!(root["body"], "root note");
    assert_eq!(root["at"], root_at);
    assert!(root["editedAt"].is_null());
    assert_eq!(reply["body"], "edited reply");
    assert_eq!(reply["at"], reply_at);
    assert_ne!(reply["editedAt"], Value::Null);
    assert_ne!(reply["editedAt"], reply["at"]);
}

#[tokio::test]
async fn block_transaction_comment_reply_targets_open_suggestion_and_edit_updates_reply() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Make this better.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 10,
                "end": 16,
                "replacement": "great"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion_id = review["suggestions"][0]["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "why this wording?"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert!(review["comments"].as_array().unwrap().is_empty());
    let reply = &review["suggestions"][0]["replies"][0];
    assert_eq!(reply["body"], "why this wording?");
    assert_eq!(reply["status"], "open");
    let reply_id = reply["id"].as_str().unwrap().to_string();
    let reply_at = reply["at"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-nested-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": reply_id,
                "body": "second reply"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let replies = review["suggestions"][0]["replies"].as_array().unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["body"], "second reply");

    tokio::time::sleep(Duration::from_millis(2)).await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-reply",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": reply_id,
                "body": "edited wording question"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let reply = &review["suggestions"][0]["replies"][0];
    assert_eq!(reply["body"], "edited wording question");
    assert_eq!(reply["at"], reply_at);
    assert_ne!(reply["editedAt"], Value::Null);
    assert_ne!(reply["editedAt"], reply["at"]);
}

#[tokio::test]
async fn block_transaction_comment_edit_rejects_non_open_comments() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "root note"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let comment_id = review["comments"][0]["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-resolve",
            serde_json::json!([{ "op": "comment.resolve", "item_id": comment_id }]),
        ),
    )
    .await;

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-resolved",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": comment_id,
                "body": "should not land"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["comments"][0]["body"], "root note");
}

#[tokio::test]
async fn block_transaction_suggestion_accept_applies_replacement_and_resolves() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Make this better.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 10,
                "end": 16,
                "replacement": "great"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion = &review["suggestions"][0];
    assert_eq!(suggestion["status"], "open");
    assert_eq!(suggestion["kind"], "replace");
    assert_eq!(suggestion["preview"]["before"], "better");
    assert_eq!(suggestion["preview"]["after"], "great");
    let suggestion_id = suggestion["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "why this replacement?"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["suggestions"][0]["replies"][0]["body"],
        "why this replacement?"
    );

    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-accept",
            serde_json::json!([{ "op": "suggestion.accept", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_eq!(
        ack["changed_block_ids"],
        serde_json::json!([block_id.as_str()])
    );
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Make this great.\n"
    );
    let review = get_block_review(&app, "doc.md", false).await;
    assert!(review["suggestions"].as_array().unwrap().is_empty());
    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["suggestions"][0]["status"], "resolved");
    assert!(
        review["suggestions"][0]["replies"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Accepting again: already resolved.
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-accept-again",
            serde_json::json!([{ "op": "suggestion.accept", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "SUGGESTION_ALREADY_RESOLVED", false);
}

#[tokio::test]
async fn block_transaction_suggestion_reject_resolves_without_changing_text() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Keep this text.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 0,
                "end": 4,
                "replacement": "Drop"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion_id = review["suggestions"][0]["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "please explain"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["suggestions"][0]["replies"][0]["body"],
        "please explain"
    );

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reject",
            serde_json::json!([{ "op": "suggestion.reject", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Keep this text.\n"
    );
    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["suggestions"][0]["status"], "resolved");
    assert!(
        review["suggestions"][0]["replies"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply-resolved",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "too late"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);
}

#[tokio::test]
async fn replace_block_content_orphans_overlapping_comments_and_shifts_suffix_anchors() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "prefix MIDDLE suffix\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    // One comment on the doomed middle, one on the surviving suffix.
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-anchors",
            serde_json::json!([
                {"op": "comment.add", "block_id": block_id, "start": 7, "end": 13, "body": "on middle"},
                {"op": "comment.add", "block_id": block_id, "start": 14, "end": 20, "body": "on suffix"}
            ]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-rewrite",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "prefix REWRITTEN-CENTER suffix"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let comments = review["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    let on_middle = comments
        .iter()
        .find(|comment| comment["body"] == "on middle")
        .unwrap();
    let on_suffix = comments
        .iter()
        .find(|comment| comment["body"] == "on suffix")
        .unwrap();
    // The overlapping comment orphaned and collapsed at the change site.
    assert_eq!(on_middle["status"], "orphaned");
    assert_eq!(
        on_middle["anchor"]["startOffset"],
        on_middle["anchor"]["endOffset"]
    );
    // The suffix comment survived with shifted offsets ("suffix" moved +10).
    assert_eq!(on_suffix["status"], "open");
    assert_eq!(on_suffix["anchor"]["startOffset"], 24);
    assert_eq!(on_suffix["anchor"]["endOffset"], 30);
}

#[tokio::test]
async fn suggestion_invalidated_by_a_content_change_cannot_be_accepted() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Suggest on this span.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 11,
                "end": 15,
                "replacement": "that"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion_id = review["suggestions"][0]["id"].as_str().unwrap().to_string();

    // Rewrite the anchored span out from under the suggestion.
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-rewrite",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Suggest on changed span."
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(review["suggestions"][0]["status"], "invalidated");

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-accept",
            serde_json::json!([{ "op": "suggestion.accept", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "SUGGESTION_INVALIDATED", false);
}

#[tokio::test]
async fn delete_block_orphans_comments_and_invalidates_suggestions() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Doomed block.\n\nSurvivor.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let doomed = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-anchors",
            serde_json::json!([
                {"op": "comment.add", "block_id": doomed, "start": 0, "end": 6, "body": "note"},
                {"op": "suggestion.add", "block_id": doomed, "start": 0, "end": 6, "replacement": "Saved"}
            ]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-delete",
            serde_json::json!([{ "op": "delete_block", "block_id": doomed }]),
        ),
    )
    .await;

    assert_eq!(get_document_markdown(&app, "doc.md").await, "Survivor.\n");
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(review["comments"][0]["status"], "orphaned");
    assert_eq!(review["suggestions"][0]["status"], "invalidated");
}
