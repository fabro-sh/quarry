#![cfg(any(feature = "tmp-documents", feature = "lib-documents"))]
#![allow(clippy::unwrap_used, reason = "native HTTP conformance fixtures")]
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use futures_util::StreamExt;
use quarry_core::{DocumentSource, WritePrecondition};
use quarry_document::{Command, CommandRequest, Document, SeedBlock, TargetOwner};
use quarry_storage::{DocumentScopeRef, QuarryStore};
use serde_json::{Value, json};
use tower::ServiceExt;
mod common;

struct Fixture {
    _root: tempfile::TempDir,
    app: axum::Router,
    store: QuarryStore,
    scope: DocumentScopeRef,
    path: String,
    path_url: String,
    url: String,
    id: String,
}
fn scopes() -> Vec<bool> {
    [
        (true, cfg!(feature = "tmp-documents")),
        (false, cfg!(feature = "lib-documents")),
    ]
    .into_iter()
    .filter_map(|(tmp, enabled)| enabled.then_some(tmp))
    .collect()
}

#[tokio::test]
async fn whole_file_http_saves_require_the_writers_read_version() {
    for tmp in scopes() {
        let fixture = Fixture::new(
            tmp,
            "See TARGET here.\n\nUnchanged separator.\n\nAgent paragraph.\n",
        )
        .await;
        let (state, base) = fixture.state().await;
        let block = first(&base);
        fixture
            .send(request(
                &base,
                "browser-prefix-before-put",
                vec![Command::InsertText {
                    at: base.point(&block, 0).unwrap(),
                    text: "Browser ".into(),
                }],
            ))
            .await;
        let before = fixture.state().await.0;
        let incoming = "See TARGET here.\n\nUnchanged separator.\n\nAgent paragraph changed.\n";
        for (header, expected, code) in [
            (
                None,
                StatusCode::PRECONDITION_REQUIRED,
                "PRECONDITION_REQUIRED",
            ),
            (
                Some(("if-match", state["document_clock"].as_str().unwrap())),
                StatusCode::PRECONDITION_FAILED,
                "PRECONDITION_FAILED",
            ),
            (
                Some(("if-none-match", "*")),
                StatusCode::PRECONDITION_FAILED,
                "PRECONDITION_FAILED",
            ),
        ] {
            let mut builder = Request::builder()
                .method(Method::PUT)
                .uri(&fixture.path_url)
                .header("content-type", "text/markdown");
            if let Some((name, value)) = header {
                builder = builder.header(name, value);
            }
            let response = fixture
                .app
                .clone()
                .oneshot(builder.body(Body::from(incoming)).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let error = common::response_json(response).await;
            assert_eq!(status, expected, "{error}");
            assert_eq!(error["code"], code);
            assert_eq!(fixture.state().await.0, before);
        }
        fixture
            .put(incoming, state["document_clock"].as_str().unwrap())
            .await;
        assert_eq!(
            fixture.markdown().await,
            "Browser See TARGET here.\n\nUnchanged separator.\n\nAgent paragraph changed.\n"
        );
        // A comment arriving from the original read still finds its characters.
        post(
            &fixture.app,
            &format!("{}/transactions", fixture.url),
            json!({
                "client_tx_id":"late-comment-after-put", "base_clock":state["document_clock"],
                "actor":{"kind":"agent"}, "ops":[{"op":"comment.add", "block_id":block,
                "start":4, "end":10, "body":"Original TARGET"}]
            }),
            StatusCode::OK,
        )
        .await;
        let saved = fixture.consistent().await;
        let comment = &saved.comments().unwrap()[0];
        let target = saved.comment_target(&comment.id).unwrap();
        assert_eq!(target.attachments[0].quote, "TARGET");
        assert_eq!(target.attachments[0].start, 12);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_http_creates_and_strict_saves_have_one_winner() {
    for tmp in scopes() {
        for creating in [true, false] {
            let fixture = Fixture::new(tmp, "Initial.\n").await;
            let (state, _) = fixture.state().await;
            let path = if creating {
                if tmp {
                    format!("/v1/tmp/documents/{}", "a".repeat(32))
                } else {
                    "/v1/libraries/native/documents/new.md".into()
                }
            } else {
                fixture.path_url.clone()
            };
            let send = |text| {
                let app = fixture.app.clone();
                let request = Request::builder()
                    .method(Method::PUT)
                    .uri(&path)
                    .header("content-type", "text/markdown")
                    .header(
                        if creating {
                            "if-none-match"
                        } else {
                            "if-match"
                        },
                        if creating {
                            "*"
                        } else {
                            state["document_clock"].as_str().unwrap()
                        },
                    )
                    .body(Body::from(text))
                    .unwrap();
                async move { app.oneshot(request).await.unwrap() }
            };
            let (a, b) = tokio::join!(send("First.\n"), send("Second.\n"));
            let mut statuses = [a.status(), b.status()];
            statuses.sort();
            assert_eq!(statuses, [StatusCode::OK, StatusCode::PRECONDITION_FAILED]);
            let winner: Value =
                common::response_json(if a.status() == StatusCode::OK { a } else { b }).await;
            let saved = fixture
                .app
                .clone()
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                saved.headers()["etag"].to_str().unwrap(),
                format!("\"{}\"", winner["version"]["id"].as_str().unwrap())
            );
        }
    }
}

#[tokio::test]
async fn agent_transactions_require_the_read_version_before_resolving_repeated_text() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let (_, base) = fixture.state().await;
        let block = first(&base);
        fixture
            .send(request(
                &base,
                "browser-prefix",
                vec![Command::InsertText {
                    at: base.point(&block, 0).unwrap(),
                    text: "See TARGET ".into(),
                }],
            ))
            .await;
        let before = fixture.state().await.0;
        // These offsets and even this exact quote now identify the wrong occurrence.
        // Only the version of the agent's read disambiguates the intended characters.
        for (index, clock) in [
            None,
            Some(Value::Null),
            Some(json!("")),
            Some(json!("  ")),
            Some(json!(7)),
            Some(json!([])),
        ]
        .into_iter()
        .enumerate()
        {
            let mut payload = json!({"client_tx_id":format!("missing-base-{index}"),
                "actor":{"kind":"agent"}, "ops":[{"op":"comment.add", "block_id":block,
                    "start":4,"end":10,"quote":"TARGET","body":"Original occurrence"}]});
            if let Some(clock) = clock {
                payload["base_clock"] = clock;
            }
            let error = post(
                &fixture.app,
                &format!("{}/transactions", fixture.url),
                payload,
                StatusCode::BAD_REQUEST,
            )
            .await;
            assert_eq!(error["code"], "INVALID_TRANSACTION");
            assert_eq!(error["details"]["field"], "base_clock");
            assert_eq!(fixture.state().await.0, before);
            assert!(fixture.consistent().await.comments().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn versioned_agent_comments_and_browser_typing_keep_the_original_occurrence_in_both_orders() {
    for tmp in scopes() {
        for agent_first in [true, false] {
            let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
            let (state, base) = fixture.state().await;
            let block = first(&base);
            let typing = request(
                &base,
                "browser-prefix",
                vec![Command::InsertText {
                    at: base.point(&block, 0).unwrap(),
                    text: "See TARGET ".into(),
                }],
            );
            let payload = json!({"client_tx_id":"agent-original", "base_clock":state["document_clock"],
                "actor":{"kind":"agent"}, "ops":[{"op":"comment.add", "block_id":block,
                    "start":4,"end":10,"quote":"TARGET","body":"Original occurrence"}]});
            let endpoint = format!("{}/transactions", fixture.url);
            if !agent_first {
                fixture.send(typing.clone()).await;
            }
            let ack = post(&fixture.app, &endpoint, payload.clone(), StatusCode::OK).await;
            assert_eq!(
                ack["status"],
                if agent_first {
                    "committed"
                } else {
                    "committed_rebased"
                }
            );
            if agent_first {
                fixture.send(typing).await;
            }
            let document = fixture.consistent().await;
            assert_eq!(
                document.block_view(&block).unwrap().text,
                "See TARGET See TARGET here."
            );
            let comments = document.comments().unwrap();
            assert_eq!(comments.len(), 1);
            let target = document.comment_target(&comments[0].id).unwrap();
            assert_eq!(target.attachments.len(), 1);
            assert_eq!(target.attachments[0].quote, "TARGET");
            assert_eq!(target.attachments[0].start, 15);
            assert_eq!(target.attachments[0].end, 21);
            let before_retry = fixture.state().await.0;
            // A lost response must replay its original receipt after a server restart,
            // even though the document has advanced. Never substitute the latest clock.
            let restarted = quarry_server::router(fixture.store.clone());
            assert_eq!(
                post(&restarted, &endpoint, payload.clone(), StatusCode::OK).await,
                ack
            );
            let mut changed_request = payload;
            changed_request["base_clock"] = before_retry["document_clock"].clone();
            post(
                &restarted,
                &endpoint,
                changed_request,
                StatusCode::PRECONDITION_FAILED,
            )
            .await;
            assert_eq!(fixture.state().await.0, before_retry);
        }
    }
}

#[tokio::test]
async fn stale_agent_deletion_cannot_remove_browser_typing_or_partially_commit_review() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let (state, base) = fixture.state().await;
        let block = first(&base);
        fixture
            .send(request(
                &base,
                "browser-prefix",
                vec![Command::InsertText {
                    at: base.point(&block, 0).unwrap(),
                    text: "Browser ".into(),
                }],
            ))
            .await;
        let before = fixture.state().await.0;
        let error = post(&fixture.app, &format!("{}/transactions", fixture.url), json!({
            "client_tx_id":"stale-delete", "base_clock":state["document_clock"], "actor":{"kind":"agent"},
            "ops":[{"op":"comment.add","block_id":block,"start":4,"end":10,"body":"Never committed"},
                {"op":"delete_block","block_id":block}]
        }), StatusCode::PRECONDITION_FAILED).await;
        assert_eq!(error["code"], "PRECONDITION_FAILED");
        assert_eq!(fixture.state().await.0, before);
        let saved = fixture.consistent().await;
        assert_eq!(
            saved.block_view(&block).unwrap().text,
            "Browser See TARGET here."
        );
        assert!(saved.comments().unwrap().is_empty());
    }
}

#[tokio::test]
async fn incremental_state_in_both_scopes_and_address_forms_preserves_pending_browser_history() {
    for tmp in scopes() {
        let content = (0..120)
            .map(|n| format!("Paragraph {n}: unchanged content and TARGET."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let fixture = Fixture::new(tmp, &content).await;
        let (_, original) = fixture.state().await;
        let base = original.heads();
        let query = base
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let id = first(&original);
        let mut browser = original.fork();
        browser
            .insert_text(&browser.point(&id, 0).unwrap(), "Pending browser ")
            .unwrap();
        fixture
            .send(request(
                &original,
                "remote-delta",
                vec![Command::InsertText {
                    at: original.point(&id, 0).unwrap(),
                    text: "Agent ".into(),
                }],
            ))
            .await;
        let (state, authority) = fixture.state().await;
        let mut expected = browser.fork();
        expected.merge(&authority).unwrap();
        for url in [&fixture.url, &fixture.path_url] {
            let delta = get(&fixture.app, &format!("{url}/document-state?since={query}")).await;
            assert_eq!(
                delta["base"],
                json!(base.iter().map(ToString::to_string).collect::<Vec<_>>())
            );
            assert_eq!(delta["document_clock"], state["document_clock"]);
            let bytes: Vec<u8> = serde_json::from_value(delta["bytes"].clone()).unwrap();
            assert!(bytes.len() < authority.save().len());
            let mut received = browser.fork();
            received
                .merge_changes(&bytes, &base, &authority.heads())
                .unwrap();
            assert_eq!(received.view().unwrap(), expected.view().unwrap());
            assert_eq!(
                delta["document"],
                serde_json::to_value(authority.view().unwrap()).unwrap()
            );
        }
        let current = authority
            .heads()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            get(
                &fixture.app,
                &format!("{}/document-state?since={current}", fixture.url)
            )
            .await["bytes"],
            json!([])
        );
        let foreign = Document::new().unwrap().heads()[0].to_string();
        for (query, expected) in [
            ("invalid", StatusCode::BAD_REQUEST),
            (&foreign, StatusCode::PRECONDITION_FAILED),
        ] {
            let response = fixture
                .app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("{}/document-state?since={query}", fixture.url))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        assert_eq!(
            fixture.state().await.0["document_clock"],
            state["document_clock"]
        );
    }
}
async fn get(app: &axum::Router, url: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    common::response_json(response).await
}
async fn post(app: &axum::Router, url: &str, payload: Value, expected: StatusCode) -> Value {
    let response = app
        .clone()
        .oneshot(common::json_request(Method::POST, url, payload))
        .await
        .unwrap();
    let status = response.status();
    let body = common::response_json(response).await;
    assert_eq!(status, expected, "{body}");
    body
}
impl Fixture {
    async fn new(tmp: bool, content: &str) -> Self {
        let (root, app, store) = common::document_test_app().await;
        let (scope, path, path_url, id) = if tmp {
            let created = store
                .create_tmp_document(
                    content.as_bytes().to_vec(),
                    json!({}),
                    "text/markdown",
                    quarry_storage::TmpTtl::Default,
                )
                .await
                .unwrap();
            let path = created.document.path;
            (
                DocumentScopeRef::Tmp,
                path.clone(),
                format!("/v1/tmp/documents/{path}"),
                created.document.id.to_string(),
            )
        } else {
            store.create_library("native").await.unwrap();
            let created = store
                .put_document(quarry_storage::PutDocumentRequest {
                    library: "native".into(),
                    path: "doc.md".into(),
                    content: content.as_bytes().to_vec(),
                    metadata: json!({}),
                    content_type: "text/markdown".into(),
                    source: DocumentSource::Rest,
                    precondition: WritePrecondition::None,
                    origin_id: None,
                    transaction: Default::default(),
                })
                .await
                .unwrap();
            (
                DocumentScopeRef::library("native"),
                "doc.md".into(),
                "/v1/libraries/native/documents/doc.md".into(),
                created.document.id.to_string(),
            )
        };
        let url = if tmp {
            path_url.clone()
        } else {
            format!("/v1/libraries/native/documents-by-id/{id}")
        };
        Self {
            _root: root,
            app,
            store,
            scope,
            path,
            path_url,
            url,
            id,
        }
    }
    async fn state(&self) -> (Value, Document) {
        let state = get(&self.app, &format!("{}/document-state", self.url)).await;
        let bytes: Vec<u8> = serde_json::from_value(state["bytes"].clone()).unwrap();
        let doc = Document::load(&bytes).unwrap();
        assert_eq!(doc.id().unwrap(), self.id);
        (state, doc)
    }
    async fn send(&self, payload: Value) -> Value {
        post(
            &self.app,
            &format!("{}/document-commands", self.url),
            payload,
            StatusCode::OK,
        )
        .await
    }
    async fn put(&self, content: &str, base: &str) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&self.path_url)
                    .header("content-type", "text/markdown")
                    .header("x-quarry-merge-base", format!("\"{base}\""))
                    .body(Body::from(content.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = common::response_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    async fn markdown(&self) -> String {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&self.path_url)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }
    async fn consistent(&self) -> Document {
        let (state, document) = self.state().await;
        let saved = self
            .store
            .durable_document_for_scope(&self.scope, &self.path)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state["document_clock"], saved.version_id);
        assert_eq!(
            Document::load(&saved.bytes).unwrap().view().unwrap(),
            document.view().unwrap()
        );
        assert_eq!(
            self.store.load_block_tree(&self.id).await.unwrap(),
            quarry_storage::document_projection(&document).unwrap()
        );
        assert_eq!(
            self.store.list_block_review_items(&self.id).await.unwrap(),
            quarry_storage::document_review_projection(&document).unwrap()
        );
        assert_eq!(
            self.markdown().await,
            self.store.export_block_document(&self.id).await.unwrap()
        );
        document
    }
}
fn request(document: &Document, id: &str, commands: Vec<Command>) -> Value {
    json!({ "request_id": id, "actor": {"kind":"browser","id":"browser:test","label":"Reviewer"}, "requests": [CommandRequest {
        request_id: id.into(), base: document.heads().iter().map(ToString::to_string).collect(), commands, at: "2026-09-05T12:00:00Z".into(),
    }] })
}
fn first(document: &Document) -> String {
    document.blocks().unwrap()[0].id.clone()
}
fn comment(document: &Document, id: &str) -> Value {
    request(
        document,
        id,
        vec![Command::AddComment {
            id: id.into(),
            author: "Reviewer".into(),
            body: "Keep".into(),
            ranges: document.selection(&first(document), 4, 10).unwrap(),
        }],
    )
}

#[tokio::test]
async fn read_only_clients_never_create_versions_and_all_writes_are_durable_before_ack() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let (before, document) = fixture.state().await;
        for _ in 0..5 {
            assert_eq!(fixture.state().await.0, before);
        }
        let ack = fixture
            .send(request(
                &document,
                "typing",
                vec![Command::InsertText {
                    at: document.point(&first(&document), 4).unwrap(),
                    text: "new ".into(),
                }],
            ))
            .await;
        let state = fixture.state().await.0;
        assert_eq!(state["document_clock"], ack["document_clock"]);
        assert_eq!(
            fixture
                .consistent()
                .await
                .block_view(&first(&document))
                .unwrap()
                .text,
            "See new TARGET here."
        );
        let restarted = quarry_server::router(fixture.store.clone());
        assert_eq!(
            get(&restarted, &format!("{}/document-state", fixture.url)).await,
            state
        );
    }
}

