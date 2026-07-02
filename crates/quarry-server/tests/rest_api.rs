#![cfg(feature = "lib-documents")]

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use quarry_collab_codec::{xmltext_to_slate, Node};
use quarry_core::DocumentSource;
use quarry_server::{app_state, router, router_with_state, serve_state_with_shutdown};
use quarry_storage::{QuarryStore, StoreConfig, StoreEvent, StoreEventKind};
use serde_json::Value;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Doc, Map, OffsetKind, Options, Out, ReadTxn, Text, Transact, Update, WriteTxn, XmlTextRef,
};

const COLLAB_ROOT: &str = "content";
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

#[cfg(feature = "tmp-documents")]
#[tokio::test(flavor = "current_thread")]
async fn request_tracing_redacts_tmp_capability_paths_without_redacting_library_paths() {
    let (logs, _guard) = capture_debug_logs();
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

    logs.clear();
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
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let output = logs.output();
    assert!(
        output.contains(&format!(
            "/v1/libraries/missing/documents/{library_secret_like_path}"
        )),
        "ordinary library paths should remain visible in request logs:\n{output}"
    );
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

#[derive(Clone, Default)]
struct CapturedLogs {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl CapturedLogs {
    fn clear(&self) {
        self.buffer.lock().unwrap().clear();
    }

    fn output(&self) -> String {
        String::from_utf8(self.buffer.lock().unwrap().clone()).unwrap()
    }
}

struct CapturedLogWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl Write for CapturedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for CapturedLogs {
    type Writer = CapturedLogWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        CapturedLogWriter {
            buffer: self.buffer.clone(),
        }
    }
}

fn capture_debug_logs() -> (CapturedLogs, tracing::dispatcher::DefaultGuard) {
    // tracing-core's callsite-interest cache has a lock-free fast path when
    // at most one dispatcher is registered (`Rebuilder::JustOne`): a callsite
    // FIRST hit on a subscriber-less test thread while this capture
    // subscriber is the only registered dispatcher caches `Interest::never`
    // computed from THAT thread's absent default — and the capturing test's
    // own events at that callsite are then skipped (its assertions see an
    // EMPTY capture) until a later subscriber registration rebuilds the
    // cache. Keeping a permanent global no-op dispatcher registered means
    // two dispatchers are live during every capture, forcing callsite
    // registration through the locked path that consults them all.
    // Reproduced by looping this file's first two tests with 2 threads.
    static GLOBAL_NO_OP: std::sync::Once = std::sync::Once::new();
    GLOBAL_NO_OP.call_once(|| {
        let _ =
            tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default());
    });
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("quarry_server=debug"))
        .with_writer(logs.clone())
        .with_ansi(false)
        .with_target(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (logs, guard)
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
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .all(|document| document["path"] != "notes/one.md"));

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
    assert!(openapi["paths"]
        ["/v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata"]
        .is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["post"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["get"].is_object());
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn rest_api_supports_tmp_documents_ttl_versions_and_promotion() {
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
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "draft one",
                "content_type": "text/markdown",
                "metadata": {"title": "Scratch"}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();
    let document_id = created["document"]["id"].as_str().unwrap().to_string();
    assert_eq!(secret.len(), 32);
    assert!(secret
        .chars()
        .all(|character| character.is_ascii_hexdigit()));
    assert_eq!(created["document"]["library_id"], Value::Null);
    assert!(created["document"]["expires_at"].as_str().is_some());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/tmp/documents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
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
    assert!(response.headers()["x-quarry-expires-at"]
        .to_str()
        .unwrap()
        .starts_with("20"));
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "draft one"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/tmp/documents/scratch/note.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/tmp/documents/{secret}/share"),
            serde_json::json!({"role": "editor"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::IF_MATCH, etag)
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("draft two"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let updated: Value = response_json(response).await;
    let updated_version = updated["version"]["id"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/versions/raw"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let versions: Value = response_json(response).await;
    assert_eq!(versions.as_array().unwrap().len(), 2);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/tmp/documents/{secret}/versions/{}",
                    created["version"]["id"].as_str().unwrap()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first: Value = response_json(response).await;
    assert_eq!(first["content"], "draft one");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/tmp/documents/{secret}/ttl"),
            serde_json::json!({"expires_at":"2099-01-01T00:00:00Z"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ttl: Value = response_json(response).await;
    assert_eq!(ttl["expires_at"], "2099-01-01T00:00:00Z");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/tmp/documents/{secret}/ttl"),
            serde_json::json!({"expires_at": null}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"promoted"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/tmp/documents/{secret}/promote"),
            serde_json::json!({
                "library": "promoted",
                "path": "notes/promoted.txt",
                "if_match": updated_version
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
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/promoted/documents/notes/promoted.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-quarry-document-id"], document_id);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "draft two\n"
    );

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/promoted/documents/notes/promoted.txt/versions/raw")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let promoted_versions: Value = response_json(response).await;
    assert_eq!(promoted_versions.as_array().unwrap().len(), 2);
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
    let markdown = "Alpha {==target==}{>>Needs work.<<}{#c1} and {==done==}{>>Fixed.<<}{#c2}.\n\nUse {~~old~>new~~}{#s1} wording and `{++literal++}{#s_code}`.\n\n```text\n{==ignored==}{>>Nope<<}{#c_code}\n{--gone--}{#s_code2}\n```\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user:a\n  c2:\n    at: \"2026-01-02T00:00:00.000Z\"\n    by: user:b\n    status: resolved\n  c_code:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: user:code\n  r1:\n    at: \"2026-01-01T01:00:00.000Z\"\n    body: Reply body.\n    by: ai:codex\n    re: c1\n  r2:\n    at: \"2026-01-03T01:00:00.000Z\"\n    body: Suggestion reply.\n    by: user:a\n    re: s1\nsuggestions:\n  s1:\n    at: \"2026-01-03T00:00:00.000Z\"\n    by: ai:codex\n  s_code:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: ai:code\n  s_code2:\n    at: \"2026-01-04T00:00:00.000Z\"\n    by: ai:code\n";
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
    assert_eq!(body["suggestions"][0]["replies"][0]["id"], "r2");
    assert_eq!(
        body["suggestions"][0]["replies"][0]["body"],
        "Suggestion reply."
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
async fn agent_review_reports_explicit_inline_markers_without_endmatter() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("agentrevieworphan").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "notes/review.md",
            b"See {==this==}{>>Check it<<}{#c_orphan}.\n\nAdd {++better++}{#s_orphan}.\n".to_vec(),
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
                .uri("/v1/libraries/agentrevieworphan/documents/notes/review.md/review")
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
    assert_eq!(body["comments"][0]["id"], "c_orphan");
    assert_eq!(body["comments"][0]["by"], "unknown");
    assert_eq!(body["comments"][0]["at"], "");
    assert_eq!(body["comments"][0]["body"], "Check it");
    assert_eq!(body["comments"][0]["quote"], "this");
    assert_eq!(body["suggestions"].as_array().unwrap().len(), 1);
    assert_eq!(body["suggestions"][0]["id"], "s_orphan");
    assert_eq!(body["suggestions"][0]["by"], "unknown");
    assert_eq!(body["suggestions"][0]["at"], "");
    assert_eq!(body["suggestions"][0]["kind"], "insert");
    assert_eq!(body["suggestions"][0]["content"], "better");
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
async fn tmp_agent_presence_omits_capability_path() {
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
    assert!(presence["current"]["updatedAt"].as_str().is_some());
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

#[cfg(feature = "tmp-documents")]
#[tokio::test(flavor = "current_thread")]
async fn tmp_sse_logging_redacts_capability_path() {
    let (logs, _guard) = capture_debug_logs();
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

async fn presence_test_app(library: &str) -> (tempfile::TempDir, axum::Router) {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library(library).await.unwrap();
    store
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

#[tokio::test]
async fn document_write_with_agent_header_touches_presence() {
    let (_root, app) = presence_test_app("presence-write").await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/presence-write/documents/live.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("X-Agent-Id", "agent-w")
                .body(Body::from("hello again"))
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
                .uri("/v1/libraries/presence-write/documents/live.md/transactions")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", "agent-t")
                .body(Body::from(
                    block_tx(
                        "tx-presence",
                        serde_json::json!([{
                            "op": "insert_block",
                            "position": 1,
                            "block_type": "p",
                            "text": "Second."
                        }]),
                    )
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut agent_ids: Vec<String> = list_presence(&app, "presence-write")
        .await
        .iter()
        .map(|entry| entry["agentId"].as_str().unwrap_or_default().to_string())
        .collect();
    agent_ids.sort();
    assert_eq!(agent_ids, vec!["agent-t", "agent-w"]);
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_document_read_with_agent_header_auto_joins_presence() {
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
    // The docs teach the session-scoped contract: stable block ids, the
    // transaction envelope with typed retryable errors, and the rows-backed
    // review projection (incl. conflict items).
    assert!(docs.contains("$DOC/blocks"));
    assert!(docs.contains("$DOC/transactions"));
    assert!(docs.contains("client_tx_id"));
    assert!(docs.contains("base_clock"));
    assert!(docs.contains("changed_block_ids"));
    assert!(docs.contains("committed_rebased"));
    assert!(docs.contains("STALE_BASE"));
    assert!(docs.contains("set_block_type"));
    // The documented block-type vocabulary is the codec's REAL one (the
    // insert example commits verbatim — see
    // agent_docs_insert_block_example_commits_as_documented). Fake friendly
    // names would 422 at commit time.
    assert!(docs.contains("`p`, `h1`\u{2013}`h6`"));
    assert!(docs.contains("`raw_markdown`"));
    assert!(docs.contains("listStyleType"));
    assert!(docs.contains("\"block_type\": \"p\""));
    assert!(!docs.contains("`paragraph`"));
    assert!(!docs.contains("list_item"));
    assert!(!docs.contains("image_embed"));
    assert!(docs.contains("comment.reply"));
    assert!(docs.contains("comment.edit"));
    assert!(docs.contains("suggestion.accept"));
    assert!(docs.contains("conflict"));
    assert!(docs.contains("GET $DOC/review"));
    assert!(docs.contains("/v1/tmp/documents/$SECRET"));
    let removed_tmp_signal = ["han", "doff"].join("");
    assert!(!docs.to_lowercase().contains(&removed_tmp_signal));
    // The legacy facade vocabulary is gone.
    assert!(!docs.contains("/edit"));
    assert!(!docs.contains("$DOC/ops"));
    assert!(!docs.contains("POST $DOC/review"));
    assert!(!docs.contains("ordinal"));
    assert!(!docs.contains("contentHash"));
    assert!(!docs.contains("baseToken\": \"version_123"));
    assert!(!docs.contains("Idempotency-Key"));
    assert!(!docs.contains("injection"));

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
    if cfg!(feature = "lib-documents") {
        assert_eq!(
            body["endpoints"]["transactions"]["method"],
            serde_json::json!("POST")
        );
        assert_eq!(
            body["route_hints"]["transactions"],
            serde_json::json!(
                "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/transactions"
            )
        );
        assert_eq!(
            body["route_hints"]["blocks"],
            serde_json::json!(
                "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/blocks"
            )
        );
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
    } else {
        assert!(body["endpoints"]["transactions"].is_null());
        assert!(body["route_hints"]["transactions"].is_null());
        assert!(body["endpoints"]["snapshot"].is_null());
    }
    if cfg!(feature = "tmp-documents") {
        assert_eq!(
            body["endpoints"]["tmp_transactions"]["method"],
            serde_json::json!("POST")
        );
        assert_eq!(
            body["route_hints"]["tmp_blocks"],
            "http://127.0.0.1:7831/v1/tmp/documents/{secret}/blocks"
        );
        let removed_tmp_signal_key = ["tmp_han", "doff"].join("");
        assert!(body["endpoints"][&removed_tmp_signal_key].is_null());
        assert!(body["route_hints"][removed_tmp_signal_key].is_null());
    } else {
        assert!(body["endpoints"]["tmp_transactions"].is_null());
        assert!(body["route_hints"]["tmp_blocks"].is_null());
    }
    // The legacy facades are gone from discovery entirely.
    assert!(body["endpoints"]["edit"].is_null());
    assert!(body["endpoints"]["ops"].is_null());
    assert!(body["endpoints"]["review_process"].is_null());
    assert!(body["route_hints"]["edit"].is_null());
    assert!(body["route_hints"]["ops"].is_null());
    assert!(body["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "presence"));
    assert!(body["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "transactions"));
    assert!(body["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "review"));
    if cfg!(feature = "tmp-documents") {
        assert!(body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "tmp_documents"));
        assert!(!body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == &removed_tmp_signal));
    }
    assert!(body["auth_note"]
        .as_str()
        .unwrap()
        .contains("trusted-localhost"));
    assert_eq!(body["auth"]["mode"], "trusted_localhost");
    assert!(body["presence_statuses"].as_array().unwrap().len() >= 6);
    assert!(body["transaction_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "replace_block_content"));
    assert!(body["transaction_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "set_block_type"));
    assert!(body["transaction_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "comment.add"));
    assert!(body["transaction_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "comment.edit"));
    assert!(body["transaction_operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|operation| operation == "suggestion.accept"));
    assert!(!body["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|limitation| limitation
            .as_str()
            .is_some_and(|limitation| limitation.contains("comment.reply"))));
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
    assert!(!block_ref_required
        .iter()
        .any(|value| value == "contentHash"));
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
    assert!(library_presence_entry["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "path"));
    let tmp_presence_entry = &openapi["components"]["schemas"]["TmpAgentPresenceEntry"];
    assert!(tmp_presence_entry.is_object());
    assert!(tmp_presence_entry["properties"].get("path").is_none());
    assert!(tmp_presence_entry["properties"].get("library").is_none());
    let tmp_presence_required = tmp_presence_entry["required"].as_array().unwrap();
    assert!(tmp_presence_required
        .iter()
        .any(|field| field == "documentId"));
    assert!(tmp_presence_required.iter().any(|field| field == "agentId"));
    assert!(tmp_presence_required.iter().any(|field| field == "status"));
    assert!(tmp_presence_required
        .iter()
        .any(|field| field == "updatedAt"));
    assert!(!tmp_presence_required.iter().any(|field| field == "path"));
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/presence"]["get"]["responses"]["200"]
            ["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/TmpAgentPresenceListResponse"
    );
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/presence"]["post"]["responses"]["200"]
            ["content"]["application/json"]["schema"]["$ref"],
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
    for endpoint in ["edit", "ops", "review"] {
        let operation = &openapi["paths"]
            [format!("/v1/libraries/{{library}}/documents/{{path}}/{endpoint}")]["post"];
        assert!(operation.is_null(), "{endpoint} POST should be deleted");
    }
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
    assert_eq!(body[0]["source"], "rest");
    assert_eq!(body[0]["actor"], "Avery");
    assert_eq!(body[0]["message"], "Imported from Git");
    assert_eq!(body[0]["provenance"]["remote"], "origin/main");
}

#[tokio::test]
async fn put_document_rejects_invalid_transaction_provenance_header() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("badprovenance").await.unwrap();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/badprovenance/documents/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("x-quarry-transaction-provenance", "{bad json")
                .body(Body::from("body"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_document_decodes_percent_encoded_transaction_actor_header() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("actorheader").await.unwrap();
    let app = router(store);

    // Each document is created first so the actor-carrying write exercises
    // the update path (first-import attribution is covered separately by
    // first_import_records_transaction_actor_header).
    put_markdown(&app, "actorheader", "a.md", "# A\n", None).await;
    put_markdown(&app, "actorheader", "b.md", "# B\n", None).await;
    put_markdown(&app, "actorheader", "c.md", "# C\n", None).await;

    // Percent-encoded UTF-8 name decodes before storage.
    let version = put_markdown(
        &app,
        "actorheader",
        "a.md",
        "# A updated\n",
        Some("Jos%C3%A9"),
    )
    .await;
    assert_eq!(
        version_actor(&app, "actorheader", "a.md", &version).await,
        "José"
    );

    // Plain ASCII passes through unchanged.
    let version = put_markdown(&app, "actorheader", "b.md", "# B updated\n", Some("Avery")).await;
    assert_eq!(
        version_actor(&app, "actorheader", "b.md", &version).await,
        "Avery"
    );

    // No header falls back to the gateway's surface label.
    let version = put_markdown(&app, "actorheader", "c.md", "# C updated\n", None).await;
    assert_eq!(
        version_actor(&app, "actorheader", "c.md", &version).await,
        "rest"
    );
}

#[tokio::test]
async fn first_import_records_transaction_actor_header() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("actorcreate").await.unwrap();
    let app = router(store);

    let version = put_markdown(&app, "actorcreate", "fresh.md", "# Fresh\n", Some("Avery")).await;

    assert_eq!(
        version_actor(&app, "actorcreate", "fresh.md", &version).await,
        "Avery"
    );
}

#[tokio::test]
async fn delete_move_and_restore_record_transaction_actor_header() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("actorops").await.unwrap();
    let app = router(store);

    let v1 = put_markdown(&app, "actorops", "keep.md", "# Doc one\n", None).await;
    let _v2 = put_markdown(&app, "actorops", "keep.md", "# Doc two\n", None).await;
    put_markdown(&app, "actorops", "doomed.md", "# Doomed\n", None).await;

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
                .unwrap(),
        )
        .await
        .unwrap();
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
                .unwrap(),
        )
        .await
        .unwrap();
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
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let restored: Value = response_json(response).await;
    let restored_version = restored["version"]["id"].as_str().unwrap();
    assert_eq!(
        version_actor(&app, "actorops", "kept.md", restored_version).await,
        "Avery"
    );
}

#[tokio::test]
async fn raw_document_restore_records_transaction_actor_header() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("actorraw").await.unwrap();
    let app = router(store);

    // A plain-text document routes as a RawDocument (not `.md`, not a
    // markdown content type), so its restore takes the legacy byte path
    // (`restore_document_version_with_origin`) rather than the markdown
    // gateway. Restoring the current head short-circuits, so write two
    // versions and restore the first.
    let v1 = put_plain_text(&app, "actorraw", "notes.txt", "raw one\n").await;
    let _v2 = put_plain_text(&app, "actorraw", "notes.txt", "raw two\n").await;

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
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let restored: Value = response_json(response).await;
    let restored_version = restored["version"]["id"].as_str().unwrap();
    assert_eq!(
        version_actor(&app, "actorraw", "notes.txt", restored_version).await,
        "Avery"
    );
}

/// PUTs plain text (a RawDocument) into `library`, returning the written
/// version id.
async fn put_plain_text(app: &axum::Router, library: &str, path: &str, body: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/libraries/{library}/documents/{path}"))
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let outcome: Value = response_json(response).await;
    outcome["version"]["id"].as_str().unwrap().to_string()
}

/// PUTs markdown into `library`, optionally with an
/// `x-quarry-transaction-actor` header, returning the written version id.
async fn put_markdown(
    app: &axum::Router,
    library: &str,
    path: &str,
    body: &str,
    actor_header: Option<&str>,
) -> String {
    let mut request = Request::builder()
        .method(Method::PUT)
        .uri(format!("/v1/libraries/{library}/documents/{path}"))
        .header(header::CONTENT_TYPE, "text/markdown");
    if let Some(actor) = actor_header {
        request = request.header("x-quarry-transaction-actor", actor);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let outcome: Value = response_json(response).await;
    outcome["version"]["id"].as_str().unwrap().to_string()
}

/// The `"actor"` recorded for `version_id` of `path`, via GET `/versions`.
async fn version_actor(app: &axum::Router, library: &str, path: &str, version_id: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/{library}/documents/{path}/versions"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    body.as_array()
        .unwrap()
        .iter()
        .find(|version| version["id"] == version_id)
        .unwrap()["actor"]
        .clone()
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
async fn rest_api_marks_external_links() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("external").await.unwrap();

    store
        .put_document(
            &library.slug,
            "source.md",
            b"[site](https://example.com)\n".to_vec(),
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
                .uri("/v1/libraries/external/documents/source.md/outgoing-links")
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
        .find(|link| {
            link["target_kind"] == "markdown_link" && link["target_text"] == "https://example.com"
        })
        .unwrap();
    assert_eq!(link["resolved"], false);
    assert_eq!(link["resolution_status"], "external");
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
    // Normalized by the Phase 4 reconciled markdown write.
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "newer\n"
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

fn review_entry_from_doc(doc: &Doc, section: &str, id: &str) -> Option<Value> {
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

async fn wait_for_yjs_review_entry<S>(
    socket: &mut S,
    doc: &Doc,
    section: &str,
    id: &str,
    matches: impl Fn(&Value) -> bool,
) -> Value
where
    S: Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    if let Some(entry) = review_entry_from_doc(doc, section, id) {
        if matches(&entry) {
            return entry;
        }
    }
    timeout(Duration::from_secs(2), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let TungsteniteMessage::Binary(bytes) = message else {
                continue;
            };
            apply_yjs_message(doc, bytes.as_ref());
            let Some(entry) = review_entry_from_doc(doc, section, id) else {
                continue;
            };
            if matches(&entry) {
                break entry;
            }
        }
    })
    .await
    .unwrap()
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

// ---------------------------------------------------------------------------
// Phase 2: semantic mutation gateway (rows-authoritative mode) + block API
// ---------------------------------------------------------------------------

async fn block_test_app() -> (tempfile::TempDir, axum::Router, QuarryStore) {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store.clone());
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

#[cfg(feature = "tmp-documents")]
async fn get_tmp_block_tree(app: &axum::Router, secret: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/blocks"))
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

fn block_tx_with_clock(client_tx_id: &str, base_clock: &str, ops: Value) -> Value {
    let mut tx = block_tx(client_tx_id, ops);
    tx["base_clock"] = Value::String(base_clock.to_string());
    tx
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

async fn raw_version_count(app: &axum::Router, path: &str) -> usize {
    raw_versions(app, path).await.as_array().unwrap().len()
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
    assert!(first["document_clock"].as_str().is_some());

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

#[tokio::test]
async fn markdown_put_rejects_raw_downgrade_without_opt_in() {
    let (_root, app, store) = block_test_app().await;
    put_block_markdown(&app, "guide", "# Guide\n\nBody.\n").await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/guide")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("raw body"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Markdown block document"));

    let document = store.get_document("blocks", "guide").await.unwrap();
    assert_eq!(document.version.content_type, "text/markdown");
    assert_eq!(
        String::from_utf8(document.content).unwrap(),
        "# Guide\n\nBody.\n"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/guide")
                .header(header::CONTENT_TYPE, "text/plain")
                .header("x-quarry-allow-document-kind-change", "true")
                .body(Body::from("raw body"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let document = store.get_document("blocks", "guide").await.unwrap();
    assert_eq!(document.version.content_type, "text/plain");
    assert_eq!(document.content, b"raw body".to_vec());
    assert_eq!(
        store.load_block_tree(&document.id).await.unwrap(),
        Vec::<quarry_collab_codec::BlockRow>::new()
    );
}

#[tokio::test]
async fn block_transaction_insert_block_commits_one_version_and_emits_events() {
    let (_root, app, store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "First.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let versions_before = raw_version_count(&app, "doc.md").await;
    let mut events = store.subscribe_events();

    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-insert",
            serde_json::json!([{
                "op": "insert_block",
                "position": 1,
                "block_type": "p",
                "text": "Second."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["status"], "committed");
    assert_eq!(ack["changed_block_ids"].as_array().unwrap().len(), 1);
    assert!(ack["transaction_id"].as_str().is_some());
    let clock = ack["document_clock"].as_str().unwrap();
    assert_ne!(clock, tree["document_clock"].as_str().unwrap());

    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "First.\n\nSecond.\n"
    );
    assert_eq!(raw_version_count(&app, "doc.md").await, versions_before + 1);

    let event = next_document_put_event(&mut events).await;
    assert_eq!(event.version_id.as_deref(), Some(clock));
    assert_eq!(event.path.as_deref(), Some("doc.md"));
}

#[tokio::test]
async fn block_transaction_replace_block_content_preserves_block_identity() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Original text.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-replace",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Rewritten text."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["changed_block_ids"], serde_json::json!([block_id]));

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], block_id.as_str());
    assert_eq!(after["blocks"][0]["text"], "Rewritten text.");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Rewritten text.\n"
    );
}

#[tokio::test]
async fn block_transaction_move_block_is_placement_only() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Alpha.\n\nBeta.\n\nGamma.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let gamma = tree["blocks"][2]["block_id"].as_str().unwrap().to_string();
    let alpha = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-move",
            serde_json::json!([{
                "op": "move_block",
                "block_id": gamma,
                "position": 0
            }]),
        ),
    )
    .await;

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], gamma.as_str());
    assert_eq!(after["blocks"][0]["text"], "Gamma.");
    assert_eq!(after["blocks"][1]["block_id"], alpha.as_str());
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Gamma.\n\nAlpha.\n\nBeta.\n"
    );
}

#[tokio::test]
async fn block_transaction_set_block_type_preserves_identity_text_and_anchors() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Heading soon.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 0,
                "end": 7,
                "body": "anchored before the type change"
            }]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-type",
            serde_json::json!([{
                "op": "set_block_type",
                "block_id": block_id,
                "block_type": "h2"
            }]),
        ),
    )
    .await;

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], block_id.as_str());
    assert_eq!(after["blocks"][0]["block_type"], "h2");
    assert_eq!(after["blocks"][0]["text"], "Heading soon.");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "## Heading soon.\n"
    );
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["comments"][0]["anchor"]["blockId"],
        block_id.as_str()
    );
    assert_eq!(review["comments"][0]["anchor"]["startOffset"], 0);
    assert_eq!(review["comments"][0]["anchor"]["endOffset"], 7);
    assert_eq!(review["comments"][0]["status"], "open");
}

#[tokio::test]
async fn block_transaction_set_block_attrs_edits_raw_markdown_blocks() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "<div>\nopaque\n</div>\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    assert_eq!(tree["blocks"][0]["block_type"], "raw_markdown");
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-attrs",
            serde_json::json!([{
                "op": "set_block_attrs",
                "block_id": block_id,
                "attrs": {"markdown": "<section>\nreplaced\n</section>"}
            }]),
        ),
    )
    .await;

    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "<section>\nreplaced\n</section>\n"
    );
    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][0]["block_id"], block_id.as_str());
}

#[tokio::test]
async fn block_transaction_marks_and_links_render_in_markdown() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Bold and linked words.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-format",
            serde_json::json!([
                {"op": "add_mark", "block_id": block_id, "start": 0, "end": 4, "marks": {"bold": true}},
                {"op": "set_link", "block_id": block_id, "start": 9, "end": 15, "url": "https://example.com"}
            ]),
        ),
    )
    .await;
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "**Bold** and [linked](https://example.com) words.\n"
    );

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-unformat",
            serde_json::json!([
                {"op": "remove_mark", "block_id": block_id, "start": 0, "end": 4, "marks": ["bold"]},
                {"op": "set_link", "block_id": block_id, "start": 9, "end": 15, "url": null}
            ]),
        ),
    )
    .await;
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Bold and linked words.\n"
    );
}

#[tokio::test]
async fn block_transaction_comment_lifecycle_projects_from_rows() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "why this?"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let comment = &review["comments"][0];
    assert_eq!(comment["status"], "open");
    assert_eq!(comment["body"], "why this?");
    assert_eq!(comment["quote"], "this");
    assert_eq!(comment["by"], "Agent One");
    assert_eq!(comment["ref"]["ordinal"], 0);
    assert_eq!(comment["anchor"]["blockId"], block_id.as_str());
    let comment_id = comment["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": comment_id,
                "body": "because reasons"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["comments"][0]["replies"][0]["body"],
        "because reasons"
    );

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-resolve",
            serde_json::json!([{ "op": "comment.resolve", "item_id": comment_id }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert!(review["comments"].as_array().unwrap().is_empty());
    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["comments"][0]["status"], "resolved");

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-delete-comment",
            serde_json::json!([{ "op": "comment.delete", "item_id": comment_id }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", true).await;
    assert!(review["comments"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn block_transaction_comment_edit_updates_body_and_edited_at() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "original note"
            }]),
        ),
    )
    .await;
    let before = get_block_review(&app, "doc.md", false).await;
    let comment = &before["comments"][0];
    assert!(comment["editedAt"].is_null());
    let comment_id = comment["id"].as_str().unwrap().to_string();
    let created_at = comment["at"].as_str().unwrap().to_string();
    let anchor = comment["anchor"].clone();
    let quote = comment["quote"].clone();

    tokio::time::sleep(Duration::from_millis(2)).await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-comment",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": comment_id,
                "body": "edited note"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let edited = &review["comments"][0];
    assert_eq!(edited["body"], "edited note");
    assert_eq!(edited["at"], created_at);
    assert_ne!(edited["editedAt"], Value::Null);
    assert_ne!(edited["editedAt"], edited["at"]);
    assert_eq!(edited["anchor"], anchor);
    assert_eq!(edited["quote"], quote);
}

#[tokio::test]
async fn block_transaction_comment_edit_updates_reply_without_changing_root() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "root note"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let root_id = review["comments"][0]["id"].as_str().unwrap().to_string();
    let root_at = review["comments"][0]["at"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": root_id,
                "body": "reply note"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let reply_id = review["comments"][0]["replies"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reply_at = review["comments"][0]["replies"][0]["at"]
        .as_str()
        .unwrap()
        .to_string();

    tokio::time::sleep(Duration::from_millis(2)).await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-reply",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": reply_id,
                "body": "edited reply"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let root = &review["comments"][0];
    let reply = &root["replies"][0];
    assert_eq!(root["body"], "root note");
    assert_eq!(root["at"], root_at);
    assert!(root["editedAt"].is_null());
    assert_eq!(reply["body"], "edited reply");
    assert_eq!(reply["at"], reply_at);
    assert_ne!(reply["editedAt"], Value::Null);
    assert_ne!(reply["editedAt"], reply["at"]);
}

#[tokio::test]
async fn block_transaction_comment_reply_targets_open_suggestion_and_edit_updates_reply() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Make this better.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 10,
                "end": 16,
                "replacement": "great"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion_id = review["suggestions"][0]["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "why this wording?"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert!(review["comments"].as_array().unwrap().is_empty());
    let reply = &review["suggestions"][0]["replies"][0];
    assert_eq!(reply["body"], "why this wording?");
    assert_eq!(reply["status"], "open");
    let reply_id = reply["id"].as_str().unwrap().to_string();
    let reply_at = reply["at"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-nested-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": reply_id,
                "body": "second reply"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let replies = review["suggestions"][0]["replies"].as_array().unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["body"], "second reply");

    tokio::time::sleep(Duration::from_millis(2)).await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-reply",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": reply_id,
                "body": "edited wording question"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let reply = &review["suggestions"][0]["replies"][0];
    assert_eq!(reply["body"], "edited wording question");
    assert_eq!(reply["at"], reply_at);
    assert_ne!(reply["editedAt"], Value::Null);
    assert_ne!(reply["editedAt"], reply["at"]);
}

#[tokio::test]
async fn block_transaction_comment_edit_rejects_non_open_comments() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Discuss this sentence.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 12,
                "body": "root note"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let comment_id = review["comments"][0]["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-resolve",
            serde_json::json!([{ "op": "comment.resolve", "item_id": comment_id }]),
        ),
    )
    .await;

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-edit-resolved",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": comment_id,
                "body": "should not land"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["comments"][0]["body"], "root note");
}

