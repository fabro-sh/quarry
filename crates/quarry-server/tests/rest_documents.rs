#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use anyhow::Context as _;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use quarry_core::DocumentSource;
use quarry_server::router;
use quarry_storage::{QuarryStore, StoreEventKind};
use serde_json::Value;
use tokio::time::{Duration, timeout};
use tower::ServiceExt;

mod common;

use common::{document_test_app, json_request, open_test_store, response_json};

fn empty_request(method: Method, uri: &str) -> anyhow::Result<Request<Body>> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .with_context(|| format!("build empty request for {uri}"))
}

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
async fn rest_api_supports_documents_transactions_etags_and_openapi() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"alpha"}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("one"))
                .context("build first markdown PUT")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .context("ETag header should be valid ASCII")?
        .to_string();
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
    let document_id = body["document"]["id"]
        .as_str()
        .context("create response should include document id")?
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .header(header::IF_MATCH, "\"wrong\"")
                .body(Body::from("bad"))
                .context("build stale markdown PUT")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .context("build document GET")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        response.headers()["x-quarry-document-id"],
        document_id.as_str()
    );
    // Markdown PUTs land via the Phase 4 reconciled write: content is the
    // deterministic normalized export (trailing newline), not the raw bytes.
    assert_eq!(to_bytes(response.into_body(), usize::MAX).await?, "one\n");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .context("build document HEAD")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        response.headers()["x-quarry-document-id"],
        document_id.as_str()
    );
    assert_eq!(to_bytes(response.into_body(), usize::MAX).await?, "");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/alpha/documents/notes/one.md/ttl",
            serde_json::json!({"expires_at":"2099-01-01T00:00:00Z"}),
        ))
        .await?;
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
                .context("build document HEAD after TTL")?,
        )
        .await?;
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
        .await?;
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
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .context("build expired document GET")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::GONE);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents")
                .body(Body::empty())
                .context("build list documents GET")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(
        body.as_array()
            .context("document list response should be an array")?
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
                .context("build If-None-Match create PUT")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/created.md")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("duplicate"))
                .context("build duplicate If-None-Match PUT")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/alpha/transactions",
            serde_json::json!({"message":"batch"}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"]
        .as_str()
        .context("transaction create response should include id")?;

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
                .context("build staged document PUT")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/two.md")
                .body(Body::empty())
                .context("build pre-commit document GET")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/alpha/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/two.md")
                .body(Body::empty())
                .context("build committed document GET")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(to_bytes(response.into_body(), usize::MAX).await?, "two");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/openapi.json")
                .body(Body::empty())
                .context("build OpenAPI GET")?,
        )
        .await?;
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
    Ok(())
}

#[tokio::test]
async fn collab_share_endpoints_mint_list_and_revoke_invite_tokens() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("shares")
        .await
        .context("create shares library")?;
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
        .context("write share target document")?;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/shares/documents/live.md/share",
            serde_json::json!({"role":"editor","byHint":"Avery"}),
        ))
        .await
        .context("mint share token request")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let token: Value = response_json(response).await;
    assert_eq!(token["document_id"], written.document.id.as_str());
    assert_eq!(token["role"], "editor");
    assert_eq!(token["by_hint"], "Avery");
    let token_id = token["id"]
        .as_str()
        .context("minted token response should include string id")?
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/shares/documents/live.md/share")
                .body(Body::empty())
                .context("build list share tokens request")?,
        )
        .await
        .context("list share tokens request")?;
    assert_eq!(response.status(), StatusCode::OK);
    let tokens: Value = response_json(response).await;
    let tokens = tokens
        .as_array()
        .context("list share tokens response should be an array")?;
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0]["id"], token_id);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/shares/documents/live.md/share/{token_id}/revoke"),
            serde_json::json!({}),
        ))
        .await
        .context("revoke share token request")?;
    assert_eq!(response.status(), StatusCode::OK);
    let token: Value = response_json(response).await;
    assert_eq!(token["id"], token_id);
    assert_json_timestamp(&token["revoked_at"]);
    Ok(())
}

#[tokio::test]
async fn agent_events_pending_and_ack_expose_sparse_event_signals() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("eventfallback")
        .await
        .context("create eventfallback library")?;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/eventfallback/documents/live.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("hello"))
                .context("build document write request")?,
        )
        .await
        .context("write document before reading pending events")?;
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
                        .context("build pending events request")?,
                )
                .await
                .context("read pending events")?;
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value = response_json(response).await;
            if body["events"]
                .as_array()
                .context("pending events response should include events array")?
                .iter()
                .any(|event| {
                    event["event"] == "doc.changed"
                        && event["data"]["path"] == "live.md"
                        && event["data"]["version_id"]
                            .as_str()
                            .is_some_and(|version_id| uuid::Uuid::parse_str(version_id).is_ok())
                })
            {
                break Ok::<_, anyhow::Error>(body);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("wait for pending doc.changed event")??;
    let event_id = pending["nextAfter"]
        .as_u64()
        .context("pending events response should include numeric nextAfter")?;

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
                .context("build pending event ack request")?,
        )
        .await
        .context("ack pending event")?;
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
                .context("build post-ack pending events request")?,
        )
        .await
        .context("read pending events after ack")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(
        body["events"]
            .as_array()
            .context("post-ack pending events response should include events array")?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn document_put_events_echo_origin_id() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("collab-events")
        .await
        .context("create collab-events library")?;
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
                .context("build document PUT request")?,
        )
        .await
        .context("write document with origin id")?;
    assert_eq!(response.status(), StatusCode::OK);

    let event = timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.context("receive store event")?;
            if event.kind() == StoreEventKind::DocumentPut {
                break Ok::<_, anyhow::Error>(event);
            }
        }
    })
    .await
    .context("wait for document put event")??;
    assert_eq!(event.origin_id(), Some("browser:session-1"));
    Ok(())
}

