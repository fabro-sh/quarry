use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use quarry_core::DocumentSource;
use quarry_server::router;
use quarry_storage::{QuarryStore, StoreConfig, StoreEventKind};
use serde_json::Value;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tower::ServiceExt;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;

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

#[tokio::test]
async fn rest_api_supports_documents_transactions_etags_and_openapi() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

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
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "one"
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
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/share"].is_object());
    assert!(
        openapi["paths"]["/v1/libraries/{library}/documents/{path}/share/{token}/revoke"]
            .is_object()
    );
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/edit"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/ops"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/presence"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/metadata"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/events/pending"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/events/ack"].is_object());
    assert!(openapi["paths"]
        ["/v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata"]
        .is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["post"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["get"].is_object());
}

#[tokio::test]
async fn agent_snapshot_exposes_snapshot_scoped_block_refs() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agent").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/one.md",
            b"# Title\n\nFirst paragraph.\n\nSecond paragraph.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agent/documents/notes/one.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let base_token = format!("\"{}\"", written.version.id);
    assert_eq!(body["documentId"], written.document.id);
    assert_eq!(body["baseToken"], base_token);
    assert_eq!(body["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(body["blocks"][0]["markdown"], "# Title\n\n");
    assert_eq!(body["blocks"][0]["ref"]["baseToken"], base_token);
    assert_eq!(body["blocks"][0]["ref"]["ordinal"], 0);
    assert_eq!(
        body["blocks"][0]["ref"]["contentHash"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
}

#[tokio::test]
async fn agent_edit_applies_block_ops_with_dry_run_stale_base_and_idempotency() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentedit").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/one.md",
            b"One\n\nTwo\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentedit/documents/notes/one.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap().to_string();
    let first_ref = snapshot["blocks"][0]["ref"].clone();
    let edit = serde_json::json!({
        "baseToken": base_token,
        "operations": [{
            "op": "replace_block",
            "ref": first_ref,
            "block": { "markdown": "Changed\n\n" }
        }]
    });

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentedit/documents/notes/one.md/edit?dryRun=1",
            edit.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["markdown"], "Changed\n\nTwo\n");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentedit/documents/notes/one.md/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "replace-first")
                .body(Body::from(edit.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first_edit_etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], false);
    assert_eq!(body["outcome"]["document"]["path"], "notes/one.md");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentedit/documents/notes/one.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "Changed\n\nTwo\n"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentedit/documents/notes/one.md/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "replace-first")
                .body(Body::from(edit.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], first_edit_etag);

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentedit/documents/notes/one.md/edit",
            edit,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body: Value = response_json(response).await;
    assert!(body["error"].as_str().unwrap().contains("STALE_BASE"));
}

#[tokio::test]
async fn agent_edit_supports_insert_and_delete_block_ops() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentops").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/ops.md",
            b"One\n\nTwo\n\nThree\n\nFour\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentops/documents/notes/ops.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let edit = serde_json::json!({
        "baseToken": snapshot["baseToken"].as_str().unwrap(),
        "operations": [
            {
                "op": "insert_before",
                "ref": snapshot["blocks"][1]["ref"].clone(),
                "block": { "markdown": "Before two\n\n" }
            },
            {
                "op": "insert_after",
                "ref": snapshot["blocks"][2]["ref"].clone(),
                "block": { "markdown": "After three\n\n" }
            },
            {
                "op": "delete_block",
                "ref": snapshot["blocks"][3]["ref"].clone()
            }
        ]
    });

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentops/documents/notes/ops.md/edit?dryRun=true",
            edit,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(
        body["markdown"],
        "One\n\nBefore two\n\nTwo\n\nThree\n\nAfter three\n\n"
    );
}