#[tokio::test]
async fn block_transaction_suggestion_accept_applies_replacement_and_resolves() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Make this better.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 10,
                "end": 16,
                "replacement": "great"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion = &review["suggestions"][0];
    assert_eq!(suggestion["status"], "open");
    assert_eq!(suggestion["kind"], "replace");
    assert_eq!(suggestion["preview"]["before"], "better");
    assert_eq!(suggestion["preview"]["after"], "great");
    let suggestion_id = suggestion["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "why this replacement?"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["suggestions"][0]["replies"][0]["body"],
        "why this replacement?"
    );

    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-accept",
            serde_json::json!([{ "op": "suggestion.accept", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_eq!(
        ack["changed_block_ids"],
        serde_json::json!([block_id.as_str()])
    );
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Make this great.\n"
    );
    let review = get_block_review(&app, "doc.md", false).await;
    assert!(review["suggestions"].as_array().unwrap().is_empty());
    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["suggestions"][0]["status"], "resolved");
    assert!(review["suggestions"][0]["replies"]
        .as_array()
        .unwrap()
        .is_empty());

    // Accepting again: already resolved.
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-accept-again",
            serde_json::json!([{ "op": "suggestion.accept", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "SUGGESTION_ALREADY_RESOLVED", false);
}

#[tokio::test]
async fn block_transaction_suggestion_reject_resolves_without_changing_text() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Keep this text.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 0,
                "end": 4,
                "replacement": "Drop"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion_id = review["suggestions"][0]["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "please explain"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(
        review["suggestions"][0]["replies"][0]["body"],
        "please explain"
    );

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reject",
            serde_json::json!([{ "op": "suggestion.reject", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Keep this text.\n"
    );
    let review = get_block_review(&app, "doc.md", true).await;
    assert_eq!(review["suggestions"][0]["status"], "resolved");
    assert!(review["suggestions"][0]["replies"]
        .as_array()
        .unwrap()
        .is_empty());

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply-resolved",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": suggestion_id,
                "body": "too late"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);
}

#[tokio::test]
async fn replace_block_content_orphans_overlapping_comments_and_shifts_suffix_anchors() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "prefix MIDDLE suffix\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    // One comment on the doomed middle, one on the surviving suffix.
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-anchors",
            serde_json::json!([
                {"op": "comment.add", "block_id": block_id, "start": 7, "end": 13, "body": "on middle"},
                {"op": "comment.add", "block_id": block_id, "start": 14, "end": 20, "body": "on suffix"}
            ]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-rewrite",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "prefix REWRITTEN-CENTER suffix"
            }]),
        ),
    )
    .await;

    let review = get_block_review(&app, "doc.md", false).await;
    let comments = review["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    let on_middle = comments
        .iter()
        .find(|comment| comment["body"] == "on middle")
        .unwrap();
    let on_suffix = comments
        .iter()
        .find(|comment| comment["body"] == "on suffix")
        .unwrap();
    // The overlapping comment orphaned and collapsed at the change site.
    assert_eq!(on_middle["status"], "orphaned");
    assert_eq!(
        on_middle["anchor"]["startOffset"],
        on_middle["anchor"]["endOffset"]
    );
    // The suffix comment survived with shifted offsets ("suffix" moved +10).
    assert_eq!(on_suffix["status"], "open");
    assert_eq!(on_suffix["anchor"]["startOffset"], 24);
    assert_eq!(on_suffix["anchor"]["endOffset"], 30);
}