#[tokio::test]
async fn document_delete_events_echo_origin_id_and_doc_id() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("delete-origin")
        .await
        .context("create delete-origin library")?;
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
        .context("write document before delete event test")?;
    let mut events = store.subscribe_events();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/v1/libraries/delete-origin/documents/live.md")
                .header("X-Quarry-Origin-Id", "browser:session-1")
                .body(Body::empty())
                .context("build document DELETE request")?,
        )
        .await
        .context("delete document with origin id")?;
    assert_eq!(response.status(), StatusCode::OK);

    let event = timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.context("receive store event")?;
            if event.kind() == StoreEventKind::DocumentDelete {
                break Ok::<_, anyhow::Error>(event);
            }
        }
    })
    .await
    .context("wait for document delete event")??;
    assert_eq!(event.doc_id(), Some(written.document.id.as_str()));
    assert_eq!(event.origin_id(), Some("browser:session-1"));
    Ok(())
}

#[tokio::test]
async fn rest_api_supports_browser_search_links_versions_and_events() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store.create_library("browser").await?;
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
        .await?;
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
        .await?;
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
        .await?;
    let latest_intro = store
        .put_document(quarry_storage::PutDocumentRequest {
library: library.slug.to_string(),
path: ("intro.md").to_string(),
content: b"# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\nupdated browser body with unique-search term.\n".to_vec(),
metadata: serde_json::json!({"title":"Intro","content_type":"text/markdown"}),
content_type: ("text/markdown").to_string(),
source: DocumentSource::Rest,
precondition: quarry_core::WritePrecondition::IfMatch(first_intro.version.id.to_string()),
origin_id: None,
transaction: quarry_storage::TransactionMetadata::default(),
})
        .await?;
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
        .await?;
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
        .await?;
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
        .await?;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=unique-search&limit=5")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["path"], "intro.md");
    assert_eq!(
        body["results"][0]["head_version_id"],
        latest_intro.version.id.as_str()
    );
    let matched_fields = body["results"][0]["matched_fields"]
        .as_array()
        .context("search result should expose matched fields")?;
    assert!(matched_fields.iter().any(|field| field == "body"));
    let snippet = body["results"][0]["snippet"]
        .as_str()
        .context("search result should expose a snippet")?;
    assert!(snippet.contains("unique-search"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=%23planning&limit=5")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let results = body["results"]
        .as_array()
        .context("tag search should expose results")?;
    assert!(results.iter().any(|result| {
        result["path"] == "intro.md"
            && result["matched_fields"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == "tag"))
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=manual%20alias&limit=5")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let results = body["results"]
        .as_array()
        .context("alias search should expose results")?;
    assert!(results.iter().any(|result| {
        result["path"] == "guide.md"
            && result["matched_fields"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == "alias"))
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search/suggest?q=dai&limit=5")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
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
        .await?;
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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let links = body["links"]
        .as_array()
        .context("outgoing links response should expose links")?;
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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let backlinks = body["links"]
        .as_array()
        .context("backlinks response should expose links")?;
    assert!(backlinks.iter().any(|link| link["src_path"] == "intro.md"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&depth=1&limit=20")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let nodes = body["nodes"]
        .as_array()
        .context("depth-1 graph should expose nodes")?;
    let edges = body["edges"]
        .as_array()
        .context("depth-1 graph should expose edges")?;
    assert!(nodes.iter().any(|node| node["path"] == "intro.md"));
    assert!(
        edges
            .iter()
            .any(|edge| { edge["source_path"] == "intro.md" && edge["target_path"] == "daily.md" })
    );
    assert!(!nodes.iter().any(|node| node["path"] == "chain.md"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&depth=2&limit=20")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let nodes = body["nodes"]
        .as_array()
        .context("depth-2 graph should expose nodes")?;
    let edges = body["edges"]
        .as_array()
        .context("depth-2 graph should expose edges")?;
    assert!(nodes.iter().any(|node| node["path"] == "chain.md"));
    assert!(
        edges
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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"]
        .as_array()
        .context("tag-filtered graph should expose edges")?;
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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let nodes = body["nodes"]
        .as_array()
        .context("folder graph should expose nodes")?;
    let edges = body["edges"]
        .as_array()
        .context("folder graph should expose edges")?;
    assert!(!nodes.is_empty());
    assert!(nodes.iter().all(|node| {
        node["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("projects/"))
    }));
    assert!(edges.iter().any(|edge| {
        edge["source_path"] == "projects/brief.md" && edge["target_path"] == "projects/roadmap.md"
    }));
    assert!(edges.iter().all(|edge| {
        edge["source_path"]
            .as_str()
            .is_some_and(|path| path.starts_with("projects/"))
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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"]
        .as_array()
        .context("tag graph should expose edges")?;
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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"]
        .as_array()
        .context("unresolved graph should expose edges")?;
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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(
        body[0]["latest_version_id"],
        latest_intro.version.id.as_str()
    );
    assert_eq!(
        body[1]["latest_version_id"],
        first_intro.version.id.as_str()
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/versions/raw")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["id"], latest_intro.version.id.as_str());
    assert_eq!(body[1]["id"], first_intro.version.id.as_str());

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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let content = body["content"]
        .as_str()
        .context("version response should expose content")?;
    assert!(content.contains("[[Daily|today]]"));

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
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let diff = body["unified_diff"]
        .as_str()
        .context("diff response should expose unified_diff")?;
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
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(
        response.headers()[header::ETAG],
        first_intro.version.id.as_str()
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    // The restore routes through the reconciling gateway (Phase 7), which
    // publishes the canonical normalized form: this version was written by a
    // legacy byte put with out-of-band `title` metadata, so the one-time
    // normalization renders that metadata as frontmatter.
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await?,
        "---\ntitle: Intro\n---\n# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\n"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/events?library=browser")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .context("events response should have a valid content-type")?
            .starts_with("text/event-stream")
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/events/stream")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .context("document event stream response should have a valid content-type")?
            .starts_with("text/event-stream")
    );

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/openapi.json")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
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
        .context("AgentBlockRef should expose required fields")?;
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
        openapi["components"]["schemas"]["ApiErrorResponse"]["properties"]["retryable"].is_object()
    );
    assert_schema_enum_contains(
        &openapi,
        &openapi["components"]["schemas"]["ApiErrorCode"],
        &[
            "INVALID_REQUEST",
            "STALE_BASE",
            "SERVICE_BUSY",
            "INTERNAL_ERROR",
        ],
    );
    assert!(openapi["components"]["schemas"]["ErrorResponse"].is_null());
    assert!(openapi["components"]["schemas"]["BlockTransactionError"].is_null());
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
    let library_presence_required = library_presence_entry["required"]
        .as_array()
        .context("AgentPresenceEntry should expose required fields")?;
    assert!(
        library_presence_required
            .iter()
            .any(|field| field == "path")
    );
    let tmp_presence_entry = &openapi["components"]["schemas"]["TmpAgentPresenceEntry"];
    assert!(tmp_presence_entry.is_object());
    assert!(tmp_presence_entry["properties"].get("path").is_none());
    assert!(tmp_presence_entry["properties"].get("library").is_none());
    let tmp_presence_required = tmp_presence_entry["required"]
        .as_array()
        .context("TmpAgentPresenceEntry should expose required fields")?;
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
    Ok(())
}

#[tokio::test]
async fn version_history_includes_transaction_metadata() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"versionmeta"}),
        ))
        .await
        .context("create versionmeta library")?;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/versionmeta/transactions",
            serde_json::json!({
                "actor": "Avery",
                "message": "Imported from Git",
                "provenance": {"remote": "origin/main"}
            }),
        ))
        .await
        .context("create transaction with metadata")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"]
        .as_str()
        .context("transaction create response should include string id")?;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/versionmeta/transactions/{tx}/documents/meta.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("# Meta\n"))
                .context("build transaction document PUT request")?,
        )
        .await
        .context("write document inside transaction")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/versionmeta/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .context("commit transaction with metadata")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/versionmeta/documents/meta.md/versions")
                .body(Body::empty())
                .context("build version history request")?,
        )
        .await
        .context("read document version history")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["source"], "rest");
    assert_eq!(body[0]["actor"], "Avery");
    assert_eq!(body[0]["message"], "Imported from Git");
    assert_eq!(body[0]["provenance"]["remote"], "origin/main");
    Ok(())
}

#[tokio::test]
async fn put_document_rejects_invalid_transaction_provenance_header() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("badprovenance")
        .await
        .context("create badprovenance library")?;
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/badprovenance/documents/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("x-quarry-transaction-provenance", "{bad json")
                .body(Body::from("body"))
                .context("build invalid provenance document PUT request")?,
        )
        .await
        .context("send invalid provenance document PUT request")?;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn put_document_decodes_percent_encoded_transaction_actor_header() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("actorheader")
        .await
        .context("create actorheader library")?;
    let app = router(store);

    // Each document is created first so the actor-carrying write exercises
    // the update path (first-import attribution is covered separately by
    // first_import_records_transaction_actor_header).
    put_markdown(&app, "actorheader", "a.md", "# A\n", None).await?;
    put_markdown(&app, "actorheader", "b.md", "# B\n", None).await?;
    put_markdown(&app, "actorheader", "c.md", "# C\n", None).await?;

    // Percent-encoded UTF-8 name decodes before storage.
    let version = put_markdown(
        &app,
        "actorheader",
        "a.md",
        "# A updated\n",
        Some("Jos%C3%A9"),
    )
    .await?;
    assert_eq!(
        version_actor(&app, "actorheader", "a.md", &version).await?,
        "José"
    );

    // Plain ASCII passes through unchanged.
    let version = put_markdown(&app, "actorheader", "b.md", "# B updated\n", Some("Avery")).await?;
    assert_eq!(
        version_actor(&app, "actorheader", "b.md", &version).await?,
        "Avery"
    );

    // No header falls back to the gateway's surface label.
    let version = put_markdown(&app, "actorheader", "c.md", "# C updated\n", None).await?;
    assert_eq!(
        version_actor(&app, "actorheader", "c.md", &version).await?,
        "rest"
    );
    Ok(())
}

#[tokio::test]
async fn first_import_records_transaction_actor_header() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("actorcreate")
        .await
        .context("create actorcreate library")?;
    let app = router(store);

    let version = put_markdown(&app, "actorcreate", "fresh.md", "# Fresh\n", Some("Avery")).await?;

    assert_eq!(
        version_actor(&app, "actorcreate", "fresh.md", &version).await?,
        "Avery"
    );
    Ok(())
}

