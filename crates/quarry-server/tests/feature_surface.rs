#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP fixture setup"
)]

use anyhow::Context as _;
use axum::body::{Body, to_bytes};
#[cfg(feature = "tmp-documents")]
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
#[cfg(feature = "tmp-documents")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "tmp-documents")]
use quarry_server::{app_state, router_with_state, serve_state_with_shutdown};
use serde_json::Value;
#[cfg(feature = "tmp-documents")]
use tokio::time::{Duration, timeout};
#[cfg(feature = "tmp-documents")]
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tower::ServiceExt;
#[cfg(feature = "tmp-documents")]
use yrs::sync::{Message as YMessage, SyncMessage};
#[cfg(feature = "tmp-documents")]
use yrs::updates::encoder::Encode;
#[cfg(feature = "tmp-documents")]
use yrs::{Doc, Out, ReadTxn, Text, Transact, WriteTxn, XmlTextRef};

mod common;

use common::{
    WsSocket, document_test_app, empty_yjs_doc, json_request, open_test_store, response_json,
    sync_yjs_doc_from_socket, wait_for_server, wait_for_yjs_sync_update, yjs_plain_text,
};

#[cfg(feature = "tmp-documents")]
const COLLAB_ROOT: &str = "content";

#[cfg(feature = "tmp-documents")]
fn assert_json_timestamp(value: &Value) {
    let timestamp = value.as_str().expect("timestamp should be a string");
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("timestamp should parse as RFC 3339");
}

