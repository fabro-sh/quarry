use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use quarry_collab_codec::{
    build_nodes, encode_update_v1_from_built, review_markdown_to_slate, xmltext_to_slate, Node,
};
use quarry_core::DocumentSource;
use quarry_server::router;
use quarry_storage::{QuarryStore, StoreConfig, StoreEvent, StoreEventKind};
use serde_json::Value;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tower::ServiceExt;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Any, Doc, Map, OffsetKind, Options, Out, ReadTxn, Transact, Update, XmlTextRef};

const COLLAB_ROOT: &str = "content";
const INJECTION_ROOT: &str = "__quarry_injection";
const REVIEW_ROOT: &str = "review";

#[tokio::test]
async fn rest_api_attaches_and_preserves_request_ids() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_id = first
        .headers()
        .get("x-quarry-request-id")
        .expect("response should include a generated request id")
        .to_str()
        .unwrap()
        .to_string();
    uuid::Uuid::parse_str(&first_id).expect("generated request id should be a UUID");

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let second_id = second
        .headers()
        .get("x-quarry-request-id")
        .expect("response should include a generated request id")
        .to_str()
        .unwrap()
        .to_string();
    uuid::Uuid::parse_str(&second_id).expect("generated request id should be a UUID");
    assert_ne!(first_id, second_id);

    let supplied = "req-from-client";
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .header("x-quarry-request-id", supplied)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["x-quarry-request-id"], supplied);
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

fn ops_request(base_token: impl serde::Serialize, operation: Value) -> Value {
    serde_json::json!({
        "baseToken": base_token,
        "operations": [operation]
    })
}

