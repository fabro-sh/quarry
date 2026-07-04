#![allow(
    dead_code,
    reason = "shared integration-test helpers are used per test target"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, header};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use quarry_collab_codec::{Node, xmltext_to_slate};
use quarry_server::router;
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::Value;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, OffsetKind, ReadTxn, Transact, Update, XmlTextRef};

const COLLAB_ROOT: &str = "content";

pub(crate) type WsSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

pub(crate) async fn wait_for_server(addr: std::net::SocketAddr) {
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

pub(crate) fn empty_yjs_doc() -> Doc {
    Doc::with_options(yrs::Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    })
}

pub(crate) async fn sync_yjs_doc_from_socket<S>(socket: &mut S, doc: &Doc)
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

pub(crate) async fn wait_for_yjs_sync_update<S>(socket: &mut S, doc: &Doc)
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

pub(crate) fn apply_yjs_message(doc: &Doc, bytes: &[u8]) -> bool {
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

pub(crate) fn yjs_slate_children(doc: &Doc) -> Vec<Node> {
    let txn = doc.transact();
    let text = txn.get_text(COLLAB_ROOT).unwrap();
    let root: &XmlTextRef = text.as_ref();
    let Node::Element { children, .. } = xmltext_to_slate(&txn, root).unwrap() else {
        panic!("collab root should decode as a Slate fragment");
    };
    children
}

pub(crate) fn yjs_plain_text(doc: &Doc) -> String {
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