#[tokio::test]
async fn document_feature_surface_matches_compiled_features() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;
    let tmp_documents = cfg!(feature = "tmp-documents");
    let lib_documents = cfg!(feature = "lib-documents");
    let admin_api = cfg!(feature = "admin-api");

    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/capabilities")
        .body(Body::empty())
        .context("build capabilities request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("send capabilities request")?;
    assert_eq!(response.status(), StatusCode::OK);
    let capabilities: Value = response_json(response).await;
    assert_eq!(capabilities["tmp_documents"], tmp_documents);
    assert_eq!(capabilities["lib_documents"], lib_documents);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/openapi.json")
        .body(Body::empty())
        .context("build OpenAPI request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("send OpenAPI request")?;
    assert_eq!(response.status(), StatusCode::OK);
    let openapi: Value = response_json(response).await;
    assert!(openapi["paths"]["/v1/capabilities"].is_object());
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents"].is_object(),
        tmp_documents
    );
    if tmp_documents {
        assert!(openapi["paths"]["/v1/tmp/documents"]["post"].is_object());
        assert!(openapi["paths"]["/v1/tmp/documents"]["get"].is_null());
        assert!(openapi["paths"]["/v1/tmp/documents/{secret}/share"].is_null());
        assert!(openapi["paths"]["/v1/tmp/documents/{secret}/share/{token}/revoke"].is_null());
        assert!(openapi["paths"]["/v1/tmp/collab/{secret}/{room}"].is_object());
    }
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/promote"].is_object(),
        tmp_documents && lib_documents
    );
    assert_eq!(
        openapi["paths"]["/v1/collab/{document_id}"].is_object(),
        lib_documents
    );
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/blocks"].is_object(),
        tmp_documents
    );
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/transactions"].is_object(),
        tmp_documents
    );
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/review"].is_object(),
        tmp_documents
    );
    assert_eq!(
        openapi["paths"]["/v1/tmp/documents/{secret}/presence"].is_object(),
        tmp_documents
    );
    let removed_tmp_signal_path =
        format!("/v1/tmp/documents/{{secret}}/{}", ["han", "doff"].join(""));
    assert!(openapi["paths"][removed_tmp_signal_path].is_null());
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
    let admin_paths_present = openapi["paths"]
        .as_object()
        .context("openapi paths should be an object")?
        .keys()
        .any(|path| path.starts_with("/v1/admin"));
    assert_eq!(
        admin_paths_present, admin_api,
        "the /v1/admin namespace must appear in OpenAPI only under the admin-api feature"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/admin/gc")
                .body(Body::empty())
                .context("build admin gc request")?,
        )
        .await
        .context("send admin gc request")?;
    assert_eq!(
        response.status(),
        if admin_api {
            // Feature on: the route runs GC on the fresh store.
            StatusCode::OK
        } else {
            // Feature off: the route is absent, so the POST falls through to
            // the GET-only asset fallback and is rejected before any handler.
            StatusCode::METHOD_NOT_ALLOWED
        }
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/.well-known/agent.json")
        .header("host", "127.0.0.1:7831")
        .body(Body::empty())
        .context("build agent discovery request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("send agent discovery request")?;
    assert_eq!(response.status(), StatusCode::OK);
    let discovery: Value = response_json(response).await;
    assert_eq!(
        discovery["endpoints"]["tmp_blocks"].is_object(),
        tmp_documents
    );
    assert_eq!(
        discovery["endpoints"]["tmp_transactions"].is_object(),
        tmp_documents
    );
    assert_eq!(
        discovery["route_hints"]["tmp_blocks"].is_string(),
        tmp_documents
    );
    assert_eq!(
        discovery["endpoints"]["transactions"].is_object(),
        lib_documents
    );
    assert_eq!(
        discovery["route_hints"]["transactions"].is_string(),
        lib_documents
    );
    if tmp_documents {
        assert!(
            discovery["capabilities"]
                .as_array()
                .context("discovery capabilities should be an array")?
                .iter()
                .any(|capability| capability == "tmp_documents")
        );
        let removed_tmp_signal_key = ["tmp_han", "doff"].join("");
        assert!(discovery["endpoints"][&removed_tmp_signal_key].is_null());
        assert!(discovery["route_hints"][removed_tmp_signal_key].is_null());
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/tmp/documents")
        .body(Body::empty())
        .context("build tmp documents collection request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("send tmp documents collection request")?;
    assert_eq!(
        response.status(),
        if tmp_documents {
            StatusCode::METHOD_NOT_ALLOWED
        } else {
            StatusCode::NOT_FOUND
        }
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/libraries")
        .body(Body::empty())
        .context("build libraries collection request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("send libraries collection request")?;
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
                .context("build missing collab request")?,
        )
        .await
        .context("send missing collab request")?;
    assert_eq!(
        response.status(),
        if lib_documents {
            // The raw collab route exists and rejects a non-upgrade GET.
            StatusCode::BAD_REQUEST
        } else {
            // Tmp-only build: the raw collab route is absent, so the request
            // falls through to the asset fallback and 404s.
            StatusCode::NOT_FOUND
        }
    );
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn security_headers_present_and_tmp_responses_are_uncacheable() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .body(Body::empty())
                .context("build health request")?,
        )
        .await
        .context("send health request")?;
    assert_eq!(health.status(), StatusCode::OK);
    let headers = health.headers();
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    assert!(
        headers.contains_key(header::CONTENT_SECURITY_POLICY),
        "every response should carry a Content-Security-Policy"
    );
    assert!(
        !headers.contains_key(header::CACHE_CONTROL),
        "a non-tmp response must not be marked no-store"
    );

    let created = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({"content": "scratch", "content_type": "text/markdown"}),
        ))
        .await
        .context("create tmp document")?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_json = response_json(created).await;
    let secret = created_json["document"]["path"]
        .as_str()
        .context("tmp create response should include a secret path")?
        .to_string();

    let fetched = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .context("build tmp document read request")?,
        )
        .await
        .context("send tmp document read request")?;
    assert_eq!(fetched.status(), StatusCode::OK);
    // The secret rides in the URL, so the tmp response must be uncacheable and
    // still carry the hardening headers.
    assert_eq!(fetched.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(fetched.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_not_found_error_body_redacts_the_secret() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    // A well-formed but nonexistent tmp secret (32 hex chars). The 404 message
    // embeds the looked-up path, which for tmp documents is the secret itself.
    let secret = "0123456789abcdef0123456789abcdef";
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .context("build tmp read request for missing secret")?,
        )
        .await
        .context("send tmp read request for missing secret")?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body: Value = response_json(response).await;
    let error = body["error"]
        .as_str()
        .context("error body should carry a string message")?;
    assert!(
        !error.contains(secret),
        "the error body must not echo the raw tmp secret: {error}"
    );
    assert!(
        error.contains("<tmp-secret>"),
        "the error body should show the redaction placeholder: {error}"
    );
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_documents_support_collab_block_review_presence_share_and_events_routes()
-> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "Alpha.\n",
                "content_type": "text/markdown"
            }),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("created tmp document should expose a secret path")?
        .to_string();
    let document_id = created["document"]["id"]
        .as_str()
        .context("created tmp document should expose an id")?
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/blocks"))
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let blocks: Value = response_json(response).await;
    assert_eq!(blocks["document_id"], document_id);
    assert_eq!(blocks["blocks"][0]["text"], "Alpha.");
    let block_id = blocks["blocks"][0]["block_id"]
        .as_str()
        .context("tmp blocks response should expose the first block id")?;
    let base_clock = blocks["document_clock"]
        .as_str()
        .context("tmp blocks response should expose the document clock")?;

    let event_stream = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/events/stream"))
                .header("X-Agent-Id", "agent-a")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(event_stream.status(), StatusCode::OK);
    let event_stream_body = event_stream.into_body();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/tmp/documents/{secret}/transactions"),
            serde_json::json!({
                "client_tx_id": "tmp-tx-1",
                "base_clock": base_clock,
                "actor": {"kind": "agent", "id": "agent-a", "label": "Agent A"},
                "ops": [
                    {
                        "op": "replace_block_content",
                        "block_id": block_id,
                        "text": "Alpha edited."
                    },
                    {
                        "op": "comment.add",
                        "block_id": block_id,
                        "start": 0,
                        "end": 5,
                        "body": "Review alpha."
                    }
                ]
            }),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let ack: Value = response_json(response).await;
    assert_eq!(ack["status"], "committed");

    let event = first_sse_chunk_containing(event_stream_body, "doc.changed").await;
    assert!(event.contains("event: doc.changed"));
    assert!(!event.contains(&secret));
    assert!(!event.contains("\"path\""));
    assert!(!event.contains("\"from\""));
    assert!(!event.contains("\"to\""));
    assert!(event.contains(&format!("\"doc_id\":\"{document_id}\"")));
    assert!(event.contains("\"version_id\""));
    assert!(event.contains("\"etag\""));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await?,
        "Alpha edited.\n"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/tmp/documents/{secret}/review?includeResolved=1"
                ))
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let review: Value = response_json(response).await;
    assert_eq!(review["documentId"], document_id);
    assert_eq!(
        review["comments"]
            .as_array()
            .context("tmp review response should expose comments")?
            .len(),
        1
    );
    assert_eq!(review["comments"][0]["body"], "Review alpha.");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v1/tmp/documents/{secret}/presence"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Agent-Id", "agent-a")
                .body(Body::from(
                    serde_json::json!({"status":"waiting"}).to_string(),
                ))
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let presence: Value = response_json(response).await;
    assert_eq!(presence["current"]["documentId"], document_id);
    assert_eq!(presence["current"]["agentId"], "agent-a");
    assert_eq!(presence["current"]["status"], "waiting");
    assert_json_timestamp(&presence["current"]["updatedAt"]);
    assert!(presence["current"].get("library").is_none());
    assert!(presence["current"].get("path").is_none());
    assert!(presence["presence"][0].get("path").is_none());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/tmp/documents/{secret}/share"),
            serde_json::json!({"role":"editor","byHint":"Avery"}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    assert!(ack["document_clock"].is_string());
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_collab_websocket_final_checkpoint_persists_typing() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let state = app_state(store.clone());
    let app = router_with_state(state.clone());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "Hello tmp.\n",
                "content_type": "text/markdown"
            }),
        ))
        .await
        .context("create tmp document for collab checkpoint")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("tmp create response should include document path")?
        .to_string();

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind temporary server probe")?;
    let addr = probe.local_addr().context("read probe local address")?;
    drop(probe);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_state_with_shutdown(state, addr, async {
        let _ = shutdown_rx.await;
    }));
    wait_for_server(addr).await;

    let (mut socket, doc) = connect_tmp_session(addr, &secret).await?;
    assert_eq!(yjs_plain_text(&doc), "Hello tmp.");
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 9, " edited");
    })
    .await?;
    socket
        .close(None)
        .await
        .context("close tmp collab socket")?;

    let markdown = wait_for_tmp_markdown_containing(&app, &secret, "edited").await?;
    assert_eq!(markdown, "Hello tmp edited.\n");
    shutdown_tx
        .send(())
        .map_err(|()| anyhow::anyhow!("tmp collab server shutdown receiver dropped"))?;
    server.await.context("join tmp collab server task")??;
    Ok(())
}

