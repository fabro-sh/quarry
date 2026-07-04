#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::Body;
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

fn block_tx(client_tx_id: &str, ops: Value) -> Value {
    serde_json::json!({
        "client_tx_id": client_tx_id,
        "actor": {"kind": "agent", "id": "agent-1", "label": "Agent One"},
        "ops": ops
    })
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
