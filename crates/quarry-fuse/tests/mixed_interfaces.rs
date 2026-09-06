#![allow(clippy::unwrap_used, reason = "mixed-interface conformance fixtures")]

use anyhow::Context as _;
use quarry_core::{DocumentSource, WritePrecondition};
use quarry_document::{Command, CommandRequest, Document};
use quarry_fuse::FuseProjection;
use quarry_git::{push_peer, sync_peer};
use quarry_storage::{DocumentScopeRef, QuarryStore, StoreConfig};
use serde_json::{Value, json};
use std::sync::Arc;

struct Fixture {
    _root: tempfile::TempDir,
    writer: Arc<dyn quarry_storage::BlockMarkdownWriter>,
    server: tokio::task::JoinHandle<()>,
    store: QuarryStore,
    client: reqwest::Client,
    url: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(markdown: &str) -> anyhow::Result<Self> {
        let root = tempfile::tempdir()?;
        let store = QuarryStore::open(StoreConfig {
            db_path: root.path().join("quarry.db"),
            cas_path: root.path().join("cas"),
            lock_path: None,
        })
        .await?;
        let state = quarry_server::app_state(store.clone());
        let writer = quarry_server::install_markdown_writer(&state);
        store.create_library("mixed").await?;
        store
            .import_block_document(
                "mixed",
                "doc.md",
                markdown,
                json!({}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::IfNoneMatch,
            )
            .await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!(
            "http://{}/v1/libraries/mixed/documents/doc.md",
            listener.local_addr()?
        );
        let app = quarry_server::router_with_state(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Ok(Self {
            _root: root,
            writer,
            server,
            store,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            url,
        })
    }
    async fn get(&self, suffix: &str) -> anyhow::Result<Value> {
        Ok(self
            .client
            .get(format!("{}/{suffix}", self.url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    async fn post(&self, suffix: &str, payload: &Value) -> anyhow::Result<Value> {
        let response = self
            .client
            .post(format!("{}/{suffix}", self.url))
            .json(payload)
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        anyhow::ensure!(status.is_success(), "{suffix}: {status}: {value}");
        Ok(value)
    }
    async fn state(&self) -> anyhow::Result<(Value, Document)> {
        let value = self.get("document-state").await?;
        let document = Document::load(&serde_json::from_value::<Vec<u8>>(value["bytes"].clone())?)?;
        assert_eq!(serde_json::to_value(document.view()?)?, value["document"]);
        Ok((value, document))
    }
}

const CONTENT: &str = "See TARGET here.\n\nGuard A.\n\nGit line.\n\nGuard B.\n\nFUSE line.\n\nGuard C.\n\nAPI line.\n\nSee TARGET here.\n";

#[tokio::test]
async fn unversioned_git_import_cannot_overwrite_concurrent_http_edits() -> anyhow::Result<()> {
    unversioned_file_scenario(true).await
}

#[tokio::test]
async fn unversioned_fuse_replacement_cannot_overwrite_concurrent_http_edits() -> anyhow::Result<()>
{
    unversioned_file_scenario(false).await
}

async fn unversioned_file_scenario(git: bool) -> anyhow::Result<()> {
    let f = Fixture::new(CONTENT).await?;
    let (state, base) = f.state().await?;
    let block = &base.blocks()?[0].id;
    let incoming = CONTENT.replace("API line.", "Incoming file edit.");
    let worktree = tempfile::tempdir()?;
    // Plain import has no recorded peer base. The original file remains
    // available if publication is rejected.
    std::fs::write(worktree.path().join("doc.md"), &incoming)?;
    let projection = FuseProjection::open(f.store.clone(), "mixed", false).await?;
    let handle = projection.create_file("doc.md.tmp").await?;
    projection
        .write_handle(handle, 0, incoming.as_bytes())
        .await?;
    projection.release_handle(handle).await?;
    f.post(
        "transactions",
        &json!({"client_tx_id":"before-unversioned-file", "base_clock":state["document_clock"],
        "actor":{"kind":"agent"}, "ops":[
            {"op":"replace_block_content","block_id":block,"text":"Browser See TARGET here."},
            {"op":"comment.add","block_id":block,"start":12,"end":18,"body":"Keep this discussion"}
        ]}),
    )
    .await?;
    let before = f.state().await?.0;
    let result = if git {
        quarry_git::import_worktree(&f.store, "mixed", worktree.path())
            .await
            .map(|_| ())
    } else {
        projection.rename("doc.md.tmp", "doc.md").await
    };
    anyhow::ensure!(
        result.is_err(),
        "unversioned whole-file replacement must require a read version"
    );
    assert_eq!(f.state().await?.0, before);
    assert_eq!(
        std::fs::read_to_string(worktree.path().join("doc.md"))?,
        incoming
    );
    assert_eq!(
        f.store.get_document("mixed", "doc.md.tmp").await?.content,
        incoming.as_bytes()
    );
    Ok(())
}

/// Run explicitly in Linux with /dev/fuse and permission to mount. Ordinary
/// projection tests cannot establish the behavior of kernel file operations.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires /dev/fuse and mount permission; run the privileged Linux FUSE job"]
async fn kernel_mounted_file_writes_merge_with_git_and_http_and_follow_renames()
-> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    let f = Fixture::new(CONTENT).await?;
    let mountpoint = tempfile::tempdir()?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let mounted = {
        let store = f.store.clone();
        let path = mountpoint.path().to_path_buf();
        tokio::spawn(async move {
            quarry_fuse::mount_library_with_shutdown(store, "mixed", &path, false, async {
                let _ = stopped.await;
            })
            .await
        })
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        Box::pin(async {
            let path = mountpoint.path().join("doc.md");
            while !tokio::fs::try_exists(&path).await? {
                anyhow::ensure!(
                    !mounted.is_finished(),
                    "FUSE mount stopped before becoming ready"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let worktree = tempfile::tempdir()?;
            let peer = f
                .store
                .create_git_peer("mixed", json!({"repo":worktree.path(),"branch":"main"}))
                .await?;
            push_peer(&f.store, "mixed", &peer.id).await?;
            let (state, base) = f.state().await?;
            let ids: Vec<_> = base.blocks()?.into_iter().map(|b| b.id).collect();
            // Open before the other interfaces change the document. Use actual
            // kernel read, truncate, write, fsync, close and rename operations.
            let mut file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .await?;
            let mut read = String::new();
            file.read_to_string(&mut read).await?;
            assert_eq!(read, CONTENT);
            std::fs::write(
                worktree.path().join("doc.md"),
                CONTENT.replace("Git line.", "Git line changed."),
            )?;
            let incoming = read.replace("FUSE line.", "FUSE line changed.");
            let api = json!({"client_tx_id":"kernel-http", "base_clock":state["document_clock"],
            "actor":{"kind":"agent"},"ops":[
                {"op":"replace_block_content","block_id":ids[6],"text":"API line changed."},
                {"op":"comment.add","block_id":ids[0],"start":4,"end":10,"body":"Kernel TARGET"}
            ]});
            let writing = async {
                file.set_len(0).await?;
                file.seek(std::io::SeekFrom::Start(0)).await?;
                file.write_all(incoming.as_bytes()).await?;
                file.flush().await?;
                file.sync_all().await?;
                anyhow::Ok(())
            };
            let (fuse, git, http) = tokio::join!(
                writing,
                sync_peer(&f.store, "mixed", &peer.id),
                f.post("transactions", &api)
            );
            fuse?;
            git?;
            http?;
            drop(file);
            let (_, saved) = f.state().await?;
            for (id, expected) in [
                (&ids[2], "Git line changed."),
                (&ids[4], "FUSE line changed."),
                (&ids[6], "API line changed."),
            ] {
                assert_eq!(saved.block_view(id)?.text, expected);
            }
            let comment = &saved.comments()?[0];
            assert_eq!(
                saved.comment_target(&comment.id)?.attachments[0].quote,
                "TARGET"
            );
            let expected = CONTENT
                .replace("Git line.", "Git line changed.")
                .replace("FUSE line.", "FUSE line changed.")
                .replace("API line.", "API line changed.");
            let visible = tokio::fs::read_to_string(&path).await?;
            assert!(
                visible.starts_with(&expected),
                "kernel reads must include committed edits: {visible}"
            );
            let mut held = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .await?;
            f.post("move", &json!({"to_path":"renamed.md"})).await?;
            held.write_all(b"New").await?;
            held.flush().await?;
            held.sync_all().await?;
            drop(held);
            let renamed = mountpoint.path().join("renamed.md");
            assert!(tokio::fs::read(&renamed).await?.starts_with(b"New"));
            assert!(!tokio::fs::try_exists(&path).await?);
            let final_doc = f.store.get_document("mixed", "renamed.md").await?;
            assert_eq!(final_doc.id.as_str(), base.id()?);
            anyhow::Ok(())
        }),
    )
    .await
    .context("kernel FUSE scenario timed out")
    .and_then(|r| r);
    let _ = stop.send(());
    let unmounted = tokio::time::timeout(std::time::Duration::from_secs(10), mounted)
        .await
        .context("FUSE unmount timed out")??;
    unmounted?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_fuse_handles_follow_http_renames_and_never_write_into_replacement_documents()
-> anyhow::Result<()> {
    for deleted in [false, true] {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let f = Fixture::new(CONTENT).await?;
            let original = f.store.get_document("mixed", "doc.md").await?;
            let projection = FuseProjection::open(f.store.clone(), "mixed", false).await?;
            let handle = projection.open_file_for_write("doc.md").await?;
            projection.write_handle(handle, 0, b"FUSE").await?;
            if deleted {
                f.client.delete(&f.url).send().await?.error_for_status()?;
            } else {
                f.post("move", &json!({"to_path":"renamed.md"})).await?;
            }
            // Reuse the old pathname for a different document. A saved file
            // handle must continue to identify the original, never this one.
            let replacement: Value = f
                .client
                .put(&f.url)
                .header("content-type", "text/markdown")
                .header("if-none-match", "*")
                .body("Replacement.\n")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let replacement_id = replacement["document"]["id"].as_str().unwrap();
            anyhow::ensure!(
                replacement_id != original.id.as_str(),
                "replacement reused deleted identity"
            );
            let flushed = projection.flush_handle(handle).await;
            if deleted {
                anyhow::ensure!(
                    flushed.is_err(),
                    "a deleted handle must not write into the replacement"
                );
                assert!(
                    projection
                        .read_handle(handle, 0, 64)
                        .await?
                        .starts_with(b"FUSE")
                );
                // Failed close must keep the unsaved buffer available for recovery.
                assert!(projection.release_handle(handle).await.is_err());
                assert!(
                    projection
                        .read_handle(handle, 0, 64)
                        .await?
                        .starts_with(b"FUSE")
                );
            } else {
                flushed?;
                projection.release_handle(handle).await?;
                let moved = f.store.get_document("mixed", "renamed.md").await?;
                assert_eq!(moved.id, original.id);
                assert!(moved.content.starts_with(b"FUSE"));
            }
            let untouched = f.store.get_document("mixed", "doc.md").await?;
            assert_eq!(untouched.id.as_str(), replacement_id);
            assert_eq!(untouched.content, b"Replacement.\n");
            anyhow::Ok(())
        })
        .await
        .context("FUSE lifecycle scenario timed out")??;
    }
    Ok(())
}

struct PausedFileWriter {
    inner: Arc<dyn quarry_storage::BlockMarkdownWriter>,
    first: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl quarry_storage::BlockMarkdownWriter for PausedFileWriter {
    fn write_markdown(
        &self,
        write: quarry_storage::BlockMarkdownWrite,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = quarry_core::Result<quarry_storage::BlockMarkdownWriteOutcome>,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            if write.surface == "fuse"
                && self.first.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.entered.notify_one();
                self.resume.notified().await;
            }
            self.inner.write_markdown(write).await
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fuse_write_during_an_unfinished_flush_survives_a_concurrent_http_edit()
-> anyhow::Result<()> {
    for close_during_flush in [false, true] {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            fuse_buffer_scenario(close_during_flush),
        )
        .await
        .context("FUSE flush/write scenario timed out")??;
    }
    Ok(())
}

async fn fuse_buffer_scenario(close_during_flush: bool) -> anyhow::Result<()> {
    let f = Fixture::new(CONTENT).await?;
    let (state, base) = f.state().await?;
    let ids: Vec<_> = base.blocks()?.into_iter().map(|b| b.id).collect();
    let paused = Arc::new(PausedFileWriter {
        inner: f.writer.clone(),
        first: std::sync::atomic::AtomicBool::new(true),
        entered: Default::default(),
        resume: Default::default(),
    });
    let writer: Arc<dyn quarry_storage::BlockMarkdownWriter> = paused.clone();
    f.store.set_block_markdown_writer(&writer);
    let projection = FuseProjection::open(f.store.clone(), "mixed", false).await?;
    let handle = projection.open_file_for_write("doc.md").await?;
    projection.set_handle_len(handle, 0).await?;
    projection
        .write_handle(handle, 0, format!("First {CONTENT}").as_bytes())
        .await?;
    let flushing = {
        let projection = projection.clone();
        tokio::spawn(async move { projection.flush_handle(handle).await })
    };
    paused.entered.notified().await;
    f.post("transactions", &json!({"client_tx_id":"intervening-http","base_clock":state["document_clock"],"actor":{"kind":"agent"},"ops":[
            {"op":"replace_block_content","block_id":ids[6],"text":"API line changed."},
            {"op":"comment.add","block_id":ids[0],"start":4,"end":10,"body":"Keep the target"}
        ]})).await?;
    // These bytes arrive after the flush has captured its input. Its
    // eventual acknowledgement must not mark this newer buffer as saved.
    projection.set_handle_len(handle, 0).await?;
    projection
        .write_handle(handle, 0, format!("Second {CONTENT}").as_bytes())
        .await?;
    let mut closing = close_during_flush.then(|| Box::pin(projection.release_handle(handle)));
    if let Some(close) = &mut closing {
        // Poll close while the first publication is held. It must wait,
        // keeping the newer buffer available until that publication ends.
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(close.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(
            projection
                .read_handle(handle, 0, 64)
                .await?
                .starts_with(b"Second ")
        );
    }
    paused.resume.notify_one();
    flushing.await??;
    if let Some(close) = closing {
        close.await?;
    } else {
        projection.flush_handle(handle).await?;
        projection.release_handle(handle).await?;
    }
    let (_, saved) = f.state().await?;
    assert_eq!(saved.block_view(&ids[0])?.text, "Second See TARGET here.");
    assert_eq!(saved.block_view(&ids[6])?.text, "API line changed.");
    let comments = saved.comments()?;
    assert_eq!(comments.len(), 1);
    let target = saved.comment_target(&comments[0].id)?;
    assert_eq!(target.attachments[0].quote, "TARGET");
    assert_eq!(target.attachments[0].start, 11);
    Ok(())
}

/// The network server, Git worktree synchronizer and FUSE publication all
/// write the same authority. The six orders force stale bases; the parallel
/// runs exercise requests in flight together. This does not mount kernel FUSE.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_fuse_and_http_writes_preserve_late_native_typing_comments_and_atomic_reads()
-> anyhow::Result<()> {
    for order in [
        Some([0, 1, 2]),
        Some([0, 2, 1]),
        Some([1, 0, 2]),
        Some([1, 2, 0]),
        Some([2, 0, 1]),
        Some([2, 1, 0]),
    ]
    .into_iter()
    .chain(std::iter::repeat_n(None, 20))
    {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            Box::pin(mixed_scenario(order)),
        )
        .await
        .with_context(|| format!("mixed interface scenario timed out: {order:?}"))??;
    }
    Ok(())
}

async fn mixed_scenario(order: Option<[u8; 3]>) -> anyhow::Result<()> {
    let f = Fixture::new(CONTENT).await?;
    let worktree = tempfile::tempdir()?;
    let peer = f
        .store
        .create_git_peer("mixed", json!({"repo":worktree.path(),"branch":"main"}))
        .await?;
    push_peer(&f.store, "mixed", &peer.id).await?;
    let (state, base) = f.state().await?;
    let blocks = base.blocks()?;
    let first = &blocks[0].id;
    let ids: Vec<_> = blocks.iter().map(|b| b.id.clone()).collect();
    let projection = FuseProjection::open(f.store.clone(), "mixed", false).await?;
    let handle = projection.open_file_for_write("doc.md").await?;
    let file = worktree.path().join("doc.md");
    std::fs::write(
        &file,
        std::fs::read_to_string(&file)?.replace("Git line.", "Git line changed."),
    )?;
    let fuse_text = CONTENT.replace("FUSE line.", "FUSE line changed.");
    projection.set_handle_len(handle, 0).await?;
    projection
        .write_handle(handle, 0, fuse_text.as_bytes())
        .await?;
    let api = json!({"client_tx_id":"api-edit","base_clock":state["document_clock"],"actor":{"kind":"agent"},
            "ops":[{"op":"replace_block_content","block_id":ids[6],"text":"API line changed."}]});
    let typing = json!({"request_id":"browser-typing","actor":{"kind":"browser","id":"browser"},"requests":[CommandRequest {
        request_id:"browser-typing".into(), base:base.heads().iter().map(ToString::to_string).collect(), at:"2026-09-05T12:00:00Z".into(),
        commands:vec![Command::InsertText { at:base.point(first,0)?,text:"See TARGET ".into() }],
    }]});
    let comment = json!({"client_tx_id":"late-comment","base_clock":state["document_clock"],"actor":{"kind":"agent"},
            "ops":[{"op":"comment.add","block_id":first,"start":4,"end":10,"quote":"TARGET","body":"Original first occurrence"}]});
    // All external writers retain the version from before the browser edit.
    f.post("document-commands", &typing).await?;
    let publish = async |surface| {
        match surface {
            0 => {
                sync_peer(&f.store, "mixed", &peer.id).await?;
            }
            1 => {
                projection.flush_handle(handle).await?;
            }
            _ => {
                f.post("transactions", &api).await?;
            }
        }
        anyhow::Ok(())
    };
    if let Some(order) = order {
        for surface in order {
            publish(surface).await?;
        }
    } else {
        let read = async {
            for _ in 0..12 {
                f.state().await?;
                tokio::task::yield_now().await;
            }
            anyhow::Ok(())
        };
        let (git, fuse, http, reads) = tokio::join!(publish(0), publish(1), publish(2), read);
        git?;
        fuse?;
        http?;
        reads?;
    }
    let receipt = f.post("transactions", &comment).await?;
    let (saved_state, saved) = f.state().await?;
    assert_eq!(
        saved
            .blocks()?
            .iter()
            .map(|b| b.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(saved.block_view(first)?.text, "See TARGET See TARGET here.");
    for (id, text) in [
        (&ids[2], "Git line changed."),
        (&ids[4], "FUSE line changed."),
        (&ids[6], "API line changed."),
        (&ids[7], "See TARGET here."),
    ] {
        assert_eq!(saved.block_view(id)?.text, text);
    }
    assert!(saved.conflicts()?.is_empty());
    let comments = saved.comments()?;
    assert_eq!(comments.len(), 1);
    let target = saved.comment_target(&comments[0].id)?;
    assert_eq!(target.attachments.len(), 1);
    assert_eq!(target.attachments[0].start, 15);
    assert_eq!(target.attachments[0].end, 21);
    assert_eq!(target.attachments[0].quote, "TARGET");
    let durable = f
        .store
        .durable_document_for_scope(&DocumentScopeRef::library("mixed"), "doc.md")
        .await?
        .context("durable state")?;
    assert_eq!(Document::load(&durable.bytes)?.view()?, saved.view()?);
    assert_eq!(f.post("transactions", &comment).await?, receipt);
    projection.flush_handle(handle).await?;
    projection.release_handle(handle).await?;
    assert_eq!(
        f.state().await?.0["document_clock"],
        saved_state["document_clock"]
    );
    // A second Git sync can advance its saved base, but must not resurrect
    // stale file text, duplicate review, or lose another interface's edit.
    sync_peer(&f.store, "mixed", &peer.id).await?;
    assert_eq!(f.state().await?.1.view()?.blocks, saved.view()?.blocks);
    assert_eq!(f.state().await?.1.comment_target(&comments[0].id)?, target);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_git_and_fuse_conflicts_retain_both_incoming_edits_and_http_review()
-> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(20), Box::pin(async {
        let f = Fixture::new(CONTENT).await?;
        let worktree = tempfile::tempdir()?;
        let peer = f.store.create_git_peer("mixed", json!({"repo":worktree.path(),"branch":"main"})).await?;
        push_peer(&f.store, "mixed", &peer.id).await?;
        let (state, base) = f.state().await?;
        let blocks = base.blocks()?;
        let projection = FuseProjection::open(f.store.clone(), "mixed", false).await?;
        let handle = projection.open_file_for_write("doc.md").await?;
        let file = worktree.path().join("doc.md");
        std::fs::write(&file, std::fs::read_to_string(&file)?.replace("API line.", "Git conflicting.").replace("Git line.", "Git clean."))?;
        projection.set_handle_len(handle, 0).await?;
        projection.write_handle(handle, 0, CONTENT.replace("API line.", "FUSE conflicting.").replace("FUSE line.", "FUSE clean.").as_bytes()).await?;
        let api = json!({"client_tx_id":"api-edit-and-review","base_clock":state["document_clock"],"actor":{"kind":"agent"},"ops":[
            {"op":"replace_block_content","block_id":blocks[6].id,"text":"API line changed."},
            {"op":"comment.add","block_id":blocks[0].id,"start":4,"end":10,"body":"Keep the original target"}
        ]});
        let receipt = f.post("transactions", &api).await?;
        let (git, fuse) = tokio::join!(sync_peer(&f.store, "mixed", &peer.id), projection.flush_handle(handle));
        git?; fuse?;
        let (_, saved) = f.state().await?;
        assert_eq!(saved.block_view(&blocks[2].id)?.text, "Git clean.");
        assert_eq!(saved.block_view(&blocks[4].id)?.text, "FUSE clean.");
        assert_eq!(saved.block_view(&blocks[6].id)?.text, "API line changed.");
        let review = f.get("review").await?;
        let conflicts = review["conflicts"].as_array().context("conflicts")?;
        assert_eq!(conflicts.len(), 2);
        let incoming: std::collections::BTreeSet<_> = conflicts.iter().map(|c| c["incomingMarkdown"].as_str().unwrap()).collect();
        assert_eq!(incoming, std::collections::BTreeSet::from(["Git conflicting.\n", "FUSE conflicting.\n"]));
        assert!(conflicts.iter().all(|c| c["canonicalMarkdown"] == "API line changed.\n"));
        assert_eq!(review["comments"][0]["target"]["attachments"][0]["quote"], "TARGET");
        assert_eq!(f.post("transactions", &api).await?, receipt);
        // A reviewer decides one conflict through HTTP while the other remains.
        f.post("transactions", &json!({"client_tx_id":"keep-current","base_clock":review["baseToken"],"actor":{"kind":"agent"},
            "ops":[{"op":"conflict.keep_canonical","item_id":conflicts[0]["id"]}]})).await?;
        assert_eq!(f.get("review").await?["conflicts"].as_array().unwrap().len(), 1);
        projection.release_handle(handle).await?;
        assert_eq!(f.state().await?.1.block_view(&blocks[6].id)?.text, "API line changed.");
        anyhow::Ok(())
    })).await.context("simultaneous conflict scenario timed out")?
}
