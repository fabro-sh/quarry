#![cfg(feature = "lib-documents")]
#![allow(clippy::unwrap_used, reason = "tests use unwrap for HTTP fixtures")]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use quarry_core::DocumentSource;
use quarry_server::router;
use serde_json::Value;
use tokio::time::Duration;
use tower::ServiceExt;

mod common;

use common::{document_test_app, json_request, open_test_store, response_json};

fn assert_json_timestamp(value: &Value) {
    let timestamp = value.as_str().expect("timestamp should be a string");
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("timestamp should parse as RFC 3339");
}

#[tokio::test]
async fn agent_presence_records_status_by_document() {
    let (_root, store) = open_test_store().await;
    let library = store.create_library("presence").await.unwrap();
    let written = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("live.md").to_string(),
            content: b"hello".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("other.md").to_string(),
            content: b"other".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let other_library = store.create_library("presence-other").await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: other_library.slug.to_string(),
            path: ("live.md").to_string(),
            content: b"other library".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/presence/documents/live.md/presence")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", "agent-a")
                .body(Body::from(
                    serde_json::json!({"status":"thinking","by":"ai:codex"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["current"]["agentId"], "agent-a");
    assert_eq!(body["current"]["status"], "thinking");
    assert_eq!(body["current"]["by"], "ai:codex");
    assert_eq!(body["current"]["documentId"], written.document.id);
    assert_eq!(body["presence"].as_array().unwrap().len(), 1);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/presence/documents/other.md/presence")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", "agent-b")
                .body(Body::from(
                    serde_json::json!({"status":"reading","by":"ai:claude"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/presence-other/documents/live.md/presence")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", "agent-c")
                .body(Body::from(
                    serde_json::json!({"status":"waiting","by":"ai:codex"}).to_string(),
                ))
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
                .uri("/v1/libraries/presence/documents/live.md/presence")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let presence = body["presence"].as_array().unwrap();
    assert_eq!(presence.len(), 1);
    assert_eq!(presence[0]["agentId"], "agent-a");
    assert_eq!(presence[0]["path"], "live.md");
}

#[tokio::test]
async fn tmp_agent_presence_omits_capability_path() {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "tmp presence",
                "content_type": "text/markdown"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();
    let document_id = created["document"]["id"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/tmp/documents/{secret}/presence"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", "agent-tmp")
                .body(Body::from(
                    serde_json::json!({"status":"thinking","by":"ai:codex"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains(&secret));
    let presence: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(presence["current"]["documentId"], document_id);
    assert_eq!(presence["current"]["agentId"], "agent-tmp");
    assert_eq!(presence["current"]["status"], "thinking");
    assert_eq!(presence["current"]["by"], "ai:codex");
    assert_json_timestamp(&presence["current"]["updatedAt"]);
    assert!(presence["current"].get("path").is_none());
    assert!(presence["current"].get("library").is_none());
    assert!(presence["presence"][0].get("path").is_none());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/presence"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(!body.contains(&secret));
    let presence: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(presence["presence"].as_array().unwrap().len(), 1);
    assert_eq!(presence["presence"][0]["documentId"], document_id);
    assert_eq!(presence["presence"][0]["agentId"], "agent-tmp");
    assert!(presence["presence"][0].get("path").is_none());
    assert!(presence["presence"][0].get("library").is_none());
}

async fn presence_test_app(library: &str) -> (tempfile::TempDir, axum::Router) {
    let (root, store) = open_test_store().await;
    let library = store.create_library(library).await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("live.md").to_string(),
            content: b"hello".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    (root, router(store))
}

async fn post_presence(app: &axum::Router, library: &str, agent_id: &str, status: &str) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/v1/libraries/{library}/documents/live.md/presence"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", agent_id)
                .body(Body::from(
                    serde_json::json!({"status": status}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn list_presence(app: &axum::Router, library: &str) -> Vec<Value> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/{library}/documents/live.md/presence"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    body["presence"].as_array().unwrap().clone()
}

#[tokio::test(start_paused = true)]
async fn agent_presence_expires_after_ttl() {
    let (_root, app) = presence_test_app("presence-ttl").await;

    post_presence(&app, "presence-ttl", "agent-a", "thinking").await;
    assert_eq!(list_presence(&app, "presence-ttl").await.len(), 1);

    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(list_presence(&app, "presence-ttl").await.len(), 0);
}

#[tokio::test(start_paused = true)]
async fn agent_presence_repost_resets_ttl() {
    let (_root, app) = presence_test_app("presence-refresh").await;

    post_presence(&app, "presence-refresh", "agent-a", "thinking").await;
    tokio::time::advance(Duration::from_secs(40)).await;
    post_presence(&app, "presence-refresh", "agent-a", "acting").await;
    tokio::time::advance(Duration::from_secs(40)).await;

    let presence = list_presence(&app, "presence-refresh").await;
    assert_eq!(presence.len(), 1);
    assert_eq!(presence[0]["status"], "acting");
}

#[tokio::test(start_paused = true)]
async fn event_stream_presence_survives_disconnect_until_ttl() {
    let (_root, app) = presence_test_app("presence-stream").await;

    let stream = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/presence-stream/documents/live.md/events/stream")
                .header("X-Agent-Id", "agent-s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);

    let presence = list_presence(&app, "presence-stream").await;
    assert_eq!(presence.len(), 1);
    assert_eq!(presence[0]["agentId"], "agent-s");
    assert_eq!(presence[0]["status"], "waiting");

    // The held stream heartbeats presence past the TTL. The paused-clock
    // current-thread runtime only polls the heartbeat task at explicit yield
    // points, so advance in sub-TTL steps with yields in between.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(40)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(40)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(40)).await;
    tokio::task::yield_now().await;
    assert_eq!(list_presence(&app, "presence-stream").await.len(), 1);

    // Disconnecting only stops the heartbeat: burst readers and stream
    // reconnects must not flap presence. Expiry is TTL-only.
    drop(stream);
    assert_eq!(list_presence(&app, "presence-stream").await.len(), 1);

    // Let the runtime process the heartbeat abort so advancing the paused
    // clock cannot fire one more touch.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(list_presence(&app, "presence-stream").await.len(), 0);
}

#[tokio::test]
async fn document_read_with_agent_header_auto_joins_presence() {
    let (_root, app) = presence_test_app("presence-auto-join").await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/presence-auto-join/documents/live.md/blocks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(list_presence(&app, "presence-auto-join").await.len(), 0);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/presence-auto-join/documents/live.md/blocks")
                .header("X-Agent-Id", "agent-r")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let presence = list_presence(&app, "presence-auto-join").await;
    assert_eq!(presence.len(), 1);
    assert_eq!(presence[0]["agentId"], "agent-r");
    assert_eq!(presence[0]["status"], "waiting");
}

#[tokio::test(start_paused = true)]
async fn document_read_with_agent_header_refreshes_ttl_without_clobbering_status() {
    let (_root, app) = presence_test_app("presence-implicit").await;

    post_presence(&app, "presence-implicit", "agent-a", "acting").await;
    tokio::time::advance(Duration::from_secs(40)).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/presence-implicit/documents/live.md/blocks")
                .header("X-Agent-Id", "agent-a")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // The read refreshed the TTL (40s + 40s > 60s) without touching the
    // declared status.
    tokio::time::advance(Duration::from_secs(40)).await;
    let presence = list_presence(&app, "presence-implicit").await;
    assert_eq!(presence.len(), 1);
    assert_eq!(presence[0]["status"], "acting");

    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(list_presence(&app, "presence-implicit").await.len(), 0);
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_document_read_with_agent_header_auto_joins_presence() {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "tmp presence",
                "content_type": "text/markdown"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header("X-Agent-Id", "agent-tmp-reader")
                .body(Body::empty())
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
                .uri(format!("/v1/tmp/documents/{secret}/presence"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let presence: Value = response_json(response).await;
    assert_eq!(presence["presence"].as_array().unwrap().len(), 1);
    assert_eq!(presence["presence"][0]["agentId"], "agent-tmp-reader");
    assert_eq!(presence["presence"][0]["status"], "waiting");
}