#[tokio::test]
async fn concurrent_http_typing_and_late_review_converge_after_split_and_move() {
    for tmp in scopes() {
        for comment_first in [true, false] {
            let fixture = Fixture::new(tmp, "See TARGET here.\n\nSee TARGET elsewhere.\n").await;
            let (_, base) = fixture.state().await;
            let block = first(&base);
            let change = request(
                &base,
                "structure",
                vec![
                    Command::SplitBlock {
                        block: block.clone(),
                        at: base.point(&block, 7).unwrap(),
                        new_block: "tail".into(),
                    },
                    Command::MoveBlock {
                        block: "tail".into(),
                        parent: None,
                        before: None,
                    },
                ],
            );
            let review = comment(&base, "late");
            if comment_first {
                fixture.send(review).await;
                fixture.send(change).await;
            } else {
                fixture.send(change).await;
                fixture.send(review).await;
            }
            let document = fixture.consistent().await;
            let target = document.comment_target("late").unwrap();
            assert_eq!(
                target
                    .attachments
                    .iter()
                    .map(|a| a.quote.as_str())
                    .collect::<String>(),
                "TARGET"
            );
            assert!(
                target
                    .attachments
                    .iter()
                    .all(|a| a.owner == TargetOwner::Block(block.clone())
                        || a.owner == TargetOwner::Block("tail".into()))
            );
        }
    }
}

