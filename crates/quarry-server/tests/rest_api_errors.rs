#![cfg(feature = "tmp-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap in shared HTTP and CRDT fixtures"
)]

use anyhow::Context as _;
use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use quarry_server::{ClientIpSource, ServerConfig, app_state_with_config, router_with_state};
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

#[tokio::test]
async fn trusted_client_ip_mode_requires_cloudfront_viewer_address() -> anyhow::Result<()> {
    let (_root, store) = common::open_test_store().await;
    let app = router_with_state(app_state_with_config(
        store,
        ServerConfig {
            client_ip_source: ClientIpSource::CloudFrontViewerAddress,
        },
    ));

    assert_error(
        &app,
        Request::builder()
            .method(Method::POST)
            .uri("/v1/tmp/documents")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"content":"missing address"}"#))
            .context("build request without trusted client address")?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
        false,
    )
    .await?;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/tmp/documents")
                .header(header::CONTENT_TYPE, "application/json")
                .header("cloudfront-viewer-address", "198.51.100.10:46532")
                .body(Body::from(r#"{"content":"trusted address"}"#))
                .context("build request with trusted client address")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response_json(response).await;
    assert!(body["document"]["created_ip_address"].is_null());
    assert!(!body.to_string().contains("198.51.100.10"));

    let mut repeated = Request::builder()
        .method(Method::POST)
        .uri("/v1/tmp/documents")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"content":"repeated address"}"#))
        .context("build request with repeated trusted client address")?;
    repeated.headers_mut().append(
        "cloudfront-viewer-address",
        HeaderValue::from_static("198.51.100.10:46532"),
    );
    repeated.headers_mut().append(
        "cloudfront-viewer-address",
        HeaderValue::from_static("198.51.100.11:46533"),
    );
    assert_error(
        &app,
        repeated,
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test]
async fn default_client_ip_mode_ignores_forwarding_headers() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/tmp/documents")
                .header(header::CONTENT_TYPE, "application/json")
                .header("cloudfront-viewer-address", "not-a-socket-address")
                .header("x-forwarded-for", "203.0.113.9")
                .body(Body::from(r#"{"content":"local"}"#))
                .context("build local-mode request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    Ok(())
}