#[tokio::test]
async fn agent_ops_add_comments_and_suggestions_with_review_endmatter() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreview").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"One target here.\n\nSecond paragraph.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreview/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreview/documents/notes/review.md/ops?dryRun=1",
            serde_json::json!({
                "baseToken": base_token,
                "op": "comment.add",
                "id": "c1",
                "ref": first_ref.clone(),
                "quote": "target",
                "body": "Needs support.",
                "by": "ai:codex"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], true);
    let markdown = body["markdown"].as_str().unwrap();
    assert!(markdown.contains("One {==target==}{>>Needs support.<<}{#c1} here."));
    assert!(markdown.contains("comments:"));
    assert!(markdown.contains("by: ai:codex"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreview/documents/notes/review.md/ops",
            serde_json::json!({
                "baseToken": base_token,
                "op": "suggestion.add",
                "id": "s1",
                "kind": "substitution",
                "ref": first_ref,
                "quote": "target",
                "content": "focus",
                "by": "ai:codex"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    assert_eq!(body["id"], "s1");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreview/documents/notes/review.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let markdown = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(markdown.contains("One {~~target~>focus~~}{#s1} here."));
    assert!(markdown.contains("suggestions:"));
    assert!(markdown.contains("s1:"));
    assert!(etag.starts_with('"'));
}

#[tokio::test]
async fn agent_ops_accept_reject_and_resolve_review_marks() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreview2").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"Keep {++added++}{#s1} and drop {--bad--}{#s2}. See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\nsuggestions:\n  s1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n  s2:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);
    let base_token = format!("\"{}\"", written.version.id);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreview2/documents/notes/review.md/ops",
            serde_json::json!({"baseToken": base_token, "op": "suggestion.accept", "id": "s1"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let base_token = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreview2/documents/notes/review.md/ops",
            serde_json::json!({"baseToken": base_token, "op": "suggestion.reject", "id": "s2"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let base_token = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreview2/documents/notes/review.md/ops",
            serde_json::json!({"baseToken": base_token, "op": "comment.resolve", "id": "c1"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreview2/documents/notes/review.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let markdown = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(markdown.contains("Keep added and drop bad."));
    assert!(!markdown.contains("{++added++}{#s1}"));
    assert!(!markdown.contains("{--bad--}{#s2}"));
    assert!(!markdown.contains("suggestions:"));
    assert!(markdown.contains("status: resolved"));
}

#[tokio::test]
async fn agent_presence_records_status_by_document() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("presence").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "live.md",
            b"hello".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "other.md",
            b"other".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let other_library = store.create_library("presence-other").await.unwrap();
    store
        .put_document(
            &other_library.slug,
            "live.md",
            b"other library".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
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
async fn agent_discovery_endpoints_expose_skill_docs_and_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/quarry.SKILL.md")
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
                .uri("/agent-docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/.well-known/agent.json")
                .header(header::HOST, "127.0.0.1:7831")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["api_base"], "http://127.0.0.1:7831/v1");
    assert_eq!(body["docs_url"], "http://127.0.0.1:7831/agent-docs");
    assert_eq!(body["skill_url"], "http://127.0.0.1:7831/quarry.SKILL.md");
    assert_eq!(body["openapi_url"], "http://127.0.0.1:7831/v1/openapi.json");
    assert!(body["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "presence"));
    assert!(body["auth_note"]
        .as_str()
        .unwrap()
        .contains("trusted-localhost"));
    assert_eq!(body["auth"]["mode"], "trusted_localhost");
    assert!(body["presence_statuses"].as_array().unwrap().len() >= 6);
    assert!(body["edit_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "replace_block"));
    assert!(body["ops_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "comment.add"));
    assert_eq!(
        body["endpoints"]["snapshot"]["url"],
        "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/snapshot"
    );
}

#[tokio::test]
async fn collab_share_endpoints_mint_list_and_revoke_invite_tokens() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("shares").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "live.md",
            b"hello".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
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
    assert!(token["revoked_at"].as_str().is_some());
}

#[tokio::test]
async fn agent_events_pending_and_ack_expose_sparse_event_signals() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
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
                    && event["data"]["version_id"].as_str().is_some()
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
async fn collab_websocket_accepts_yjs_updates_by_document_id() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/doc-route"))
            .await
            .unwrap();
    let update = vec![
        1, 1, 7, 0, 4, 1, 7, 99, 111, 110, 116, 101, 110, 116, 5, 104, 101, 108, 108, 111, 0,
    ];
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::Update(update))
                .encode_v1()
                .into(),
        ))
        .await
        .unwrap();

    let reply = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let bytes = match reply {
        TungsteniteMessage::Binary(bytes) => bytes,
        other => panic!("expected binary yjs reply, got {other:?}"),
    };
    let message = YMessage::decode_v1(bytes.as_ref()).unwrap();
    assert!(matches!(message, YMessage::Sync(SyncMessage::Update(_))));

    server.abort();
}

#[tokio::test]
async fn document_put_events_echo_collab_session_id() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("collab-events").await.unwrap();
    let mut events = store.subscribe_events();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/collab-events/documents/live.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("X-Quarry-Collab-Session-Id", "browser:session-1")
                .body(Body::from("live"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let event = timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == StoreEventKind::DocumentPut {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        event.collab_session_id.as_deref(),
        Some("browser:session-1")
    );
}

#[tokio::test]
async fn document_put_with_collab_session_marks_recovery_state_clean() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("collab-clean").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "live.md",
            b"old".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_collab_recovery_state(
            &written.document.id,
            Some(written.version.id.clone()),
            vec![1, 2, 3],
            true,
        )
        .await
        .unwrap();
    let app = router(store.clone());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/collab-clean/documents/live.md")
                .header(header::IF_MATCH, format!("\"{}\"", written.version.id))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("X-Quarry-Collab-Session-Id", "browser:session-1")
                .body(Body::from("new"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let next_version = body["version"]["id"].as_str().unwrap();

    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!recovery.dirty);
    assert_eq!(recovery.base_version_id.as_deref(), Some(next_version));
}

#[tokio::test]
async fn rest_api_supports_browser_search_links_versions_and_events() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("browser").await.unwrap();
    let first_intro = store
        .put_document(
            &library.slug,
            "intro.md",
            b"# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\n".to_vec(),
            serde_json::json!({"title":"Intro","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "daily.md",
            b"# Daily\n\nBacklinked target with [[Chain]].\n".to_vec(),
            serde_json::json!({"title":"Daily","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "guide.md",
            b"# Guide\n".to_vec(),
            serde_json::json!({
                "aliases": ["Manual Alias"],
                "title":"Guide",
                "content_type":"text/markdown"
            }),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let latest_intro = store
        .put_document(
            &library.slug,
            "intro.md",
            b"# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\nupdated browser body with unique-search term.\n".to_vec(),
            serde_json::json!({"title":"Intro","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::IfMatch(first_intro.version.id.clone()),
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "chain.md",
            b"# Chain\n".to_vec(),
            serde_json::json!({"title":"Chain","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "projects/roadmap.md",
            b"# Roadmap\n".to_vec(),
            serde_json::json!({"title":"Roadmap","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "projects/brief.md",
            b"# Brief\n\nSee [[Roadmap]] and #planning.\n".to_vec(),
            serde_json::json!({"title":"Brief","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
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
    assert!(body["results"][0]["matched_fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "body"));
    assert!(body["results"][0]["snippet"]
        .as_str()
        .unwrap()
        .contains("unique-search"));

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
    assert!(links
        .iter()
        .any(|link| link["target_kind"] == "tag" && link["target_text"] == "planning"));

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
    assert!(body["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["src_path"] == "intro.md"));

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
    assert!(body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["path"] == "intro.md"));
    assert!(body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| { edge["source_path"] == "intro.md" && edge["target_path"] == "daily.md" }));
    assert!(!body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["path"] == "chain.md"));

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
    assert!(body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["path"] == "chain.md"));
    assert!(body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| { edge["source_path"] == "daily.md" && edge["target_path"] == "chain.md" }));

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
    assert!(nodes
        .iter()
        .all(|node| node["path"].as_str().unwrap().starts_with("projects/")));
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
    assert!(edges
        .iter()
        .all(|edge| edge["target_kind"] == "tag" && edge["target_text"] == "planning"));

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
    assert!(body["content"]
        .as_str()
        .unwrap()
        .contains("[[Daily|today]]"));

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
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\n"
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
    assert!(response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

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
    assert!(response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

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
    assert!(openapi["paths"]["/v1/events"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/events/stream"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/events/pending"].is_object());
    assert_schema_enum_contains(
        &openapi,
        &openapi["components"]["schemas"]["AgentBlockOperation"]["properties"]["op"],
        &[
            "replace_block",
            "insert_before",
            "insert_after",
            "delete_block",
        ],
    );
    assert_schema_enum_contains(
        &openapi,
        &openapi["components"]["schemas"]["AgentOpsRequest"]["properties"]["op"],
        &[
            "comment.add",
            "suggestion.add",
            "suggestion.accept",
            "suggestion.reject",
            "comment.resolve",
            "accept",
            "reject",
        ],
    );
    assert_schema_enum_contains(
        &openapi,
        &openapi["components"]["schemas"]["AgentOpsRequest"]["properties"]["kind"],
        &["insert", "delete", "remove", "replace", "substitution"],
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
    assert_path_parameter_enum_contains(
        &openapi,
        "/v1/libraries/{library}/documents/{path}/edit",
        "post",
        "dryRun",
        &["1", "true", "yes", "0", "false", "no"],
    );
    assert_path_parameter_enum_contains(
        &openapi,
        "/v1/libraries/{library}/documents/{path}/ops",
        "post",
        "dryRun",
        &["1", "true", "yes", "0", "false", "no"],
    );
}

#[tokio::test]
async fn version_history_includes_transaction_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"versionmeta"}),
        ))
        .await
        .unwrap();
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
                    "/v1/libraries/versionmeta/transactions/{tx}/documents/meta.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("# Meta\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/versionmeta/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/versionmeta/documents/meta.md/versions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["transaction_source"], "rest");
    assert_eq!(body[0]["transaction_actor"], "Avery");
    assert_eq!(body[0]["transaction_message"], "Imported from Git");
    assert_eq!(body[0]["transaction_provenance"]["remote"], "origin/main");
}

#[tokio::test]
async fn rest_api_supports_move_metadata_and_conflict_lookup_endpoints() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("actions").await.unwrap();
    store.create_library("other").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "a.md",
            b"hello".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let sibling = store
        .put_document(
            &library.slug,
            "a.conflict.md",
            b"git version".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Git,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let conflict = store
        .record_conflict(
            &library.slug,
            "a.md",
            Some(written.version.id.clone()),
            Some(sibling.version.id.clone()),
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/actions/documents/a.md/move",
            serde_json::json!({"to_path":"b.md"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/actions/documents/b.md/metadata",
            serde_json::json!({"reviewed":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/actions/documents/b.md",
            serde_json::json!({"wrong":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/actions/documents/b.md")
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
                .uri(format!("/v1/libraries/actions/conflicts/{}", conflict.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["id"], conflict.id);
    assert_eq!(body["conflict_path"], "a.conflict.md");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/other/conflicts/{}", conflict.id))
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
            &format!("/v1/libraries/other/conflicts/{}/resolve", conflict.id),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/actions/conflicts/{}/resolve", conflict.id),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["status"], "resolved");
    assert!(body["resolved_at"].as_str().is_some());
}

#[tokio::test]
async fn rest_api_marks_ambiguous_links() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("ambiguous").await.unwrap();

    for path in ["alpha/target.md", "omega/target.md"] {
        store
            .put_document(
                &library.slug,
                path,
                b"# Target\n".to_vec(),
                serde_json::json!({"content_type": "text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                quarry_core::WritePrecondition::None,
            )
            .await
            .unwrap();
    }
    store
        .put_document(
            &library.slug,
            "source.md",
            b"See [[target]].\n".to_vec(),
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/ambiguous/documents/source.md/outgoing-links")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let link = body["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["target_kind"] == "wiki_link" && link["target_text"] == "target")
        .unwrap();
    assert_eq!(link["target_path"], Value::Null);
    assert_eq!(link["resolved"], false);
    assert_eq!(link["resolution_status"], "ambiguous");
}

#[tokio::test]
async fn rest_api_supports_transaction_metadata_patch_and_move() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("txactions").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/a.md",
            b"draft".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txactions/transactions",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md"),
            serde_json::json!({"wrong":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md/metadata"),
            serde_json::json!({"reviewed":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md/move"),
            serde_json::json!({"to_path":"published/a.md"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txactions/transactions/{tx}/commit"),
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
                .uri("/v1/libraries/txactions/documents/published/a.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/txactions/documents?prefix=published/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["metadata"]["reviewed"], true);
}

#[tokio::test]
async fn rest_api_rejects_stale_transaction_commit_with_precondition_failed() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"txpreconditions"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("base"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txpreconditions/transactions",
            serde_json::json!({}),
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
                    "/v1/libraries/txpreconditions/transactions/{tx}/documents/docs/a.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("staged"))
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
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("newer"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txpreconditions/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "newer"
    );

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txpreconditions/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn rest_api_scopes_transaction_routes_to_the_url_library() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("txscope").await.unwrap();
    store.create_library("other").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/a.md",
            b"draft".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txscope/transactions",
            serde_json::json!({}),
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
                    "/v1/libraries/other/transactions/{tx}/documents/drafts/leak.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("leak"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md/metadata"),
            serde_json::json!({"wrong_library":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md/move"),
            serde_json::json!({"to_path":"published/a.md"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txscope/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

fn json_request(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}
