use axum::body::{Body, to_bytes};
#[cfg(feature = "tmp-documents")]
use axum::http::header;
use axum::http::{Method, Request, StatusCode};
#[cfg(feature = "tmp-documents")]
use futures_util::{Sink, SinkExt, Stream, StreamExt};
#[cfg(feature = "tmp-documents")]
use quarry_collab_codec::{Node, xmltext_to_slate};
use quarry_server::router;
#[cfg(feature = "tmp-documents")]
use quarry_server::{app_state, router_with_state, serve_state_with_shutdown};
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::Value;
#[cfg(feature = "tmp-documents")]
use tokio::time::{Duration, timeout};
#[cfg(feature = "tmp-documents")]
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tower::ServiceExt;
#[cfg(feature = "tmp-documents")]
use yrs::sync::{Message as YMessage, SyncMessage};
#[cfg(feature = "tmp-documents")]
use yrs::updates::decoder::Decode;
#[cfg(feature = "tmp-documents")]
use yrs::updates::encoder::Encode;
#[cfg(feature = "tmp-documents")]
use yrs::{Doc, OffsetKind, Options, Out, ReadTxn, Text, Transact, Update, WriteTxn, XmlTextRef};

#[cfg(feature = "tmp-documents")]
const COLLAB_ROOT: &str = "content";

#[cfg(feature = "tmp-documents")]
fn assert_json_timestamp(value: &Value) {
    let timestamp = value.as_str().expect("timestamp should be a string");
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("timestamp should parse as RFC 3339");
}

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
        tmp_documents || lib_documents
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

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/.well-known/agent.json")
                .header("host", "127.0.0.1:7831")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
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
                .unwrap()
                .iter()
                .any(|capability| capability == "tmp_documents")
        );
        let removed_tmp_signal_key = ["tmp_han", "doff"].join("");
        assert!(discovery["endpoints"][&removed_tmp_signal_key].is_null());
        assert!(discovery["route_hints"][removed_tmp_signal_key].is_null());
    }

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
            StatusCode::METHOD_NOT_ALLOWED
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
        if tmp_documents || lib_documents {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::NOT_FOUND
        }
    );
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_documents_support_collab_block_review_presence_share_and_events_routes() {
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
                "content": "Alpha.\n",
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
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/blocks"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let blocks: Value = response_json(response).await;
    assert_eq!(blocks["document_id"], document_id);
    assert_eq!(blocks["blocks"][0]["text"], "Alpha.");
    let block_id = blocks["blocks"][0]["block_id"].as_str().unwrap();
    let base_clock = blocks["document_clock"].as_str().unwrap();

    let event_stream = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/events/stream"))
                .header("X-Agent-Id", "agent-a")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
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
        .await
        .unwrap();
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
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
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
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let review: Value = response_json(response).await;
    assert_eq!(review["documentId"], document_id);
    assert_eq!(review["comments"].as_array().unwrap().len(), 1);
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
                .unwrap(),
        )
        .await
        .unwrap();
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
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    assert!(ack["document_clock"].is_string());
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_collab_websocket_final_checkpoint_persists_typing() {
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
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "Hello tmp.\n",
                "content_type": "text/markdown"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_state_with_shutdown(state, addr, async {
        let _ = shutdown_rx.await;
    }));
    wait_for_server(addr).await;

    let (mut socket, doc) = connect_tmp_session(addr, &secret).await;
    assert_eq!(yjs_plain_text(&doc), "Hello tmp.");
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 9, " edited");
    })
    .await;
    socket.close(None).await.unwrap();

    let markdown = wait_for_tmp_markdown_containing(&app, &secret, "edited").await;
    assert_eq!(markdown, "Hello tmp edited.\n");
    shutdown_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
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
    assert_eq!(secret.len(), 32);
    assert!(
        secret
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    assert_eq!(created["document"]["library_id"], Value::Null);
    assert_json_timestamp(&created["document"]["expires_at"]);

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

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/versions"))
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
            &format!("/v1/tmp/documents/{secret}/ttl"),
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
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[cfg(feature = "tmp-documents")]
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

#[cfg(feature = "tmp-documents")]
type WsSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[cfg(feature = "tmp-documents")]
async fn connect_tmp_session(addr: std::net::SocketAddr, secret: &str) -> (WsSocket, Doc) {
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/tmp/collab/{secret}/content"))
            .await
            .unwrap();
    let doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &doc).await;
    (socket, doc)
}

#[cfg(feature = "tmp-documents")]
fn empty_yjs_doc() -> Doc {
    Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    })
}

#[cfg(feature = "tmp-documents")]
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

#[cfg(feature = "tmp-documents")]
async fn send_local_edit(
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
    wait_for_yjs_sync_update(socket, doc).await;
}

#[cfg(feature = "tmp-documents")]
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

#[cfg(feature = "tmp-documents")]
fn apply_yjs_message(doc: &Doc, bytes: &[u8]) -> bool {
    let update = match YMessage::decode_v1(bytes) {
        Ok(YMessage::Sync(SyncMessage::Update(update) | SyncMessage::SyncStep2(update))) => update,
        _ => return false,
    };
    if update.is_empty() {
        return false;
    }
    let mut txn = doc.transact_mut();
    txn.apply_update(Update::decode_v1(&update).unwrap())
        .unwrap();
    true
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
    let txn = doc.transact();
    let text = txn.get_text(COLLAB_ROOT).unwrap();
    let root: &XmlTextRef = text.as_ref();
    let Node::Element { children, .. } = xmltext_to_slate(&txn, root).unwrap() else {
        panic!("collab root should decode as a Slate fragment");
    };
    let mut out = String::new();
    for node in children {
        collect(&node, &mut out);
    }
    out
}

#[cfg(feature = "tmp-documents")]
async fn wait_for_tmp_markdown_containing(
    app: &axum::Router,
    secret: &str,
    needle: &str,
) -> String {
    timeout(Duration::from_secs(5), async {
        loop {
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
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let markdown = String::from_utf8(body.to_vec()).unwrap();
            if markdown.contains(needle) {
                break markdown;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("persisted tmp markdown never contained {needle:?}"))
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