#[tokio::test]
async fn two_simultaneous_requests_commit_once_each_without_losing_either_edit() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let (_, base) = fixture.state().await;
        let typing = request(
            &base,
            "typing",
            vec![Command::InsertText {
                at: base.point(&first(&base), 0).unwrap(),
                text: "Browser: ".into(),
            }],
        );
        let review = comment(&base, "review");
        let (a, b) = tokio::join!(fixture.send(typing.clone()), fixture.send(review.clone()));
        assert_ne!(a["document_clock"], b["document_clock"]);
        let saved = fixture.consistent().await;
        assert_eq!(
            saved.block_view(&first(&base)).unwrap().text,
            "Browser: See TARGET here."
        );
        assert_eq!(
            saved.comment_target("review").unwrap().attachments[0].quote,
            "TARGET"
        );
        assert_eq!(fixture.send(typing).await, a);
        assert_eq!(fixture.send(review).await, b);
        assert_eq!(fixture.state().await.1.heads(), saved.heads());
    }
}

#[tokio::test]
async fn a_file_write_and_unsent_browser_typing_preserve_the_same_text_identity() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let (initial, base) = fixture.state().await;
        let typing = request(
            &base,
            "typing",
            vec![Command::InsertText {
                at: base.point(&first(&base), 0).unwrap(),
                text: "Browser: ".into(),
            }],
        );
        fixture
            .put(
                "See TARGET here. Agent suffix.\n",
                initial["document_clock"].as_str().unwrap(),
            )
            .await;
        fixture.send(typing).await;
        fixture.send(comment(&base, "late")).await;
        let saved = fixture.consistent().await;
        assert_eq!(
            saved.block_view(&first(&base)).unwrap().text,
            "Browser: See TARGET here. Agent suffix."
        );
        assert_eq!(
            saved.comment_target("late").unwrap().attachments[0].quote,
            "TARGET"
        );
    }
}