#[tokio::test]
async fn suggestion_invalidated_by_a_content_change_cannot_be_accepted() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Suggest on this span.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-suggest",
            serde_json::json!([{
                "op": "suggestion.add",
                "block_id": block_id,
                "start": 11,
                "end": 15,
                "replacement": "that"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    let suggestion_id = review["suggestions"][0]["id"].as_str().unwrap().to_string();

    // Rewrite the anchored span out from under the suggestion.
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-rewrite",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Suggest on changed span."
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(review["suggestions"][0]["status"], "invalidated");

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-accept",
            serde_json::json!([{ "op": "suggestion.accept", "item_id": suggestion_id }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "SUGGESTION_INVALIDATED", false);
}

#[tokio::test]
async fn delete_block_orphans_comments_and_invalidates_suggestions() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Doomed block.\n\nSurvivor.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let doomed = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-anchors",
            serde_json::json!([
                {"op": "comment.add", "block_id": doomed, "start": 0, "end": 6, "body": "note"},
                {"op": "suggestion.add", "block_id": doomed, "start": 0, "end": 6, "replacement": "Saved"}
            ]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-delete",
            serde_json::json!([{ "op": "delete_block", "block_id": doomed }]),
        ),
    )
    .await;

    assert_eq!(get_document_markdown(&app, "doc.md").await, "Survivor.\n");
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(review["comments"][0]["status"], "orphaned");
    assert_eq!(review["suggestions"][0]["status"], "invalidated");
}

#[tokio::test]
async fn block_transaction_duplicate_client_tx_id_replays_the_original_ack() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Idempotent.\n").await;
    get_block_tree(&app, "doc.md").await;
    let request = block_tx(
        "tx-same",
        serde_json::json!([{
            "op": "insert_block",
            "position": 1,
            "block_type": "p",
            "text": "Appended."
        }]),
    );

    let first = commit_block_transaction(&app, "doc.md", request.clone()).await;
    let versions_after_first = raw_version_count(&app, "doc.md").await;
    let second = commit_block_transaction(&app, "doc.md", request).await;

    assert_eq!(second, first);
    assert_eq!(
        raw_version_count(&app, "doc.md").await,
        versions_after_first
    );
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Idempotent.\n\nAppended.\n"
    );
}

#[tokio::test]
async fn block_transaction_clock_handling_commits_rebases_and_rejects() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Clocked.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();
    let clock_one = tree["document_clock"].as_str().unwrap().to_string();

    // Matching clock (ETag-quoted) applies as `committed`.
    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx_with_clock(
            "tx-matching",
            &format!("\"{clock_one}\""),
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Clocked once."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["status"], "committed");

    // A stale-but-valid clock (clock_one is now one version behind) applies
    // as `committed_rebased` because the referenced block still validates.
    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx_with_clock(
            "tx-stale-valid",
            &clock_one,
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Clocked twice."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["status"], "committed_rebased");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Clocked twice.\n"
    );

    // An unknown clock is retryable STALE_BASE.
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx_with_clock(
            "tx-unknown-clock",
            "no-such-version",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Never lands."
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "STALE_BASE", true);
}

#[tokio::test]
async fn block_transaction_typed_reference_errors() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Reference target.\n").await;
    get_block_tree(&app, "doc.md").await;

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-missing-block",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": "no-such-block",
                "text": "nope"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "BLOCK_DELETED", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-missing-anchor",
            serde_json::json!([{ "op": "comment.resolve", "item_id": "no-such-item" }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "ANCHOR_NOT_FOUND", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-reply-missing-parent",
            serde_json::json!([{
                "op": "comment.reply",
                "item_id": "no-such-parent",
                "body": "orphan reply"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "ANCHOR_NOT_FOUND", false);

    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-bad-move",
            serde_json::json!([{
                "op": "move_block",
                "block_id": block_id,
                "parent_block_id": "no-such-parent",
                "position": 0
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "BLOCK_MOVE_CONFLICT", true);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-bad-op",
            serde_json::json!([{ "op": "explode_block", "block_id": "x" }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);
}

#[tokio::test]
async fn block_transaction_unsupported_markdown_rolls_back() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "A text paragraph.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    // Nesting a block under a text-bearing leaf produces an unexportable
    // tree (containers carry no inline content).
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-nest",
            serde_json::json!([{
                "op": "insert_block",
                "parent_block_id": block_id,
                "position": 0,
                "block_type": "p",
                "text": "nested"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "UNSUPPORTED_MARKDOWN", false);
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "A text paragraph.\n"
    );
}

#[tokio::test]
async fn block_transaction_multi_op_failure_rolls_back_the_whole_transaction() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Atomic.\n").await;
    let before = get_block_tree(&app, "doc.md").await;
    let versions_before = raw_version_count(&app, "doc.md").await;

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-atomic",
            serde_json::json!([
                {"op": "insert_block", "position": 1, "block_type": "p", "text": "Would apply."},
                {"op": "delete_block", "block_id": "no-such-block"}
            ]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "BLOCK_DELETED", false);
    assert_eq!(get_document_markdown(&app, "doc.md").await, "Atomic.\n");
    assert_eq!(get_block_tree(&app, "doc.md").await, before);
    assert_eq!(raw_version_count(&app, "doc.md").await, versions_before);
}

#[tokio::test]
async fn block_transaction_multi_op_success_commits_one_version() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Start.\n").await;
    get_block_tree(&app, "doc.md").await;
    let versions_before = raw_version_count(&app, "doc.md").await;

    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-two-inserts",
            serde_json::json!([
                {"op": "insert_block", "position": 1, "block_type": "p", "text": "Middle."},
                {"op": "insert_block", "position": 2, "block_type": "p", "text": "End."}
            ]),
        ),
    )
    .await;
    assert_eq!(ack["changed_block_ids"].as_array().unwrap().len(), 2);
    assert_eq!(raw_version_count(&app, "doc.md").await, versions_before + 1);
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Start.\n\nMiddle.\n\nEnd.\n"
    );
}

#[tokio::test]
async fn orphaned_anchor_survives_a_later_insertion_at_the_orphan_seam() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "prefix MIDDLE suffix\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 7,
                "end": 13,
                "body": "doomed"
            }]),
        ),
    )
    .await;
    // Rewriting the middle orphans the comment, collapsed at offset 7.
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-orphan",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "prefix CHANGED suffix"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(review["comments"][0]["status"], "orphaned");
    assert_eq!(review["comments"][0]["anchor"]["startOffset"], 7);
    assert_eq!(review["comments"][0]["anchor"]["endOffset"], 7);

    // Regression: a pure insertion exactly at the orphan seam used to invert
    // the collapsed anchor to [8, 7) and poison the document with an untyped
    // 400. It must commit, and the dead anchor must stay a point.
    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-insert-at-seam",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "prefix XCHANGED suffix"
            }]),
        ),
    )
    .await;
    assert_eq!(ack["status"], "committed");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "prefix XCHANGED suffix\n"
    );
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(review["comments"][0]["status"], "orphaned");
    assert_eq!(review["comments"][0]["anchor"]["startOffset"], 7);
    assert_eq!(review["comments"][0]["anchor"]["endOffset"], 7);
}