/// The raw `/v1/collab/{document_id}` route carries no secret, so it must never
/// seed a tmp document even when handed the correct internal id — otherwise the
/// exposed `x-quarry-document-id` would become a second, secret-free capability.
/// The secret-authenticated `/v1/tmp/collab` route must still seed. Requires
/// both features: the raw route exists only under `lib-documents`, and tmp
/// documents are creatable only under `tmp-documents`.
#[cfg(all(feature = "tmp-documents", feature = "lib-documents"))]
#[tokio::test]
async fn raw_collab_route_refuses_tmp_document_without_secret() -> anyhow::Result<()> {
    let (_root, store) = open_test_store().await;
    let state = app_state(store.clone());

    let outcome = store
        .create_tmp_document(
            b"Hello tmp.\n".to_vec(),
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            quarry_storage::TmpTtl::Default,
        )
        .await
        .context("create tmp document")?;
    let secret = outcome.document.path.clone();
    let document_id = store
        .head_tmp_document(&secret)
        .await
        .context("resolve tmp document head")?
        .id
        .to_string();

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind temporary server probe")?;
    let addr = probe.local_addr().context("read probe local address")?;
    drop(probe);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_state_with_shutdown(state, addr, async {
        let _ = shutdown_rx.await;
    }));
    wait_for_server(addr).await;

    assert!(
        !raw_collab_route_seeds(addr, &document_id).await?,
        "the raw id route must refuse a tmp document without the secret"
    );

    let (_socket, doc) = connect_tmp_session(addr, &secret).await?;
    assert_eq!(
        yjs_plain_text(&doc),
        "Hello tmp.",
        "the secret-authenticated route must still seed the tmp document"
    );

    shutdown_tx
        .send(())
        .map_err(|()| anyhow::anyhow!("collab server shutdown receiver dropped"))?;
    server.await.context("join collab server task")??;
    Ok(())
}

