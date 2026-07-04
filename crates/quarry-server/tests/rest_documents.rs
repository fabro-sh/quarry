#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use quarry_core::DocumentSource;
use quarry_server::router;
use quarry_storage::StoreEventKind;
use serde_json::Value;
use tokio::time::{Duration, timeout};
use tower::ServiceExt;

mod common;

use common::{document_test_app, json_request, open_test_store, response_json};

fn assert_json_timestamp(value: &Value) {
    let timestamp = value.as_str().expect("timestamp should be a string");
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("timestamp should parse as RFC 3339");
}

fn assert_schema_enum_contains(openapi: &Value, schema: &Value, expected: &[&str]) {
    let resolved = if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/components/schemas/")
            .expect("schema enum references must point at components schemas");
        &openapi["components"]["schemas"][name]
    } else if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        one_of
            .iter()
            .find(|candidate| candidate.get("enum").is_some() || candidate.get("$ref").is_some())
            .expect("nullable enum schema must include an enum branch")
    } else {
        schema
    };
    let resolved = if let Some(reference) = resolved.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/components/schemas/")
            .expect("schema enum references must point at components schemas");
        &openapi["components"]["schemas"][name]
    } else {
        resolved
    };
    let values = resolved["enum"]
        .as_array()
        .expect("schema property should expose enum values");
    for expected_value in expected {
        assert!(
            values.iter().any(|value| value == expected_value),
            "schema enum {values:?} should include {expected_value}"
        );
    }
}

fn assert_path_parameter_enum_contains(
    openapi: &Value,
    path: &str,
    method: &str,
    name: &str,
    expected: &[&str],
) {
    let parameters = openapi["paths"][path][method]["parameters"]
        .as_array()
        .expect("path operation should expose parameters");
    let parameter = parameters
        .iter()
        .find(|parameter| parameter["name"] == name)
        .expect("path operation should expose named parameter");
    assert_schema_enum_contains(openapi, &parameter["schema"], expected);
}

fn assert_schema_type_contains(schema: &Value, expected: &str) {
    if let Some(schema_type) = schema.get("type").and_then(Value::as_str) {
        assert_eq!(schema_type, expected);
        return;
    }
    if let Some(types) = schema.get("type").and_then(Value::as_array) {
        assert!(
            types.iter().any(|schema_type| schema_type == expected),
            "schema type {types:?} should include {expected}"
        );
        return;
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        assert!(
            one_of
                .iter()
                .any(|schema| schema.get("type").and_then(Value::as_str) == Some(expected)),
            "schema oneOf {one_of:?} should include type {expected}"
        );
        return;
    }
    panic!("schema {schema:?} should expose a type");
}

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