#[tokio::test]
async fn raw_markdown_attrs_must_keep_the_markdown_key() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "<div>\nopaque\n</div>\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    // Wholesale attrs replacement without the markdown key would silently
    // erase the block's content; it must be rejected instead.
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-erase",
            serde_json::json!([{
                "op": "set_block_attrs",
                "block_id": block_id,
                "attrs": {"note": "markdown key missing"}
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "<div>\nopaque\n</div>\n"
    );

    // Inserting a raw block without (or with a blank) markdown attribute is
    // rejected the same way; a valid raw insert commits with its content.
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-empty-raw",
            serde_json::json!([{
                "op": "insert_block",
                "position": 1,
                "block_type": "raw_markdown",
                "attrs": {}
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);
    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-valid-raw",
            serde_json::json!([{
                "op": "insert_block",
                "position": 1,
                "block_type": "raw_markdown",
                "attrs": {"markdown": "<span>kept</span>"}
            }]),
        ),
    )
    .await;
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "<div>\nopaque\n</div>\n\n<span>kept</span>\n"
    );
}

#[tokio::test]
async fn ops_against_raw_markdown_blocks_are_invalid_transactions() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Para.\n\n<div>\nopaque\n</div>\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    assert_eq!(tree["blocks"][1]["block_type"], "raw_markdown");
    let para = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();
    let raw = tree["blocks"][1]["block_id"].as_str().unwrap().to_string();

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-raw-text",
            serde_json::json!([{ "op": "replace_block_content", "block_id": raw, "text": "x" }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-raw-add-mark",
            serde_json::json!([{
                "op": "add_mark", "block_id": raw, "start": 0, "end": 1, "marks": {"bold": true}
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-raw-remove-mark",
            serde_json::json!([{
                "op": "remove_mark", "block_id": raw, "start": 0, "end": 1, "marks": ["bold"]
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-raw-link",
            serde_json::json!([{
                "op": "set_link", "block_id": raw, "start": 0, "end": 1, "url": "https://example.com"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-raw-comment",
            serde_json::json!([{
                "op": "comment.add", "block_id": raw, "start": 0, "end": 1, "body": "?"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-raw-suggest",
            serde_json::json!([{
                "op": "suggestion.add", "block_id": raw, "start": 0, "end": 1, "replacement": "y"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    // Type changes to or from raw_markdown lose the content model; both
    // directions are rejected.
    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-from-raw",
            serde_json::json!([{ "op": "set_block_type", "block_id": raw, "block_type": "p" }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    let (status, body) = post_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-to-raw",
            serde_json::json!([{
                "op": "set_block_type", "block_id": para, "block_type": "raw_markdown"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "INVALID_TRANSACTION", false);

    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "Para.\n\n<div>\nopaque\n</div>\n"
    );
}

#[tokio::test]
async fn move_block_preserves_children_and_review_anchors() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "```rust\nline one\n```\n\nAfter.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    assert_eq!(tree["blocks"][0]["block_type"], "code_block");
    assert_eq!(tree["blocks"][1]["block_type"], "code_line");
    let code_block = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();
    let code_line = tree["blocks"][1]["block_id"].as_str().unwrap().to_string();
    assert_eq!(tree["blocks"][1]["parent_block_id"], code_block.as_str());

    commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-anchor",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": code_line,
                "start": 0,
                "end": 4,
                "body": "on the moved subtree"
            }]),
        ),
    )
    .await;
    let ack = commit_block_transaction(
        &app,
        "doc.md",
        block_tx(
            "tx-move",
            serde_json::json!([{
                "op": "move_block",
                "block_id": code_block,
                "position": 1
            }]),
        ),
    )
    .await;
    assert_eq!(
        ack["changed_block_ids"],
        serde_json::json!([code_block.as_str()])
    );

    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "After.\n\n```rust\nline one\n```\n"
    );
    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][1]["block_id"], code_block.as_str());
    assert_eq!(after["blocks"][2]["block_id"], code_line.as_str());
    assert_eq!(after["blocks"][2]["parent_block_id"], code_block.as_str());
    assert_eq!(after["blocks"][2]["text"], "line one");

    let review = get_block_review(&app, "doc.md", false).await;
    let comment = &review["comments"][0];
    assert_eq!(comment["status"], "open");
    assert_eq!(comment["quote"], "line");
    assert_eq!(comment["anchor"]["blockId"], code_line.as_str());
    assert_eq!(comment["anchor"]["startOffset"], 0);
    assert_eq!(comment["anchor"]["endOffset"], 4);
}

// ---------------------------------------------------------------------------
// Phase 3: ephemeral sessions and the mode switch
// ---------------------------------------------------------------------------

/// A live server (real listener for websockets) sharing state with a router
/// clone for in-process REST calls.
async fn spawn_session_server() -> (
    tempfile::TempDir,
    std::net::SocketAddr,
    axum::Router,
    QuarryStore,
    tokio::task::JoinHandle<()>,
) {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store.clone());
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    (root, addr, app, store, server)
}

async fn spawn_shutdown_session_server() -> (
    tempfile::TempDir,
    std::net::SocketAddr,
    axum::Router,
    QuarryStore,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let state = app_state(store.clone());
    let app = router_with_state(state.clone());
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

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_state_with_shutdown(state, addr, async {
        let _ = shutdown_rx.await;
    }));
    wait_for_server(addr).await;
    (root, addr, app, store, shutdown_tx, server)
}

async fn wait_for_server(addr: std::net::SocketAddr) {
    timeout(Duration::from_secs(2), async {
        loop {
            match tokio::net::TcpStream::connect(addr).await {
                Ok(_) => break,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("server did not start listening");
}

type WsSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connects a y-websocket-style client to a document's session and completes
/// the initial sync into a fresh local doc.
async fn connect_session(addr: std::net::SocketAddr, document_id: &str) -> (WsSocket, Doc) {
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{document_id}"))
            .await
            .unwrap();
    let doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &doc).await;
    (socket, doc)
}

/// Applies a local edit to the client doc and sends the resulting update to
/// the server, then waits for the server's echo (proof it was applied).
async fn send_local_edit(
    socket: &mut WsSocket,
    doc: &Doc,
    edit: impl FnOnce(&mut yrs::TransactionMut<'_>, &XmlTextRef),
) {
    send_local_edit_unechoed(socket, doc, edit).await;
    // The session broadcasts every applied update, including back to its
    // origin; the echo proves the server's doc has it.
    wait_for_yjs_sync_update(socket, doc).await;
}

/// Like [`send_local_edit`] but without waiting for the server's echo:
/// several updates can be packed back-to-back into one debounce window
/// without round trips between them (a stalled echo wait would otherwise
/// let the debounce fire early and split the checkpoint).
async fn send_local_edit_unechoed(
    socket: &mut WsSocket,
    doc: &Doc,
    edit: impl FnOnce(&mut yrs::TransactionMut<'_>, &XmlTextRef),
) {
    let before = doc.transact().state_vector();
    {
        let mut txn = doc.transact_mut();
        let text = txn.get_or_insert_text(COLLAB_ROOT);
        let root: &XmlTextRef = text.as_ref();
        let root = root.clone();
        edit(&mut txn, &root);
    }
    let update = doc.transact().encode_state_as_update_v1(&before);
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::Update(update))
                .encode_v1()
                .into(),
        ))
        .await
        .unwrap();
}

/// Publishes a slate-yjs-style awareness state carrying the author name,
/// exactly as the Plate editor's cursor data does.
async fn send_awareness_name(socket: &mut WsSocket, doc: &Doc, name: &str) {
    let json = format!(r##"{{"data":{{"name":"{name}","color":"#8be9fd"}}}}"##);
    send_awareness_state(socket, doc, 1, &json).await;
}

/// Withdraws this client's awareness state: the y-protocol `null` entry a
/// client publishes on clean departure (clock bumped past the set above).
async fn send_awareness_removal(socket: &mut WsSocket, doc: &Doc) {
    send_awareness_state(socket, doc, 2, "null").await;
}

async fn send_awareness_state(socket: &mut WsSocket, doc: &Doc, clock: u32, json: &str) {
    use yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
    let update = AwarenessUpdate {
        clients: std::collections::HashMap::from([(
            doc.client_id(),
            AwarenessUpdateEntry {
                clock,
                json: json.into(),
            },
        )]),
    };
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Awareness(update).encode_v1().into(),
        ))
        .await
        .unwrap();
}

async fn document_id_of(store: &QuarryStore, path: &str) -> String {
    store.head_document("blocks", path).await.unwrap().id
}

/// Polls the persisted markdown until it contains `needle` (checkpoints are
/// asynchronous after a socket closes).
async fn wait_for_markdown_containing(app: &axum::Router, path: &str, needle: &str) -> String {
    timeout(Duration::from_secs(5), async {
        loop {
            let markdown = get_document_markdown(app, path).await;
            if markdown.contains(needle) {
                break markdown;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("persisted markdown never contained {needle:?}"))
}

#[tokio::test]
async fn legacy_edit_ops_and_review_process_endpoints_are_gone() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "Hello.\n").await;

    // The deleted facades 404 like any unknown route (the simplest honest
    // end state); `POST .../transactions` is the single mutation contract.
    async fn assert_not_found(app: &axum::Router, endpoint: &str, body: Value) {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                &format!("/v1/libraries/blocks/documents/doc.md{endpoint}"),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{endpoint}");
    }

    assert_not_found(
        &app,
        "/edit",
        serde_json::json!({"baseToken": "x", "operations": []}),
    )
    .await;
    assert_not_found(
        &app,
        "/ops",
        ops_request("x", serde_json::json!({"op": "comment.add"})),
    )
    .await;
    assert_not_found(
        &app,
        "/review",
        serde_json::json!({
            "baseToken": "x",
            "operations": [{ "op": "comment.resolve", "id": "c1" }]
        }),
    )
    .await;

    // The read-side review projection is unaffected by the deletion.
    let review = get_block_review(&app, "doc.md", false).await;
    assert_eq!(review["comments"], serde_json::json!([]));
}

#[tokio::test]
async fn session_seeds_from_rows_and_final_checkpoint_persists_typing() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Hello session.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    assert_eq!(yjs_plain_text(&doc), "Hello session.");

    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 14, " Typed live.");
    })
    .await;
    socket.close(None).await.unwrap();

    // Last subscriber left: the final checkpoint persists the typing as
    // canonical rows + one coalesced browser_session history row.
    let markdown = wait_for_markdown_containing(&app, "live.md", "Typed live.").await;
    assert_eq!(markdown, "Hello session. Typed live.\n");
    let tree = get_block_tree(&app, "live.md").await;
    assert_eq!(tree["blocks"][0]["text"], "Hello session. Typed live.");

    server.abort();
}

#[tokio::test]
async fn shutdown_closes_live_collab_socket_and_runs_final_checkpoint() {
    let (_root, addr, app, store, shutdown, server) = spawn_shutdown_session_server().await;
    put_block_markdown(&app, "live.md", "Shutdown target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 16, " Preserved.");
    })
    .await;

    shutdown.send(()).unwrap();
    let next = timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("collab socket should close promptly after shutdown");
    assert!(matches!(
        next,
        None | Some(Ok(TungsteniteMessage::Close(_))) | Some(Err(_))
    ));

    let markdown = wait_for_markdown_containing(&app, "live.md", "Preserved.").await;
    assert_eq!(markdown, "Shutdown target. Preserved.\n");
    let tree = get_block_tree(&app, "live.md").await;
    assert_eq!(tree["blocks"][0]["text"], "Shutdown target. Preserved.");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server should finish after cooperative shutdown")
        .unwrap()
        .unwrap();
}

/// Phase 5 checkpoint-ack protocol: a custom `MSG_QUARRY_CHECKPOINT` frame
/// carrying the committed doc snapshot is sent to each new subscriber on
/// join and broadcast after every durable commit (debounced checkpoint or
/// session-mode transaction). A client compares the acked snapshot against
/// its own doc — equality means "everything I see is canonical" (`Saved`).
#[tokio::test]
async fn checkpoint_commits_broadcast_snapshot_ack_frames() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Ack target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    // Join: the seed's committed snapshot arrives as the first ack frame and
    // matches the synced client doc exactly (clean session = Saved).
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{document_id}"))
            .await
            .unwrap();
    let doc = empty_yjs_doc();
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::SyncStep1(doc.transact().state_vector()))
                .encode_v1()
                .into(),
        ))
        .await
        .unwrap();
    let seed_ack = next_checkpoint_ack(&mut socket, &doc).await;
    wait_for_yjs_plain_text(&mut socket, &doc, "Ack target.").await;
    assert_eq!(
        decode_snapshot(&seed_ack),
        doc.transact().snapshot(),
        "the join ack covers the seeded state"
    );

    // Typing makes the local doc run ahead of the last ack (Saving…); the
    // debounced checkpoint commits and broadcasts a new ack that covers it.
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 11, " Typed.");
    })
    .await;
    assert_ne!(decode_snapshot(&seed_ack), doc.transact().snapshot());
    let checkpoint_ack = next_checkpoint_ack(&mut socket, &doc).await;
    assert_eq!(decode_snapshot(&checkpoint_ack), doc.transact().snapshot());
    assert_eq!(
        get_document_markdown(&app, "live.md").await,
        "Ack target. Typed.\n"
    );

    // A session-mode transaction commits before acking and broadcasts the
    // covering snapshot the same way (its doc update precedes the ack).
    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-ack",
            serde_json::json!([{
                "op": "insert_block",
                "position": 1,
                "block_type": "p",
                "text": "Agent block."
            }]),
        ),
    )
    .await;
    let transaction_ack = next_checkpoint_ack(&mut socket, &doc).await;
    assert_eq!(decode_snapshot(&transaction_ack), doc.transact().snapshot());
    assert!(yjs_plain_text(&doc).contains("Agent block."));

    socket.close(None).await.unwrap();
    server.abort();
}

