#![cfg(feature = "tmp-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap in shared HTTP and CRDT fixtures"
)]

use anyhow::Context as _;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::{document_test_app, response_json};

async fn assert_error(
    app: &axum::Router,
    request: Request<Body>,
    status: StatusCode,
    code: &str,
    retryable: bool,
) -> anyhow::Result<()> {
    let response = app.clone().oneshot(request).await?;
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body: Value = response_json(response).await;
    assert_eq!(body["code"], code);
    assert_eq!(body["retryable"], retryable);
    assert!(body["message"].as_str().is_some());
    assert_eq!(body.as_object().map(serde_json::Map::len), Some(3));
    Ok(())
}

#[tokio::test]
async fn framework_and_routing_failures_use_the_api_error_envelope() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    assert_error(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/v1/no-such-route")
            .body(Body::empty())
            .context("build unknown-route request")?,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        false,
    )
    .await?;

    assert_error(
        &app,
        Request::builder()
            .method(Method::POST)
            .uri("/v1/health")
            .body(Body::empty())
            .context("build method-not-allowed request")?,
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        false,
    )
    .await?;

    assert_error(
        &app,
        Request::builder()
            .method(Method::POST)
            .uri("/v1/tmp/documents")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{"))
            .context("build malformed-json request")?,
        StatusCode::BAD_REQUEST,
        "INVALID_REQUEST",
        false,
    )
    .await?;

    assert_error(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/v1/tmp/documents/missing/review?includeResolved=maybe")
            .body(Body::empty())
            .context("build invalid-query request")?,
        StatusCode::BAD_REQUEST,
        "INVALID_REQUEST",
        false,
    )
    .await?;

    let oversized = format!(r#"{{"content":"{}"}}"#, "x".repeat(2 * 1024 * 1024));
    assert_error(
        &app,
        Request::builder()
            .method(Method::POST)
            .uri("/v1/tmp/documents")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(oversized))
            .context("build oversized request")?,
        StatusCode::PAYLOAD_TOO_LARGE,
        "PAYLOAD_TOO_LARGE",
        false,
    )
    .await?;

    Ok(())
}