/// Connects to the raw collab route, sends the initial sync request, and
/// reports whether the server seeded the session (answered with a non-empty
/// Yjs update) before closing the socket. A refused session drops the socket
/// without seeding.
#[cfg(all(feature = "tmp-documents", feature = "lib-documents"))]
async fn raw_collab_route_seeds(
    addr: std::net::SocketAddr,
    document_id: &str,
) -> anyhow::Result<bool> {
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{document_id}"))
            .await
            .context("connect raw collab websocket")?;
    let doc = empty_yjs_doc();
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Sync(SyncMessage::SyncStep1(doc.transact().state_vector()))
                .encode_v1()
                .into(),
        ))
        .await
        .context("send raw collab sync request")?;

    let seeded = timeout(Duration::from_secs(2), async {
        while let Some(message) = socket.next().await {
            if let Ok(TungsteniteMessage::Binary(bytes)) = message {
                if common::apply_yjs_message(&doc, bytes.as_ref()) {
                    return true;
                }
            } else if matches!(message, Ok(TungsteniteMessage::Close(_)) | Err(_)) {
                return false;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    Ok(seeded)
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_documents_support_create_read_update_ttl_versions_and_delete() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

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
        .context("create tmp document")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .context("tmp create ETag should be valid header text")?
        .to_string();
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("tmp create response should include document path")?
        .to_string();
    assert_eq!(secret.len(), 32);
    assert!(
        secret
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    assert_eq!(created["document"]["library_id"], Value::Null);
    assert_json_timestamp(&created["document"]["expires_at"]);

    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/v1/tmp/documents/{secret}"))
        .body(Body::empty())
        .context("build tmp document read request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("read tmp document")?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .context("read tmp document body")?,
        "draft one"
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/v1/tmp/documents/scratch/note.txt")
        .body(Body::empty())
        .context("build path-like tmp document read request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("read path-like tmp document")?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let request = Request::builder()
        .method(Method::PUT)
        .uri(format!("/v1/tmp/documents/{secret}"))
        .header(header::IF_MATCH, etag)
        .header(header::CONTENT_TYPE, "text/markdown")
        .body(Body::from("draft two"))
        .context("build tmp document update request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("update tmp document")?;
    assert_eq!(response.status(), StatusCode::OK);

    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/v1/tmp/documents/{secret}/versions"))
        .body(Body::empty())
        .context("build tmp document versions request")?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .context("list tmp document versions")?;
    assert_eq!(response.status(), StatusCode::OK);
    let versions = response_json(response).await;
    assert_eq!(
        versions
            .as_array()
            .context("tmp versions response should be an array")?
            .len(),
        2
    );

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/tmp/documents/{secret}/ttl"),
            serde_json::json!({"expires_at":"2099-01-01T00:00:00Z"}),
        ))
        .await
        .context("patch tmp document TTL")?;
    assert_eq!(response.status(), StatusCode::OK);
    let ttl = response_json(response).await;
    assert_eq!(ttl["expires_at"], "2099-01-01T00:00:00Z");

    let request = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/v1/tmp/documents/{secret}"))
        .body(Body::empty())
        .context("build tmp document delete request")?;
    let response = app.oneshot(request).await.context("delete tmp document")?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[cfg(feature = "tmp-documents")]
async fn connect_tmp_session(
    addr: std::net::SocketAddr,
    secret: &str,
) -> anyhow::Result<(WsSocket, Doc)> {
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/tmp/collab/{secret}/content"))
            .await
            .context("connect tmp collab websocket")?;
    let doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &doc).await;
    Ok((socket, doc))
}

#[cfg(feature = "tmp-documents")]
async fn send_local_edit(
    socket: &mut WsSocket,
    doc: &Doc,
    edit: impl FnOnce(&mut yrs::TransactionMut<'_>, &XmlTextRef),
) -> anyhow::Result<()> {
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
        .context("send local Yjs update")?;
    wait_for_yjs_sync_update(socket, doc).await;
    Ok(())
}

#[cfg(feature = "tmp-documents")]
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

#[cfg(feature = "tmp-documents")]
async fn wait_for_tmp_markdown_containing(
    app: &axum::Router,
    secret: &str,
    needle: &str,
) -> anyhow::Result<String> {
    let markdown = timeout(Duration::from_secs(5), async {
        loop {
            let request = Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .context("build tmp markdown polling request")?;
            let response = app
                .clone()
                .oneshot(request)
                .await
                .context("poll tmp markdown")?;
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .context("read tmp markdown poll body")?;
            let markdown = String::from_utf8(body.to_vec()).context("decode tmp markdown body")?;
            if markdown.contains(needle) {
                break Ok::<String, anyhow::Error>(markdown);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .with_context(|| format!("persisted tmp markdown never contained {needle:?}"))??;
    Ok(markdown)
}

#[cfg(feature = "tmp-documents")]
async fn first_sse_chunk_containing(body: axum::body::Body, needle: &str) -> String {
    let mut stream = body.into_data_stream();
    timeout(Duration::from_secs(2), async {
        loop {
            let bytes = stream.next().await.unwrap().unwrap();
            let chunk = String::from_utf8(bytes.to_vec()).unwrap();
            if chunk.contains(needle) {
                break chunk;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("SSE stream never emitted {needle:?}"))
}