/// A checkpoint that cannot project (here: a bare text node at block level,
/// a shape the session projection rejects) broadcasts a
/// `MSG_QUARRY_CHECKPOINT_FAILED` frame so still-connected browsers surface
/// "Save failed" instead of a benign "Saving…".
#[tokio::test]
async fn failing_checkpoints_broadcast_a_checkpoint_failed_frame() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Failure probe.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, root| {
        root.insert(txn, 0, "bare text at block level");
    })
    .await;

    next_checkpoint_failure(&mut socket, &doc).await;
    server.abort();
}

/// Waits for the next `MSG_QUARRY_CHECKPOINT_FAILED` frame, applying
/// interleaved y-sync messages and skipping ack frames.
async fn next_checkpoint_failure<S>(socket: &mut S, doc: &Doc)
where
    S: Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    timeout(Duration::from_secs(5), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let TungsteniteMessage::Binary(bytes) = message else {
                continue;
            };
            use yrs::encoding::read::Read;
            let mut cursor = yrs::encoding::read::Cursor::new(bytes.as_ref());
            match cursor.read_var::<u8>() {
                Ok(quarry_server::MSG_QUARRY_CHECKPOINT_FAILED) => break,
                Ok(quarry_server::MSG_QUARRY_CHECKPOINT) => continue,
                _ => {
                    apply_yjs_message(doc, bytes.as_ref());
                }
            }
        }
    })
    .await
    .expect("no checkpoint-failed frame arrived")
}

/// Waits for the next `MSG_QUARRY_CHECKPOINT` frame, applying interleaved
/// y-sync messages to the local doc (updates broadcast before their ack).
async fn next_checkpoint_ack<S>(socket: &mut S, doc: &Doc) -> Vec<u8>
where
    S: Stream<Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    timeout(Duration::from_secs(5), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let TungsteniteMessage::Binary(bytes) = message else {
                continue;
            };
            if let Some(snapshot) = decode_checkpoint_ack_frame(bytes.as_ref()) {
                break snapshot;
            }
            apply_yjs_message(doc, bytes.as_ref());
        }
    })
    .await
    .expect("no checkpoint ack frame arrived")
}

fn decode_checkpoint_ack_frame(bytes: &[u8]) -> Option<Vec<u8>> {
    use yrs::encoding::read::Read;
    let mut cursor = yrs::encoding::read::Cursor::new(bytes);
    let message_type: u8 = cursor.read_var().ok()?;
    if message_type != quarry_server::MSG_QUARRY_CHECKPOINT {
        return None;
    }
    Some(cursor.read_buf().ok()?.to_vec())
}

fn decode_snapshot(bytes: &[u8]) -> yrs::Snapshot {
    yrs::Snapshot::decode_v1(bytes).expect("ack frames carry a v1-encoded snapshot")
}

/// Resolves the nth top-level block inside an open transaction (the client
/// doc's root must be fetched through the same txn that edits it).
fn nth_block_text_in(txn: &mut yrs::TransactionMut<'_>, index: usize) -> XmlTextRef {
    use yrs::types::text::YChange;
    let text = txn.get_or_insert_text(COLLAB_ROOT);
    let root: &XmlTextRef = text.as_ref();
    let root = root.clone();
    let embeds: Vec<XmlTextRef> = root
        .diff(txn, YChange::identity)
        .into_iter()
        .filter_map(|diff| match diff.insert {
            Out::YXmlText(child) => Some(child),
            Out::YText(child) => {
                let child: &XmlTextRef = child.as_ref();
                Some(child.clone())
            }
            _ => None,
        })
        .collect();
    embeds[index].clone()
}

/// The reviewer's C1 probe: an unknown inline mark (a future Plate plugin,
/// or arbitrary bytes on the unauthenticated socket) must never wedge the
/// session into unpersistable state. The checkpoint drops the unknown mark
/// and persists everything else.
#[tokio::test]
async fn checkpoint_succeeds_despite_unknown_inline_marks() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Mark target text.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 17, " Typed.");
        block.format(
            txn,
            0,
            4,
            [("weird_mark".into(), yrs::Any::Bool(true))]
                .into_iter()
                .collect(),
        );
        block.format(
            txn,
            5,
            6,
            [("bold".into(), yrs::Any::Bool(true))]
                .into_iter()
                .collect(),
        );
    })
    .await;

    // The debounced checkpoint must succeed, persist the typing and the
    // known mark, and drop the unknown one.
    let markdown = wait_for_markdown_containing(&app, "live.md", "Typed.").await;
    assert_eq!(markdown, "Mark **target** text. Typed.\n");
    let tree = get_block_tree(&app, "live.md").await;
    assert_eq!(tree["blocks"][0]["text"], "Mark target text. Typed.");
    let marks = tree["blocks"][0]["marks"].as_array().unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0]["marks"], serde_json::json!({"bold": true}));

    socket.close(None).await.unwrap();
    server.abort();
}

/// The R1 probe: a KNOWN `code` mark spanning a link's inner text (the
/// editor's CodePlugin + LinkPlugin shape). Drop-containment does not apply
/// (`code` is renderable), so the writer must render the code span INSIDE
/// the link text instead of wedging every checkpoint with
/// "code mark on a non-text span".
#[tokio::test]
async fn checkpoint_succeeds_with_code_marks_inside_link_text() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "See [docs](https://example.test) now.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        let link = first_embed_in(txn, &block);
        link.format(
            txn,
            0,
            4,
            [("code".into(), yrs::Any::Bool(true))]
                .into_iter()
                .collect(),
        );
        // Block-local indices: "See " (4) + link embed (1) + " now." — type
        // at the end of the block alongside the formatting change.
        block.insert(txn, 10, " Typed.");
    })
    .await;

    let markdown = wait_for_markdown_containing(&app, "live.md", "Typed.").await;
    assert_eq!(markdown, "See [`docs`](https://example.test) now. Typed.\n");
    let tree = get_block_tree(&app, "live.md").await;
    assert_eq!(tree["blocks"][0]["text"], "See docs now. Typed.");
    assert_eq!(
        tree["blocks"][0]["marks"],
        serde_json::json!([{"start": 4, "end": 8, "marks": {"code": true}}])
    );
    assert_eq!(tree["blocks"][0]["links"][0]["start"], 4);
    assert_eq!(tree["blocks"][0]["links"][0]["end"], 8);

    socket.close(None).await.unwrap();
    server.abort();
}

/// Resolves the first inline embed (a link) inside an open transaction.
fn first_embed_in(txn: &mut yrs::TransactionMut<'_>, block: &XmlTextRef) -> XmlTextRef {
    use yrs::types::text::YChange;
    block
        .diff(txn, YChange::identity)
        .into_iter()
        .find_map(|diff| match diff.insert {
            Out::YXmlText(child) => Some(child),
            Out::YText(child) => {
                let child: &XmlTextRef = child.as_ref();
                Some(child.clone())
            }
            _ => None,
        })
        .expect("block contains an inline embed")
}

/// The discard variant of the C1 probe: previously the final checkpoint
/// failed on the unknown mark and ALL un-checkpointed edits (including
/// plain typing) were lost with only a warn log.
#[tokio::test]
async fn final_checkpoint_persists_typing_despite_unknown_inline_marks() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Discard probe.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 14, " Survives.");
        block.format(
            txn,
            0,
            7,
            [("weird_mark".into(), yrs::Any::Bool(true))]
                .into_iter()
                .collect(),
        );
    })
    .await;
    socket.close(None).await.unwrap();

    let markdown = wait_for_markdown_containing(&app, "live.md", "Survives.").await;
    assert_eq!(markdown, "Discard probe. Survives.\n");
    let tree = get_block_tree(&app, "live.md").await;
    assert_eq!(tree["blocks"][0]["marks"], serde_json::json!([]));
    server.abort();
}

#[tokio::test]
async fn multiple_typed_updates_coalesce_into_one_debounced_checkpoint() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Coalesce target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let versions_before = raw_version_count(&app, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    // Packed back-to-back (no echo round trips) so all three land inside
    // one debounce window even on a stalled CI runner.
    send_local_edit_unechoed(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 16, " one");
    })
    .await;
    send_local_edit_unechoed(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 20, " two");
    })
    .await;
    send_local_edit_unechoed(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 24, " three");
    })
    .await;

    // All three updates land within one debounce window: exactly ONE new
    // version, whose history row is the coalesced browser_session commit.
    let markdown = wait_for_markdown_containing(&app, "live.md", "three").await;
    assert_eq!(markdown, "Coalesce target. one two three\n");
    let versions = raw_versions(&app, "live.md").await;
    let versions = versions.as_array().unwrap();
    assert_eq!(versions.len(), versions_before + 1);
    let checkpoint = &versions[0];
    assert_eq!(checkpoint["transaction_actor"], "browser");
    assert_eq!(checkpoint["transaction_message"], "Live session edits");
    assert_eq!(
        checkpoint["transaction_provenance"]["history"]["kind"],
        "autosave"
    );
    assert_eq!(
        checkpoint["transaction_provenance"]["history"]["reason"],
        "session_checkpoint"
    );

    // Leaving with nothing new to persist adds no further version.
    socket.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        raw_version_count(&app, "live.md").await,
        versions_before + 1
    );
    server.abort();
}

async fn raw_versions(app: &axum::Router, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/blocks/documents/{path}/versions/raw"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

#[tokio::test]
async fn session_checkpoint_attributes_awareness_author() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Attribution target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_awareness_name(&mut socket, &doc, "Avery").await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 19, " Signed.");
    })
    .await;

    let markdown = wait_for_markdown_containing(&app, "live.md", "Signed.").await;
    assert_eq!(markdown, "Attribution target. Signed.\n");
    let versions = raw_versions(&app, "live.md").await;
    assert_eq!(
        versions.as_array().unwrap()[0]["transaction_actor"],
        "Avery"
    );
    server.abort();
}

/// The abrupt tab-close case: the client never sends an awareness removal,
/// so the author name survives into the final post-disconnect checkpoint
/// (directly in awareness, or via the session's cached label).
#[tokio::test]
async fn final_checkpoint_after_disconnect_attributes_author() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Disconnect target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_awareness_name(&mut socket, &doc, "Avery").await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 18, " Closed.");
    })
    .await;
    socket.close(None).await.unwrap();

    let markdown = wait_for_markdown_containing(&app, "live.md", "Closed.").await;
    assert_eq!(markdown, "Disconnect target. Closed.\n");
    let versions = raw_versions(&app, "live.md").await;
    assert_eq!(
        versions.as_array().unwrap()[0]["transaction_actor"],
        "Avery"
    );
    server.abort();
}

/// Forces the `live_actor` cache path: after the named participant withdraws
/// their awareness state, the next checkpoint observes a name-less awareness
/// and must fall back to the label cached by the first checkpoint.
#[tokio::test]
async fn checkpoint_after_awareness_removal_uses_cached_author() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Cache target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_awareness_name(&mut socket, &doc, "Avery").await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 13, " First.");
    })
    .await;
    // A committed version containing the first edit can only come from
    // `commit_doc_state`, which primes the cache while awareness still
    // carries the name (single-socket ordering: the removal is sent later).
    wait_for_markdown_containing(&app, "live.md", "First.").await;

    send_awareness_removal(&mut socket, &doc).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 20, " Second.");
    })
    .await;
    socket.close(None).await.unwrap();

    let markdown = wait_for_markdown_containing(&app, "live.md", "Second.").await;
    assert_eq!(markdown, "Cache target. First. Second.\n");
    let versions = raw_versions(&app, "live.md").await;
    assert_eq!(
        versions.as_array().unwrap()[0]["transaction_actor"],
        "Avery"
    );
    server.abort();
}

