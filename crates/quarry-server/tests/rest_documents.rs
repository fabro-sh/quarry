#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::{document_test_app, json_request, response_json};

#[tokio::test]
async fn rest_api_supports_documents_transactions_etags_and_openapi() {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"alpha"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("one"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    let document_id = body["document"]["id"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .header(header::IF_MATCH, "\"wrong\"")
                .body(Body::from("bad"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        response.headers()["x-quarry-document-id"],
        document_id.as_str()
    );
    // Markdown PUTs land via the Phase 4 reconciled write: content is the
    // deterministic normalized export (trailing newline), not the raw bytes.
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "one\n"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        response.headers()["x-quarry-document-id"],
        document_id.as_str()
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        ""
    );

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/alpha/documents/notes/one.md/ttl",
            serde_json::json!({"expires_at":"2099-01-01T00:00:00Z"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["expires_at"], "2099-01-01T00:00:00Z");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-quarry-expires-at"],
        "2099-01-01T00:00:00Z"
    );

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/alpha/documents/notes/one.md/ttl",
            serde_json::json!({"expires_at": null}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["expires_at"].is_null());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/alpha/documents/notes/one.md/ttl",
            serde_json::json!({"expires_at":"2000-01-01T00:00:00Z"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(
        body.as_array()
            .unwrap()
            .iter()
            .all(|document| document["path"] != "notes/one.md")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/created.md")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("created"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/created.md")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("duplicate"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/alpha/transactions",
            serde_json::json!({"message":"batch"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/alpha/transactions/{tx}/documents/notes/two.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("two"))
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
                .uri("/v1/libraries/alpha/documents/notes/two.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/alpha/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/two.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "two"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let openapi: Value = response_json(response).await;
    assert!(openapi["paths"]["/v1/libraries"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}"]["head"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/snapshot"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"]["get"].is_object());
    // The legacy mutation facades are deleted routes (404), absent from the
    // OpenAPI document entirely; GET /review (read projection) remains.
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"]["post"].is_null());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/edit"].is_null());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/ops"].is_null());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/share"].is_object());
    assert!(
        openapi["paths"]["/v1/libraries/{library}/documents/{path}/share/{token}/revoke"]
            .is_object()
    );
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/blocks"].is_object());
    assert!(
        openapi["paths"]["/v1/libraries/{library}/documents/{path}/transactions"]["post"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentReviewResponse"]["properties"]["comments"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentSuggestionPreview"]["properties"]["before"]
            .is_object()
    );
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/presence"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/metadata"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/ttl"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/events/pending"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/events/ack"].is_object());
    assert!(
        openapi["paths"]["/v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata"]
            .is_object()
    );
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["post"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["get"].is_object());
}