#[tokio::test]
async fn comments_on_proposed_text_survive_both_orders_of_acceptance_and_delivery() {
    for tmp in scopes() {
        for comment_first in [true, false] {
            let fixture = Fixture::new(tmp, "Body.\n").await;
            let (_, base) = fixture.state().await;
            fixture
                .send(request(
                    &base,
                    "proposal",
                    vec![Command::ProposeInsertion {
                        id: "proposal".into(),
                        author: "Agent".into(),
                        block: first(&base),
                        at: base.point(&first(&base), 0).unwrap(),
                        text: "😀PROPOSED ".into(),
                    }],
                ))
                .await;
            let (_, proposed) = fixture.state().await;
            let comment = request(
                &proposed,
                "comment",
                vec![Command::AddComment {
                    id: "comment".into(),
                    author: "Reviewer".into(),
                    body: "Keep".into(),
                    ranges: proposed.proposal_selection("proposal", 0, 10).unwrap(),
                }],
            );
            let accept = request(
                &proposed,
                "accept",
                vec![Command::AcceptProposal {
                    id: "proposal".into(),
                }],
            );
            if comment_first {
                fixture.send(comment).await;
                fixture.send(accept).await;
            } else {
                fixture.send(accept).await;
                fixture.send(comment).await;
            }
            let saved = fixture.consistent().await;
            assert_eq!(
                saved.comment_target("comment").unwrap().attachments[0].quote,
                "😀PROPOSED"
            );
            assert_eq!(
                saved.comment_target("comment").unwrap().attachments[0].owner,
                TargetOwner::Block(first(&base))
            );
        }
    }
}