#[tokio::test]
async fn delete_move_and_restore_record_transaction_actor_header() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("actorops")
        .await
        .context("create actorops library")?;
    let app = router(store);

    let v1 = put_markdown(&app, "actorops", "keep.md", "# Doc one\n", None).await?;
    let _v2 = put_markdown(&app, "actorops", "keep.md", "# Doc two\n", None).await?;
    put_markdown(&app, "actorops", "doomed.md", "# Doomed\n", None).await?;

    // Move records the actor on its transaction.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/actorops/documents/keep.md/move")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::from(r#"{"to_path":"kept.md"}"#))
                .context("build document move request")?,
        )
        .await
        .context("move document with transaction actor")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["actor"], "Avery");

    // Delete records the actor on its transaction.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/v1/libraries/actorops/documents/doomed.md")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::empty())
                .context("build document delete request")?,
        )
        .await
        .context("delete document with transaction actor")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["actor"], "Avery");

    // Restore (markdown/BlockDocument path) records the actor on the
    // restored version.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/v1/libraries/actorops/documents/kept.md/versions/{v1}/restore"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::from("{}"))
                .context("build markdown version restore request")?,
        )
        .await
        .context("restore markdown document version with transaction actor")?;
    assert_eq!(response.status(), StatusCode::OK);
    let restored: Value = response_json(response).await;
    let restored_version = restored["version"]["id"]
        .as_str()
        .context("restore response should include version id")?;
    assert_eq!(
        version_actor(&app, "actorops", "kept.md", restored_version).await?,
        "Avery"
    );
    Ok(())
}

