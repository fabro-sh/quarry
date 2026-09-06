#![allow(clippy::unwrap_used, reason = "real socket regression fixtures")]
use crate::{AppState, app_state, router_with_state};
use axum::{body::Body, http::Request};
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "lib-documents")]
use quarry_core::{DocumentSource, WritePrecondition};
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
struct Server {
    _root: tempfile::TempDir,
    store: QuarryStore,
    state: AppState,
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = QuarryStore::open(StoreConfig {
            db_path: root.path().join("quarry.db"),
            cas_path: root.path().join("cas"),
            lock_path: None,
        })
        .await
        .unwrap();
        let state = app_state(store.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = router_with_state(state.clone());
        let stop = state.shutdown_token();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            _root: root,
            store,
            state,
            address,
            task,
        }
    }
    async fn connect(&self, path: &str) -> Socket {
        let mut request = format!("ws://{}{path}", self.address)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "origin",
            format!("http://{}", self.address).parse().unwrap(),
        );
        let (socket, response) = connect_async(request).await.unwrap();
        assert_eq!(response.status(), 101);
        socket
    }
    async fn refused(&self, path: &str, origin: &str, status: u16) {
        let mut request = format!("ws://{}{path}", self.address)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        let result = connect_async(request).await;
        match result {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), status)
            }
            _ => panic!("Expected a refused subscription"),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.shutdown_token().cancel();
        self.task.abort();
    }
}
async fn next_event(socket: &mut Socket, kind: &str) -> Value {
    timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Text(text) => {
                    let payload: Value = serde_json::from_str(&text).unwrap();
                    if payload["type"] == kind {
                        return payload;
                    }
                }
                Message::Ping(data) => socket.send(Message::Pong(data)).await.unwrap(),
                _ => panic!("Subscription closed before its event"),
            }
        }
    })
    .await
    .unwrap()
}
async fn closed(socket: &mut Socket) {
    timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Ping(data))) => {
                    let _ = socket.send(Message::Pong(data)).await;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
}

#[cfg(feature = "lib-documents")]
async fn put(store: &QuarryStore, library: &str, path: &str, text: &str) -> String {
    let precondition = store
        .head_document(library, path)
        .await
        .ok()
        .map(|document| WritePrecondition::IfMatch(document.head_version_id.to_string()))
        .unwrap_or(WritePrecondition::IfNoneMatch);
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.into(),
            path: path.into(),
            content: text.as_bytes().to_vec(),
            metadata: json!({}),
            content_type: "text/markdown".into(),
            source: DocumentSource::Rest,
            precondition,
            origin_id: None,
            transaction: Default::default(),
        })
        .await
        .unwrap()
        .document
        .id
        .to_string()
}

#[cfg(feature = "lib-documents")]
#[tokio::test]
async fn socket_events_retain_library_scope_document_identity_and_sse_compatibility() {
    let server = Server::new().await;
    server.store.create_library("first").await.unwrap();
    server.store.create_library("other").await.unwrap();
    let id = put(&server.store, "first", "doc.md", "Original").await;
    let route = format!("/v1/libraries/first/documents-by-id/{id}/events/stream");
    let mut document = server.connect(&route).await;
    let mut library = server.connect("/v1/events?library=first").await;
    let response = router_with_state(server.state.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/events?library=first")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut sse = response.into_body().into_data_stream();
    put(
        &server.store,
        "other",
        "doc.md",
        "Must not reach first library",
    )
    .await;
    put(&server.store, "first", "doc.md", "Changed").await;
    assert_eq!(next_event(&mut document, "doc.changed").await["doc_id"], id);
    assert_eq!(next_event(&mut library, "doc.changed").await["doc_id"], id);
    let first_event = timeout(Duration::from_secs(2), sse.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first_event).contains(&id));
    server
        .store
        .move_document("first", "doc.md", "moved.md", DocumentSource::Rest)
        .await
        .unwrap();
    assert_eq!(next_event(&mut document, "doc.moved").await["doc_id"], id);
    put(&server.store, "first", "moved.md", "Still attached").await;
    assert_eq!(next_event(&mut document, "doc.changed").await["doc_id"], id);
    server
        .refused(&route, "http://unrelated.example", 403)
        .await;
    server
        .refused(
            &format!("{route}?token=invalid"),
            &format!("http://{}", server.address),
            404,
        )
        .await;
    let invite = server
        .store
        .create_collab_invite_token("first", "moved.md", "viewer", None)
        .await
        .unwrap();
    let mut viewer = server
        .connect(&format!("{route}?token={}", invite.id))
        .await;
    let before = server
        .store
        .head_document("first", "moved.md")
        .await
        .unwrap()
        .head_version_id;
    viewer
        .send(Message::Text("{\"op\":\"delete_document\"}".into()))
        .await
        .unwrap();
    closed(&mut viewer).await;
    assert_eq!(
        server
            .store
            .head_document("first", "moved.md")
            .await
            .unwrap()
            .head_version_id,
        before
    );
    server.state.shutdown_token().cancel();
    closed(&mut document).await;
    closed(&mut library).await;
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_socket_events_check_capabilities_omit_secret_paths_and_stop_on_shutdown() {
    let server = Server::new().await;
    let tmp = server
        .store
        .create_tmp_document(
            b"Original".to_vec(),
            json!({}),
            "text/markdown",
            quarry_storage::TmpTtl::Default,
        )
        .await
        .unwrap();
    let path = tmp.document.path;
    let route = format!("/v1/tmp/documents/{path}/events/stream");
    let mut socket = server.connect(&route).await;
    let response = router_with_state(server.state.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/tmp/documents/{path}"))
                .header("content-type", "text/markdown")
                .header("if-match", format!("\"{}\"", tmp.document.head_version_id))
                .body(Body::from("Changed"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let event = next_event(&mut socket, "doc.changed").await;
    assert_eq!(event["doc_id"], tmp.document.id.to_string());
    assert!(event.get("path").is_none());
    assert!(!event.to_string().contains(&path));
    server.refused(&route, "null", 403).await;
    let missing = format!("/v1/tmp/documents/{}/events/stream", "a".repeat(path.len()));
    server
        .refused(&missing, &format!("http://{}", server.address), 404)
        .await;
    server.state.shutdown_token().cancel();
    closed(&mut socket).await;
}
