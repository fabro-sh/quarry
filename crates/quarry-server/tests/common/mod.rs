use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, header};
use quarry_server::router;
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::Value;

pub(crate) async fn open_test_store() -> (tempfile::TempDir, QuarryStore) {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    (root, store)
}

pub(crate) async fn document_test_app() -> (tempfile::TempDir, axum::Router, QuarryStore) {
    let (root, store) = open_test_store().await;
    let app = router(store.clone());
    (root, app, store)
}

pub(crate) fn json_request(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub(crate) async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}