#[tokio::test]
async fn raw_document_restore_records_transaction_actor_header() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    store
        .create_library("actorraw")
        .await
        .context("create actorraw library")?;
    let app = router(store);

    // A plain-text document routes as a RawDocument (not `.md`, not a
    // markdown content type), so its restore takes the legacy byte path
    // (`restore_document_version_with_origin`) rather than the markdown
    // gateway. Restoring the current head short-circuits, so write two
    // versions and restore the first.
    let v1 = put_plain_text(&app, "actorraw", "notes.txt", "raw one\n").await?;
    let _v2 = put_plain_text(&app, "actorraw", "notes.txt", "raw two\n").await?;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/v1/libraries/actorraw/documents/notes.txt/versions/{v1}/restore"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::from("{}"))
                .context("build raw version restore request")?,
        )
        .await
        .context("restore raw document version with transaction actor")?;
    assert_eq!(response.status(), StatusCode::OK);
    let restored: Value = response_json(response).await;
    let restored_version = restored["version"]["id"]
        .as_str()
        .context("raw restore response should include version id")?;
    assert_eq!(
        version_actor(&app, "actorraw", "notes.txt", restored_version).await?,
        "Avery"
    );
    Ok(())
}

/// PUTs plain text (a RawDocument) into `library`, returning the written
/// version id.
async fn put_plain_text(
    app: &axum::Router,
    library: &str,
    path: &str,
    body: &str,
) -> anyhow::Result<String> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/libraries/{library}/documents/{path}"))
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(body.to_string()))
                .with_context(|| format!("build plain text PUT request for {library}/{path}"))?,
        )
        .await
        .with_context(|| format!("PUT plain text document {library}/{path}"))?;
    assert_eq!(response.status(), StatusCode::OK);
    let outcome: Value = response_json(response).await;
    Ok(outcome["version"]["id"]
        .as_str()
        .context("plain text PUT response should include version id")?
        .to_string())
}

/// PUTs markdown into `library`, optionally with an
/// `x-quarry-transaction-actor` header, returning the written version id.
async fn put_markdown(
    app: &axum::Router,
    library: &str,
    path: &str,
    body: &str,
    actor_header: Option<&str>,
) -> anyhow::Result<String> {
    let mut request = Request::builder()
        .method(Method::PUT)
        .uri(format!("/v1/libraries/{library}/documents/{path}"))
        .header(header::CONTENT_TYPE, "text/markdown");
    if let Some(actor) = actor_header {
        request = request.header("x-quarry-transaction-actor", actor);
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(body.to_string()))
                .with_context(|| format!("build markdown PUT request for {library}/{path}"))?,
        )
        .await
        .with_context(|| format!("PUT markdown document {library}/{path}"))?;
    assert_eq!(response.status(), StatusCode::OK);
    let outcome: Value = response_json(response).await;
    Ok(outcome["version"]["id"]
        .as_str()
        .context("markdown PUT response should include version id")?
        .to_string())
}