#[tokio::test]
async fn invalid_commands_do_not_poison_later_valid_requests_or_emit_commit_events() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let (initial, base) = fixture.state().await;
        let mut events = fixture.store.subscribe_events();
        let invalid = request(
            &base,
            "invalid",
            vec![
                Command::InsertText {
                    at: base.point(&first(&base), 0).unwrap(),
                    text: "Wrong ".into(),
                },
                Command::InsertBlock {
                    block: SeedBlock {
                        id: "bad".into(),
                        kind: "unknown".into(),
                        parent: None,
                        position: 1,
                        attrs: Default::default(),
                        text: String::new(),
                    },
                },
            ],
        );
        post(
            &fixture.app,
            &format!("{}/document-commands", fixture.url),
            invalid,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(fixture.state().await.0, initial);
        assert!(events.try_recv().is_err());
        fixture.send(comment(&base, "valid")).await;
        assert_eq!(fixture.consistent().await.comments().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn formatting_links_unicode_and_list_attributes_round_trip_through_native_state() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See 😀TARGET here.\n").await;
        let (_, base) = fixture.state().await;
        let block = first(&base);
        let ranges = base.selection(&block, 4, 12).unwrap();
        fixture
            .send(request(
                &base,
                "format",
                vec![
                    Command::Format {
                        ranges: ranges.clone(),
                        name: "bold".into(),
                        value: json!(true),
                    },
                    Command::Format {
                        ranges: ranges.clone(),
                        name: "link".into(),
                        value: json!("https://example.com"),
                    },
                    Command::SetBlock {
                        block,
                        kind: "p".into(),
                        attrs: [
                            ("listStyleType".into(), json!("todo")),
                            ("indent".into(), json!(1)),
                            ("listStart".into(), json!(7)),
                        ]
                        .into_iter()
                        .collect(),
                    },
                ],
            ))
            .await;
        let saved = fixture.consistent().await;
        let block = &saved.view().unwrap().blocks[0].block;
        assert_eq!(block.attrs["checked"], false);
        assert!(!block.attrs.contains_key("listStart"));
        let markdown = fixture.markdown().await;
        assert!(markdown.contains("😀TARGET"));
        assert!(markdown.contains("https://example.com"));
        assert!(markdown.contains("- [ ]"));
    }
}