fn ops_request_by(base_token: impl serde::Serialize, by: &str, operation: Value) -> Value {
    serde_json::json!({
        "baseToken": base_token,
        "by": by,
        "operations": [operation]
    })
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
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"]["get"].is_object());
    assert!(
        openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"]["post"].is_object()
    );
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/share"].is_object());
    assert!(
        openapi["paths"]["/v1/libraries/{library}/documents/{path}/share/{token}/revoke"]
            .is_object()
    );
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/edit"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/ops"].is_object());
    assert!(
        openapi["components"]["schemas"]["AgentEditResponse"]["properties"]["nextBaseToken"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentOpsResponse"]["properties"]["nextBaseToken"]
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
    assert_eq!(body["documentId"], written.document.id);
    assert_eq!(body["baseToken"], written.version.id);
    assert_eq!(body["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(body["blocks"][0]["markdown"], "# Title\n\n");
    assert!(body["blocks"][0]["ref"].get("baseToken").is_none());
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
async fn agent_review_lists_open_comments_replies_and_suggestions() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreviewread").await.unwrap();
    let markdown = "Alpha {==target==}{>>Needs work.<<}{#c1} and {==done==}{>>Fixed.<<}{#c2}.\n\nUse {~~old~>new~~}{#s1} wording and `{++literal++}{#s_code}`.\n\n```text\n{==ignored==}{>>Nope<<}{#c_code}\n{--gone--}{#s_code2}\n```\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user:a\n  c2:\n    at: \"2026-01-02T00:00:00.000Z\"\n    by: user:b\n    status: resolved\n  c_code:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: user:code\n  r1:\n    at: \"2026-01-01T01:00:00.000Z\"\n    body: Reply body.\n    by: ai:codex\n    re: c1\nsuggestions:\n  s1:\n    at: \"2026-01-03T00:00:00.000Z\"\n    by: ai:codex\n  s_code:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: ai:code\n  s_code2:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: ai:code\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
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
                .uri("/v1/libraries/agentreviewread/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewread/documents/notes/review.md/review")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["documentId"], written.document.id);
    assert_eq!(body["baseToken"], written.version.id);
    assert_eq!(body["comments"].as_array().unwrap().len(), 1);
    assert_eq!(body["comments"][0]["id"], "c1");
    assert_eq!(body["comments"][0]["status"], "open");
    assert_eq!(body["comments"][0]["by"], "user:a");
    assert_eq!(body["comments"][0]["at"], "2026-01-01T00:00:00.000Z");
    assert_eq!(body["comments"][0]["ref"], snapshot["blocks"][0]["ref"]);
    assert_eq!(body["comments"][0]["quote"], "target");
    assert_eq!(body["comments"][0]["body"], "Needs work.");
    assert_eq!(body["comments"][0]["replies"].as_array().unwrap().len(), 1);
    assert_eq!(body["comments"][0]["replies"][0]["id"], "r1");
    assert_eq!(body["comments"][0]["replies"][0]["status"], "open");
    assert_eq!(body["comments"][0]["replies"][0]["by"], "ai:codex");
    assert_eq!(body["comments"][0]["replies"][0]["body"], "Reply body.");

    assert_eq!(body["suggestions"].as_array().unwrap().len(), 1);
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

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewread/documents/notes/review.md/review?includeResolved=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["comments"].as_array().unwrap().len(), 2);
    assert_eq!(body["comments"][1]["id"], "c2");
    assert_eq!(body["comments"][1]["status"], "resolved");
    assert_eq!(body["comments"][1]["quote"], "done");
    assert_eq!(body["comments"][1]["body"], "Fixed.");
}

#[tokio::test]
async fn agent_review_omits_suggestions_after_accept_and_reject() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreviewgone").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"Keep {++added++}{#s1} and drop {--bad--}{#s2}.\n\n---\nsuggestions:\n  s1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n  s2:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n".to_vec(),
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
            "/v1/libraries/agentreviewgone/documents/notes/review.md/ops",
            ops_request(
                format!("\"{}\"", written.version.id),
                serde_json::json!({"op": "suggestion.accept", "id": "s1"}),
            ),
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
            "/v1/libraries/agentreviewgone/documents/notes/review.md/ops",
            ops_request(
                &base_token,
                serde_json::json!({"op": "suggestion.reject", "id": "s2"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewgone/documents/notes/review.md/review")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["suggestions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn agent_review_matches_snapshot_errors_for_missing_and_non_markdown() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreviewerrors").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/plain.txt",
            b"plain text".to_vec(),
            serde_json::json!({"content_type":"text/plain"}),
            "text/plain",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewerrors/documents/notes/missing.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let review = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewerrors/documents/notes/missing.md/review")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(review.status(), snapshot.status());
    assert_eq!(response_json(review).await, response_json(snapshot).await);

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewerrors/documents/notes/plain.txt/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let review = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewerrors/documents/notes/plain.txt/review")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(review.status(), snapshot.status());
    assert_eq!(response_json(review).await, response_json(snapshot).await);
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
    assert!(body.get("nextBaseToken").is_none());

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
    let first_edit_version_id = body["outcome"]["version"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        body["nextBaseToken"].as_str().unwrap(),
        first_edit_version_id.as_str()
    );
    assert_eq!(first_edit_etag, format!("\"{first_edit_version_id}\""));
    assert_eq!(body["outcome"]["document"]["path"], "notes/one.md");
    // No browser is connected in this headless test, so the edit reports that it
    // could not be injected into a live room (vs falling back silently).
    assert_eq!(body["injection"], "no_live_room");

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
    let body: Value = response_json(response).await;
    assert_eq!(
        body["nextBaseToken"].as_str().unwrap(),
        first_edit_version_id.as_str()
    );

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
async fn agent_edit_accepts_block_refs_without_content_hash() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agenteditordinal").await.unwrap();
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
                .uri("/v1/libraries/agenteditordinal/documents/notes/one.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let edit = serde_json::json!({
        "baseToken": snapshot["baseToken"].as_str().unwrap(),
        "operations": [{
            "op": "replace_block",
            "ref": { "ordinal": 0 },
            "block": { "markdown": "Changed\n\n" }
        }]
    });

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agenteditordinal/documents/notes/one.md/edit",
            edit,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agenteditordinal/documents/notes/one.md")
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
}

#[tokio::test]
async fn agent_edit_rejects_wrong_content_hash_when_present() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agenteditwronghash").await.unwrap();
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
                .uri("/v1/libraries/agenteditwronghash/documents/notes/one.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let edit = serde_json::json!({
        "baseToken": snapshot["baseToken"].as_str().unwrap(),
        "operations": [{
            "op": "replace_block",
            "ref": {
                "ordinal": 0,
                "contentHash": "0000000000000000000000000000000000000000000000000000000000000000"
            },
            "block": { "markdown": "Changed\n\n" }
        }]
    });

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agenteditwronghash/documents/notes/one.md/edit",
            edit,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body: Value = response_json(response).await;
    assert!(body["error"].as_str().unwrap().contains("STALE_BASE"));
}

#[tokio::test]
async fn agent_edit_accepts_raw_quoted_and_weak_base_tokens() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agenttokens").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/tokens.md",
            b"One\n\nTwo\n\nThree\n".to_vec(),
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
                .uri("/v1/libraries/agenttokens/documents/notes/tokens.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let raw_base_token = snapshot["baseToken"].as_str().unwrap().to_string();
    assert!(!raw_base_token.contains('"'));
    let first_ref = snapshot["blocks"][0]["ref"].clone();
    let second_ref = snapshot["blocks"][1]["ref"].clone();
    let third_ref = snapshot["blocks"][2]["ref"].clone();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agenttokens/documents/notes/tokens.md/edit",
            serde_json::json!({
                "baseToken": raw_base_token,
                "operations": [{
                    "op": "replace_block",
                    "ref": first_ref,
                    "block": { "markdown": "Raw\n\n" }
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let next_base_token = body["nextBaseToken"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agenttokens/documents/notes/tokens.md/edit",
            serde_json::json!({
                "baseToken": format!("\"{next_base_token}\""),
                "operations": [{
                    "op": "replace_block",
                    "ref": second_ref,
                    "block": { "markdown": "Quoted\n\n" }
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let next_base_token = body["nextBaseToken"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agenttokens/documents/notes/tokens.md/edit",
            serde_json::json!({
                "baseToken": format!("W/\"{next_base_token}\""),
                "operations": [{
                    "op": "replace_block",
                    "ref": third_ref,
                    "block": { "markdown": "Weak\n" }
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(!body["nextBaseToken"].as_str().unwrap().contains('"'));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agenttokens/documents/notes/tokens.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "Raw\n\nQuoted\n\nWeak\n"
    );
}

#[tokio::test]
async fn agent_edit_rejects_malformed_base_tokens_but_preserves_stale_base() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbadtoken").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/bad.md",
            b"One\n".to_vec(),
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
                .uri("/v1/libraries/agentbadtoken/documents/notes/bad.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let block_ref = snapshot["blocks"][0]["ref"].clone();

    let malformed_tokens = [
        "",
        " ",
        "\"\"",
        "\"unterminated",
        "unterminated\"",
        "\"\"nested\"\"",
        "has\"quote",
        "W/token",
        "W/\"\"",
        "W/\"has\"quote\"",
    ];
    for base_token in malformed_tokens {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/v1/libraries/agentbadtoken/documents/notes/bad.md/edit",
                serde_json::json!({
                    "baseToken": base_token,
                    "operations": [{
                        "op": "replace_block",
                        "ref": block_ref.clone(),
                        "block": { "markdown": "Changed\n" }
                    }]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{base_token:?}");
        let body: Value = response_json(response).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("INVALID_BASE_TOKEN"),
            "{body:?}"
        );
    }

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbadtoken/documents/notes/bad.md/edit",
            serde_json::json!({
                "baseToken": "well-formed-but-wrong",
                "operations": [{
                    "op": "replace_block",
                    "ref": block_ref.clone(),
                    "block": { "markdown": "Changed\n" }
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body: Value = response_json(response).await;
    assert!(body["error"].as_str().unwrap().contains("STALE_BASE"));
}

#[tokio::test]
async fn agent_edit_idempotency_hash_normalizes_base_token_shape() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentidempotenttoken").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/idempotent.md",
            b"One\n".to_vec(),
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
                .uri("/v1/libraries/agentidempotenttoken/documents/notes/idempotent.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let block_ref = snapshot["blocks"][0]["ref"].clone();
    let operation = serde_json::json!({
        "op": "replace_block",
        "ref": block_ref,
        "block": { "markdown": "Changed\n" }
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentidempotenttoken/documents/notes/idempotent.md/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "same-token")
                .body(Body::from(
                    serde_json::json!({
                        "baseToken": base_token,
                        "operations": [operation.clone()]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first_etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let first_body: Value = response_json(response).await;

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentidempotenttoken/documents/notes/idempotent.md/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "same-token")
                .body(Body::from(
                    serde_json::json!({
                        "baseToken": format!("\"{base_token}\""),
                        "operations": [operation]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], first_etag);
    let body: Value = response_json(response).await;
    assert_eq!(body, first_body);
}

#[tokio::test]
async fn agent_edit_replace_document_supports_dry_run_commit_idempotency_stale_base_and_empty() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreplace").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/whole.md",
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
                .uri("/v1/libraries/agentreplace/documents/notes/whole.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap().to_string();
    let replacement = "# Title\n\nBody\n";
    let edit = serde_json::json!({
        "baseToken": base_token,
        "operations": [{
            "op": "replace_document",
            "markdown": replacement
        }]
    });

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreplace/documents/notes/whole.md/edit?dryRun=1",
            edit.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["markdown"], replacement);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentreplace/documents/notes/whole.md/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "replace-whole")
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
    assert_eq!(body["outcome"]["document"]["path"], "notes/whole.md");
    assert_eq!(body["injection"], "no_live_room");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreplace/documents/notes/whole.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        replacement
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreplace/documents/notes/whole.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let replaced_snapshot: Value = response_json(response).await;
    let blocks = replaced_snapshot["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["markdown"], "# Title\n\n");
    assert_eq!(blocks[1]["markdown"], "Body\n");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentreplace/documents/notes/whole.md/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "replace-whole")
                .body(Body::from(edit.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], first_edit_etag);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreplace/documents/notes/whole.md/edit",
            edit,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body: Value = response_json(response).await;
    assert!(body["error"].as_str().unwrap().contains("STALE_BASE"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreplace/documents/notes/whole.md/edit",
            serde_json::json!({
                "baseToken": replaced_snapshot["baseToken"].as_str().unwrap(),
                "operations": [{
                    "op": "replace_document",
                    "markdown": ""
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreplace/documents/notes/whole.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        ""
    );

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreplace/documents/notes/whole.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let empty_snapshot: Value = response_json(response).await;
    assert!(empty_snapshot["blocks"].as_array().unwrap().is_empty());
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
async fn agent_edit_supports_bulk_insert_blocks_and_snapshot_refs() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbulk").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/bulk.md",
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
                .uri("/v1/libraries/agentbulk/documents/notes/bulk.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let edit = serde_json::json!({
        "baseToken": snapshot["baseToken"].as_str().unwrap(),
        "operations": [{
            "op": "insert_after",
            "ref": snapshot["blocks"][0]["ref"].clone(),
            "blocks": [
                { "markdown": "A\n" },
                { "markdown": "B\n" }
            ]
        }]
    });

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbulk/documents/notes/bulk.md/edit?dryRun=true",
            edit.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["markdown"], "One\n\nA\n\nB\n\nTwo\n");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbulk/documents/notes/bulk.md/edit",
            edit,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "no_live_room");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentbulk/documents/notes/bulk.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let blocks = snapshot["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[0]["markdown"], "One\n\n");
    assert_eq!(blocks[1]["markdown"], "A\n\n");
    assert_eq!(blocks[2]["markdown"], "B\n\n");
    assert_eq!(blocks[3]["markdown"], "Two\n");
}

#[tokio::test]
async fn agent_edit_bulk_insert_before_preserves_caller_order() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbulkbefore").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/bulk.md",
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
                .uri("/v1/libraries/agentbulkbefore/documents/notes/bulk.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbulkbefore/documents/notes/bulk.md/edit?dryRun=1",
            serde_json::json!({
                "baseToken": snapshot["baseToken"].as_str().unwrap(),
                "operations": [{
                    "op": "insert_before",
                    "ref": snapshot["blocks"][1]["ref"].clone(),
                    "blocks": [
                        { "markdown": "A\n" },
                        { "markdown": "B\n" }
                    ]
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["markdown"], "One\n\nA\n\nB\n\nTwo\n");
}

#[tokio::test]
async fn agent_edit_rejects_invalid_bulk_insert_shapes() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbulkbad").await.unwrap();
    store
        .put_document(
            &library.slug,
            "bad.md",
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
                .uri("/v1/libraries/agentbulkbad/documents/bad.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();

    let invalid_cases = [
        (
            serde_json::json!({
                "op": "insert_after",
                "ref": first_ref.clone(),
                "block": { "markdown": "A\n\n" },
                "blocks": [{ "markdown": "B\n" }]
            }),
            "exactly one of block or blocks",
        ),
        (
            serde_json::json!({
                "op": "insert_after",
                "ref": first_ref.clone()
            }),
            "exactly one of block or blocks",
        ),
        (
            serde_json::json!({
                "op": "insert_after",
                "ref": first_ref.clone(),
                "blocks": []
            }),
            "blocks must not be empty",
        ),
        (
            serde_json::json!({
                "op": "replace_block",
                "ref": first_ref.clone(),
                "blocks": [{ "markdown": "A\n" }]
            }),
            "replace_block operation does not accept blocks",
        ),
        (
            serde_json::json!({
                "op": "delete_block",
                "ref": first_ref.clone(),
                "blocks": [{ "markdown": "A\n" }]
            }),
            "delete_block operation does not accept blocks",
        ),
        (
            serde_json::json!({
                "op": "insert_after",
                "ref": first_ref.clone(),
                "blocks": [{ "markdown": "A\n\nB\n" }]
            }),
            "edit block markdown must parse as one top-level block",
        ),
    ];

    for (operation, expected_error) in invalid_cases {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/v1/libraries/agentbulkbad/documents/bad.md/edit?dryRun=1",
                serde_json::json!({
                    "baseToken": base_token,
                    "operations": [operation]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response_json(response).await;
        assert!(
            body["error"].as_str().unwrap().contains(expected_error),
            "expected error to contain {expected_error:?}, got {body:?}"
        );
    }

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbulkbad/documents/bad.md/edit?dryRun=1",
            serde_json::json!({
                "baseToken": base_token,
                "operations": [
                    {
                        "op": "insert_after",
                        "ref": first_ref.clone(),
                        "block": { "markdown": "A\n\n" }
                    },
                    {
                        "op": "insert_after",
                        "ref": first_ref,
                        "block": { "markdown": "B\n\n" }
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response_json(response).await;
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("use one insert operation with blocks"));
}

#[tokio::test]
async fn agent_edit_rejects_invalid_replace_document_shapes() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreplacebad").await.unwrap();
    store
        .put_document(
            &library.slug,
            "bad.md",
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
                .uri("/v1/libraries/agentreplacebad/documents/bad.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();

    let invalid_cases = [
        (
            serde_json::json!({
                "operations": [{
                    "op": "replace_document"
                }]
            }),
            "replace_document operation missing markdown",
        ),
        (
            serde_json::json!({
                "operations": [
                    {
                        "op": "replace_document",
                        "markdown": "Replacement\n"
                    },
                    {
                        "op": "delete_block",
                        "ref": first_ref.clone()
                    }
                ]
            }),
            "replace_document must be the only operation",
        ),
        (
            serde_json::json!({
                "operations": [{
                    "op": "replace_document",
                    "ref": first_ref.clone(),
                    "markdown": "Replacement\n"
                }]
            }),
            "replace_document operation does not accept ref",
        ),
        (
            serde_json::json!({
                "operations": [{
                    "op": "replace_document",
                    "block": { "markdown": "Replacement\n" },
                    "markdown": "Replacement\n"
                }]
            }),
            "replace_document operation does not accept block",
        ),
        (
            serde_json::json!({
                "operations": [{
                    "op": "replace_document",
                    "blocks": [{ "markdown": "Replacement\n" }],
                    "markdown": "Replacement\n"
                }]
            }),
            "replace_document operation does not accept blocks",
        ),
    ];

    for (request, expected_error) in invalid_cases {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/v1/libraries/agentreplacebad/documents/bad.md/edit?dryRun=1",
                serde_json::json!({
                    "baseToken": base_token,
                    "operations": request["operations"].clone()
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response_json(response).await;
        assert!(
            body["error"].as_str().unwrap().contains(expected_error),
            "expected error to contain {expected_error:?}, got {body:?}"
        );
    }
}

#[tokio::test]
async fn agent_edit_bulk_insert_injects_into_live_collab_room() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentlivebulk").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/live.md",
            b"One\n\nTwo\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let (client_doc, update) = yjs_doc_with_markdown("One\n\nTwo\n");
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::Update(update))
                .encode_v1()
                .into(),
        ))
        .await
        .unwrap();
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentlivebulk/documents/notes/live.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: Value = response_json(snapshot).await;
    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentlivebulk/documents/notes/live.md/edit",
            serde_json::json!({
                "baseToken": snapshot["baseToken"].as_str().unwrap(),
                "operations": [{
                    "op": "insert_after",
                    "ref": snapshot["blocks"][0]["ref"].clone(),
                    "blocks": [
                        { "markdown": "A\n" },
                        { "markdown": "B\n" }
                    ]
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "injected");

    wait_for_yjs_plain_text(&mut socket, &client_doc, "OneABTwo").await;
    assert_eq!(yjs_plain_text(&client_doc), "OneABTwo");

    server.abort();
}

#[tokio::test]
async fn agent_edit_replace_document_injects_into_live_collab_room() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentlivereplace").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/live.md",
            b"One\n\nTwo\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "OneTwo");

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentlivereplace/documents/notes/live.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: Value = response_json(snapshot).await;
    let replacement = "# New\n\nFresh body\n";
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentlivereplace/documents/notes/live.md/edit",
            serde_json::json!({
                "baseToken": snapshot["baseToken"].as_str().unwrap(),
                "operations": [{
                    "op": "replace_document",
                    "markdown": replacement
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "injected");

    wait_for_yjs_plain_text(&mut socket, &client_doc, "NewFresh body").await;
    assert_eq!(yjs_plain_text(&client_doc), "NewFresh body");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentlivereplace/documents/notes/live.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        replacement
    );

    server.abort();
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
            ops_request_by(
                base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "comment.add",
                    "id": "c1",
                    "ref": first_ref.clone(),
                    "quote": "target",
                    "body": "Needs support."
                }),
            ),
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
            ops_request_by(
                base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "suggestion.add",
                    "id": "s1",
                    "kind": "substitution",
                    "ref": first_ref,
                    "quote": "target",
                    "content": "focus"
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "s1");

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
async fn agent_ops_accepts_ordinal_only_and_null_content_hash_refs() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreviewordinal").await.unwrap();
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
                .uri("/v1/libraries/agentreviewordinal/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let batch = serde_json::json!({
        "baseToken": snapshot["baseToken"].as_str().unwrap(),
        "by": "ai:codex",
        "operations": [
            {
                "op": "comment.add",
                "id": "c1",
                "ref": { "ordinal": 0 },
                "quote": "target",
                "body": "Needs support."
            },
            {
                "op": "suggestion.add",
                "id": "s1",
                "kind": "replace",
                "ref": { "ordinal": 1, "contentHash": null },
                "quote": "Second",
                "content": "Better"
            }
        ]
    });

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreviewordinal/documents/notes/review.md/ops",
            batch,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewordinal/documents/notes/review.md")
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
    assert!(markdown.contains("One {==target==}{>>Needs support.<<}{#c1} here."));
    assert!(markdown.contains("{~~Second~>Better~~}{#s1} paragraph."));
}

#[tokio::test]
async fn agent_ops_accepts_etag_shaped_base_token_for_review_operations() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreviewtoken").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"One target here.\n".to_vec(),
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
                .uri("/v1/libraries/agentreviewtoken/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreviewtoken/documents/notes/review.md/ops",
            ops_request_by(
                format!("W/\"{base_token}\""),
                "ai:codex",
                serde_json::json!({
                    "op": "comment.add",
                    "id": "c1",
                    "ref": snapshot["blocks"][0]["ref"].clone(),
                    "quote": "target",
                    "body": "Needs support."
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    let version_id = body["outcome"]["version"]["id"].as_str().unwrap();
    assert_eq!(body["nextBaseToken"].as_str().unwrap(), version_id);
    assert_eq!(etag, format!("\"{version_id}\""));
}

#[tokio::test]
async fn agent_ops_batch_applies_multiple_review_ops_atomically() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbatchreview").await.unwrap();
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
                .uri("/v1/libraries/agentbatchreview/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value = response_json(response).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();
    let second_ref = snapshot["blocks"][1]["ref"].clone();
    let batch = serde_json::json!({
        "baseToken": base_token,
        "by": "ai:codex",
        "operations": [
            {
                "op": "comment.add",
                "id": "c1",
                "ref": first_ref,
                "quote": "target",
                "body": "Needs support."
            },
            {
                "op": "suggestion.add",
                "id": "s1",
                "kind": "replace",
                "ref": second_ref,
                "quote": "Second",
                "content": "Better"
            }
        ]
    });

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbatchreview/documents/notes/review.md/ops?dryRun=1",
            batch.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], true);
    assert_eq!(
        body["results"][0],
        serde_json::json!({"op": "comment.add", "id": "c1"})
    );
    assert_eq!(
        body["results"][1],
        serde_json::json!({"op": "suggestion.add", "id": "s1"})
    );
    assert!(body.get("id").is_none());
    let markdown = body["markdown"].as_str().unwrap();
    assert!(markdown.contains("One {==target==}{>>Needs support.<<}{#c1} here."));
    assert!(markdown.contains("{~~Second~>Better~~}{#s1} paragraph."));
    assert!(markdown.contains("comments:"));
    assert!(markdown.contains("suggestions:"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbatchreview/documents/notes/review.md/ops",
            batch,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], false);
    assert_eq!(body["results"].as_array().unwrap().len(), 2);
    assert!(body.get("id").is_none());
    assert!(etag.starts_with('"'));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentbatchreview/documents/notes/review.md")
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
    assert!(markdown.contains("One {==target==}{>>Needs support.<<}{#c1} here."));
    assert!(markdown.contains("{~~Second~>Better~~}{#s1} paragraph."));
}

#[tokio::test]
async fn agent_ops_batch_validation_failure_is_atomic_and_not_idempotency_cached() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbatchatomic").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"One target here.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentbatchatomic/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: Value = response_json(snapshot).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();
    let invalid_batch = serde_json::json!({
        "baseToken": base_token,
        "by": "ai:codex",
        "operations": [
            {
                "op": "comment.add",
                "id": "c1",
                "ref": first_ref.clone(),
                "quote": "target",
                "body": "Needs support."
            },
            {
                "op": "suggestion.add",
                "id": "s1",
                "kind": "replace",
                "ref": first_ref.clone(),
                "quote": "missing",
                "content": "focus"
            }
        ]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentbatchatomic/documents/notes/review.md/ops")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "atomic-key")
                .body(Body::from(invalid_batch.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentbatchatomic/documents/notes/review.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "One target here.\n"
    );

    let valid_batch = serde_json::json!({
        "baseToken": base_token,
        "by": "ai:codex",
        "operations": [{
            "op": "comment.add",
            "id": "c1",
            "ref": first_ref,
            "quote": "target",
            "body": "Needs support."
        }]
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentbatchatomic/documents/notes/review.md/ops")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "atomic-key")
                .body(Body::from(valid_batch.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "c1");
}

#[tokio::test]
async fn agent_ops_batch_same_block_overlap_rules() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbatchoverlap").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"alpha beta gamma\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentbatchoverlap/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: Value = response_json(snapshot).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let block_ref = snapshot["blocks"][0]["ref"].clone();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbatchoverlap/documents/notes/review.md/ops?dryRun=1",
            serde_json::json!({
                "baseToken": base_token,
                "operations": [
                    {
                        "op": "comment.add",
                        "id": "c1",
                        "ref": block_ref.clone(),
                        "quote": "alpha",
                        "body": "Clarify."
                    },
                    {
                        "op": "suggestion.add",
                        "id": "s1",
                        "kind": "replace",
                        "ref": block_ref.clone(),
                        "quote": "gamma",
                        "content": "delta"
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let markdown = body["markdown"].as_str().unwrap();
    assert!(markdown.contains("{==alpha==}{>>Clarify.<<}{#c1} beta {~~gamma~>delta~~}{#s1}"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbatchoverlap/documents/notes/review.md/ops?dryRun=1",
            serde_json::json!({
                "baseToken": base_token,
                "operations": [
                    {
                        "op": "suggestion.add",
                        "id": "s2",
                        "kind": "insert",
                        "ref": block_ref.clone(),
                        "quote": "beta",
                        "content": "X"
                    },
                    {
                        "op": "suggestion.add",
                        "id": "s3",
                        "kind": "insert",
                        "ref": block_ref.clone(),
                        "quote": "beta",
                        "content": "Y"
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["markdown"]
        .as_str()
        .unwrap()
        .contains("beta{++X++}{#s2}{++Y++}{#s3}"));

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbatchoverlap/documents/notes/review.md/ops?dryRun=1",
            serde_json::json!({
                "baseToken": base_token,
                "operations": [
                    {
                        "op": "comment.add",
                        "id": "c2",
                        "ref": block_ref.clone(),
                        "quote": "beta",
                        "body": "Clarify."
                    },
                    {
                        "op": "suggestion.add",
                        "id": "s4",
                        "kind": "replace",
                        "ref": block_ref,
                        "quote": "beta",
                        "content": "theta"
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn agent_ops_idempotency_replays_same_batch_and_conflicts_on_changed_body() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbatchidempotency").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"One target here.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentbatchidempotency/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: Value = response_json(snapshot).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let block_ref = snapshot["blocks"][0]["ref"].clone();
    let batch = serde_json::json!({
        "baseToken": base_token,
        "operations": [{
            "op": "comment.add",
            "id": "c1",
            "ref": block_ref,
            "quote": "target",
            "body": "Needs support."
        }]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentbatchidempotency/documents/notes/review.md/ops")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "ops-key")
                .body(Body::from(batch.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "c1");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentbatchidempotency/documents/notes/review.md/ops")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "ops-key")
                .body(Body::from(batch.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG].to_str().unwrap(), etag);

    let mut changed_batch = batch;
    changed_batch["operations"][0]["body"] = serde_json::json!("Different body.");
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/agentbatchidempotency/documents/notes/review.md/ops")
                .header(header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", "ops-key")
                .body(Body::from(changed_batch.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn agent_ops_batch_injects_multiple_changed_blocks_into_live_room() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbatchlive").await.unwrap();
    let written = store
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
    let mut events = store.subscribe_events();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentbatchlive/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: Value = response_json(snapshot).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();
    let second_ref = snapshot["blocks"][1]["ref"].clone();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbatchlive/documents/notes/review.md/ops",
            serde_json::json!({
                "baseToken": base_token,
                "by": "ai:codex",
                "operations": [
                    {
                        "op": "comment.add",
                        "id": "c1",
                        "ref": first_ref,
                        "quote": "target",
                        "body": "Needs support."
                    },
                    {
                        "op": "suggestion.add",
                        "id": "s1",
                        "kind": "replace",
                        "ref": second_ref,
                        "quote": "Second",
                        "content": "Better"
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "injected");
    assert_eq!(body["results"].as_array().unwrap().len(), 2);

    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "c1").unwrap()["body"],
        "Needs support."
    );
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "suggestions", "s1").unwrap()["by"],
        "ai:codex"
    );

    wait_for_yjs_comment_mark(&mut socket, &client_doc, "c1").await;
    wait_for_yjs_suggestion_mark(&mut socket, &client_doc, "s1").await;
    assert!(yjs_has_comment_mark(&client_doc, "c1"));
    assert!(yjs_has_suggestion_mark(&client_doc, "s1"));

    server.abort();
}

#[tokio::test]
async fn agent_ops_batch_merges_metadata_live_patch_for_mixed_review_ops() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentbatchmetalive").await.unwrap();
    let markdown = "Keep {++added++}{#s1} and drop {--bad--}{#s2}. See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n  r1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    body: Existing reply.\n    by: user\n    re: c1\nsuggestions:\n  s1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n  s2:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let mut events = store.subscribe_events();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;
    assert!(yjs_has_suggestion_mark(&client_doc, "s1"));
    assert!(yjs_has_suggestion_mark(&client_doc, "s2"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentbatchmetalive/documents/notes/review.md/ops",
            serde_json::json!({
                "baseToken": format!("\"{}\"", written.version.id),
                "by": "ai:codex",
                "operations": [
                    {
                        "op": "comment.reply",
                        "id": "r2",
                        "parentId": "c1",
                        "body": "Following up."
                    },
                    {
                        "op": "comment.resolve",
                        "id": "c1"
                    },
                    {
                        "op": "comment.delete",
                        "id": "r1"
                    },
                    {
                        "op": "suggestion.accept",
                        "id": "s1"
                    },
                    {
                        "op": "suggestion.reject",
                        "id": "s2"
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "injected");
    assert_eq!(body["results"].as_array().unwrap().len(), 5);

    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "r2").unwrap()["body"],
        "Following up."
    );
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "r2").unwrap()["re"],
        "c1"
    );
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "c1").unwrap()["status"],
        "resolved"
    );
    assert!(review_entry_from_update(&recovery.update_v1, "comments", "r1").is_none());
    assert!(review_entry_from_update(&recovery.update_v1, "suggestions", "s1").is_none());
    assert!(review_entry_from_update(&recovery.update_v1, "suggestions", "s2").is_none());

    wait_for_yjs_sync_update(&mut socket, &client_doc).await;
    assert_eq!(
        yjs_plain_text(&client_doc),
        "Keep added and drop bad. See this."
    );
    assert!(!yjs_has_suggestion_mark(&client_doc, "s1"));
    assert!(!yjs_has_suggestion_mark(&client_doc, "s2"));

    server.abort();
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
            ops_request(
                &base_token,
                serde_json::json!({"op": "suggestion.accept", "id": "s1"}),
            ),
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
            ops_request(
                &base_token,
                serde_json::json!({"op": "suggestion.reject", "id": "s2"}),
            ),
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
            ops_request(
                &base_token,
                serde_json::json!({"op": "comment.resolve", "id": "c1"}),
            ),
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
async fn agent_review_post_processes_mixed_review_and_edit_operations() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreviewpost").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"Keep {++good++}{#s1}. {==Needs text==}{>>Expand this.<<}{#c1}\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\nsuggestions:\n  s1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n"
                .to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewpost/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: Value = response_json(snapshot).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreviewpost/documents/notes/review.md/review",
            serde_json::json!({
                "baseToken": base_token,
                "by": "Codex",
                "operations": [
                    { "op": "suggestion.accept", "id": "s1" },
                    {
                        "op": "edit.replace_block",
                        "ref": first_ref,
                        "block": { "markdown": "Keep good. Expanded text.\n" }
                    },
                    { "op": "comment.resolve", "id": "c1" }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], false);
    assert_eq!(body["results"].as_array().unwrap().len(), 3);
    assert_eq!(body["review"]["comments"], serde_json::json!([]));
    assert_eq!(body["review"]["suggestions"], serde_json::json!([]));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewpost/documents/notes/review.md")
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
    assert!(markdown.contains("Keep good. Expanded text."));
    assert!(!markdown.contains("{++good++}{#s1}"));
    assert!(!markdown.contains("{==Needs text==}"));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewpost/documents/notes/review.md/review")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let review: Value = response_json(response).await;
    assert_eq!(review["comments"], serde_json::json!([]));
    assert_eq!(review["suggestions"], serde_json::json!([]));
}

#[tokio::test]
async fn agent_ops_reply_and_delete_comments_without_live_room() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreviewdelete").await.unwrap();
    let markdown = "See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
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
            "/v1/libraries/agentreviewdelete/documents/notes/review.md/ops?dryRun=1",
            ops_request_by(
                &base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "comment.reply",
                    "id": "r1",
                    "parentId": "c1",
                    "body": "Following up."
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["results"][0]["id"], "r1");
    let dry_run_markdown = body["markdown"].as_str().unwrap();
    assert!(dry_run_markdown.contains("r1:"));
    assert!(dry_run_markdown.contains("body: Following up."));
    assert!(dry_run_markdown.contains("re: c1"));
    assert!(dry_run_markdown.contains("See {==this==}{>>Check it<<}{#c1}."));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentreviewdelete/documents/notes/review.md/ops",
            ops_request_by(
                &base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "comment.reply",
                    "id": "r1",
                    "parentId": "c1",
                    "body": "Following up."
                }),
            ),
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
            "/v1/libraries/agentreviewdelete/documents/notes/review.md/ops",
            ops_request(
                &base_token,
                serde_json::json!({
                    "op": "comment.delete",
                    "id": "r1"
                }),
            ),
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
            "/v1/libraries/agentreviewdelete/documents/notes/review.md/ops",
            ops_request_by(
                &base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "comment.reply",
                    "id": "r2",
                    "parentId": "c1",
                    "body": "Second reply."
                }),
            ),
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
            "/v1/libraries/agentreviewdelete/documents/notes/review.md/ops",
            ops_request(
                &base_token,
                serde_json::json!({
                    "op": "comment.delete",
                    "id": "c1"
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "c1");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentreviewdelete/documents/notes/review.md")
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
    assert_eq!(markdown, "See this.\n");
}

#[tokio::test]
async fn agent_ops_comment_reply_validates_parent_and_body() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentreplyvalidation").await.unwrap();
    let markdown = "See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n  r1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    body: Existing reply.\n    by: user\n    re: c1\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);
    let base_token = format!("\"{}\"", written.version.id);

    for request in [
        ops_request(
            &base_token,
            serde_json::json!({
                "op": "comment.reply",
                "id": "r2",
                "body": "Missing parent."
            }),
        ),
        ops_request(
            &base_token,
            serde_json::json!({
                "op": "comment.reply",
                "id": "r2",
                "parentId": "missing",
                "body": "Missing parent."
            }),
        ),
        ops_request(
            &base_token,
            serde_json::json!({
                "op": "comment.reply",
                "id": "r2",
                "parentId": "r1",
                "body": "Reply to a reply."
            }),
        ),
        ops_request(
            &base_token,
            serde_json::json!({
                "op": "comment.reply",
                "id": "r1",
                "parentId": "c1",
                "body": "Duplicate id."
            }),
        ),
        ops_request(
            &base_token,
            serde_json::json!({
                "op": "comment.reply",
                "id": "r2",
                "parentId": "c1"
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/v1/libraries/agentreplyvalidation/documents/notes/review.md/ops",
                request,
            ))
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "expected client error, got {}",
            response.status()
        );
    }
}

#[tokio::test]
async fn agent_ops_comment_add_injects_into_live_collab_room() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentlivecomment").await.unwrap();
    let written = store
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
    let mut events = store.subscribe_events();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentlivecomment/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: Value = response_json(snapshot).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();
    let first_ref = snapshot["blocks"][0]["ref"].clone();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentlivecomment/documents/notes/review.md/ops",
            ops_request_by(
                base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "comment.add",
                    "id": "c1",
                    "ref": first_ref,
                    "quote": "target",
                    "body": "Needs support."
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "c1");

    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|id| id.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert_eq!(envelope["version_id"], event.version_id.as_deref().unwrap());
    assert_eq!(
        envelope["etag"],
        format!("\"{}\"", event.version_id.as_deref().unwrap())
    );
    assert!(envelope.get("review").is_none());
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "c1").unwrap()["body"],
        "Needs support."
    );
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "c1").unwrap()["by"],
        "ai:codex"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentlivecomment/documents/notes/review.md")
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
    assert!(markdown.contains("One {==target==}{>>Needs support.<<}{#c1} here."));
    assert!(markdown.contains("comments:"));

    wait_for_yjs_comment_mark(&mut socket, &client_doc, "c1").await;
    assert!(yjs_has_comment_mark(&client_doc, "c1"));

    server.abort();
}

#[tokio::test]
async fn agent_ops_suggestion_add_kinds_inject_into_live_collab_room() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentlivesuggestion").await.unwrap();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    for (kind, id) in [
        ("insert", "s_insert"),
        ("delete", "s_delete"),
        ("remove", "s_remove"),
        ("replace", "s_replace"),
        ("substitution", "s_substitution"),
    ] {
        let path = format!("notes/{kind}.md");
        let written = store
            .put_document(
                &library.slug,
                &path,
                b"One target here.\n".to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                quarry_core::WritePrecondition::None,
            )
            .await
            .unwrap();
        let mut events = store.subscribe_events();
        let (mut socket, _) = tokio_tungstenite::connect_async(format!(
            "ws://{addr}/v1/collab/{}",
            written.document.id
        ))
        .await
        .unwrap();
        let client_doc = empty_yjs_doc();
        sync_yjs_doc_from_socket(&mut socket, &client_doc).await;

        let snapshot = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/v1/libraries/agentlivesuggestion/documents/{path}/snapshot"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snapshot.status(), StatusCode::OK);
        let snapshot: Value = response_json(snapshot).await;
        let base_token = snapshot["baseToken"].as_str().unwrap();
        let first_ref = snapshot["blocks"][0]["ref"].clone();

        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("/v1/libraries/agentlivesuggestion/documents/{path}/ops"),
                ops_request_by(
                    base_token,
                    "ai:codex",
                    serde_json::json!({
                        "op": "suggestion.add",
                        "id": id,
                        "kind": kind,
                        "ref": first_ref,
                        "quote": "target",
                        "content": "focus"
                    }),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response_json(response).await;
        assert_eq!(body["results"][0]["id"], id);
        assert_eq!(body["injection"], "injected");

        let event = next_document_put_event(&mut events).await;
        assert!(event
            .origin_id
            .as_deref()
            .is_some_and(|session| session.starts_with("agent-injected:")));
        let recovery = store
            .collab_recovery_state(&written.document.id)
            .await
            .unwrap()
            .unwrap();
        let envelope = injection_envelope_from_update(&recovery.update_v1);
        assert_eq!(envelope["version_id"], event.version_id.as_deref().unwrap());
        assert!(envelope.get("review").is_none());
        assert_eq!(
            review_entry_from_update(&recovery.update_v1, "suggestions", id).unwrap()["by"],
            "ai:codex"
        );

        wait_for_yjs_suggestion_mark(&mut socket, &client_doc, id).await;
        assert!(yjs_has_suggestion_mark(&client_doc, id));
    }

    server.abort();
}

#[tokio::test]
async fn agent_ops_accept_reject_inject_and_remove_suggestion_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentliveaccept").await.unwrap();
    let markdown = "Keep {++added++}{#s1} and drop {--bad--}{#s2}.\n\n---\nsuggestions:\n  s1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n  s2:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: AI\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let mut events = store.subscribe_events();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;
    assert!(yjs_has_suggestion_mark(&client_doc, "s1"));
    assert!(yjs_has_suggestion_mark(&client_doc, "s2"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentliveaccept/documents/notes/review.md/ops",
            ops_request(
                format!("\"{}\"", written.version.id),
                serde_json::json!({"op": "suggestion.accept", "id": "s1"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let base_token = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "injected");
    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert!(review_entry_from_update(&recovery.update_v1, "suggestions", "s1").is_none());
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;
    assert!(!yjs_has_suggestion_mark(&client_doc, "s1"));
    assert!(yjs_has_suggestion_mark(&client_doc, "s2"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentliveaccept/documents/notes/review.md/ops",
            ops_request(
                &base_token,
                serde_json::json!({"op": "suggestion.reject", "id": "s2"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "injected");
    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert!(review_entry_from_update(&recovery.update_v1, "suggestions", "s2").is_none());
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "Keep added and drop bad.");
    assert!(!yjs_has_suggestion_mark(&client_doc, "s2"));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentliveaccept/documents/notes/review.md")
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
    assert_eq!(markdown, "Keep added and drop bad.\n");

    server.abort();
}

#[tokio::test]
async fn agent_ops_comment_resolve_injects_metadata_only_live_patch() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentliveresolve").await.unwrap();
    let markdown = "See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let mut events = store.subscribe_events();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "See this.");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentliveresolve/documents/notes/review.md/ops",
            ops_request(
                format!("\"{}\"", written.version.id),
                serde_json::json!({"op": "comment.resolve", "id": "c1"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["injection"], "metadata_only_injected");

    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "c1").unwrap()["status"],
        "resolved"
    );
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "See this.");

    server.abort();
}

#[tokio::test]
async fn agent_ops_comment_reply_and_reply_delete_inject_metadata_only_live_patch() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentlivereply").await.unwrap();
    let markdown = "See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let mut events = store.subscribe_events();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "See this.");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentlivereply/documents/notes/review.md/ops",
            ops_request_by(
                format!("\"{}\"", written.version.id),
                "ai:codex",
                serde_json::json!({
                    "op": "comment.reply",
                    "id": "r1",
                    "parentId": "c1",
                    "body": "Following up."
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let base_token = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "r1");
    assert_eq!(body["injection"], "metadata_only_injected");

    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "r1").unwrap()["body"],
        "Following up."
    );
    assert_eq!(
        review_entry_from_update(&recovery.update_v1, "comments", "r1").unwrap()["re"],
        "c1"
    );
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "See this.");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentlivereply/documents/notes/review.md/ops",
            ops_request(
                &base_token,
                serde_json::json!({
                    "op": "comment.delete",
                    "id": "r1"
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "r1");
    assert_eq!(body["injection"], "metadata_only_injected");
    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert!(review_entry_from_update(&recovery.update_v1, "comments", "r1").is_none());
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "See this.");

    server.abort();
}

#[tokio::test]
async fn agent_ops_comment_delete_root_injects_content_and_removes_replies() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentlivedelete").await.unwrap();
    let markdown = "See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n  r1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    body: Existing reply.\n    by: user\n    re: c1\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let mut events = store.subscribe_events();
    let app = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let client_doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &client_doc).await;
    assert!(yjs_has_comment_mark(&client_doc, "c1"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentlivedelete/documents/notes/review.md/ops",
            ops_request(
                format!("\"{}\"", written.version.id),
                serde_json::json!({"op": "comment.delete", "id": "c1"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["id"], "c1");
    assert_eq!(body["injection"], "injected");

    let event = next_document_put_event(&mut events).await;
    assert!(event
        .origin_id
        .as_deref()
        .is_some_and(|session| session.starts_with("agent-injected:")));
    let recovery = store
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    let envelope = injection_envelope_from_update(&recovery.update_v1);
    assert!(envelope.get("review").is_none());
    assert!(review_entry_from_update(&recovery.update_v1, "comments", "c1").is_none());
    assert!(review_entry_from_update(&recovery.update_v1, "comments", "r1").is_none());
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;
    assert_eq!(yjs_plain_text(&client_doc), "See this.");
    assert!(!yjs_has_comment_mark(&client_doc, "c1"));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentlivedelete/documents/notes/review.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "See this.\n"
    );

    server.abort();
}

#[tokio::test]
async fn agent_ops_comment_add_without_live_room_uses_external_write() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentcommentfallback").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"One target here.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let mut events = store.subscribe_events();
    let app = router(store);
    let base_token = format!("\"{}\"", written.version.id);
    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentcommentfallback/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: Value = response_json(snapshot).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentcommentfallback/documents/notes/review.md/ops",
            ops_request_by(
                &base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "comment.add",
                    "id": "c1",
                    "ref": snapshot["blocks"][0]["ref"].clone(),
                    "quote": "target",
                    "body": "Needs support."
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ops_etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let body: Value = response_json(response).await;
    let ops_version_id = body["outcome"]["version"]["id"].as_str().unwrap();
    assert_eq!(body["nextBaseToken"].as_str().unwrap(), ops_version_id);
    assert_eq!(ops_etag, format!("\"{ops_version_id}\""));

    let event = next_document_put_event(&mut events).await;
    assert_eq!(event.origin_id, None);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentcommentfallback/documents/notes/review.md")
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
    assert!(markdown.contains("One {==target==}{>>Needs support.<<}{#c1} here."));
    assert!(markdown.contains("by: ai:codex"));
}

#[tokio::test]
async fn agent_ops_comment_add_dirty_live_room_rejects_without_persisting() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentdirtycomment").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"One target here.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let (client_doc, update) = yjs_doc_with_markdown("Dirty target here.\n");
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::Update(update))
                .encode_v1()
                .into(),
        ))
        .await
        .unwrap();
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;

    let snapshot = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentdirtycomment/documents/notes/review.md/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: Value = response_json(snapshot).await;
    let base_token = snapshot["baseToken"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentdirtycomment/documents/notes/review.md/ops",
            ops_request_by(
                base_token,
                "ai:codex",
                serde_json::json!({
                    "op": "comment.add",
                    "id": "c1",
                    "ref": snapshot["blocks"][0]["ref"].clone(),
                    "quote": "target",
                    "body": "Needs support."
                }),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response_json(response).await;
    assert_eq!(body["error"], "LIVE_GATE_REJECTED");

    assert_eq!(yjs_plain_text(&client_doc), "Dirty target here.");
    assert!(!yjs_has_comment_mark(&client_doc, "c1"));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentdirtycomment/documents/notes/review.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "One target here.\n"
    );

    server.abort();
}

#[tokio::test]
async fn agent_ops_comment_delete_dirty_live_room_rejects_without_persisting() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentdirtydelete").await.unwrap();
    let markdown = "See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n";
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            markdown.as_bytes().to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap();
    });

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{}", written.document.id))
            .await
            .unwrap();
    let (client_doc, update) = yjs_doc_with_markdown("Dirty this.\n");
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::Update(update))
                .encode_v1()
                .into(),
        ))
        .await
        .unwrap();
    wait_for_yjs_sync_update(&mut socket, &client_doc).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/agentdirtydelete/documents/notes/review.md/ops",
            ops_request(
                format!("\"{}\"", written.version.id),
                serde_json::json!({"op": "comment.delete", "id": "c1"}),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response_json(response).await;
    assert_eq!(body["error"], "LIVE_GATE_REJECTED");
    assert_eq!(yjs_plain_text(&client_doc), "Dirty this.");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/agentdirtydelete/documents/notes/review.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        markdown
    );

    server.abort();
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
    let docs = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(docs.contains("comment.reply"));
    assert!(docs.contains("comment.delete"));
    assert!(docs.contains("GET $DOC/review"));
    assert!(docs.contains("POST $DOC/review"));
    assert!(docs.contains("Processing Review Feedback"));
    assert!(docs.contains("edit.replace_block"));
    assert!(docs.contains("comments: []"));
    assert!(docs.contains("suggestions: []"));
    assert!(docs.contains("\"blocks\""));
    assert!(docs.contains("replace_document"));
    assert!(docs.contains("repeated `insert_after`"));
    assert!(!docs.contains(
        "does not currently support Proof operations such as `rewrite.apply` or `comment.reply`"
    ));

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
    assert_eq!(
        body["endpoints"]["review_process"]["method"],
        serde_json::json!("POST")
    );
    assert_eq!(
        body["route_hints"]["review_process"],
        serde_json::json!("http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/review")
    );
    assert!(body["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "presence"));
    assert!(body["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "bulk_block_insert"));
    assert!(body["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "review"));
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
    assert!(body["edit_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "replace_document"));
    assert!(body["ops_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "comment.add"));
    assert!(body["ops_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "comment.reply"));
    assert!(body["ops_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "comment.delete"));
    assert!(!body["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|limitation| limitation
            .as_str()
            .is_some_and(|limitation| limitation.contains("comment.reply"))));
    assert_eq!(
        body["endpoints"]["snapshot"]["url"],
        "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/snapshot"
    );
    assert_eq!(
        body["endpoints"]["review"]["url"],
        "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/review"
    );
    assert_eq!(
        body["route_hints"]["review"],
        "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/review"
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
async fn document_put_events_echo_origin_id() {
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
            if event.kind == StoreEventKind::DocumentPut {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event.origin_id.as_deref(), Some("browser:session-1"));
}

#[tokio::test]
async fn document_delete_events_echo_origin_id_and_doc_id() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("delete-origin").await.unwrap();
    let written = store
        .put_document(
            "delete-origin",
            "live.md",
            b"live".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
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
            if event.kind == StoreEventKind::DocumentDelete {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event.doc_id.as_deref(), Some(written.document.id.as_str()));
    assert_eq!(event.origin_id.as_deref(), Some("browser:session-1"));
}

#[tokio::test]
async fn document_put_with_browser_origin_persists_fresh_clean_recovery_seed() {
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
    let (_, stale_update) = yjs_doc_with_markdown("stale\n");
    store
        .put_collab_recovery_state(
            &written.document.id,
            Some(written.version.id.clone()),
            stale_update.clone(),
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
                .header("X-Quarry-Origin-Id", "browser:session-1")
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
    assert_ne!(recovery.update_v1, stale_update);
    let recovered = empty_yjs_doc();
    {
        let mut txn = recovered.transact_mut();
        txn.apply_update(Update::decode_v1(&recovery.update_v1).unwrap())
            .unwrap();
    }
    assert_eq!(
        yjs_slate_children(&recovered),
        review_markdown_to_slate("new\n").unwrap()
    );
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
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"].is_object());
    assert!(
        openapi["paths"]["/v1/libraries/{library}/documents/{path}/review"]["post"].is_object()
    );
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
            "replace_document",
        ],
    );
    assert!(
        openapi["components"]["schemas"]["AgentBlockOperation"]["properties"]["blocks"].is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentBlockOperation"]["properties"]["markdown"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentBlockRef"]["properties"]
            .get("baseToken")
            .is_none()
    );
    let block_ref_required = openapi["components"]["schemas"]["AgentBlockRef"]["required"]
        .as_array()
        .expect("AgentBlockRef should expose required fields");
    assert!(block_ref_required.iter().any(|value| value == "ordinal"));
    assert!(!block_ref_required
        .iter()
        .any(|value| value == "contentHash"));
    let content_hash_schema =
        &openapi["components"]["schemas"]["AgentBlockRef"]["properties"]["contentHash"];
    assert_schema_type_contains(content_hash_schema, "string");
    assert_schema_type_contains(content_hash_schema, "null");
    assert!(
        openapi["components"]["schemas"]["AgentOpsRequest"]["properties"]["operations"].is_object()
    );
    assert_schema_enum_contains(
        &openapi,
        &openapi["components"]["schemas"]["AgentOpsOperationRequest"]["properties"]["op"],
        &[
            "comment.add",
            "comment.reply",
            "comment.delete",
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
        &openapi["components"]["schemas"]["AgentOpsOperationRequest"]["properties"]["kind"],
        &["insert", "delete", "remove", "replace", "substitution"],
    );
    assert!(
        openapi["components"]["schemas"]["AgentOpsOperationRequest"]["properties"]["parentId"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentOpsResponse"]["properties"]["results"].is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentReviewProcessRequest"]["properties"]["operations"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentReviewProcessOperation"]["properties"]["block"]
            .is_object()
    );
    assert!(
        openapi["components"]["schemas"]["AgentReviewProcessResponse"]["properties"]["review"]
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
    assert_path_parameter_enum_contains(
        &openapi,
        "/v1/libraries/{library}/documents/{path}/review",
        "get",
        "includeResolved",
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

async fn next_document_put_event(
    events: &mut tokio::sync::broadcast::Receiver<StoreEvent>,
) -> StoreEvent {
    timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == StoreEventKind::DocumentPut {
                break event;
            }
        }
    })
    .await
    .unwrap()
}

fn yjs_doc_with_markdown(markdown: &str) -> (Doc, Vec<u8>) {
    let nodes = review_markdown_to_slate(markdown).unwrap();
    let built = build_nodes(&nodes).unwrap();
    let update = encode_update_v1_from_built(&built, COLLAB_ROOT);
    let doc = empty_yjs_doc();
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(Update::decode_v1(&update).unwrap())
            .unwrap();
    }
    (doc, update)
}

fn empty_yjs_doc() -> Doc {
    Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    })
}

async fn sync_yjs_doc_from_socket<S>(socket: &mut S, doc: &Doc)
where
    S: Sink<TungsteniteMessage>
        + Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
    <S as Sink<TungsteniteMessage>>::Error: std::fmt::Debug,
{
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::SyncStep1(doc.transact().state_vector()))
                .encode_v1()
                .into(),
        ))
        .await
        .unwrap();
    wait_for_yjs_sync_update(socket, doc).await;
}

fn injection_envelope_from_update(update: &[u8]) -> Value {
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(Update::decode_v1(update).unwrap())
            .unwrap();
    }
    let txn = doc.transact();
    let envelope = txn
        .get_map(INJECTION_ROOT)
        .expect("injection envelope root exists");
    let mut object = serde_json::Map::new();
    for (key, value) in envelope.iter(&txn) {
        if let yrs::Out::Any(Any::String(value)) = value {
            object.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    Value::Object(object)
}

fn review_entry_from_update(update: &[u8], section: &str, id: &str) -> Option<Value> {
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(Update::decode_v1(update).unwrap())
            .unwrap();
    }
    let txn = doc.transact();
    let review = txn.get_map(REVIEW_ROOT)?;
    let Out::YMap(section) = review.get(&txn, section)? else {
        return None;
    };
    if !section.contains_key(&txn, id) {
        return None;
    }
    section.get_as(&txn, id).ok()
}

async fn wait_for_yjs_comment_mark<S>(socket: &mut S, doc: &Doc, id: &str)
where
    S: Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    if yjs_has_comment_mark(doc, id) {
        return;
    }
    timeout(Duration::from_secs(2), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let TungsteniteMessage::Binary(bytes) = message else {
                continue;
            };
            apply_yjs_message(doc, bytes.as_ref());
            if yjs_has_comment_mark(doc, id) {
                break;
            }
        }
    })
    .await
    .unwrap();
}

async fn wait_for_yjs_suggestion_mark<S>(socket: &mut S, doc: &Doc, id: &str)
where
    S: Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    if yjs_has_suggestion_mark(doc, id) {
        return;
    }
    timeout(Duration::from_secs(2), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let TungsteniteMessage::Binary(bytes) = message else {
                continue;
            };
            apply_yjs_message(doc, bytes.as_ref());
            if yjs_has_suggestion_mark(doc, id) {
                break;
            }
        }
    })
    .await
    .unwrap();
}

async fn wait_for_yjs_plain_text<S>(socket: &mut S, doc: &Doc, expected: &str)
where
    S: Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    timeout(Duration::from_secs(2), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let TungsteniteMessage::Binary(bytes) = message else {
                continue;
            };
            apply_yjs_message(doc, bytes.as_ref());
            if yjs_plain_text(doc) == expected {
                break;
            }
        }
    })
    .await
    .unwrap();
}

async fn wait_for_yjs_sync_update<S>(socket: &mut S, doc: &Doc)
where
    S: Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    timeout(Duration::from_secs(2), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let TungsteniteMessage::Binary(bytes) = message else {
                continue;
            };
            if apply_yjs_message(doc, bytes.as_ref()) {
                break;
            }
        }
    })
    .await
    .unwrap();
}

fn apply_yjs_message(doc: &Doc, bytes: &[u8]) -> bool {
    let update = match YMessage::decode_v1(bytes) {
        Ok(YMessage::Sync(SyncMessage::Update(update) | SyncMessage::SyncStep2(update))) => update,
        _ => {
            return false;
        }
    };
    if update.is_empty() {
        return false;
    }
    let mut txn = doc.transact_mut();
    txn.apply_update(Update::decode_v1(&update).unwrap())
        .unwrap();
    true
}

fn yjs_slate_children(doc: &Doc) -> Vec<Node> {
    let txn = doc.transact();
    let text = txn.get_text(COLLAB_ROOT).unwrap();
    let root: &XmlTextRef = text.as_ref();
    let Node::Element { children, .. } = xmltext_to_slate(&txn, root).unwrap() else {
        panic!("collab root should decode as a Slate fragment");
    };
    children
}

fn yjs_has_comment_mark(doc: &Doc, id: &str) -> bool {
    let key = format!("comment_{id}");
    fn visit(node: &Node, key: &str) -> bool {
        match node {
            Node::Text { marks, .. } => {
                marks.get("comment").and_then(Value::as_bool) == Some(true)
                    && marks.get(key).and_then(Value::as_bool) == Some(true)
            }
            Node::Element { children, .. } => children.iter().any(|child| visit(child, key)),
        }
    }
    yjs_slate_children(doc).iter().any(|node| visit(node, &key))
}

fn yjs_has_suggestion_mark(doc: &Doc, id: &str) -> bool {
    let key = format!("suggestion_{id}");
    fn visit(node: &Node, key: &str) -> bool {
        match node {
            Node::Text { marks, .. } => {
                marks.get("suggestion").and_then(Value::as_bool) == Some(true)
                    && marks.get(key).is_some()
            }
            Node::Element { children, .. } => children.iter().any(|child| visit(child, key)),
        }
    }
    yjs_slate_children(doc).iter().any(|node| visit(node, &key))
}

fn yjs_plain_text(doc: &Doc) -> String {
    fn collect(node: &Node, out: &mut String) {
        match node {
            Node::Text { text, .. } => out.push_str(text),
            Node::Element { children, .. } => {
                for child in children {
                    collect(child, out);
                }
            }
        }
    }
    let mut out = String::new();
    for node in yjs_slate_children(doc) {
        collect(&node, &mut out);
    }
    out
}