/// The `"actor"` recorded for `version_id` of `path`, via GET `/versions`.
async fn version_actor(
    app: &axum::Router,
    library: &str,
    path: &str,
    version_id: &str,
) -> anyhow::Result<Value> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/{library}/documents/{path}/versions"))
                .body(Body::empty())
                .with_context(|| format!("build version history request for {library}/{path}"))?,
        )
        .await
        .with_context(|| format!("read version history for {library}/{path}"))?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    Ok(body
        .as_array()
        .context("version history response should be an array")?
        .iter()
        .find(|version| version["id"] == version_id)
        .with_context(|| format!("version history should contain version id {version_id}"))?
        ["actor"]
        .clone())
}

#[tokio::test]
async fn agent_snapshot_exposes_snapshot_scoped_block_refs() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("agent")
        .await
        .context("create agent library")?;
    let written = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/one.md").to_string(),
            content: b"# Title\n\nFirst paragraph.\n\nSecond paragraph.\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write snapshot source document")?;
    let app = router(store);

    let response = app
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agent/documents/notes/one.md/snapshot",
        )?)
        .await
        .context("read agent snapshot")?;

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["documentId"], written.document.id.as_str());
    assert_eq!(body["baseToken"], written.version.id.as_str());
    assert_eq!(
        body["blocks"]
            .as_array()
            .context("snapshot response should include blocks")?
            .len(),
        3
    );
    assert_eq!(body["blocks"][0]["markdown"], "# Title\n\n");
    assert!(body["blocks"][0]["ref"].get("baseToken").is_none());
    assert_eq!(body["blocks"][0]["ref"]["ordinal"], 0);
    assert_eq!(
        body["blocks"][0]["ref"]["contentHash"]
            .as_str()
            .context("snapshot block ref should include content hash")?
            .len(),
        64
    );
    Ok(())
}

#[tokio::test]
async fn agent_review_lists_open_comments_replies_and_suggestions() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("agentreviewread")
        .await
        .context("create agentreviewread library")?;
    let markdown = "Alpha {==target==}{>>Needs work.<<}{#c1} and {==done==}{>>Fixed.<<}{#c2}.\n\nUse {~~old~>new~~}{#s1} wording and `{++literal++}{#s_code}`.\n\n```text\n{==ignored==}{>>Nope<<}{#c_code}\n{--gone--}{#s_code2}\n```\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user:a\n  c2:\n    at: \"2026-01-02T00:00:00.000Z\"\n    by: user:b\n    status: resolved\n  c_code:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: user:code\n  r1:\n    at: \"2026-01-01T01:00:00.000Z\"\n    body: Reply body.\n    by: ai:codex\n    re: c1\n  r2:\n    at: \"2026-01-03T01:00:00.000Z\"\n    body: Suggestion reply.\n    by: user:a\n    re: s1\nsuggestions:\n  s1:\n    at: \"2026-01-03T00:00:00.000Z\"\n    by: ai:codex\n  s_code:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: ai:code\n  s_code2:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: ai:code\n";
    let written = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/review.md").to_string(),
            content: markdown.as_bytes().to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write review source document")?;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentreviewread/documents/notes/review.md/snapshot",
        )?)
        .await
        .context("read review document snapshot")?;
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;

    let response = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentreviewread/documents/notes/review.md/review",
        )?)
        .await
        .context("read open review items")?;

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["documentId"], written.document.id.as_str());
    assert_eq!(body["baseToken"], written.version.id.as_str());
    assert_eq!(
        body["comments"]
            .as_array()
            .context("review response should include comments")?
            .len(),
        1
    );
    assert_eq!(body["comments"][0]["id"], "c1");
    assert_eq!(body["comments"][0]["status"], "open");
    assert_eq!(body["comments"][0]["by"], "user:a");
    assert_eq!(body["comments"][0]["at"], "2026-01-01T00:00:00.000Z");
    assert_eq!(body["comments"][0]["ref"], snapshot["blocks"][0]["ref"]);
    assert_eq!(body["comments"][0]["quote"], "target");
    assert_eq!(body["comments"][0]["body"], "Needs work.");
    assert_eq!(
        body["comments"][0]["replies"]
            .as_array()
            .context("comment should include replies")?
            .len(),
        1
    );
    assert_eq!(body["comments"][0]["replies"][0]["id"], "r1");
    assert_eq!(body["comments"][0]["replies"][0]["status"], "open");
    assert_eq!(body["comments"][0]["replies"][0]["by"], "ai:codex");
    assert_eq!(body["comments"][0]["replies"][0]["body"], "Reply body.");

    assert_eq!(
        body["suggestions"]
            .as_array()
            .context("review response should include suggestions")?
            .len(),
        1
    );
    assert_eq!(body["suggestions"][0]["id"], "s1");
    assert_eq!(body["suggestions"][0]["status"], "open");
    assert_eq!(body["suggestions"][0]["kind"], "substitution");
    assert_eq!(body["suggestions"][0]["by"], "ai:codex");
    assert_eq!(body["suggestions"][0]["at"], "2026-01-03T00:00:00.000Z");
    assert_eq!(body["suggestions"][0]["ref"], snapshot["blocks"][1]["ref"]);
    assert_eq!(body["suggestions"][0]["quote"], "old");
    assert_eq!(body["suggestions"][0]["content"], "new");
    assert_eq!(
        body["suggestions"][0]["preview"],
        serde_json::json!({"before": "old", "after": "new"})
    );
    assert_eq!(body["suggestions"][0]["replies"][0]["id"], "r2");
    assert_eq!(
        body["suggestions"][0]["replies"][0]["body"],
        "Suggestion reply."
    );

    let response = app
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentreviewread/documents/notes/review.md/review?includeResolved=1",
        )?)
        .await
        .context("read review items including resolved")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(
        body["comments"]
            .as_array()
            .context("resolved review response should include comments")?
            .len(),
        2
    );
    assert_eq!(body["comments"][1]["id"], "c2");
    assert_eq!(body["comments"][1]["status"], "resolved");
    assert_eq!(body["comments"][1]["quote"], "done");
    assert_eq!(body["comments"][1]["body"], "Fixed.");
    Ok(())
}