#[tokio::test]
async fn native_review_edits_resolution_and_replies_are_persistent_and_exported_to_agent_reads() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let (_, base) = fixture.state().await;
        fixture.send(comment(&base, "c")).await;
        let (_, current) = fixture.state().await;
        fixture
            .send(request(
                &current,
                "discussion",
                vec![
                    Command::EditComment {
                        id: "c".into(),
                        body: "Updated".into(),
                    },
                    Command::ReplyComment {
                        id: "r".into(),
                        parent: "c".into(),
                        author: "Agent".into(),
                        body: "Agreed".into(),
                    },
                    Command::ResolveComment {
                        id: "c".into(),
                        resolved: true,
                    },
                ],
            ))
            .await;
        let saved = fixture.consistent().await;
        assert_eq!(saved.comment("c").unwrap().body, "Updated");
        assert_eq!(saved.comment("r").unwrap().parent_id.as_deref(), Some("c"));
        assert_eq!(
            saved.comment_target("c").unwrap().attachments[0].quote,
            "TARGET"
        );
        let review = get(
            &fixture.app,
            &format!("{}/review?includeResolved=1", fixture.path_url),
        )
        .await;
        assert_eq!(review["comments"][0]["status"], "resolved");
        assert_eq!(review["comments"][0]["body"], "Updated");
    }
}

