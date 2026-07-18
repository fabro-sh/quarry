#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use futures_util::{SinkExt, Stream, StreamExt};
use quarry_collab_codec::Node;
use quarry_server::{app_state, router, router_with_state, serve_state_with_shutdown};
use quarry_storage::QuarryStore;
use serde_json::Value;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tower::ServiceExt;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, Map, Out, ReadTxn, Text, Transact, WriteTxn, XmlTextRef};

mod common;

use common::{
    WsSocket, apply_yjs_message, capture_debug_logs, document_test_app, empty_yjs_doc,
    json_request, open_test_store, response_json, sync_yjs_doc_from_socket, wait_for_server,
    wait_for_yjs_sync_update, yjs_plain_text, yjs_slate_children,
};

const COLLAB_ROOT: &str = "content";
const REVIEW_ROOT: &str = "review";

fn assert_json_timestamp(value: &Value) {
    let timestamp = value.as_str().expect("timestamp should be a string");
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("timestamp should parse as RFC 3339");
}

fn ops_request(base_token: impl serde::Serialize, operation: Value) -> Value {
    serde_json::json!({
        "baseToken": base_token,
        "operations": [operation]
    })
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
    if let Some(entry) = review_entry_from_doc(doc, section, id)
        && matches(&entry)
    {
        return entry;
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

async fn spawn_session_server() -> (
    tempfile::TempDir,
    std::net::SocketAddr,
    axum::Router,
    QuarryStore,
    tokio::task::JoinHandle<()>,
) {
    let (root, store) = open_test_store().await;
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
    let (root, store) = open_test_store().await;
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

async fn connect_session(addr: std::net::SocketAddr, document_id: &str) -> (WsSocket, Doc) {
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{document_id}"))
            .await
            .unwrap();
    let doc = empty_yjs_doc();
    sync_yjs_doc_from_socket(&mut socket, &doc).await;
    (socket, doc)
}

/// Connects to a tmp document over the secret-authenticated collab route. Tmp
/// documents are reachable only this way; the raw `/v1/collab/{id}` route
/// refuses them regardless of the internal id.
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

async fn send_awareness_name(socket: &mut WsSocket, doc: &Doc, name: &str) {
    let json = format!(r##"{{"data":{{"name":"{name}","color":"#8be9fd"}}}}"##);
    send_awareness_state(socket, doc, 1, &json).await;
}

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
    store
        .head_document("blocks", path)
        .await
        .unwrap()
        .id
        .to_string()
}

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
async fn legacy_edit_ops_and_review_process_endpoints_are_gone() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn session_seeds_from_rows_and_final_checkpoint_persists_typing() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn shutdown_closes_live_collab_socket_and_runs_final_checkpoint() -> anyhow::Result<()> {
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

    Ok(())
}

/// A restore during a live session dispatches through the session mode
/// switch: the restored content lands in the live doc as a collaborator
/// edit, never by clearing the projection underneath the session.
#[tokio::test]
async fn version_restore_lands_in_a_live_session_as_a_collaborator_edit() -> anyhow::Result<()> {
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

    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_put_lands_in_an_active_session_as_a_collaborator_edit() -> anyhow::Result<()>
{
    let (_root, addr, app, _store, server) = spawn_session_server().await;
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

    let (mut socket, doc) = connect_tmp_session(addr, &secret).await;
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

    Ok(())
}

/// A stored document the codec cannot seed (real CriticMarkup in plain text)
/// must refuse the session loudly: the socket closes with the application
/// close code and reason so the browser stops retrying and shows the error
/// instead of an endless "Reconnecting (read-only)".
#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn refused_session_closes_the_socket_with_a_typed_close_frame() -> anyhow::Result<()> {
    let (_root, addr, app, _store, server) = spawn_session_server().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "Edited {==this==}{>>why<<}{#c1} text.\n",
                "content_type": "text/markdown"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let secret = created["document"]["path"].as_str().unwrap().to_string();

    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/tmp/collab/{secret}/content"))
            .await
            .unwrap();
    let message = timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("refused session should close promptly")
        .expect("socket yields the close frame before ending")
        .unwrap();
    let TungsteniteMessage::Close(Some(frame)) = message else {
        panic!("expected a close frame with a reason, got {message:?}");
    };
    assert_eq!(u16::from(frame.code), 4400);
    assert_eq!(frame.reason.as_str(), "unsupported markdown: critic markup");

    server.abort();

    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test(flavor = "current_thread")]
async fn tmp_session_and_markdown_write_logs_do_not_emit_capability_secret() -> anyhow::Result<()> {
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
    let (mut socket, doc) = connect_tmp_session(addr, &secret).await;
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
        output.contains("scope=tmp") && output.contains(document_id.as_str()),
        "tmp logs should retain scope and document id diagnostics:\n{output}"
    );

    socket.close(None).await.unwrap();
    server.abort();

    Ok(())
}

/// A metadata patch composes with a live session: it waits on the document
/// mutex, flushes pending typing, and commits the typed rows under the new
/// metadata -- typing and frontmatter both land, the session stays alive.
#[tokio::test]
async fn metadata_patch_composes_with_an_active_session() -> anyhow::Result<()> {
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

    Ok(())
}

/// The reviewer's probe, pinned directly: in-flight (un-checkpointed) typing
/// plus a concurrent whole-file write through the live session. The file
/// write carries its base clock, so the merge is a true three-way: BOTH
/// edits survive -- no last-writer-wins in either direction.
#[tokio::test]
async fn in_flight_typing_and_concurrent_file_write_both_survive_through_the_session()
-> anyhow::Result<()> {
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

    Ok(())
}

/// A session-mode gateway transaction targeting the block a collaborator's
/// cursor sits in must SPLICE (only changed spans edited): the cursor's Yjs
/// item survives and a sticky index keeps resolving to the same character.
#[tokio::test]
async fn session_transaction_splices_so_cursors_in_the_edited_block_survive() -> anyhow::Result<()>
{
    use yrs::{Assoc, IndexedSequence};
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(
        &app,
        "live.md",
        "Cursor anchor paragraph for Blair. The agent will rewrite this tail shortly.\n",
    )
    .await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    // The browser's caret nudge: type one character, delete it again (the
    // e2e does exactly this to make Slate adopt a programmatic selection).
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 13, "x");
    })
    .await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.remove_range(txn, 13, 1);
    })
    .await;
    // The collaborator's caret right after "Cursor anchor" (offset 13).
    let cursor = {
        let mut txn = doc.transact_mut();
        let block = nth_block_text_in(&mut txn, 0);
        block
            .sticky_index(&txn, 13, Assoc::After)
            .expect("sticky index placed")
    };

    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-splice",
            serde_json::json!([{
                "op": "replace_block_content",
                "block_id": block_id,
                "text": "Cursor anchor paragraph for Blair. A freshly spliced tail took its place."
            }]),
        ),
    )
    .await;
    wait_for_yjs_plain_text(
        &mut socket,
        &doc,
        "Cursor anchor paragraph for Blair. A freshly spliced tail took its place.",
    )
    .await;

    let txn = doc.transact();
    let resolved = cursor
        .get_offset(&txn)
        .expect("sticky index resolves")
        .index;
    assert_eq!(
        resolved, 13,
        "the cursor's item must survive the same-block splice"
    );
    drop(txn);
    socket.close(None).await.unwrap();
    server.abort();

    Ok(())
}

