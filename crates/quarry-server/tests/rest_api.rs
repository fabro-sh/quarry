#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use futures_util::SinkExt;
use quarry_server::router;
use quarry_storage::QuarryStore;
use serde_json::Value;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tower::ServiceExt;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::encoder::Encode;
use yrs::{Doc, Out, ReadTxn, Text, Transact, WriteTxn, XmlTextRef};

mod common;

use common::{
    WsSocket, capture_debug_logs, empty_yjs_doc, json_request, open_test_store, response_json,
    sync_yjs_doc_from_socket, wait_for_yjs_sync_update, yjs_plain_text,
};

const COLLAB_ROOT: &str = "content";

// ---------------------------------------------------------------------------
// Phase 2: semantic mutation gateway (rows-authoritative mode) + block API
// ---------------------------------------------------------------------------

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
/// Withdraws this client's awareness state: the y-protocol `null` entry a
/// client publishes on clean departure (clock bumped past the set above).
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

// ---------------------------------------------------------------------------
// Phase 4 review fixes: metadata patches, session-concurrent file writes,
// conflict reply boundary.
// ---------------------------------------------------------------------------

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