#[tokio::test]
async fn session_transaction_lands_in_live_doc_and_rows_before_ack() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Humans here.\n\nAgent target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let target = tree["blocks"][1]["block_id"].as_str().unwrap().to_string();

    let (mut socket, doc) = connect_session(addr, &document_id).await;

    let ack = commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-live-1",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": target,
                "text": "Agent rewrote the target."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["status"], "committed");
    assert_eq!(ack["changed_block_ids"], serde_json::json!([target]));

    // Checkpoint-before-ack: rows are durable the moment the ack returns.
    let after = get_block_tree(&app, "live.md").await;
    assert_eq!(after["blocks"][1]["text"], "Agent rewrote the target.");
    assert_eq!(after["blocks"][1]["block_id"], target.as_str());
    assert_eq!(after["document_clock"], ack["document_clock"]);

    // And the live session converged through the websocket.
    wait_for_yjs_plain_text(&mut socket, &doc, "Humans here.Agent rewrote the target.").await;

    socket.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn session_transaction_coalesces_unflushed_typing_first() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Typing block.\n\nAgent block.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let agent_block = tree["blocks"][1]["block_id"].as_str().unwrap().to_string();
    let versions_before = raw_version_count(&app, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 13, " 123");
    })
    .await;

    let ack = commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-live-2",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": agent_block,
                "text": "Agent landed."
            }]),
        ),
    )
    .await;

    // Both the unflushed typing (coalesced browser_session checkpoint) and
    // the transaction are durable at ack time, as separate versions with
    // separate attribution.
    let markdown = get_document_markdown(&app, "live.md").await;
    assert_eq!(markdown, "Typing block. 123\n\nAgent landed.\n");
    assert_eq!(ack["status"], "committed");
    let versions_after = raw_version_count(&app, "live.md").await;
    assert_eq!(versions_after, versions_before + 2);

    socket.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn transaction_racing_session_seed_is_never_rejected() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "race.md", "Race target.\n").await;
    let document_id = document_id_of(&store, "race.md").await;
    let tree = get_block_tree(&app, "race.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    let tx_app = app.clone();
    let tx_block = block_id.clone();
    let ((mut socket, doc), ack) = tokio::join!(connect_session(addr, &document_id), async move {
        commit_block_transaction(
            &tx_app,
            "race.md",
            block_tx(
                "tx-race-seed",
                serde_json::json!([{
                    "op": "replace_block_content",
                    "block_id": tx_block,
                    "text": "Race won by everyone."
                }]),
            ),
        )
        .await
    });
    assert_eq!(ack["changed_block_ids"], serde_json::json!([block_id]));
    assert_eq!(
        get_document_markdown(&app, "race.md").await,
        "Race won by everyone.\n"
    );
    // The session converges regardless of which side won the mutex: either
    // the seed already contained the change or the broadcast delivers it.
    if yjs_plain_text(&doc) != "Race won by everyone." {
        wait_for_yjs_plain_text(&mut socket, &doc, "Race won by everyone.").await;
    }
    socket.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn transaction_racing_final_checkpoint_and_discard_succeeds() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "race2.md", "First block.\n\nSecond block.\n").await;
    let document_id = document_id_of(&store, "race2.md").await;
    let tree = get_block_tree(&app, "race2.md").await;
    let second = tree["blocks"][1]["block_id"].as_str().unwrap().to_string();

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 12, " typed");
    })
    .await;
    socket.close(None).await.unwrap();

    // The transaction races the leave/final-checkpoint/discard transition;
    // the per-document mutex serializes them and the write is never
    // rejected. Both the typing and the op are durable afterwards.
    let ack = commit_block_transaction(
        &app,
        "race2.md",
        block_tx(
            "tx-race-discard",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": second,
                "text": "Op landed."
            }]),
        ),
    )
    .await;
    assert_eq!(ack["changed_block_ids"], serde_json::json!([second]));
    let markdown = wait_for_markdown_containing(&app, "race2.md", "typed").await;
    assert_eq!(markdown, "First block. typed\n\nOp landed.\n");
    server.abort();
}

#[tokio::test]
async fn two_transactions_share_one_session() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Alpha.\n\nBeta.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let alpha = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();
    let beta = tree["blocks"][1]["block_id"].as_str().unwrap().to_string();

    let (mut socket, doc) = connect_session(addr, &document_id).await;

    let first = commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-a",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": alpha,
                "text": "Alpha rewritten."
            }]),
        ),
    )
    .await;
    let second = commit_block_transaction(
        &app,
        "live.md",
        block_tx_with_clock(
            "tx-b",
            first["document_clock"].as_str().unwrap(),
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": beta,
                "text": "Beta rewritten."
            }]),
        ),
    )
    .await;
    assert_eq!(second["status"], "committed");

    assert_eq!(
        get_document_markdown(&app, "live.md").await,
        "Alpha rewritten.\n\nBeta rewritten.\n"
    );
    wait_for_yjs_plain_text(&mut socket, &doc, "Alpha rewritten.Beta rewritten.").await;
    socket.close(None).await.unwrap();
    server.abort();
}

/// Phase 5 deleted the PUT-as-checkpoint transitional rule: a Markdown PUT
/// carrying a `browser:*` origin on a session-active document is an
/// ordinary whole-file write through the Phase 4 reconciler — its body is
/// honored, it merges into the live doc as a collaborator edit, and the
/// session's own typing survives the merge.
#[tokio::test]
async fn browser_origin_markdown_put_is_an_ordinary_reconciled_write() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(
        &app,
        "live.md",
        "Session content.\n\nStable separator.\n\nPut target.\n",
    )
    .await;
    let document_id = document_id_of(&store, "live.md").await;
    let base_etag = head_etag(&app, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 16, " Typed.");
    })
    .await;

    // A browser-origin PUT editing the LAST block, based on the pre-typing
    // version: diff3 applies its hunk while the session keeps the typing.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/live.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("If-Match", &base_etag)
                .header("X-Quarry-Origin-Id", "browser:writer-1")
                .body(Body::from(
                    "Session content.\n\nStable separator.\n\nPut target rewritten.\n",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        get_document_markdown(&app, "live.md").await,
        "Session content. Typed.\n\nStable separator.\n\nPut target rewritten.\n"
    );
    wait_for_yjs_plain_text(
        &mut socket,
        &doc,
        "Session content. Typed.Stable separator.Put target rewritten.",
    )
    .await;

    socket.close(None).await.unwrap();
    server.abort();
}

