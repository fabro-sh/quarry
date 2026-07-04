#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use futures_util::{SinkExt, Stream, StreamExt};
use quarry_collab_codec::Node;
use quarry_core::DocumentSource;
use quarry_server::{app_state, router, router_with_state, serve_state_with_shutdown};
use quarry_storage::{QuarryStore, StoreEvent, StoreEventKind};
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

fn assert_json_uuid(value: &Value) {
    let id = value.as_str().expect("id should be a string");
    uuid::Uuid::parse_str(id).expect("id should parse as a UUID");
}

fn ops_request(base_token: impl serde::Serialize, operation: Value) -> Value {
    serde_json::json!({
        "baseToken": base_token,
        "operations": [operation]
    })
}

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

async fn next_document_put_event(
    events: &mut tokio::sync::broadcast::Receiver<StoreEvent>,
) -> StoreEvent {
    timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind() == StoreEventKind::DocumentPut {
                break event;
            }
        }
    })
    .await
    .unwrap()
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
    assert_json_uuid(&ack["transaction_id"]);
    let clock = ack["document_clock"].as_str().unwrap();
    assert_ne!(clock, tree["document_clock"].as_str().unwrap());

    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "First.\n\nSecond.\n"
    );
    assert_eq!(raw_version_count(&app, "doc.md").await, versions_before + 1);

    let event = next_document_put_event(&mut events).await;
    assert_eq!(event.version_id(), Some(clock));
    assert_eq!(event.path(), Some("doc.md"));
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
    assert!(
        review["suggestions"][0]["replies"]
            .as_array()
            .unwrap()
            .is_empty()
    );

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
    assert!(
        review["suggestions"][0]["replies"]
            .as_array()
            .unwrap()
            .is_empty()
    );

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

/// A session-mode gateway transaction targeting the block a collaborator's
/// cursor sits in must SPLICE (only changed spans edited): the cursor's Yjs
/// item survives and a sticky index keeps resolving to the same character.
#[tokio::test]
async fn session_transaction_splices_so_cursors_in_the_edited_block_survive() {
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

/// Resolving a comment from the browser (a meta-map status flip; the text
/// mark stays) must keep the anchor in the committed rows so the NEXT
/// session still seeds the resolved comment's mark.
#[tokio::test]
async fn browser_resolved_comment_keeps_its_anchor_for_the_next_seed() {
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