#[tokio::test]
async fn committed_events_reference_the_same_native_version_as_the_receipt() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See TARGET here.\n").await;
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{}/events/stream", fixture.url))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut events = response.into_body().into_data_stream();
        let (_, base) = fixture.state().await;
        let ack = fixture.send(comment(&base, "c")).await;
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event = String::from_utf8(event.to_vec()).unwrap();
        assert!(event.contains("doc.changed"));
        assert!(event.contains(ack["document_clock"].as_str().unwrap()));
        if tmp {
            assert!(!event.contains(&fixture.path));
        }
        fixture.consistent().await;
    }
}

#[cfg(feature = "lib-documents")]
#[tokio::test]
async fn renames_do_not_change_the_editor_address_or_strand_queued_requests() {
    let mut fixture = Fixture::new(false, "See TARGET here.\n").await;
    let (_, base) = fixture.state().await;
    let pending = comment(&base, "pending");
    fixture
        .store
        .move_document("native", "doc.md", "moved.md", DocumentSource::Rest)
        .await
        .unwrap();
    fixture.path = "moved.md".into();
    fixture.path_url = "/v1/libraries/native/documents/moved.md".into();
    fixture.send(pending).await;
    assert_eq!(
        fixture
            .consistent()
            .await
            .comment_target("pending")
            .unwrap()
            .attachments[0]
            .quote,
        "TARGET"
    );
}

#[cfg(feature = "lib-documents")]
#[tokio::test]
async fn invitations_are_scoped_revocable_and_enforce_viewing_for_native_transport() {
    let fixture = Fixture::new(false, "See TARGET here.\n").await;
    let viewer = fixture
        .store
        .create_collab_invite_token("native", "doc.md", "viewer", None)
        .await
        .unwrap();
    let state = get(
        &fixture.app,
        &format!("{}/document-state?token={}", fixture.url, viewer.id),
    )
    .await;
    assert_eq!(state["writable"], false);
    let (_, base) = fixture.state().await;
    post(
        &fixture.app,
        &format!("{}/document-commands?token={}", fixture.url, viewer.id),
        comment(&base, "refused"),
        StatusCode::FORBIDDEN,
    )
    .await;
    fixture
        .store
        .revoke_collab_invite_token(&viewer.id)
        .await
        .unwrap();
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{}/document-state?token={}",
                    fixture.url, viewer.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(fixture.state().await.1.comments().unwrap().is_empty());
}