#[tokio::test]
async fn collab_share_endpoints_mint_list_and_revoke_invite_tokens() {
    let (_root, store) = open_test_store().await;
    let library = store.create_library("shares").await.unwrap();
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
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/shares/documents/live.md/share",
            serde_json::json!({"role":"editor","byHint":"Avery"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let token: Value = response_json(response).await;
    assert_eq!(token["document_id"], written.document.id);
    assert_eq!(token["role"], "editor");
    assert_eq!(token["by_hint"], "Avery");
    let token_id = token["id"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/shares/documents/live.md/share")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let tokens: Value = response_json(response).await;
    assert_eq!(tokens.as_array().unwrap().len(), 1);
    assert_eq!(tokens[0]["id"], token_id);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/shares/documents/live.md/share/{token_id}/revoke"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let token: Value = response_json(response).await;
    assert_eq!(token["id"], token_id);
    assert_json_timestamp(&token["revoked_at"]);
}

#[tokio::test]
async fn agent_events_pending_and_ack_expose_sparse_event_signals() {
    let (_root, store) = open_test_store().await;
    store.create_library("eventfallback").await.unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/eventfallback/documents/live.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("hello"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let pending = timeout(Duration::from_secs(1), async {
        loop {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/v1/libraries/eventfallback/events/pending?after=0")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value = response_json(response).await;
            if body["events"].as_array().unwrap().iter().any(|event| {
                event["event"] == "doc.changed"
                    && event["data"]["path"] == "live.md"
                    && event["data"]["version_id"]
                        .as_str()
                        .is_some_and(|version_id| uuid::Uuid::parse_str(version_id).is_ok())
            }) {
                break body;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let event_id = pending["nextAfter"].as_u64().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/eventfallback/events/ack")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", "agent-a")
                .body(Body::from(
                    serde_json::json!({"eventId": event_id}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["agentId"], "agent-a");
    assert_eq!(body["ackedThrough"], event_id);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/eventfallback/events/pending?after={event_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn document_put_events_echo_origin_id() {
    let (_root, store) = open_test_store().await;
    store.create_library("collab-events").await.unwrap();
    let mut events = store.subscribe_events();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/collab-events/documents/live.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("X-Quarry-Origin-Id", "browser:session-1")
                .body(Body::from("live"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let event = timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind() == StoreEventKind::DocumentPut {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event.origin_id(), Some("browser:session-1"));
}

#[tokio::test]
async fn document_delete_events_echo_origin_id_and_doc_id() {
    let (_root, store) = open_test_store().await;
    store.create_library("delete-origin").await.unwrap();
    let written = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: ("delete-origin").to_string(),
            path: ("live.md").to_string(),
            content: b"live".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let mut events = store.subscribe_events();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/v1/libraries/delete-origin/documents/live.md")
                .header("X-Quarry-Origin-Id", "browser:session-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let event = timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind() == StoreEventKind::DocumentDelete {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event.doc_id(), Some(written.document.id.as_str()));
    assert_eq!(event.origin_id(), Some("browser:session-1"));
}

#[tokio::test]
async fn rest_api_supports_browser_search_links_versions_and_events() {
    let (_root, store) = open_test_store().await;
    let library = store.create_library("browser").await.unwrap();
    let first_intro = store
        .put_document(quarry_storage::PutDocumentRequest {
library: library.slug.to_string(),
path: ("intro.md").to_string(),
content: b"# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\n".to_vec(),
metadata: serde_json::json!({"title":"Intro","content_type":"text/markdown"}),
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
            path: ("daily.md").to_string(),
            content: b"# Daily\n\nBacklinked target with [[Chain]].\n".to_vec(),
            metadata: serde_json::json!({"title":"Daily","content_type":"text/markdown"}),
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
            path: ("guide.md").to_string(),
            content: b"# Guide\n".to_vec(),
            metadata: serde_json::json!({
                "aliases": ["Manual Alias"],
                "title":"Guide",
                "content_type":"text/markdown"
            }),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let latest_intro = store
        .put_document(quarry_storage::PutDocumentRequest {
library: library.slug.to_string(),
path: ("intro.md").to_string(),
content: b"# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\nupdated browser body with unique-search term.\n".to_vec(),
metadata: serde_json::json!({"title":"Intro","content_type":"text/markdown"}),
content_type: ("text/markdown").to_string(),
source: DocumentSource::Rest,
precondition: quarry_core::WritePrecondition::IfMatch(first_intro.version.id.clone()),
origin_id: None,
transaction: quarry_storage::TransactionMetadata::default(),
})
        .await
        .unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("chain.md").to_string(),
            content: b"# Chain\n".to_vec(),
            metadata: serde_json::json!({"title":"Chain","content_type":"text/markdown"}),
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
            path: ("projects/roadmap.md").to_string(),
            content: b"# Roadmap\n".to_vec(),
            metadata: serde_json::json!({"title":"Roadmap","content_type":"text/markdown"}),
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
            path: ("projects/brief.md").to_string(),
            content: b"# Brief\n\nSee [[Roadmap]] and #planning.\n".to_vec(),
            metadata: serde_json::json!({"title":"Brief","content_type":"text/markdown"}),
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
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=unique-search&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["path"], "intro.md");
    assert_eq!(
        body["results"][0]["head_version_id"],
        latest_intro.version.id
    );
    assert!(
        body["results"][0]["matched_fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "body")
    );
    assert!(
        body["results"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("unique-search")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=%23planning&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "intro.md"
            && result["matched_fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "tag")
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=manual%20alias&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "guide.md"
            && result["matched_fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "alias")
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search/suggest?q=dai&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["path"], "daily.md");
    assert_eq!(body[0]["match_type"], "title");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/browser/reindex",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["indexed_documents"], 6);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/outgoing-links")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let links = body["links"].as_array().unwrap();
    assert!(links.iter().any(|link| link["target_kind"] == "wiki_link"
        && link["target_path"] == "daily.md"
        && link["alias"] == "today"));
    assert!(links
        .iter()
        .any(|link| link["target_kind"] == "markdown_link" && link["target_path"] == "guide.md"));
    assert!(
        links
            .iter()
            .any(|link| link["target_kind"] == "tag" && link["target_text"] == "planning")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/daily.md/backlinks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(
        body["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["src_path"] == "intro.md")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&depth=1&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(
        body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["path"] == "intro.md")
    );
    assert!(
        body["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| { edge["source_path"] == "intro.md" && edge["target_path"] == "daily.md" })
    );
    assert!(
        !body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["path"] == "chain.md")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&depth=2&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(
        body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["path"] == "chain.md")
    );
    assert!(
        body["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| { edge["source_path"] == "daily.md" && edge["target_path"] == "chain.md" })
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&link_kind=tag&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["target_kind"], "tag");
    assert_eq!(edges[0]["target_text"], "planning");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?folder=projects&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let nodes = body["nodes"].as_array().unwrap();
    assert!(!nodes.is_empty());
    assert!(
        nodes
            .iter()
            .all(|node| node["path"].as_str().unwrap().starts_with("projects/"))
    );
    assert!(body["edges"].as_array().unwrap().iter().any(|edge| {
        edge["source_path"] == "projects/brief.md" && edge["target_path"] == "projects/roadmap.md"
    }));
    assert!(body["edges"].as_array().unwrap().iter().all(|edge| {
        edge["source_path"]
            .as_str()
            .unwrap()
            .starts_with("projects/")
            && edge["target_path"]
                .as_str()
                .is_none_or(|path| path.starts_with("projects/"))
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?tag=planning&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"].as_array().unwrap();
    assert!(!edges.is_empty());
    assert!(
        edges
            .iter()
            .all(|edge| edge["target_kind"] == "tag" && edge["target_text"] == "planning")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&link_kind=wiki_link&resolved=false&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["target_kind"], "wiki_link");
    assert_eq!(edges[0]["target_text"], "Missing");
    assert_eq!(edges[0]["resolved"], false);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/versions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["latest_version_id"], latest_intro.version.id);
    assert_eq!(body[1]["latest_version_id"], first_intro.version.id);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/versions/raw")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["id"], latest_intro.version.id);
    assert_eq!(body[1]["id"], first_intro.version.id);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/browser/documents/intro.md/versions/{}",
                    first_intro.version.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(
        body["content"]
            .as_str()
            .unwrap()
            .contains("[[Daily|today]]")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/browser/documents/intro.md/versions/{}/diff?against={}",
                    first_intro.version.id, latest_intro.version.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let diff = body["unified_diff"].as_str().unwrap();
    assert!(diff.contains("+updated browser body"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!(
                "/v1/libraries/browser/documents/intro.md/versions/{}/restore",
                first_intro.version.id
            ),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(response.headers()[header::ETAG], first_intro.version.id);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // The restore routes through the reconciling gateway (Phase 7), which
    // publishes the canonical normalized form: this version was written by a
    // legacy byte put with out-of-band `title` metadata, so the one-time
    // normalization renders that metadata as frontmatter.
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "---\ntitle: Intro\n---\n# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\n"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/events?library=browser")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/events/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let openapi: Value = response_json(response).await;
    assert!(openapi["paths"]["/v1/libraries/{library}/search"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/backlinks"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/versions"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/versions/raw"].is_object());
    assert!(openapi["components"]["schemas"]["DocumentHistoryEntry"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"].is_object());
    assert!(openapi["paths"]["/v1/events"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/events/stream"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/events/pending"].is_object());
    assert!(
        openapi["components"]["schemas"]["AgentBlockRef"]["properties"]
            .get("baseToken")
            .is_none()
    );
    let block_ref_required = openapi["components"]["schemas"]["AgentBlockRef"]["required"]
        .as_array()
        .expect("AgentBlockRef should expose required fields");
    assert!(block_ref_required.iter().any(|value| value == "ordinal"));
    assert!(
        !block_ref_required
            .iter()
            .any(|value| value == "contentHash")
    );
    let content_hash_schema =
        &openapi["components"]["schemas"]["AgentBlockRef"]["properties"]["contentHash"];
    assert_schema_type_contains(content_hash_schema, "string");
    assert_schema_type_contains(content_hash_schema, "null");
    // The single mutation contract: the transaction envelope and ack.
    assert!(
        openapi["components"]["schemas"]["BlockTransactionRequest"]["properties"]["ops"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["BlockTransactionAck"]["properties"]["changed_block_ids"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["BlockTransactionError"]["properties"]["retryable"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentReviewResponse"]["properties"]["suggestions"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentReviewSuggestion"]["properties"]["preview"]
            .is_object()
    );
    assert_schema_enum_contains(
        &openapi,
        &openapi["components"]["schemas"]["AgentPresenceRequest"]["properties"]["status"],
        &[
            "reading",
            "thinking",
            "acting",
            "waiting",
            "completed",
            "error",
        ],
    );
    let library_presence_entry = &openapi["components"]["schemas"]["AgentPresenceEntry"];
    assert!(library_presence_entry["properties"]["path"].is_object());
    assert!(
        library_presence_entry["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "path")
    );
    let tmp_presence_entry = &openapi["components"]["schemas"]["TmpAgentPresenceEntry"];
    assert!(tmp_presence_entry.is_object());
    assert!(tmp_presence_entry["properties"].get("path").is_none());
    assert!(tmp_presence_entry["properties"].get("library").is_none());
    let tmp_presence_required = tmp_presence_entry["required"].as_array().unwrap();
    assert!(
        tmp_presence_required
            .iter()
            .any(|field| field == "documentId")
    );
    assert!(tmp_presence_required.iter().any(|field| field == "agentId"));
    assert!(tmp_presence_required.iter().any(|field| field == "status"));
    assert!(
        tmp_presence_required
            .iter()
            .any(|field| field == "updatedAt")
    );
    assert!(!tmp_presence_required.iter().any(|field| field == "path"));
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/presence"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/TmpAgentPresenceListResponse"
    );
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/presence"]["post"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/TmpAgentPresenceResponse"
    );
    assert_path_parameter_enum_contains(
        &openapi,
        "/v1/libraries/{library}/documents/{path}/review",
        "get",
        "includeResolved",
        &["1", "true", "yes", "0", "false", "no"],
    );
    // The legacy mutation facades are gone from the OpenAPI document.
    let edit_operation = &openapi["paths"]["/v1/libraries/{library}/documents/{path}/edit"]["post"];
    assert!(edit_operation.is_null(), "edit POST should be deleted");
    let ops_operation = &openapi["paths"]["/v1/libraries/{library}/documents/{path}/ops"]["post"];
    assert!(ops_operation.is_null(), "ops POST should be deleted");
    let review_operation =
        &openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"]["post"];
    assert!(review_operation.is_null(), "review POST should be deleted");
}