#[tokio::test]
async fn agent_review_reports_explicit_inline_markers_without_endmatter() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("agentrevieworphan")
        .await
        .context("create agentrevieworphan library")?;
    let written = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/review.md").to_string(),
            content: b"See {==this==}{>>Check it<<}{#c_orphan}.\n\nAdd {++better++}{#s_orphan}.\n"
                .to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write orphan review marker document")?;
    let app = router(store);

    let response = app
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentrevieworphan/documents/notes/review.md/review",
        )?)
        .await
        .context("read orphan review markers")?;

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["documentId"], written.document.id.as_str());
    assert_eq!(body["baseToken"], written.version.id.as_str());
    assert_eq!(
        body["comments"]
            .as_array()
            .context("orphan review response should include comments")?
            .len(),
        1
    );
    assert_eq!(body["comments"][0]["id"], "c_orphan");
    assert_eq!(body["comments"][0]["by"], "unknown");
    assert_eq!(body["comments"][0]["at"], "");
    assert_eq!(body["comments"][0]["body"], "Check it");
    assert_eq!(body["comments"][0]["quote"], "this");
    assert_eq!(
        body["suggestions"]
            .as_array()
            .context("orphan review response should include suggestions")?
            .len(),
        1
    );
    assert_eq!(body["suggestions"][0]["id"], "s_orphan");
    assert_eq!(body["suggestions"][0]["by"], "unknown");
    assert_eq!(body["suggestions"][0]["at"], "");
    assert_eq!(body["suggestions"][0]["kind"], "insert");
    assert_eq!(body["suggestions"][0]["content"], "better");
    Ok(())
}

#[tokio::test]
async fn agent_review_matches_snapshot_errors_for_missing_and_non_markdown() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("agentreviewerrors")
        .await
        .context("create agentreviewerrors library")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/plain.txt").to_string(),
            content: b"plain text".to_vec(),
            metadata: serde_json::json!({"content_type":"text/plain"}),
            content_type: ("text/plain").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write non-markdown review error fixture")?;
    let app = router(store);

    let snapshot = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentreviewerrors/documents/notes/missing.md/snapshot",
        )?)
        .await
        .context("read missing-document snapshot error")?;
    let review = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentreviewerrors/documents/notes/missing.md/review",
        )?)
        .await
        .context("read missing-document review error")?;
    assert_eq!(review.status(), snapshot.status());
    assert_eq!(response_json(review).await, response_json(snapshot).await);

    let snapshot = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentreviewerrors/documents/notes/plain.txt/snapshot",
        )?)
        .await
        .context("read non-markdown snapshot error")?;
    let review = app
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/agentreviewerrors/documents/notes/plain.txt/review",
        )?)
        .await
        .context("read non-markdown review error")?;
    assert_eq!(review.status(), snapshot.status());
    assert_eq!(response_json(review).await, response_json(snapshot).await);
    Ok(())
}

#[tokio::test]
async fn rest_api_supports_move_metadata_and_conflict_lookup_endpoints() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("actions")
        .await
        .context("create actions library")?;
    store
        .create_library("other")
        .await
        .context("create other library")?;
    let written = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("a.md").to_string(),
            content: b"hello".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write source action document")?;
    let sibling = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("a.conflict.md").to_string(),
            content: b"git version".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Git,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write conflict sibling document")?;
    let conflict = store
        .record_conflict(
            &library.slug,
            "a.md",
            Some(written.version.id.to_string()),
            Some(sibling.version.id.to_string()),
        )
        .await
        .context("record action document conflict")?;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/actions/documents/a.md/move",
            serde_json::json!({"to_path":"b.md"}),
        ))
        .await
        .context("move action document")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/actions/documents/b.md/metadata",
            serde_json::json!({"reviewed":true}),
        ))
        .await
        .context("patch moved document metadata")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/actions/documents/b.md",
            serde_json::json!({"wrong":true}),
        ))
        .await
        .context("send invalid direct document patch")?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/actions/documents/b.md",
        )?)
        .await
        .context("read moved action document")?;
    assert_eq!(response.status(), StatusCode::OK);

    let actions_conflict_uri = format!("/v1/libraries/actions/conflicts/{}", conflict.id);
    let response = app
        .clone()
        .oneshot(empty_request(Method::GET, &actions_conflict_uri)?)
        .await
        .context("read action conflict by id")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["id"], conflict.id);
    assert_eq!(body["conflict_path"], "a.conflict.md");

    let other_conflict_uri = format!("/v1/libraries/other/conflicts/{}", conflict.id);
    let response = app
        .clone()
        .oneshot(empty_request(Method::GET, &other_conflict_uri)?)
        .await
        .context("read action conflict through wrong library")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/conflicts/{}/resolve", conflict.id),
            serde_json::json!({}),
        ))
        .await
        .context("resolve action conflict through wrong library")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/actions/conflicts/{}/resolve", conflict.id),
            serde_json::json!({}),
        ))
        .await
        .context("resolve action conflict")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["status"], "resolved");
    assert_json_timestamp(&body["resolved_at"]);
    Ok(())
}

