#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use futures_util::{SinkExt, Stream, StreamExt};
use quarry_core::DocumentSource;
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
    WsSocket, apply_yjs_message, capture_debug_logs, document_test_app, empty_yjs_doc,
    json_request, open_test_store, response_json, sync_yjs_doc_from_socket,
    wait_for_yjs_sync_update, yjs_plain_text,
};

const COLLAB_ROOT: &str = "content";

async fn presence_test_app(library: &str) -> (tempfile::TempDir, axum::Router) {
    let (root, store) = open_test_store().await;
    let library = store.create_library(library).await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("live.md").to_string(),
            content: b"hello".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: quarry_core::WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    (root, router(store))
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

// ---------------------------------------------------------------------------
// Phase 2: semantic mutation gateway (rows-authoritative mode) + block API
// ---------------------------------------------------------------------------

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
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Markdown block document")
    );

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