/// Phase 5 checkpoint-ack protocol: a custom `MSG_QUARRY_CHECKPOINT` frame
/// carrying the committed doc snapshot is sent to each new subscriber on
/// join and broadcast after every durable commit (debounced checkpoint or
/// session-mode transaction). A client compares the acked snapshot against
/// its own doc — equality means "everything I see is canonical" (`Saved`).
#[tokio::test]
async fn checkpoint_commits_broadcast_snapshot_ack_frames() -> anyhow::Result<()> {
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

    Ok(())
}

/// A checkpoint that cannot project (here: a bare text node at block level,
/// a shape the session projection rejects) broadcasts a
/// `MSG_QUARRY_CHECKPOINT_FAILED` frame so still-connected browsers surface
/// "Save failed" instead of a benign "Saving…".
#[tokio::test]
async fn failing_checkpoints_broadcast_a_checkpoint_failed_frame() -> anyhow::Result<()> {
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

    Ok(())
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
async fn checkpoint_succeeds_despite_unknown_inline_marks() -> anyhow::Result<()> {
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

    Ok(())
}

/// The R1 probe: a KNOWN `code` mark spanning a link's inner text (the
/// editor's CodePlugin + LinkPlugin shape). Drop-containment does not apply
/// (`code` is renderable), so the writer must render the code span INSIDE
/// the link text instead of wedging every checkpoint with
/// "code mark on a non-text span".
#[tokio::test]
async fn checkpoint_succeeds_with_code_marks_inside_link_text() -> anyhow::Result<()> {
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

    Ok(())
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
async fn final_checkpoint_persists_typing_despite_unknown_inline_marks() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn multiple_typed_updates_coalesce_into_one_debounced_checkpoint() -> anyhow::Result<()> {
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

    Ok(())
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
async fn session_checkpoint_attributes_awareness_author() -> anyhow::Result<()> {
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

    Ok(())
}

/// The abrupt tab-close case: the client never sends an awareness removal,
/// so the author name survives into the final post-disconnect checkpoint
/// (directly in awareness, or via the session's cached label).
#[tokio::test]
async fn final_checkpoint_after_disconnect_attributes_author() -> anyhow::Result<()> {
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

    Ok(())
}

/// Forces the `live_actor` cache path: after the named participant withdraws
/// their awareness state, the next checkpoint observes a name-less awareness
/// and must fall back to the label cached by the first checkpoint.
#[tokio::test]
async fn checkpoint_after_awareness_removal_uses_cached_author() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn session_transaction_lands_in_live_doc_and_rows_before_ack() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn session_transaction_coalesces_unflushed_typing_first() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn transaction_racing_session_seed_is_never_rejected() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn transaction_racing_final_checkpoint_and_discard_succeeds() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn two_transactions_share_one_session() -> anyhow::Result<()> {
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

    Ok(())
}

/// Phase 5 deleted the PUT-as-checkpoint transitional rule: a Markdown PUT
/// carrying a `browser:*` origin on a session-active document is an
/// ordinary whole-file write through the Phase 4 reconciler — its body is
/// honored, it merges into the live doc as a collaborator edit, and the
/// session's own typing survives the merge.
#[tokio::test]
async fn browser_origin_markdown_put_is_an_ordinary_reconciled_write() -> anyhow::Result<()> {
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

    Ok(())
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
async fn server_restart_reseeds_sessions_from_last_checkpoint() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn session_review_transaction_renders_marks_and_meta_for_browsers() -> anyhow::Result<()> {
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
        entry["body"] == "Edited live comment."
            && entry["editedAt"]
                .as_str()
                .is_some_and(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).is_ok())
    })
    .await;
    assert_eq!(entry["body"], "Edited live comment.");
    assert_json_timestamp(&entry["editedAt"]);
    assert_ne!(entry["editedAt"], entry["at"]);

    socket.close(None).await.unwrap();
    server.abort();

    Ok(())
}

#[tokio::test]
async fn block_delete_suggestion_resolves_through_an_active_session() -> anyhow::Result<()> {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "# Remove me\n\nKeep me.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let heading_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    let (mut socket, _doc) = connect_session(addr, &document_id).await;
    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-suggest-block-delete",
            serde_json::json!([{
                "op": "suggestion.add_block_delete",
                "block_id": heading_id,
                "body": "Remove the obsolete heading."
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "live.md", false).await;
    assert_eq!(review["suggestions"][0]["kind"], "block_delete");
    assert_eq!(
        review["suggestions"][0]["body"],
        "Remove the obsolete heading."
    );
    let suggestion_id = review["suggestions"][0]["id"].as_str().unwrap().to_string();

    commit_block_transaction(
        &app,
        "live.md",
        block_tx(
            "tx-accept-block-delete",
            serde_json::json!([{
                "op": "suggestion.accept",
                "item_id": suggestion_id
            }]),
        ),
    )
    .await;

    assert_eq!(get_document_markdown(&app, "live.md").await, "Keep me.\n");
    let review = get_block_review(&app, "live.md", true).await;
    assert_eq!(review["suggestions"][0]["status"], "resolved");

    socket.close(None).await.unwrap();
    server.abort();

    Ok(())
}

#[tokio::test]
async fn browser_created_comment_checkpoints_into_review_rows() -> anyhow::Result<()> {
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

    Ok(())
}

/// Resolving a comment from the browser (a meta-map status flip; the text
/// mark stays) must keep the anchor in the committed rows so the NEXT
/// session still seeds the resolved comment's mark.
#[tokio::test]
async fn browser_resolved_comment_keeps_its_anchor_for_the_next_seed() -> anyhow::Result<()> {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Resolve keeps anchors.\n").await;
    let document_id = document_id_of(&store, "live.md").await;
    let tree = get_block_tree(&app, "live.md").await;
    let block_id = tree["blocks"][0]["block_id"].as_str().unwrap().to_string();

    // The agent comments through the gateway WHILE the session is live —
    // the op lands in the session doc as a collaborator edit (the e2e
    // agent-smoke sequence).
    let (mut socket, doc) = connect_session(addr, &document_id).await;
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
                "body": "Anchored while resolved"
            }]),
        ),
    )
    .await;
    let review = get_block_review(&app, "live.md", false).await;
    let comment = &review["comments"][0];
    let comment_id = comment["id"].as_str().unwrap().to_string();
    let created_at = comment["at"].as_str().unwrap().to_string();
    wait_for_yjs_comment_mark(&mut socket, &doc, &comment_id).await;
    // The browser resolve: the meta entry gains status=resolved, the text
    // mark is left in place (review-store.ts resolveComment).
    let meta_json = serde_json::json!({
        "by": "Avery",
        "at": created_at,
        "body": "Anchored while resolved",
        "status": "resolved"
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
            let review = get_block_review(&app, "live.md", true).await;
            let comment = &review["comments"][0];
            if comment["status"] == "resolved" {
                break review;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the checkpoint commits the resolution");
    let comment = &review["comments"][0];
    assert_eq!(
        (
            comment["anchor"]["startOffset"].as_u64(),
            comment["anchor"]["endOffset"].as_u64()
        ),
        (Some(8), Some(13)),
        "resolution must not collapse the anchor: {comment}"
    );

    // A fresh session seeds the resolved comment's mark again.
    let (_socket, doc) = connect_session(addr, &document_id).await;
    assert!(
        yjs_has_comment_mark(&doc, &comment_id),
        "the reseeded doc carries the resolved comment's mark"
    );
    server.abort();

    Ok(())
}

#[tokio::test]
async fn browser_review_map_body_edit_checkpoints_into_review_rows() -> anyhow::Result<()> {
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

    Ok(())
}

#[tokio::test]
async fn raw_documents_refuse_sessions() -> anyhow::Result<()> {
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

    Ok(())
}