#[tokio::test]
async fn rest_api_marks_ambiguous_links() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("ambiguous")
        .await
        .context("create ambiguous library")?;

    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("alpha/target.md").to_string(),
            content: b"# Target\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write first ambiguous link target")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("omega/target.md").to_string(),
            content: b"# Target\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write second ambiguous link target")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("source.md").to_string(),
            content: b"See [[target]].\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write ambiguous wikilink source")?;
    let app = router(store);

    let response = app
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/ambiguous/documents/source.md/outgoing-links",
        )?)
        .await
        .context("read ambiguous outgoing links")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let link = body["links"]
        .as_array()
        .context("outgoing links response should include links")?
        .iter()
        .find(|link| link["target_kind"] == "wiki_link" && link["target_text"] == "target")
        .context("outgoing links should include ambiguous wiki link")?;
    assert_eq!(link["target_path"], Value::Null);
    assert_eq!(link["resolved"], false);
    assert_eq!(link["resolution_status"], "ambiguous");
    Ok(())
}

#[tokio::test]
async fn rest_api_marks_external_links() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("external")
        .await
        .context("create external library")?;

    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("source.md").to_string(),
            content: b"[site](https://example.com)\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write external markdown link source")?;
    let app = router(store);

    let response = app
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/external/documents/source.md/outgoing-links",
        )?)
        .await
        .context("read external outgoing links")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let link = body["links"]
        .as_array()
        .context("outgoing links response should include links")?
        .iter()
        .find(|link| {
            link["target_kind"] == "markdown_link" && link["target_text"] == "https://example.com"
        })
        .context("outgoing links should include external markdown link")?;
    assert_eq!(link["resolved"], false);
    assert_eq!(link["resolution_status"], "external");
    Ok(())
}

#[tokio::test]
async fn rest_api_supports_transaction_metadata_patch_and_move() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("txactions")
        .await
        .context("create txactions library")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/a.md").to_string(),
            content: b"draft".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write transaction metadata source document")?;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txactions/transactions",
            serde_json::json!({}),
        ))
        .await
        .context("begin txactions transaction")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"]
        .as_str()
        .context("transaction create response should include id")?;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md"),
            serde_json::json!({"wrong":true}),
        ))
        .await
        .context("send invalid transaction document patch")?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md/metadata"),
            serde_json::json!({"reviewed":true}),
        ))
        .await
        .context("patch transaction document metadata")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md/move"),
            serde_json::json!({"to_path":"published/a.md"}),
        ))
        .await
        .context("move transaction document")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txactions/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .context("commit txactions transaction")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/txactions/documents/published/a.md",
        )?)
        .await
        .context("read committed moved document")?;
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/txactions/documents?prefix=published/",
        )?)
        .await
        .context("list committed moved documents")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["metadata"]["reviewed"], true);
    Ok(())
}

#[tokio::test]
async fn rest_api_rejects_stale_transaction_commit_with_precondition_failed() -> anyhow::Result<()>
{
    let (_root, store) = open_test_store().await;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"txpreconditions"}),
        ))
        .await
        .context("create txpreconditions library through REST")?;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("base"))
                .context("build base document PUT request")?,
        )
        .await
        .context("write base txpreconditions document")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txpreconditions/transactions",
            serde_json::json!({}),
        ))
        .await
        .context("begin txpreconditions transaction")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"]
        .as_str()
        .context("transaction create response should include id")?;

    let staged_uri = format!("/v1/libraries/txpreconditions/transactions/{tx}/documents/docs/a.md");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(staged_uri)
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("staged"))
                .context("build staged transaction document PUT request")?,
        )
        .await
        .context("stage txpreconditions document update")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("newer"))
                .context("build newer document PUT request")?,
        )
        .await
        .context("write newer txpreconditions document outside transaction")?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txpreconditions/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .context("commit stale txpreconditions transaction")?;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/txpreconditions/documents/docs/a.md",
        )?)
        .await
        .context("read txpreconditions document after stale commit")?;
    assert_eq!(response.status(), StatusCode::OK);
    // Normalized by the Phase 4 reconciled markdown write.
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .context("read txpreconditions response body")?,
        "newer\n"
    );

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txpreconditions/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .context("rollback stale txpreconditions transaction")?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn rest_api_scopes_transaction_routes_to_the_url_library() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let library = store
        .create_library("txscope")
        .await
        .context("create txscope library")?;
    store
        .create_library("other")
        .await
        .context("create other library")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/a.md").to_string(),
            content: b"draft".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("seed txscope draft document")?;
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txscope/transactions",
            serde_json::json!({}),
        ))
        .await
        .context("begin txscope transaction")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"]
        .as_str()
        .context("transaction create response should include id")?;

    let leak_uri = format!("/v1/libraries/other/transactions/{tx}/documents/drafts/leak.md");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(leak_uri)
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("leak"))
                .context("build cross-library transaction PUT request")?,
        )
        .await
        .context("attempt cross-library transaction PUT")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md/metadata"),
            serde_json::json!({"wrong_library":true}),
        ))
        .await
        .context("attempt cross-library transaction metadata patch")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md/move"),
            serde_json::json!({"to_path":"published/a.md"}),
        ))
        .await
        .context("attempt cross-library transaction move")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md"),
            serde_json::json!({}),
        ))
        .await
        .context("attempt cross-library transaction delete")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .context("attempt cross-library transaction commit")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .context("attempt cross-library transaction rollback")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txscope/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .context("rollback txscope transaction")?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