async fn head_etag(app: &axum::Router, path: &str) -> String {
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
    response
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn server_restart_reseeds_sessions_from_last_checkpoint() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Before restart.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 15, " Checkpointed.");
    })
    .await;
    // Wait for the debounced checkpoint so the typing is canonical.
    wait_for_markdown_containing(&app, "live.md", "Checkpointed.").await;

    // "Restart": the process dies (sessions vanish with it); a new server
    // opens over the same store.
    server.abort();
    drop(socket);
    drop(doc);
    let restarted = router(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let new_addr = listener.local_addr().unwrap();
    let serve_app = restarted.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });

    // A reconnecting browser reseeds from rows: content equals the last
    // checkpoint.
    let (mut socket, doc) = connect_session(new_addr, &document_id).await;
    assert_eq!(yjs_plain_text(&doc), "Before restart. Checkpointed.");
    socket.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn session_review_transaction_renders_marks_and_meta_for_browsers() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Comment on this text.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    let (mut socket, doc) = connect_session(addr, &document_id).await;

    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 11, "end": 15,
                "body": "Live comment."
            }]),
        ),
    )
    .await;

    // The unmodified browser renders comments from text marks plus the
    // review meta map; the session-mode transaction produced both.
    let review = get_block_review(&app, "live.md", false).await;
    let comment_id = review["comments"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(review["comments"][0]["body"], "Live comment.");
    wait_for_yjs_comment_mark(&mut socket, &doc, &comment_id).await;
    let entry = review_entry_from_doc(&doc, "comments", &comment_id).unwrap();
    assert_eq!(entry["body"], "Live comment.");

    tokio::time::sleep(Duration::from_millis(2)).await;
    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-edit-live-comment",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": comment_id,
                "body": "Edited live comment."
            }]),
        ),
    )
    .await;
    let entry = wait_for_yjs_review_entry(&mut socket, &doc, "comments", &comment_id, |entry| {
        entry["body"] == "Edited live comment." && entry["editedAt"].as_str().is_some()
    })
    .await;
    assert_eq!(entry["body"], "Edited live comment.");
    assert_ne!(entry["editedAt"], entry["at"]);

    socket.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn browser_created_comment_checkpoints_into_review_rows() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Browser comments here.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    // The browser marks "comments" [8, 16) and writes the meta entry, the
    // same shape the Plate review plugins produce.
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.format(
            txn,
            8,
            8,
            [
                ("comment".into(), yrs::Any::Bool(true)),
                ("comment_c-browser".into(), yrs::Any::Bool(true)),
            ]
            .into_iter()
            .collect(),
        );
        let review = txn.get_or_insert_map(REVIEW_ROOT);
        let comments: yrs::MapRef = review.get_or_init(txn, "comments");
        comments.insert(
            txn,
            "c-browser",
            yrs::Any::from_json(
                r#"{"by":"Avery","at":"2026-06-09T00:00:00.000Z","body":"From the browser"}"#,
            )
            .unwrap(),
        );
    })
    .await;
    socket.close(None).await.unwrap();

    let _ = wait_for_markdown_containing(&app, "live.md", "Browser comments here.").await;
    let review = timeout(Duration::from_secs(5), async {
        loop {
            let review = get_block_review(&app, "live.md", false).await;
            if !review["comments"].as_array().unwrap().is_empty() {
                break review;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let comment = &review["comments"][0];
    assert_eq!(comment["id"], "c-browser");
    assert_eq!(comment["body"], "From the browser");
    assert_eq!(comment["by"], "Avery");
    assert_eq!(comment["anchor"]["startOffset"], 8);
    assert_eq!(comment["anchor"]["endOffset"], 16);
    assert_eq!(comment["quote"], "comments");
    server.abort();
}

#[tokio::test]
async fn browser_review_map_body_edit_checkpoints_into_review_rows() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Browser edits comments.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-comment",
            serde_json::json!([{
                "op": "comment.add",
                "block_id": block_id,
                "start": 8,
                "end": 13,
                "body": "Original browser-visible body"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "live.md", false).await;
    let comment = &review["comments"][0];
    let comment_id = comment["id"].as_str().unwrap().to_string();
    let created_at = comment["at"].as_str().unwrap().to_string();
    let edited_at = "2026-06-09T00:05:00.000Z";

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    wait_for_yjs_comment_mark(&mut socket, &doc, &comment_id).await;
    let meta_json = serde_json::json!({
        "by": "Agent One",
        "at": created_at,
        "body": "Edited from browser map",
        "editedAt": edited_at
    })
    .to_string();
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let review = txn.get_or_insert_map(REVIEW_ROOT);
        let comments: yrs::MapRef = review.get_or_init(txn, "comments");
        comments.insert(
            txn,
            comment_id.as_str(),
            yrs::Any::from_json(&meta_json).unwrap(),
        );
    })
    .await;
    socket.close(None).await.unwrap();

    let review = timeout(Duration::from_secs(5), async {
        loop {
            let review = get_block_review(&app, "live.md", false).await;
            let comment = &review["comments"][0];
            if comment["body"] == "Edited from browser map" {
                break review;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(review["comments"][0]["body"], "Edited from browser map");
    assert_eq!(review["comments"][0]["editedAt"], edited_at);
    server.abort();
}

#[tokio::test]
async fn raw_documents_refuse_sessions() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/raw.bin")
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(vec![0u8, 159, 146, 150]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let document_id = store.head_document("blocks", "raw.bin").await.unwrap().id;

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{document_id}"))
            .await
            .unwrap();
    // The server refuses the session and closes the socket.
    let next = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(matches!(
        next,
        None | Some(Ok(TungsteniteMessage::Close(_))) | Some(Err(_))
    ));
    server.abort();
}

// ---------------------------------------------------------------------------
// Phase 4: conflict review items (conflict.add, projection, resolution).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conflict_items_persist_project_and_resolve_without_mutating_the_document() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf.md", "Alpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "conf.md").await;
    let alpha_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();
    let markdown_before = get_document_markdown(&app, "conf.md").await;

    let ack = commit_block_transaction(
        &app,
        "conf.md",
        block_tx(
            "tx-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "after_block_id": alpha_id,
                "base_markdown": "Bravo, base.\n",
                "incoming_markdown": "Bravo, incoming edit.\n",
                "canonical_markdown": "Bravo.\n"
            }]),
        ),
    )
    .await;
    // The op never mutates the document: no changed blocks, content intact
    // (modulo the one-version commit).
    assert_eq!(ack["changed_block_ids"], serde_json::json!([]));
    assert_eq!(
        get_document_markdown(&app, "conf.md").await,
        markdown_before
    );

    let review = get_block_review(&app, "conf.md", false).await;
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["status"], "open");
    assert_eq!(
        conflicts[0]["afterBlockId"].as_str(),
        Some(alpha_id.as_str())
    );
    assert_eq!(conflicts[0]["baseMarkdown"], "Bravo, base.\n");
    assert_eq!(conflicts[0]["incomingMarkdown"], "Bravo, incoming edit.\n");
    assert_eq!(conflicts[0]["canonicalMarkdown"], "Bravo.\n");
    let conflict_id = conflicts[0]["id"].as_str().unwrap().to_string();

    // Conflicts resolve with the comment vocabulary; resolution never
    // mutates the document.
    commit_block_transaction(
        &app,
        "conf.md",
        block_tx(
            "tx-resolve-conflict",
            serde_json::json!([{ "op": "comment.resolve", "item_id": conflict_id }]),
        ),
    )
    .await;
    let open_review = get_block_review(&app, "conf.md", false).await;
    assert_eq!(open_review["conflicts"].as_array().unwrap().len(), 0);
    let full_review = get_block_review(&app, "conf.md", true).await;
    assert_eq!(full_review["conflicts"][0]["status"], "resolved");
    assert_eq!(
        get_document_markdown(&app, "conf.md").await,
        markdown_before
    );
}

#[tokio::test]
async fn comment_edit_on_conflict_id_returns_anchor_not_found() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-edit.md", "Alpha.\n").await;

    commit_block_transaction(
        &app,
        "conf-edit.md",
        block_tx(
            "tx-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "base_markdown": "Alpha.\n",
                "incoming_markdown": "Incoming.\n",
                "canonical_markdown": "Alpha.\n"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-edit.md", false).await;
    let conflict_id = review["conflicts"][0]["id"].as_str().unwrap().to_string();

    let (status, body) = post_block_transaction(
        &app,
        "conf-edit.md",
        block_tx(
            "tx-edit-conflict",
            serde_json::json!([{
                "op": "comment.edit",
                "item_id": conflict_id,
                "body": "not a comment"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "ANCHOR_NOT_FOUND", false);
}

#[tokio::test]
async fn document_start_conflicts_anchor_null_and_delete_dismisses_them() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-start.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "conf-start.md").await;

    commit_block_transaction(
        &app,
        "conf-start.md",
        block_tx(
            "tx-start-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "base_markdown": "Old heading.\n",
                "incoming_markdown": "New heading.\n",
                "canonical_markdown": ""
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-start.md", false).await;
    let conflict = &review["conflicts"][0];
    assert!(conflict["afterBlockId"].is_null());
    assert_eq!(conflict["canonicalMarkdown"], "");
    let conflict_id = conflict["id"].as_str().unwrap().to_string();

    // comment.delete removes the conflict row outright.
    commit_block_transaction(
        &app,
        "conf-start.md",
        block_tx(
            "tx-delete-conflict",
            serde_json::json!([{ "op": "comment.delete", "item_id": conflict_id }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-start.md", true).await;
    assert_eq!(review["conflicts"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn conflict_add_requires_an_existing_attachment_block() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-missing.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "conf-missing.md").await;

    let (status, body) = post_block_transaction(
        &app,
        "conf-missing.md",
        block_tx(
            "tx-bad-conflict",
            serde_json::json!([{
                "op": "conflict.add",
                "after_block_id": "no-such-block",
                "incoming_markdown": "Hunk.\n"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "BLOCK_DELETED", false);
}

// ---------------------------------------------------------------------------
// Phase 4: the reconciled Markdown PUT.
// ---------------------------------------------------------------------------

/// A stale-but-known `If-Match` is a BASE SELECTOR now, not a failing
/// precondition: the canonical edit and the external edit (computed against
/// the old version) both land, sibling block ids survive, and anchors
/// outside the changed hunks stay open.
#[tokio::test]
async fn markdown_put_merges_against_the_if_match_base_preserving_ids_and_anchors() {
    let (_root, app, _store) = block_test_app().await;
    // The separator keeps the two edited regions apart: edits to ADJACENT
    // blocks (no stable block between them) are conflict-absorbed by design.
    put_block_markdown(
        &app,
        "merge.md",
        "# Title\n\nAlpha.\n\nSeparator.\n\nBravo.\n",
    )
    .await;
    let tree = get_block_tree(&app, "merge.md").await;
    let base_clock = tree["document_clock"].as_str().unwrap().to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    let base_export = get_document_markdown(&app, "merge.md").await;

    // A live anchor on the Title block (untouched by either side).
    commit_block_transaction(
        &app,
        "merge.md",
        block_tx(
            "tx-anchor-title",
            serde_json::json!([{
                "op": "comment.add", "block_id": ids[0], "start": 0, "end": 5, "body": "keep me"
            }]),
        ),
    )
    .await;
    // Canonical edit to Alpha (a browser/agent write after the export).
    commit_block_transaction(
        &app,
        "merge.md",
        block_tx(
            "tx-canonical-alpha",
            serde_json::json!([{
                "op": "replace_block_content", "block_id": ids[1], "text": "Alpha, canonical."
            }]),
        ),
    )
    .await;

    // The external writer edits Bravo against the OLD export and PUTs with
    // the old clock.
    let incoming = base_export.replace("Bravo.", "Bravo, external.");
    assert_ne!(incoming, base_export);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/merge.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{base_clock}\""))
                .body(Body::from(incoming))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Both sides landed; nothing conflicted.
    let merged = get_document_markdown(&app, "merge.md").await;
    assert_eq!(
        merged,
        "# Title\n\nAlpha, canonical.\n\nSeparator.\n\nBravo, external.\n"
    );
    let tree = get_block_tree(&app, "merge.md").await;
    let merged_ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(merged_ids, ids, "sibling block ids survive the file write");
    let review = get_block_review(&app, "merge.md", false).await;
    assert_eq!(review["conflicts"].as_array().unwrap().len(), 0);
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(review["comments"][0]["anchor"]["blockId"], ids[0].as_str());
}

/// Overlapping edits (canonical and incoming both touched Bravo since the
/// base) never fail the write: the canonical side is retained and the losing
/// hunk surfaces as a conflict review item anchored after Alpha.
#[tokio::test]
async fn markdown_put_overlapping_edits_become_conflict_review_items() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "clash.md", "# Title\n\nAlpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "clash.md").await;
    let base_clock = tree["document_clock"].as_str().unwrap().to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    let base_export = get_document_markdown(&app, "clash.md").await;

    commit_block_transaction(
        &app,
        "clash.md",
        block_tx(
            "tx-canonical-bravo",
            serde_json::json!([{
                "op": "replace_block_content", "block_id": ids[2], "text": "Bravo, canonical."
            }]),
        ),
    )
    .await;

    let incoming = base_export.replace("Bravo.", "Bravo, external.");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/clash.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{base_clock}\""))
                .body(Body::from(incoming))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "conflicts never fail the write"
    );

    // Canonical side retained…
    assert_eq!(
        get_document_markdown(&app, "clash.md").await,
        "# Title\n\nAlpha.\n\nBravo, canonical.\n"
    );
    // …and the losing hunk rides in a conflict review item.
    let review = get_block_review(&app, "clash.md", false).await;
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["afterBlockId"].as_str(), Some(ids[1].as_str()));
    assert_eq!(conflicts[0]["incomingMarkdown"], "Bravo, external.\n");
    assert_eq!(conflicts[0]["baseMarkdown"], "Bravo.\n");
    assert_eq!(conflicts[0]["canonicalMarkdown"], "Bravo, canonical.\n");
}

#[tokio::test(flavor = "current_thread")]
async fn whole_file_writes_log_a_reconcile_outcome() {
    let (logs, _guard) = capture_debug_logs();
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "obs.md", "Alpha.\n").await;
    logs.clear();

    put_block_markdown(&app, "obs.md", "Alpha changed.\n").await;

    let output = logs.output();
    assert!(
        output.contains("document.block_write.reconciled"),
        "reconciled writes should log their outcome:\n{output}"
    );
    assert!(
        output.contains("result=merged"),
        "the outcome log should classify the merge:\n{output}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_reconciles_warn_about_lcs_degradation() {
    let (logs, _guard) = capture_debug_logs();
    let (_root, app, _store) = block_test_app().await;
    // 1030² changed-middle cells exceed the 2^20 LCS budget; every block
    // differs so prefix/suffix trimming cannot shrink the matrix.
    let base: String = (0..1030)
        .map(|index| format!("Base {index}.\n\n"))
        .collect();
    put_block_markdown(&app, "big.md", &base).await;
    logs.clear();

    let incoming: String = (0..1030)
        .map(|index| format!("Incoming {index}.\n\n"))
        .collect();
    put_block_markdown(&app, "big.md", &incoming).await;

    let output = logs.output();
    assert!(
        output.contains("document.block_write.lcs_degraded"),
        "degraded reconciles should warn:\n{output}"
    );
}

/// Half-resolved git merges: incoming content carrying `<<<<<<<` marker soup
/// still commits (writes never fail) but flags a conflict review item in the
/// same transaction.
const CONFLICT_MARKER_SOUP: &str =
    "<<<<<<< HEAD\nOurs line.\n=======\nTheirs line.\n>>>>>>> feature\n";

#[tokio::test]
async fn markdown_put_with_conflict_markers_flags_a_review_item() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "soup.md", "Alpha.\n").await;

    put_block_markdown(
        &app,
        "soup.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;

    let markdown = get_document_markdown(&app, "soup.md").await;
    assert_ne!(markdown, "Alpha.\n", "the soup write still committed");
    let review = get_block_review(&app, "soup.md", false).await;
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["status"], "open");
    assert_eq!(conflicts[0]["incomingMarkdown"], CONFLICT_MARKER_SOUP);
    assert!(conflicts[0]["afterBlockId"].is_null());
}

#[tokio::test]
async fn first_import_with_conflict_markers_flags_a_review_item() {
    let (_root, app, _store) = block_test_app().await;

    put_block_markdown(
        &app,
        "soup-new.md",
        &format!("# Notes\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;

    let review = get_block_review(&app, "soup-new.md", false).await;
    let conflicts = review["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["incomingMarkdown"], CONFLICT_MARKER_SOUP);
}

#[tokio::test]
async fn unchanged_conflict_markers_do_not_stack_flags() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "soup-again.md", "Alpha.\n").await;
    put_block_markdown(
        &app,
        "soup-again.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;

    put_block_markdown(
        &app,
        "soup-again.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}\nMore prose.\n"),
    )
    .await;

    let review = get_block_review(&app, "soup-again.md", true).await;
    assert_eq!(review["conflicts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn dismissed_conflict_marker_flags_stay_dismissed() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "soup-dismissed.md", "Alpha.\n").await;
    put_block_markdown(
        &app,
        "soup-dismissed.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}"),
    )
    .await;
    let review = get_block_review(&app, "soup-dismissed.md", false).await;
    let conflict_id = review["conflicts"][0]["id"].as_str().unwrap().to_string();
    commit_block_transaction(
        &app,
        "soup-dismissed.md",
        block_tx(
            "tx-dismiss-soup",
            serde_json::json!([{ "op": "comment.resolve", "item_id": conflict_id }]),
        ),
    )
    .await;

    put_block_markdown(
        &app,
        "soup-dismissed.md",
        &format!("Alpha.\n\n{CONFLICT_MARKER_SOUP}\nMore prose.\n"),
    )
    .await;

    let open_review = get_block_review(&app, "soup-dismissed.md", false).await;
    assert_eq!(open_review["conflicts"].as_array().unwrap().len(), 0);
    let full_review = get_block_review(&app, "soup-dismissed.md", true).await;
    assert_eq!(full_review["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(full_review["conflicts"][0]["status"], "resolved");
}

/// The agent-docs `insert_block` example must be a WORKING request: extract
/// the documented transaction body verbatim from the served docs, point its
/// `base_clock` at the real document, and commit it. Vocabulary drift between
/// the docs and the codec fails here.
#[tokio::test]
async fn agent_docs_insert_block_example_commits_as_documented() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "# Title\n\nAlpha.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let clock = tree["document_clock"].as_str().unwrap().to_string();

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
    let docs = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();

    // The example is the curl payload after "Insert a paragraph…": the JSON
    // between `-d '` and the closing `}'` (no single quotes inside JSON).
    let anchor = docs
        .find("Insert a paragraph after the current second block")
        .expect("docs keep the insert example");
    let body_start = docs[anchor..].find("-d '").expect("curl -d payload") + anchor + 4;
    let body_end = docs[body_start..].find("}'").expect("payload terminator") + body_start + 1;
    let documented = docs[body_start..body_end].replace("version_124", &clock);
    let payload: Value = serde_json::from_str(&documented)
        .unwrap_or_else(|error| panic!("documented example must be valid JSON: {error}"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/blocks/documents/doc.md/transactions",
            payload,
        ))
        .await
        .unwrap();
    let status = response.status();
    let ack = response_json(response).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "documented example must commit: {ack}"
    );
    assert_eq!(ack["status"], "committed");

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][2]["block_type"], "p");
    assert_eq!(after["blocks"][2]["text"], "A new paragraph.");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "# Title\n\nAlpha.\n\nA new paragraph.\n"
    );
}

/// Phase 7: a version restore on a BlockDocument is a whole-file write
/// through the reconciler (the two-way degenerate merge), not a legacy byte
/// put — the block projection survives (ids stable, anchors live) and the
/// content equals the restored version exactly.
#[tokio::test]
async fn version_restore_merges_through_the_gateway_preserving_ids_and_anchors() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "undo.md", "# Title\n\nAlpha.\n\nBravo.\n").await;
    let tree = get_block_tree(&app, "undo.md").await;
    let restore_to = tree["document_clock"].as_str().unwrap().to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    let original = get_document_markdown(&app, "undo.md").await;

    // A live anchor on the Title block and a later content edit to Alpha.
    commit_block_transaction(
        &app,
        "undo.md",
        block_tx(
            "tx-anchor-title",
            serde_json::json!([{
                "op": "comment.add", "block_id": ids[0], "start": 0, "end": 5, "body": "survive the restore"
            }]),
        ),
    )
    .await;
    commit_block_transaction(
        &app,
        "undo.md",
        block_tx(
            "tx-edit-alpha",
            serde_json::json!([{
                "op": "replace_block_content", "block_id": ids[1], "text": "Alpha, edited."
            }]),
        ),
    )
    .await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/blocks/documents/undo.md/versions/{restore_to}/restore"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // The content is the restored version, as a NEW head.
    assert_eq!(get_document_markdown(&app, "undo.md").await, original);
    let restored = get_block_tree(&app, "undo.md").await;
    assert_ne!(restored["document_clock"], serde_json::json!(restore_to));
    let restored_ids: Vec<String> = restored["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        restored_ids, ids,
        "the restore merges through the reconciler instead of clearing the projection"
    );
    let review = get_block_review(&app, "undo.md", false).await;
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(review["comments"][0]["anchor"]["blockId"], ids[0].as_str());
    assert_eq!(review["conflicts"], serde_json::json!([]));
}

/// A restore during a live session dispatches through the session mode
/// switch: the restored content lands in the live doc as a collaborator
/// edit, never by clearing the projection underneath the session.
#[tokio::test]
async fn version_restore_lands_in_a_live_session_as_a_collaborator_edit() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Stable block.\n\nOld text.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let restore_to = tree["document_clock"].as_str().unwrap().to_string();
    let edited = tree["blocks"][1]["block_id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-edit",
            serde_json::json!([{
                "op": "replace_block_content", "block_id": edited, "text": "New text."
            }]),
        ),
    )
    .await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/blocks/documents/live.md/versions/{restore_to}/restore"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Rows are durable at ack time and the live doc converged via the socket.
    let after = get_block_tree(&app, "live.md").await;
    assert_eq!(after["blocks"][1]["text"], "Old text.");
    assert_eq!(after["blocks"][1]["block_id"], edited.as_str());
    wait_for_yjs_plain_text(&mut socket, &doc, "Stable block.Old text.").await;

    socket.close(None).await.unwrap();
    server.abort();
}

/// A byte-identical PUT acks with the current head and commits nothing.
#[tokio::test]
async fn byte_identical_markdown_put_commits_no_new_version() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "noop.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "noop.md").await; // materialize + normalize
    let content = get_document_markdown(&app, "noop.md").await;
    let versions_before = raw_version_count(&app, "noop.md").await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/noop.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from(content.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let outcome = response_json(response).await;
    assert!(outcome["version"]["id"].is_string());
    assert_eq!(raw_version_count(&app, "noop.md").await, versions_before);
    assert_eq!(get_document_markdown(&app, "noop.md").await, content);
}

/// CriticMarkup is a content error on API import paths (it collides with the
/// review codec): the PUT fails typed, not silently as bytes.
#[tokio::test]
async fn markdown_put_with_critic_markup_fails_typed_unsupported() {
    let (_root, app, _store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/critic.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("Some {++inserted++} text.\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_typed_error(status, &body, "UNSUPPORTED_MARKDOWN", false);
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_put_replaces_materialized_blocks_and_preserves_ttl() {
    let (_root, app, store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "# Original\n\nOld body.\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();

    let before = get_tmp_block_tree(&app, &secret).await;
    assert_eq!(before["blocks"].as_array().unwrap().len(), 2);
    assert_eq!(before["blocks"][0]["text"], "Original");
    assert_eq!(before["blocks"][1]["text"], "Old body.");
    let clock = before["document_clock"].as_str().unwrap().to_string();
    let expires_before = store.head_tmp_document(&secret).await.unwrap().expires_at;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{clock}\""))
                .body(Body::from("# Uploaded\n\nNew body.\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let outcome = response_json(response).await;
    assert_eq!(
        etag,
        format!("\"{}\"", outcome["version"]["id"].as_str().unwrap())
    );
    assert_eq!(
        store.head_tmp_document(&secret).await.unwrap().expires_at,
        expires_before
    );

    let after = get_tmp_block_tree(&app, &secret).await;
    let blocks = after["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["text"], "Uploaded");
    assert_eq!(blocks[1]["text"], "New body.");

    let document = store.get_tmp_document(&secret).await.unwrap();
    assert_eq!(
        String::from_utf8(document.content).unwrap(),
        "# Uploaded\n\nNew body.\n"
    );
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_put_lands_in_an_active_session_as_a_collaborator_edit() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "Old first.\n\nOld second.\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();
    let tree = get_tmp_block_tree(&app, &secret).await;
    let clock = tree["document_clock"].as_str().unwrap().to_string();
    let document_id = store.head_tmp_document(&secret).await.unwrap().id;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    assert_eq!(yjs_plain_text(&doc), "Old first.Old second.");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{clock}\""))
                .body(Body::from("Uploaded first.\n\nUploaded second.\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let after = get_tmp_block_tree(&app, &secret).await;
    assert_eq!(after["blocks"][0]["text"], "Uploaded first.");
    assert_eq!(after["blocks"][1]["text"], "Uploaded second.");
    wait_for_yjs_plain_text(&mut socket, &doc, "Uploaded first.Uploaded second.").await;

    socket.close(None).await.unwrap();
    server.abort();
}

#[cfg(feature = "tmp-documents")]
#[tokio::test(flavor = "current_thread")]
async fn tmp_session_and_markdown_write_logs_do_not_emit_capability_secret() {
    let (logs, _guard) = capture_debug_logs();
    let (_root, addr, app, store, server) = spawn_session_server().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "Seeded first.\n\nSeeded second.\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();
    let tree = get_tmp_block_tree(&app, &secret).await;
    let clock = tree["document_clock"].as_str().unwrap().to_string();
    let document_id = store.head_tmp_document(&secret).await.unwrap().id;

    logs.clear();
    let (mut socket, doc) = connect_session(addr, &document_id).await;
    assert_eq!(yjs_plain_text(&doc), "Seeded first.Seeded second.");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{clock}\""))
                .body(Body::from("Uploaded first.\n\nUploaded second.\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let output = logs.output();
    assert!(
        !output.contains(&secret),
        "tmp session/write logs must not contain tmp secret:\n{output}"
    );
    assert!(
        output.contains("collab.session.seeded"),
        "session seed event should still be logged:\n{output}"
    );
    assert!(
        output.contains("document.block_write.started"),
        "tmp markdown write event should still be logged:\n{output}"
    );
    assert!(
        output.contains("scope=tmp") && output.contains(&document_id),
        "tmp logs should retain scope and document id diagnostics:\n{output}"
    );

    socket.close(None).await.unwrap();
    server.abort();
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_put_requires_markdown_content_type() {
    let (_root, app, store) = block_test_app().await;
    let secret = "0123456789abcdef0123456789abcdef";

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("# Draft\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["error"],
        "tmp writes require Content-Type: text/markdown"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("# Draft\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["error"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type application/x-www-form-urlencoded"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("# Draft\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["error"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type application/json"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "application/markdown; charset=utf-8")
                .header(header::IF_NONE_MATCH, "*")
                .header(
                    "x-quarry-metadata",
                    r#"{"content_type":"text/plain","title":"kept"}"#,
                )
                .body(Body::from("# Draft\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let document = store.get_tmp_document(secret).await.unwrap();
    assert_eq!(document.version.content_type, "application/markdown");
    assert_eq!(
        document.version.metadata,
        serde_json::json!({"content_type": "application/markdown", "title": "kept"})
    );
    let blocks = get_tmp_block_tree(&app, secret).await;
    assert_eq!(blocks["blocks"][0]["text"], "Draft");
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_create_and_put_reject_oversized_markdown() {
    let (_root, app, _store) = block_test_app().await;
    let oversized = "a".repeat(quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 1);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": oversized,
                "content_type": "text/markdown",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "# Draft\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let created = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();

    let oversized = "a".repeat(quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 1);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, etag)
                .body(Body::from(oversized))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["code"], "PAYLOAD_TOO_LARGE");
    assert_eq!(body["retryable"], false);
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_put_rejects_non_markdown_content_type() {
    let (_root, app, store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "# Draft\n\nBody.\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let created = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::IF_MATCH, etag.clone())
                .body(Body::from("raw body"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["error"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type text/plain"
    );

    let document = store.get_tmp_document(&secret).await.unwrap();
    assert_eq!(document.version.content_type, "text/markdown");
    assert_eq!(
        String::from_utf8(document.content).unwrap(),
        "# Draft\n\nBody.\n"
    );
    let blocks = get_tmp_block_tree(&app, &secret).await;
    assert_eq!(blocks["blocks"][0]["text"], "Draft");
    let latest_etag = format!(
        "\"{}\"",
        store
            .head_tmp_document(&secret)
            .await
            .unwrap()
            .head_version_id
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::IF_MATCH, latest_etag)
                .header("x-quarry-allow-document-kind-change", "true")
                .body(Body::from("raw body"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["error"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type text/plain"
    );
    let document = store.get_tmp_document(&secret).await.unwrap();
    assert_eq!(document.version.content_type, "text/markdown");
    assert_eq!(
        String::from_utf8(document.content).unwrap(),
        "# Draft\n\nBody.\n"
    );
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_create_rejects_non_markdown_content_type() {
    let (_root, app, _store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "draft one",
                "content_type": "text/plain",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["error"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type text/plain"
    );
}

/// RawDocuments keep the untouched byte path: bytes round-trip exactly and
/// no block tables are touched.
#[tokio::test]
async fn raw_document_put_bypasses_the_block_model_entirely() {
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
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let document = store.get_document("blocks", "data.bin").await.unwrap();
    assert_eq!(document.content, bytes);
    assert_eq!(
        store.load_block_tree(&document.id).await.unwrap(),
        Vec::<quarry_collab_codec::BlockRow>::new()
    );
}

// ---------------------------------------------------------------------------
// Phase 4 review fixes: metadata patches, session-concurrent file writes,
// conflict reply boundary.
// ---------------------------------------------------------------------------

/// A metadata patch is frontmatter-only: it must NOT destroy the block
/// projection. Rows, ids, review anchors, and conflict artifacts all survive;
/// only the rendered frontmatter (and the version clock) moves.
#[tokio::test]
async fn metadata_patch_preserves_rows_anchors_and_conflict_items() {
    let (_root, app, store) = block_test_app().await;
    put_block_markdown(&app, "meta.md", "# Title\n\nAlpha.\n").await;
    let tree = get_block_tree(&app, "meta.md").await;
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    commit_block_transaction(
        &app,
        "meta.md",
        block_tx(
            "tx-meta-anchor",
            serde_json::json!([
                {"op": "comment.add", "block_id": ids[1], "start": 0, "end": 5, "body": "keep"},
                {"op": "conflict.add", "after_block_id": ids[0],
                 "base_markdown": "Old.\n", "incoming_markdown": "New.\n",
                 "canonical_markdown": "Alpha.\n"}
            ]),
        ),
    )
    .await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/blocks/documents/meta.md/metadata",
            serde_json::json!({"title": "Patched Title", "rank": 7}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let outcome = response_json(response).await;
    assert_eq!(outcome["version"]["metadata"]["title"], "Patched Title");

    // The projection survived: same block ids, anchored comment still open,
    // conflict artifact intact.
    let tree = get_block_tree(&app, "meta.md").await;
    let ids_after: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids_after, ids);
    let review = get_block_review(&app, "meta.md", false).await;
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(review["comments"][0]["anchor"]["blockId"], ids[1].as_str());
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
    let document_id = store.head_document("blocks", "meta.md").await.unwrap().id;
    assert_eq!(store.load_block_tree(&document_id).await.unwrap().len(), 2);
}

/// A metadata patch composes with a live session: it waits on the document
/// mutex, flushes pending typing, and commits the typed rows under the new
/// metadata — typing and frontmatter both land, the session stays alive.
#[tokio::test]
async fn metadata_patch_composes_with_an_active_session() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "meta-live.md", "Session content.\n").await;
    let document_id = document_id_of(&store, "meta-live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 16, " Typed.");
    })
    .await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/blocks/documents/meta-live.md/metadata",
            serde_json::json!({"title": "Live Patch"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Both the in-flight typing and the new frontmatter are durable.
    let content = get_document_markdown(&app, "meta-live.md").await;
    assert!(content.contains("title: Live Patch"), "{content}");
    assert!(content.contains("Session content. Typed."), "{content}");

    // The session is still live: further typing checkpoints normally.
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 0, "Still here: ");
    })
    .await;
    let content = wait_for_markdown_containing(&app, "meta-live.md", "Still here:").await;
    assert!(content.contains("title: Live Patch"), "{content}");
    socket.close(None).await.ok();
    server.abort();
}

/// The reviewer's probe, pinned directly: in-flight (un-checkpointed) typing
/// plus a concurrent whole-file write through the live session. The file
/// write carries its base clock, so the merge is a true three-way: BOTH
/// edits survive — no last-writer-wins in either direction.
#[tokio::test]
async fn in_flight_typing_and_concurrent_file_write_both_survive_through_the_session() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    // The separator keeps the typed region and the file-edited region apart:
    // edits to ADJACENT blocks are conflict-absorbed by design (pinned by
    // the codec suite), which would mask the both-edits-survive assertion.
    put_block_markdown(
        &app,
        "race.md",
        "# Title\n\nAlpha.\n\nSeparator.\n\nBravo.\n",
    )
    .await;
    let tree = get_block_tree(&app, "race.md").await;
    let base_clock = tree["document_clock"].as_str().unwrap().to_string();
    let ids: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    let base_export = get_document_markdown(&app, "race.md").await;
    let document_id = document_id_of(&store, "race.md").await;

    // A browser types into Alpha; the debounce has not checkpointed yet.
    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 1);
        block.insert(txn, 6, " Typed mid-flight.");
    })
    .await;

    // An external writer edits Bravo against the pre-typing export.
    let incoming = base_export.replace("Bravo.", "Bravo, from the file write.");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/blocks/documents/race.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{base_clock}\""))
                .body(Body::from(incoming))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Both edits landed durably (the write flushed the typing, then merged).
    let content = get_document_markdown(&app, "race.md").await;
    assert_eq!(
        content,
        "# Title\n\nAlpha. Typed mid-flight.\n\nSeparator.\n\nBravo, from the file write.\n"
    );
    let tree = get_block_tree(&app, "race.md").await;
    let ids_after: Vec<String> = tree["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["block_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids_after, ids, "the merge preserved every block id");
    let review = get_block_review(&app, "race.md", false).await;
    assert_eq!(review["conflicts"].as_array().unwrap().len(), 0);
    socket.close(None).await.ok();
    server.abort();
}

/// Replies stay comment-only: `comment.reply` on a conflict item is
/// `ANCHOR_NOT_FOUND` (conflicts resolve/delete with the comment vocabulary
/// but cannot host threads).
#[tokio::test]
async fn comment_reply_on_a_conflict_item_is_anchor_not_found() {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "conf-reply.md", "Alpha.\n").await;
    let _ = get_block_tree(&app, "conf-reply.md").await;
    commit_block_transaction(
        &app,
        "conf-reply.md",
        block_tx(
            "tx-conflict-for-reply",
            serde_json::json!([{
                "op": "conflict.add",
                "incoming_markdown": "Hunk.\n"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "conf-reply.md", false).await;
    let conflict_id = review["conflicts"][0]["id"].as_str().unwrap().to_string();

    let (status, body) = post_block_transaction(
        &app,
        "conf-reply.md",
        block_tx(
            "tx-reply-to-conflict",
            serde_json::json!([{
                "op": "comment.reply", "item_id": conflict_id, "body": "no threads here"
            }]),
        ),
    )
    .await;
    assert_typed_error(status, &body, "ANCHOR_NOT_FOUND", false);
}
