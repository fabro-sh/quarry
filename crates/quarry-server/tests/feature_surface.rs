use axum::body::{to_bytes, Body};
#[cfg(feature = "tmp-documents")]
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
use quarry_server::router;
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn document_feature_surface_matches_compiled_features() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);
    let tmp_documents = cfg!(feature = "tmp-documents");
    let lib_documents = cfg!(feature = "lib-documents");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/capabilities")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let capabilities: Value = response_json(response).await;
    assert_eq!(capabilities["tmp_documents"], tmp_documents);
    assert_eq!(capabilities["lib_documents"], lib_documents);

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
    assert!(openapi["paths"]["/v1/capabilities"].is_object());
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents"].is_object(),
        tmp_documents
    );
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{path}/promote"].is_object(),
        tmp_documents && lib_documents
    );
    assert_eq!(openapi["paths"]["/v1/libraries"].is_object(), lib_documents);
    assert_eq!(openapi["paths"]["/v1/events"].is_object(), lib_documents);
    assert_eq!(
        openapi["paths"]["/v1/libraries/{library}/git/peers"].is_object(),
        lib_documents
    );
    assert_eq!(
        openapi["paths"]["/v1/libraries/{library}/conflicts"].is_object(),
        lib_documents
    );

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
    assert_eq!(
        response.status(),
        if tmp_documents {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        }
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        if lib_documents {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        }
    );

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/collab/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        if lib_documents {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::NOT_FOUND
        }
    );
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_documents_support_create_read_update_ttl_versions_and_delete() {
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
                "path": "scratch/note.txt",
                "content": "draft one",
                "content_type": "text/plain",
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
    assert_eq!(created["document"]["library_id"], Value::Null);
    assert!(created["document"]["expires_at"].as_str().is_some());

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
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "draft one"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/tmp/documents/scratch/note.txt")
                .header(header::IF_MATCH, etag)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("draft two"))
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
                .uri("/v1/tmp/documents/scratch/note.txt/versions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let versions = response_json(response).await;
    assert_eq!(versions.as_array().unwrap().len(), 2);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/tmp/documents/scratch/note.txt/ttl",
            serde_json::json!({"expires_at":"2099-01-01T00:00:00Z"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ttl = response_json(response).await;
    assert_eq!(ttl["expires_at"], "2099-01-01T00:00:00Z");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/v1/tmp/documents/scratch/note.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[cfg(feature = "tmp-documents")]
fn json_request(method: Method, uri: &str, value: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