/// RawDocuments keep the untouched byte path: bytes round-trip exactly and
/// no block tables are touched.
#[tokio::test]
async fn raw_document_put_bypasses_the_block_model_entirely() -> anyhow::Result<()> {
    let (_root, app, store) = block_test_app().await;
    let bytes: Vec<u8> = vec![0u8, 159, 146, 150, 13, 10, 0];
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/data.bin")
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(bytes.clone()))
                .context("build raw document PUT request")?,
        )
        .await
        .context("write raw document through REST")?;
    assert_eq!(response.status(), StatusCode::OK);
    let document = store
        .get_document("blocks", "data.bin")
        .await
        .context("read stored raw document")?;
    assert_eq!(document.content, bytes);
    assert_eq!(
        store
            .load_block_tree(&document.id)
            .await
            .context("load raw document block projection")?,
        Vec::<quarry_collab_codec::BlockRow>::new()
    );
    Ok(())
}

/// A metadata patch is frontmatter-only: it must NOT destroy the block
/// projection. Rows, ids, review anchors, and conflict artifacts all survive;
/// only the rendered frontmatter (and the version clock) moves.
#[tokio::test]
async fn metadata_patch_preserves_rows_anchors_and_conflict_items() -> anyhow::Result<()> {
    let (_root, app, store) = block_test_app().await;
    put_block_markdown(&app, "meta.md", "# Title\n\nAlpha.\n").await;
    let tree = get_block_tree(&app, "meta.md").await;
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .context("block tree response should include blocks array")?
        .iter()
        .map(|block| {
            block["block_id"]
                .as_str()
                .context("block row should include block_id")
                .map(str::to_string)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let title_block_id = ids
        .first()
        .context("metadata test document should include title block")?;
    let body_block_id = ids
        .get(1)
        .context("metadata test document should include body block")?;
    let ops = serde_json::json!([
        {"op": "comment.add", "block_id": body_block_id, "start": 0, "end": 5, "body": "keep"},
        {"op": "conflict.add", "after_block_id": title_block_id,
         "base_markdown": "Old.\n", "incoming_markdown": "New.\n",
         "canonical_markdown": "Alpha.\n"}
    ]);
    commit_block_transaction(&app, "meta.md", block_tx("tx-meta-anchor", ops)).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/blocks/documents/meta.md/metadata",
            serde_json::json!({"title": "Patched Title", "rank": 7}),
        ))
        .await
        .context("patch metadata for block document")?;
    assert_eq!(response.status(), StatusCode::OK);
    let outcome = response_json(response).await;
    assert_eq!(outcome["version"]["metadata"]["title"], "Patched Title");

    // The projection survived: same block ids, anchored comment still open,
    // conflict artifact intact.
    let tree = get_block_tree(&app, "meta.md").await;
    let ids_after: Vec<String> = tree["blocks"]
        .as_array()
        .context("block tree response after metadata patch should include blocks array")?
        .iter()
        .map(|block| {
            block["block_id"]
                .as_str()
                .context("post-patch block row should include block_id")
                .map(str::to_string)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(ids_after, ids);
    let review = get_block_review(&app, "meta.md", false).await;
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(
        review["comments"][0]["anchor"]["blockId"]
            .as_str()
            .context("comment anchor should include block id")?,
        body_block_id
    );
    assert_eq!(review["conflicts"][0]["incomingMarkdown"], "New.\n");
    // The new frontmatter rides in the normalized content.
    let content = get_document_markdown(&app, "meta.md").await;
    assert!(
        content.starts_with("---\n"),
        "frontmatter present: {content}"
    );
    assert!(content.contains("title: Patched Title"));
    assert!(content.ends_with("# Title\n\nAlpha.\n"));
    // Rows persist in storage too.
    let document_id = store
        .head_document("blocks", "meta.md")
        .await
        .context("read metadata-patched document head")?
        .id;
    assert_eq!(
        store
            .load_block_tree(&document_id)
            .await
            .context("load metadata-patched block tree from storage")?
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn library_agent_prompt_returns_connect_instructions() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "notes/live%20doc.md", "hello").await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/blocks/documents/notes/live%20doc.md/agent-prompt?token=invite-token")
                .header(header::HOST, "quarry.example.com")
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await?;
    let prompt = String::from_utf8(body.to_vec())?;
    assert!(prompt.contains(
        "https://quarry.example.com/lib/blocks/documents/notes/live%20doc.md?token=invite-token"
    ));
    assert!(prompt.contains("Library: blocks"));
    assert!(prompt.contains("trusted-localhost"));
    assert!(prompt.contains("Connected in Quarry and ready."));
    Ok(())
}

#[tokio::test]
async fn library_agent_prompt_requires_token() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "notes/live.md", "hello").await;

    let response = app
        .clone()
        .oneshot(empty_request(
            Method::GET,
            "/v1/libraries/blocks/documents/notes/live.md/agent-prompt",
        )?)
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    Ok(())
}