#[tokio::test]
async fn native_selections_are_ephemeral_scoped_and_do_not_publish_versions() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "TARGET\n").await;
        let (before, document) = fixture.state().await;
        let id = document.blocks().unwrap()[0].id.clone();
        let points = json!([
            document.point(&id, 0).unwrap(),
            document.point(&id, 3).unwrap()
        ]);
        let first = post(
            &fixture.app,
            &format!("{}/selection", fixture.url),
            json!({"client_id":"first","author":"Reviewer","points":points}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(first.as_array().unwrap().len(), 1);
        let peers = post(
            &fixture.app,
            &format!("{}/selection", fixture.url),
            json!({"client_id":"second","author":"Other","points":points}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(peers.as_array().unwrap().len(), 2);
        post(
            &fixture.app,
            &format!("{}/selection", fixture.url),
            json!({"client_id":"invalid","author":"Other","points":[{"source":"s","cursor":"c"}]}),
            StatusCode::BAD_REQUEST,
        )
        .await;
        let peers = post(
            &fixture.app,
            &format!("{}/selection", fixture.url),
            json!({"client_id":"first","author":"Reviewer","points":[]}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(peers.as_array().unwrap().len(), 1);
        assert_eq!(fixture.state().await.0, before);
        let invalid = fixture
            .app
            .clone()
            .oneshot(common::json_request(
                Method::POST,
                &format!("{}/selection", fixture.url.replace(&fixture.id, "missing")),
                json!({"client_id":"first","author":"Reviewer","points":points}),
            ))
            .await
            .unwrap();
        if !tmp {
            assert_eq!(invalid.status(), StatusCode::NOT_FOUND);
        }
    }
}

#[cfg(feature = "lib-documents")]
#[tokio::test]
async fn viewers_can_share_selections_and_revoked_invitations_cannot() {
    let fixture = Fixture::new(false, "TARGET\n").await;
    let invite = fixture
        .store
        .create_collab_invite_token("native", "doc.md", "viewer", None)
        .await
        .unwrap();
    let document = fixture.state().await.1;
    let id = document.blocks().unwrap()[0].id.clone();
    let payload = json!({"client_id":"viewer","author":"Viewer","points":[document.point(&id,0).unwrap(),document.point(&id,3).unwrap()]});
    let url = format!("{}/selection?token={}", fixture.url, invite.id);
    post(&fixture.app, &url, payload.clone(), StatusCode::OK).await;
    fixture
        .store
        .revoke_collab_invite_token(&invite.id)
        .await
        .unwrap();
    post(&fixture.app, &url, payload, StatusCode::NOT_FOUND).await;
}

#[tokio::test]
async fn archive_import_is_create_only_and_validates_bytes_and_metadata_in_both_scopes() {
    for tmp in scopes() {
        let fixture = Fixture::new(tmp, "See {==TARGET==}{>>Keep<<}{#c}.\n").await;
        let archive = get(&fixture.app, &format!("{}/archive", fixture.path_url)).await;
        let destination = if tmp {
            "/v1/tmp/documents/0123456789abcdef0123456789abcdef"
        } else {
            "/v1/libraries/native/documents/imported.md"
        };
        let mut bad_metadata = archive.clone();
        bad_metadata["metadata"] = json!([]);
        for invalid in [
            json!({"format":"quarry-document","version":2,"bytes":[]}),
            json!({"format":"quarry-document","version":1,"bytes":[1,2,3]}),
            bad_metadata,
        ] {
            post(
                &fixture.app,
                &format!("{destination}/archive"),
                invalid,
                StatusCode::BAD_REQUEST,
            )
            .await;
        }
        post(
            &fixture.app,
            &format!("{destination}/archive"),
            archive.clone(),
            StatusCode::CREATED,
        )
        .await;
        let restored = get(&fixture.app, &format!("{destination}/document-state")).await;
        assert_ne!(restored["document"]["document_id"], fixture.id);
        assert_eq!(
            restored["document"]["comments"][0]["target"]["attachments"][0]["quote"],
            "TARGET"
        );
        post(
            &fixture.app,
            &format!("{destination}/archive"),
            archive,
            StatusCode::PRECONDITION_FAILED,
        )
        .await;
        assert_eq!(
            get(&fixture.app, &format!("{destination}/document-state")).await,
            restored
        );
    }
}

#[tokio::test]
#[cfg(feature = "lib-documents")]
async fn path_routes_enforce_supplied_invitation_permissions_for_native_state_archive_and_commands()
{
    let fixture = Fixture::new(false, "TARGET\n").await;
    let token = fixture
        .store
        .create_collab_invite_token("native", "doc.md", "viewer", None)
        .await
        .unwrap();
    let state_url = format!("{}/document-state?token={}", fixture.path_url, token.id);
    assert_eq!(get(&fixture.app, &state_url).await["writable"], false);
    get(
        &fixture.app,
        &format!("{}/archive?token={}", fixture.path_url, token.id),
    )
    .await;
    for suffix in ["document-commands", "transactions", "archive"] {
        post(
            &fixture.app,
            &format!("{}/{suffix}?token={}", fixture.path_url, token.id),
            json!({}),
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    fixture
        .store
        .revoke_collab_invite_token(&token.id)
        .await
        .unwrap();
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(state_url)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
