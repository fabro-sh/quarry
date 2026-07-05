#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use anyhow::Context as _;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::{capture_debug_logs, document_test_app, json_request, response_json};

#[tokio::test]
async fn rest_api_attaches_and_preserves_request_ids() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .body(Body::empty())
                .context("build first health check request")?,
        )
        .await
        .context("send first health check request")?;
    assert_eq!(first.status(), StatusCode::OK);
    let first_id = first
        .headers()
        .get("x-quarry-request-id")
        .context("response should include a generated request id")?
        .to_str()
        .context("generated request id should be valid header text")?
        .to_string();
    uuid::Uuid::parse_str(&first_id).context("generated request id should be a UUID")?;

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .body(Body::empty())
                .context("build second health check request")?,
        )
        .await
        .context("send second health check request")?;
    let second_id = second
        .headers()
        .get("x-quarry-request-id")
        .context("response should include a generated request id")?
        .to_str()
        .context("generated request id should be valid header text")?
        .to_string();
    uuid::Uuid::parse_str(&second_id).context("generated request id should be a UUID")?;
    assert_ne!(first_id, second_id);

    let supplied = "req-from-client";
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .header("x-quarry-request-id", supplied)
                .body(Body::empty())
                .context("build health check request with supplied request id")?,
        )
        .await
        .context("send health check request with supplied request id")?;
    assert_eq!(response.headers()["x-quarry-request-id"], supplied);
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test(flavor = "current_thread")]
async fn request_tracing_redacts_tmp_capability_paths_without_redacting_library_paths()
-> anyhow::Result<()> {
    let (logs, _guard) = capture_debug_logs();
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
        .context("create tmp document for request log redaction test")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("tmp create response should include document path")?
        .to_string();

    logs.clear();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/presence"))
                .body(Body::empty())
                .context("build tmp presence request for request log redaction test")?,
        )
        .await
        .context("send tmp presence request for request log redaction test")?;
    assert_eq!(response.status(), StatusCode::OK);
    let output = logs.output();
    assert!(
        !output.contains(&secret),
        "request logs must not contain tmp secret:\n{output}"
    );
    assert!(
        output.contains("<tmp-secret>") || output.contains("/v1/tmp/documents/{*path}"),
        "request logs should retain useful route context:\n{output}"
    );

    let library_secret_like_path = "0123456789abcdefABCDEF0123456789";
    logs.clear();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/missing/documents/{library_secret_like_path}"
                ))
                .body(Body::empty())
                .context("build library missing-document request for request log test")?,
        )
        .await
        .context("send library missing-document request for request log test")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let output = logs.output();
    assert!(
        output.contains(&format!(
            "/v1/libraries/missing/documents/{library_secret_like_path}"
        )),
        "ordinary library paths should remain visible in request logs:\n{output}"
    );
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test(flavor = "current_thread")]
async fn tmp_sse_logging_redacts_capability_path() {
    let (logs, _guard) = capture_debug_logs();
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "tmp stream",
                "content_type": "text/markdown"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();
    let document_id = created["document"]["id"].as_str().unwrap().to_string();

    logs.clear();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/events/stream"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let output = logs.output();
    assert!(
        !output.contains(&secret),
        "tmp SSE logs must not contain tmp secret:\n{output}"
    );
    assert!(
        output.contains("sse.stream.opened"),
        "tmp SSE open event should still be logged:\n{output}"
    );
    assert!(
        output.contains("scope=tmp") && output.contains(&document_id),
        "tmp SSE logs should keep scope and document id diagnostics:\n{output}"
    );
}
