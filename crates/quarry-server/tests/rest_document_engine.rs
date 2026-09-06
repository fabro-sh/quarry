#![cfg(any(feature = "tmp-documents", feature = "lib-documents"))]
#![allow(
    clippy::unwrap_used,
    reason = "tests use explicit HTTP and native document fixtures"
)]
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use quarry_document::{Command, CommandRequest, Document};
use serde_json::{Value, json};
use tower::ServiceExt;
mod common;
use common::{document_test_app, json_request, response_json};

async fn get(app: &axum::Router, url: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}
async fn post(app: &axum::Router, url: &str, payload: Value, status: StatusCode) -> Value {
    let id = payload["request_id"].clone();
    let response = app
        .clone()
        .oneshot(json_request(Method::POST, url, payload))
        .await
        .unwrap();
    let actual = response.status();
    let body = response_json(response).await;
    assert_eq!(
        actual,
        status,
        "action={} request={id} {body}",
        url.rsplit('/').next().unwrap()
    );
    body
}
fn request(doc: &Document, id: &str, commands: Vec<Command>) -> CommandRequest {
    CommandRequest {
        request_id: id.into(),
        base: doc.heads().iter().map(ToString::to_string).collect(),
        commands,
        at: "2026-09-04T12:00:00Z".into(),
    }
}
fn batch(id: &str, request: &CommandRequest) -> Value {
    json!({"request_id":id,"actor":{"kind":"browser","id":"browser:test","label":"Reviewer"},"requests":[request]})
}
fn native(state: &Value) -> Document {
    let bytes: Vec<u8> = serde_json::from_value(state["bytes"].clone()).unwrap();
    Document::load(&bytes).unwrap()
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn markdown_reconciliation_uses_current_offsets_and_preserves_native_review() {
    let (_root, app, _store) = document_test_app().await;
    let created = post(
        &app,
        "/v1/tmp/documents",
        json!({"content":"See [TARGET](https://example.com) here.\n"}),
        StatusCode::CREATED,
    )
    .await;
    let url = format!(
        "/v1/tmp/documents/{}",
        created["document"]["path"].as_str().unwrap()
    );
    let initial = get(&app, &format!("{url}/document-state")).await;
    let base = native(&initial);
    let block = base.blocks().unwrap()[0].id.clone();
    let add = request(
        &base,
        "comment",
        vec![Command::AddComment {
            id: "comment".into(),
            author: "Reviewer".into(),
            body: "Keep this".into(),
            ranges: base.selection(&block, 4, 10).unwrap(),
        }],
    );
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("comment", &add),
        StatusCode::OK,
    )
    .await;
    let current = native(&get(&app, &format!("{url}/document-state")).await);
    let typing = request(
        &current,
        "typing",
        vec![Command::InsertText {
            at: current.point(&block, 0).unwrap(),
            text: "Browser: ".into(),
        }],
    );
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("typing", &typing),
        StatusCode::OK,
    )
    .await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(&url)
                .header("content-type", "text/markdown")
                .header(
                    "x-quarry-merge-base",
                    format!("\"{}\"", initial["document_clock"].as_str().unwrap()),
                )
                .body(Body::from(
                    "See [TARGET](https://example.com) here. Agent suffix.\n",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let payload = response_json(response).await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    let result = native(&get(&app, &format!("{url}/document-state")).await);
    // The existing Markdown diff3 treats edits on the same line as a conflict.
    // Its native adapter must preserve the browser text and retain the proposal.
    assert_eq!(
        result.block_view(&block).unwrap().text,
        "Browser: See TARGET here."
    );
    assert_eq!(result.conflicts().unwrap().len(), 1);
    assert!(
        result.conflicts().unwrap()[0]
            .incoming
            .contains("Agent suffix.")
    );
    assert_eq!(
        result.comment_target("comment").unwrap().attachments[0].quote,
        "TARGET"
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(&url)
                .header("content-type", "text/markdown")
                .header(
                    "if-match",
                    common::markdown_precondition(&app, &url).await.1,
                )
                .body(Body::from(
                    "Browser: See [TARGET](https://example.com) here. Agent suffix.\n",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = native(&get(&app, &format!("{url}/document-state")).await);
    assert_eq!(
        result.block_view(&block).unwrap().text,
        "Browser: See TARGET here. Agent suffix."
    );
    assert_eq!(
        result.comment_target("comment").unwrap().attachments[0].quote,
        "TARGET"
    );
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn native_fork_preserves_review_identity_and_isolates_future_writes() {
    let (_root, app, store) = document_test_app().await;
    let created = post(
        &app,
        "/v1/tmp/documents",
        json!({"content":"TARGET\n"}),
        StatusCode::CREATED,
    )
    .await;
    let path = created["document"]["path"].as_str().unwrap();
    let url = format!("/v1/tmp/documents/{path}");
    let initial = get(&app, &format!("{url}/document-state")).await;
    let base = native(&initial);
    let block = base.blocks().unwrap()[0].id.clone();
    let add = request(
        &base,
        "review",
        vec![Command::AddComment {
            id: "thread".into(),
            author: "Reviewer".into(),
            body: "Keep".into(),
            ranges: base.selection(&block, 0, 6).unwrap(),
        }],
    );
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("review", &add),
        StatusCode::OK,
    )
    .await;
    let source = native(&get(&app, &format!("{url}/document-state")).await);
    let target = store.fork_tmp_document(path).await.unwrap();
    let target_url = format!("/v1/tmp/documents/{}", target.path);
    let mut copy = native(&get(&app, &format!("{target_url}/document-state")).await);
    assert_ne!(copy.id().unwrap(), source.id().unwrap());
    assert_eq!(
        copy.comment_target("thread").unwrap(),
        source.comment_target("thread").unwrap()
    );
    assert!(copy.merge(&source).is_err());
    let edit = request(
        &copy,
        "edit-copy",
        vec![Command::InsertText {
            at: copy.point(&block, 3).unwrap(),
            text: "!".into(),
        }],
    );
    post(
        &app,
        &format!("{target_url}/document-commands"),
        batch("edit-copy", &edit),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        native(&get(&app, &format!("{url}/document-state")).await)
            .block_view(&block)
            .unwrap()
            .text,
        "TARGET"
    );
    assert_eq!(
        native(&get(&app, &format!("{target_url}/document-state")).await)
            .comment_target("thread")
            .unwrap()
            .attachments[0]
            .quote,
        "TAR!GET"
    );
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn proposals_accept_once_and_keep_late_comments_and_replies() {
    let (_root, app, _store) = document_test_app().await;
    let created = post(
        &app,
        "/v1/tmp/documents",
        json!({"content":"TARGET\n"}),
        StatusCode::CREATED,
    )
    .await;
    let url = format!(
        "/v1/tmp/documents/{}",
        created["document"]["path"].as_str().unwrap()
    );
    let initial = get(&app, &format!("{url}/document-state")).await;
    let base = native(&initial);
    let block = base.blocks().unwrap()[0].id.clone();
    let propose = request(
        &base,
        "propose",
        vec![Command::ProposeInsertion {
            id: "proposal".into(),
            author: "Agent".into(),
            block: block.clone(),
            at: base.point(&block, 0).unwrap(),
            text: "APPROVED ".into(),
        }],
    );
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("propose", &propose),
        StatusCode::OK,
    )
    .await;
    let proposed = native(&get(&app, &format!("{url}/document-state")).await);
    let late = request(
        &proposed,
        "late",
        vec![Command::AddComment {
            id: "late".into(),
            author: "Reviewer".into(),
            body: "On the insertion".into(),
            ranges: proposed.proposal_selection("proposal", 0, 8).unwrap(),
        }],
    );
    let accept = request(
        &proposed,
        "accept",
        vec![Command::AcceptProposal {
            id: "proposal".into(),
        }],
    );
    let ack = post(
        &app,
        &format!("{url}/document-commands"),
        batch("accept", &accept),
        StatusCode::OK,
    )
    .await;
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("late", &late),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        ack,
        post(
            &app,
            &format!("{url}/document-commands"),
            batch("accept", &accept),
            StatusCode::OK
        )
        .await
    );
    let current = native(&get(&app, &format!("{url}/document-state")).await);
    let reply = request(
        &current,
        "reply",
        vec![Command::ReplyComment {
            id: "reply".into(),
            parent: "proposal".into(),
            author: "Reviewer".into(),
            body: "Accepted once".into(),
        }],
    );
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("reply", &reply),
        StatusCode::OK,
    )
    .await;
    let review = get(&app, &format!("{url}/review?includeResolved=1")).await;
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(
        review["comments"][0]["target"]["attachments"][0]["quote"],
        "APPROVED"
    );
    assert_eq!(
        review["suggestions"][0]["replies"][0]["body"],
        "Accepted once"
    );
    assert_eq!(current.block_view(&block).unwrap().text, "APPROVED TARGET");
}

#[tokio::test]
async fn native_http_default_delayed_review_restart_and_exact_receipts() {
    let (_root, app, store) = document_test_app().await;
    let mut urls: Vec<String> = Vec::new();
    #[cfg(feature = "tmp-documents")]
    {
        let created = post(
            &app,
            "/v1/tmp/documents",
            json!({"content":"See TARGET here.\n\nSee TARGET elsewhere.\n"}),
            StatusCode::CREATED,
        )
        .await;
        urls.push(format!(
            "/v1/tmp/documents/{}",
            created["document"]["path"].as_str().unwrap()
        ));
    }
    #[cfg(feature = "lib-documents")]
    {
        store.create_library("native").await.unwrap();
        store
            .import_block_document(
                "native",
                "doc.md",
                "See TARGET here.\n\nSee TARGET elsewhere.\n",
                json!({}),
                "text/markdown",
                quarry_core::DocumentSource::Rest,
                quarry_core::WritePrecondition::None,
            )
            .await
            .unwrap();
        urls.push("/v1/libraries/native/documents/doc.md".into());
    }
    for url in urls {
        assert_eq!(
            get(&app, &format!("{url}/document-state")).await["format"],
            "automerge"
        );
        let initial = get(&app, &format!("{url}/document-state")).await;
        let base = native(&initial);
        let block = base.blocks().unwrap()[0].id.clone();
        let comment = request(
            &base,
            "comment",
            vec![Command::AddComment {
                id: "comment".into(),
                author: "Reviewer".into(),
                body: "Keep this".into(),
                ranges: base.selection(&block, 4, 10).unwrap(),
            }],
        );
        let split = request(
            &base,
            "split",
            vec![Command::SplitBlock {
                block: block.clone(),
                at: base.point(&block, 7).unwrap(),
                new_block: format!("tail-{}", base.id().unwrap()),
            }],
        );
        post(
            &app,
            &format!("{url}/document-commands"),
            batch("split-batch", &split),
            StatusCode::OK,
        )
        .await;
        // A new hub has no live state. The late command still has its exact base.
        let restarted = quarry_server::router(store.clone());
        let ack = post(
            &restarted,
            &format!("{url}/document-commands"),
            batch("comment-batch", &comment),
            StatusCode::OK,
        )
        .await;
        let result = get(&restarted, &format!("{url}/document-state")).await;
        let current = native(&result);
        let target = current.comment_target("comment").unwrap();
        assert_eq!(target.attachments.len(), 2);
        assert_eq!(
            target
                .attachments
                .iter()
                .map(|a| a.quote.as_str())
                .collect::<String>(),
            "TARGET"
        );
        assert_eq!(
            ack,
            post(
                &restarted,
                &format!("{url}/document-commands"),
                batch("comment-batch", &comment),
                StatusCode::OK
            )
            .await
        );
        let mut reused = batch("comment-batch", &comment);
        reused["actor"]["label"] = json!("Different request");
        post(
            &restarted,
            &format!("{url}/document-commands"),
            reused,
            StatusCode::PRECONDITION_FAILED,
        )
        .await;
        assert_eq!(
            result,
            get(&restarted, &format!("{url}/document-state")).await
        );
        let mapped = store
            .durable_heads_for_scope(
                &if url.starts_with("/v1/tmp") {
                    quarry_storage::DocumentScopeRef::Tmp
                } else {
                    quarry_storage::DocumentScopeRef::library("native")
                },
                url.rsplit('/').next().unwrap(),
                ack["document_clock"].as_str().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            mapped.unwrap(),
            current
                .heads()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn failed_native_batch_publishes_no_candidate_and_can_be_corrected() {
    let (_root, app, store) = document_test_app().await;
    let created = post(
        &app,
        "/v1/tmp/documents",
        json!({"content":"TARGET\n"}),
        StatusCode::CREATED,
    )
    .await;
    let url = format!(
        "/v1/tmp/documents/{}",
        created["document"]["path"].as_str().unwrap()
    );
    let initial = get(&app, &format!("{url}/document-state")).await;
    let doc = native(&initial);
    let block = doc.blocks().unwrap()[0].id.clone();
    let point = doc.point(&block, 0).unwrap();
    let mut events = store.subscribe_events();
    let invalid = request(
        &doc,
        "invalid",
        vec![
            Command::InsertText {
                at: point.clone(),
                text: "Should roll back ".into(),
            },
            Command::DeleteBlock {
                block: "missing".into(),
            },
        ],
    );
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("retryable", &invalid),
        StatusCode::PRECONDITION_FAILED,
    )
    .await;
    assert_eq!(initial, get(&app, &format!("{url}/document-state")).await);
    assert!(events.try_recv().is_err());
    let corrected = request(
        &doc,
        "corrected",
        vec![Command::InsertText {
            at: point,
            text: "Saved ".into(),
        }],
    );
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("retryable", &corrected),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        native(&get(&app, &format!("{url}/document-state")).await)
            .block_view(&block)
            .unwrap()
            .text,
        "Saved TARGET"
    );
    let missing = format!("/v1/tmp/documents/{}/document-state", doc.id().unwrap());
    let response = app
        .clone()
        .oneshot(Request::builder().uri(missing).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
    ));
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn existing_agent_commands_share_native_identity_with_delayed_browser_edits() {
    let (_root, app, _store) = document_test_app().await;
    let created = post(
        &app,
        "/v1/tmp/documents",
        json!({"content":"See [TARGET](https://example.com) here.\n\nSee TARGET elsewhere.\n"}),
        StatusCode::CREATED,
    )
    .await;
    let url = format!(
        "/v1/tmp/documents/{}",
        created["document"]["path"].as_str().unwrap()
    );
    let initial = get(&app, &format!("{url}/document-state")).await;
    let base = native(&initial);
    let block = base.blocks().unwrap()[0].id.clone();
    let browser = request(
        &base,
        "browser-typing",
        vec![Command::InsertText {
            at: base.point(&block, 7).unwrap(),
            text: "!".into(),
        }],
    );
    let agent = |id: &str, clock: &Value, ops: Value| json!({"client_tx_id":id,"base_clock":clock,"actor":{"kind":"agent","id":"agent:test"},"ops":ops});
    let comment = agent(
        "agent-comment",
        &initial["document_clock"],
        json!([{"op":"comment.add","block_id":block,"start":4,"end":10,"quote":"TARGET","body":"Keep this"}]),
    );
    let original_ack = post(
        &app,
        &format!("{url}/transactions"),
        comment.clone(),
        StatusCode::OK,
    )
    .await;
    post(
        &app,
        &format!("{url}/document-commands"),
        batch("browser-typing-batch", &browser),
        StatusCode::OK,
    )
    .await;
    // Retry the agent's original request after another write: its exact ack remains.
    assert_eq!(
        original_ack,
        post(
            &app,
            &format!("{url}/transactions"),
            comment,
            StatusCode::OK
        )
        .await
    );
    let replace = agent(
        "agent-replace",
        &initial["document_clock"],
        json!([{"op":"replace_block_content","block_id":block,"text":"Prefix See TARGET here. Suffix"}]),
    );
    post(
        &app,
        &format!("{url}/transactions"),
        replace,
        StatusCode::OK,
    )
    .await;
    let current = get(&app, &format!("{url}/document-state")).await;
    assert_eq!(
        native(&current).block_view(&block).unwrap().text,
        "Prefix See TAR!GET here. Suffix"
    );
    let move_op = agent(
        "agent-move",
        &current["document_clock"],
        json!([{"op":"move_block","block_id":block,"position":1}]),
    );
    post(
        &app,
        &format!("{url}/transactions"),
        move_op,
        StatusCode::OK,
    )
    .await;
    let result = native(&get(&app, &format!("{url}/document-state")).await);
    let comment = result.comments().unwrap()[0].id.clone();
    assert_eq!(
        result.comment_target(&comment).unwrap().attachments[0].quote,
        "TAR!GET"
    );
    let review = get(&app, &format!("{url}/review")).await;
    assert_eq!(review["comments"][0]["status"], "open");
    assert_eq!(review["comments"][0]["target"]["state"], "attached");
    assert_eq!(
        review["comments"][0]["target"]["attachments"][0]["owner"]["id"],
        block
    );
}
