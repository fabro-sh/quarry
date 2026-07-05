#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for filesystem fixtures"
)]

use anyhow::Context as _;
use quarry_core::{DocumentSource, QuarryError, WritePrecondition};
use quarry_fuse::{FuseNodeKind, FuseProjection};
use quarry_storage::{QuarryStore, StoreConfig};

async fn test_store() -> QuarryStore {
    let store = bare_store().await;
    // Phase 4: Markdown writes route through the reconciling writer the
    // owning (serving) process installs; these tests play that process and
    // leak the keepalive handle for the test's lifetime.
    let state = quarry_server::app_state(store.clone());
    std::mem::forget(quarry_server::install_markdown_writer(&state));
    store
}

async fn bare_store() -> QuarryStore {
    let root = tempfile::tempdir().unwrap().keep();
    QuarryStore::open(StoreConfig {
        db_path: root.join("quarry.db"),
        cas_path: root.join("cas"),
        lock_path: None,
    })
    .await
    .unwrap()
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn mount_library_with_shutdown_reports_unsupported_on_non_linux() {
    let store = test_store().await;
    let error = quarry_fuse::mount_library_with_shutdown(
        store,
        "notes",
        std::path::Path::new("/tmp/quarry-mount"),
        true,
        async {},
    )
    .await
    .unwrap_err();

    assert!(matches!(error, quarry_core::QuarryError::Unsupported(_)));
}

#[tokio::test]
async fn projection_lists_virtual_directories_and_reads_committed_documents() -> anyhow::Result<()>
{
    let store = test_store().await;
    let library = store
        .create_library("notes")
        .await
        .context("create notes library")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("plans/one.md").to_string(),
            content: b"one\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write first plan document")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("plans/two.md").to_string(),
            content: b"two\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("write second plan document")?;

    let projection = FuseProjection::open(store.clone(), &library.slug, true)
        .await
        .context("open read-only projection")?;

    let root_entries = projection
        .list_dir("")
        .await
        .context("list projection root")?;
    assert_eq!(root_entries.len(), 1);
    assert_eq!(root_entries[0].name, "plans");
    assert_eq!(root_entries[0].kind, FuseNodeKind::Directory);

    let plan_entries = projection
        .list_dir("plans")
        .await
        .context("list plans directory")?;
    assert_eq!(
        plan_entries
            .iter()
            .map(|entry| (&entry.name, entry.kind.clone()))
            .collect::<Vec<_>>(),
        vec![
            (&"one.md".to_string(), FuseNodeKind::File),
            (&"two.md".to_string(), FuseNodeKind::File),
        ]
    );
    assert_eq!(
        projection
            .read_file("plans/one.md", 1, 2)
            .await
            .context("read projection file slice")?,
        b"ne"
    );
    Ok(())
}

#[tokio::test]
async fn projection_coalesces_writes_and_auto_commits_on_release() -> anyhow::Result<()> {
    let store = test_store().await;
    let library = store
        .create_library("notes")
        .await
        .context("create notes library")?;
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .context("open writable projection")?;

    projection
        .mkdir("drafts")
        .await
        .context("create drafts directory")?;
    let handle = projection
        .create_file("drafts/new.md")
        .await
        .context("create draft file")?;
    projection
        .write_handle(handle, 0, b"hello")
        .await
        .context("write draft body")?;
    projection
        .write_handle(handle, 5, b"\n")
        .await
        .context("append draft newline")?;
    projection
        .release_handle(handle)
        .await
        .context("release draft file handle")?;

    let document = store
        .get_document(&library.slug, "drafts/new.md")
        .await
        .context("load committed draft document")?;
    assert_eq!(document.content, b"hello\n");
    assert_eq!(document.version.content_type, "text/markdown");
    Ok(())
}

#[tokio::test]
async fn projection_unlink_then_create_same_path_allocates_new_file_inode() -> anyhow::Result<()> {
    let store = test_store().await;
    let library = store
        .create_library("notes")
        .await
        .context("create notes library")?;
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .context("open writable projection")?;

    projection
        .mkdir("drafts")
        .await
        .context("create drafts directory")?;
    let first_handle = projection
        .create_file("drafts/reused.md")
        .await
        .context("create first reused file")?;
    projection
        .release_handle(first_handle)
        .await
        .context("release first reused file")?;
    let first_inode = projection
        .attr("drafts/reused.md")
        .await
        .context("stat first reused file")?
        .inode;

    projection
        .unlink("drafts/reused.md")
        .await
        .context("unlink first reused file")?;
    assert!(projection.attr("drafts/reused.md").await.is_err());

    let second_handle = projection
        .create_file("drafts/reused.md")
        .await
        .context("create second reused file")?;
    projection
        .release_handle(second_handle)
        .await
        .context("release second reused file")?;
    let second_inode = projection
        .attr("drafts/reused.md")
        .await
        .context("stat second reused file")?
        .inode;

    assert_ne!(first_inode, second_inode);
    Ok(())
}

#[tokio::test]
async fn projection_truncate_open_replaces_existing_content_on_release() -> anyhow::Result<()> {
    let store = test_store().await;
    let library = store
        .create_library("notes")
        .await
        .context("create notes library")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/existing.md").to_string(),
            content: b"old content that should be removed".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("seed existing draft")?;
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .context("open writable projection")?;

    let handle = projection
        .open_file_for_write_truncating("drafts/existing.md")
        .await
        .context("open existing draft for truncating write")?;
    projection
        .write_handle(handle, 0, b"new")
        .await
        .context("write replacement content")?;
    projection
        .release_handle(handle)
        .await
        .context("release truncating write handle")?;

    let document = store
        .get_document(&library.slug, "drafts/existing.md")
        .await
        .context("load rewritten draft")?;
    // Markdown lands via the Phase 4 reconciled write: deterministic
    // normalized export (trailing newline), not raw bytes.
    assert_eq!(document.content, b"new\n");
    Ok(())
}

#[tokio::test]
async fn projection_rejects_writes_when_read_only() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store, &library.slug, true)
        .await
        .unwrap();

    let error = projection.mkdir("drafts").await.unwrap_err();

    assert!(error.to_string().contains("read-only"));
}

#[tokio::test]
async fn projection_tolerates_duplicate_flush_and_release_cleanup() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store, &library.slug, false)
        .await
        .unwrap();

    projection.mkdir("drafts").await.unwrap();
    let handle = projection.create_file("drafts/new.md").await.unwrap();
    projection.flush_handle(handle).await.unwrap();
    projection.release_handle(handle).await.unwrap();
    projection.flush_handle(handle).await.unwrap();
    projection.release_handle(handle).await.unwrap();
}

#[tokio::test]
async fn projection_keeps_handle_truncate_and_later_writes_in_one_publication() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    projection.mkdir("drafts").await.unwrap();
    let handle = projection.create_file("drafts/vim.md").await.unwrap();
    projection.set_handle_len(handle, 0).await.unwrap();
    projection
        .write_handle(handle, 0, b"from-vim\n")
        .await
        .unwrap();
    projection.release_handle(handle).await.unwrap();

    let document = store
        .get_document(&library.slug, "drafts/vim.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"from-vim\n");
}

#[tokio::test]
async fn projection_renames_unlinks_and_removes_empty_directories() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/old.md").to_string(),
            content: b"old\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    projection
        .rename("drafts/old.md", "drafts/new.md")
        .await
        .unwrap();
    projection.unlink("drafts/new.md").await.unwrap();
    projection.rmdir("drafts").await.unwrap();

    assert!(
        store
            .get_document(&library.slug, "drafts/new.md")
            .await
            .is_err()
    );
    assert!(projection.list_dir("").await.unwrap().is_empty());
}

#[tokio::test]
async fn projection_rename_file_over_existing_file_replaces_target() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/current.md").to_string(),
            content: b"old\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/.current.md.tmp").to_string(),
            content: b"new\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();
    let target_id = store
        .head_document(&library.slug, "drafts/current.md")
        .await
        .unwrap()
        .id;
    let target_inode = projection.attr("drafts/current.md").await.unwrap().inode;

    projection
        .rename("drafts/.current.md.tmp", "drafts/current.md")
        .await
        .unwrap();

    // Phase 4: renaming over a markdown document is a whole-file write to
    // the TARGET — its identity (document id, inode) survives and the temp
    // file's content lands through the reconciler.
    let document = store
        .get_document(&library.slug, "drafts/current.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"new\n");
    assert_eq!(document.id, target_id);
    assert_eq!(
        projection.attr("drafts/current.md").await.unwrap().inode,
        target_inode
    );
    assert!(projection.attr("drafts/.current.md.tmp").await.is_err());
}

#[tokio::test]
async fn projection_empty_directories_survive_reopening() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    projection.mkdir("drafts").await.unwrap();

    let reopened = FuseProjection::open(store, &library.slug, false)
        .await
        .unwrap();
    let root_entries = reopened.list_dir("").await.unwrap();
    assert_eq!(root_entries.len(), 1);
    assert_eq!(root_entries[0].name, "drafts");
    assert_eq!(root_entries[0].kind, FuseNodeKind::Directory);
}

#[tokio::test]
async fn projection_updates_directory_metadata_and_preserves_it_on_reopen() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    projection.mkdir_with_mode("drafts", 0o750).await.unwrap();
    assert_eq!(projection.attr("drafts").await.unwrap().mode, Some(0o750));

    projection
        .set_directory_metadata("drafts", Some(0o700), Some("2026-05-28T06:00:00.000Z"))
        .await
        .unwrap();

    let attr = projection.attr("drafts").await.unwrap();
    assert_eq!(attr.mode, Some(0o700));
    assert_eq!(attr.mtime.as_deref(), Some("2026-05-28T06:00:00.000Z"));

    let reopened = FuseProjection::open(store, &library.slug, false)
        .await
        .unwrap();
    let attr = reopened.attr("drafts").await.unwrap();
    assert_eq!(attr.mode, Some(0o700));
    assert_eq!(attr.mtime.as_deref(), Some("2026-05-28T06:00:00.000Z"));
}

#[tokio::test]
async fn projection_observes_store_events_for_cache_invalidation() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, true)
        .await
        .unwrap();
    let before = projection.invalidation_generation();

    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("plans/event.md").to_string(),
            content: b"event\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        projection.wait_for_invalidation_after(before),
    )
    .await
    .unwrap();
    assert!(projection.invalidation_generation() > before);
}

#[tokio::test]
async fn projection_uses_storage_backed_stable_inodes() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("plans/one.md").to_string(),
            content: b"one\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, true)
        .await
        .unwrap();
    let dir_inode = projection.attr("plans").await.unwrap().inode;
    let file_inode = projection.attr("plans/one.md").await.unwrap().inode;

    let reopened = FuseProjection::open(store, &library.slug, true)
        .await
        .unwrap();

    assert_ne!(dir_inode, file_inode);
    assert_eq!(reopened.attr("plans").await.unwrap().inode, dir_inode);
    assert_eq!(
        reopened.attr("plans/one.md").await.unwrap().inode,
        file_inode
    );
}

#[tokio::test]
async fn projection_preserves_file_inode_across_rename() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("plans/one.md").to_string(),
            content: b"one\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let projection = FuseProjection::open(store, &library.slug, false)
        .await
        .unwrap();
    let inode = projection.attr("plans/one.md").await.unwrap().inode;

    projection
        .rename("plans/one.md", "plans/two.md")
        .await
        .unwrap();

    assert_eq!(projection.attr("plans/two.md").await.unwrap().inode, inode);
    assert!(projection.attr("plans/one.md").await.is_err());
}

#[tokio::test]
async fn projection_preserves_empty_directory_inode_across_rename_and_reopen() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    projection.mkdir("drafts").await.unwrap();
    let inode = projection.attr("drafts").await.unwrap().inode;
    projection.rename("drafts", "archive").await.unwrap();

    assert_eq!(projection.attr("archive").await.unwrap().inode, inode);
    assert!(projection.attr("drafts").await.is_err());

    let reopened = FuseProjection::open(store, &library.slug, false)
        .await
        .unwrap();
    assert_eq!(reopened.attr("archive").await.unwrap().inode, inode);
    assert!(reopened.attr("drafts").await.is_err());
}

#[tokio::test]
async fn projection_preserves_tree_inodes_across_directory_rename_and_reopen() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/one.md").to_string(),
            content: b"one\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();
    let dir_inode = projection.attr("drafts").await.unwrap().inode;
    let file_inode = projection.attr("drafts/one.md").await.unwrap().inode;

    projection.rename("drafts", "archive").await.unwrap();

    assert_eq!(projection.attr("archive").await.unwrap().inode, dir_inode);
    assert_eq!(
        projection.attr("archive/one.md").await.unwrap().inode,
        file_inode
    );
    assert_eq!(
        projection.read_file("archive/one.md", 0, 16).await.unwrap(),
        b"one\n"
    );

    let reopened = FuseProjection::open(store, &library.slug, false)
        .await
        .unwrap();
    assert_eq!(reopened.attr("archive").await.unwrap().inode, dir_inode);
    assert_eq!(
        reopened.attr("archive/one.md").await.unwrap().inode,
        file_inode
    );
}

// ---------------------------------------------------------------------------
// Phase 4: whole-file Markdown writes reconcile via diff3.
// ---------------------------------------------------------------------------

async fn import_markdown(
    store: &QuarryStore,
    library: &str,
    path: &str,
    markdown: &str,
) -> anyhow::Result<String> {
    let outcome = store
        .import_block_document(
            library,
            path,
            markdown,
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await?;
    Ok(outcome.document.id.to_string())
}

async fn overwrite_through_handle(
    projection: &FuseProjection,
    path: &str,
    content: &str,
) -> quarry_core::Result<()> {
    let handle = projection.open_file_for_write(path).await?;
    projection.set_handle_len(handle, 0).await?;
    projection
        .write_handle(handle, 0, content.as_bytes())
        .await?;
    projection.release_handle(handle).await
}

fn top_level_ids(rows: &[quarry_storage::BlockRow]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.parent_block_id.is_none())
        .map(|row| row.block_id.clone())
        .collect()
}

/// Editing one block through a FUSE handle preserves the sibling block ids
/// and live review anchors — the file write reconciles instead of replacing
/// the projection.
#[tokio::test]
async fn markdown_write_preserves_sibling_block_ids_and_live_anchors() -> anyhow::Result<()> {
    let store = test_store().await;
    let library = store.create_library("notes").await?;
    let document_id = import_markdown(
        &store,
        &library.slug,
        "doc.md",
        "# Title\n\nAlpha.\n\nBravo.\n",
    )
    .await?;
    let ids_before = top_level_ids(&store.load_block_tree(&document_id).await?);
    let anchor = store
        .put_block_review_item(quarry_storage::NewBlockReviewItem {
            document_id: document_id.to_string(),
            block_id: ids_before[0].clone(),
            kind: quarry_storage::BlockReviewKind::Comment,
            start_offset: 0,
            end_offset: 5,
            body: Some("anchored on the title".to_string()),
            replacement: None,
            author: Some("Avery".to_string()),
            state: quarry_storage::BlockReviewState::Open,
            quote: None,
            context_before: None,
            context_after: None,
            parent_item_id: None,
        })
        .await?;

    let projection = FuseProjection::open(store.clone(), &library.slug, false).await?;
    let current = store.get_document(&library.slug, "doc.md").await?.content;
    let edited = String::from_utf8(current)
        .context("stored markdown should be valid UTF-8")?
        .replace("Bravo.", "Bravo, edited over FUSE.");
    overwrite_through_handle(&projection, "doc.md", &edited).await?;

    let rows = store.load_block_tree(&document_id).await?;
    assert_eq!(top_level_ids(&rows), ids_before, "sibling ids survive");
    assert_eq!(
        rows.iter()
            .find(|row| row.block_id == ids_before[2])
            .context("edited block should still exist")?
            .text,
        "Bravo, edited over FUSE."
    );
    let items = store.list_block_review_items(&document_id).await?;
    let kept = items
        .iter()
        .find(|item| item.id == anchor.id)
        .context("anchor review item should survive")?;
    assert_eq!(kept.state, quarry_storage::BlockReviewState::Open);
    assert_eq!(kept.block_id, ids_before[0]);
    assert_eq!((kept.start_offset, kept.end_offset), (0, 5));
    Ok(())
}

/// A canonical edit lands between `open()` and the flush: the handle's base
/// makes it a three-way merge — non-overlapping hunks both apply, the
/// overlapping hunk keeps the canonical side and surfaces as a conflict
/// review item. The flush itself NEVER fails.
#[tokio::test]
async fn concurrent_canonical_edit_and_fuse_write_converge_with_conflict_items()
-> anyhow::Result<()> {
    let store = test_store().await;
    let library = store.create_library("notes").await?;
    let document_id = import_markdown(
        &store,
        &library.slug,
        "doc.md",
        "# Title\n\nAlpha.\n\nSeparator.\n\nBravo.\n",
    )
    .await?;
    let base_export = String::from_utf8(store.get_document(&library.slug, "doc.md").await?.content)
        .context("stored markdown should be valid UTF-8")?;

    let projection = FuseProjection::open(store.clone(), &library.slug, false).await?;
    // The handle opens BEFORE the canonical edit: its base is the old text.
    let handle = projection.open_file_for_write("doc.md").await?;

    // Canonical edits Alpha (e.g. a browser/agent write).
    let head = store.head_document(&library.slug, "doc.md").await?;
    store
        .write_block_markdown(quarry_storage::BlockMarkdownWrite {
            scope: quarry_storage::DocumentScopeRef::library(&library.slug),
            path: "doc.md".to_string(),
            markdown: base_export.replace("Alpha.", "Alpha, canonical."),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            base: quarry_storage::BlockWriteBase::Markdown {
                markdown: base_export.clone(),
                version_id: Some(head.head_version_id.to_string()),
            },
            source: DocumentSource::Rest,
            surface: "rest".to_string(),
            actor_label: None,
        })
        .await?;

    // The FUSE writer edits Alpha DIFFERENTLY and Bravo (stably separated).
    let incoming = base_export
        .replace("Alpha.", "Alpha, from FUSE.")
        .replace("Bravo.", "Bravo, from FUSE.");
    projection.set_handle_len(handle, 0).await?;
    projection
        .write_handle(handle, 0, incoming.as_bytes())
        .await?;
    projection.release_handle(handle).await?;

    let merged = String::from_utf8(store.get_document(&library.slug, "doc.md").await?.content)
        .context("merged markdown should be valid UTF-8")?;
    assert_eq!(
        merged,
        "# Title\n\nAlpha, canonical.\n\nSeparator.\n\nBravo, from FUSE.\n"
    );
    let items = store.list_block_review_items(&document_id).await?;
    let conflicts: Vec<_> = items
        .iter()
        .filter(|item| item.kind == quarry_storage::BlockReviewKind::Conflict)
        .collect();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].state, quarry_storage::BlockReviewState::Open);
    assert_eq!(conflicts[0].body.as_deref(), Some("Alpha, from FUSE.\n"));
    assert_eq!(conflicts[0].quote.as_deref(), Some("Alpha, canonical.\n"));
    Ok(())
}

/// RawDocument bypass: bytes round-trip exactly and no block tables are
/// touched.
#[tokio::test]
async fn raw_document_writes_bypass_the_block_model() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    let bytes: Vec<u8> = vec![0, 159, 146, 150, 13, 10, 0];
    let handle = projection.create_file("data.bin").await.unwrap();
    projection.write_handle(handle, 0, &bytes).await.unwrap();
    projection.release_handle(handle).await.unwrap();

    let document = store.get_document(&library.slug, "data.bin").await.unwrap();
    assert_eq!(document.content, bytes);
    assert_eq!(
        store.load_block_tree(&document.id).await.unwrap(),
        Vec::<quarry_storage::BlockRow>::new()
    );
}

/// CriticMarkup is a CONTENT error (the codec rejects it outright), not a
/// reconciliation outcome: the flush fails with a typed error (errno-land
/// maps it to EIO). Merge conflicts, by contrast, never fail (see above).
#[tokio::test]
async fn critic_markup_content_is_a_write_error_not_a_silent_byte_write() -> anyhow::Result<()> {
    let store = test_store().await;
    let library = store.create_library("notes").await?;
    import_markdown(&store, &library.slug, "doc.md", "Alpha.\n").await?;
    let projection = FuseProjection::open(store.clone(), &library.slug, false).await?;

    let error = overwrite_through_handle(&projection, "doc.md", "Some {++inserted++} text.\n")
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::UnsupportedMarkdown(_)));
    // The canonical content is untouched.
    let document = store.get_document(&library.slug, "doc.md").await?;
    assert_eq!(document.content, b"Alpha.\n");
    Ok(())
}

/// A FUSE flush while a browser session is active converges THROUGH the
/// session: the write succeeds (no errno), the merge is durable
/// (checkpoint-before-ack), and the live doc broadcasts the change to the
/// connected client like any collaborator edit.
#[tokio::test]
async fn fuse_flush_during_an_active_session_converges_through_the_session() -> anyhow::Result<()> {
    use futures_util::StreamExt;

    let store = bare_store().await;
    let state = quarry_server::app_state(store.clone());
    let _writer = quarry_server::install_markdown_writer(&state);
    let library = store.create_library("notes").await?;
    let document_id = import_markdown(
        &store,
        &library.slug,
        "live.md",
        "# Title\n\nAlpha.\n\nBravo.\n",
    )
    .await?;
    let ids_before = top_level_ids(&store.load_block_tree(&document_id).await?);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = quarry_server::router_with_state(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // A connected browser opens the live session.
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/collab/{document_id}")).await?;
    // Drain the seed/sync frames until the line goes quiet.
    while let Ok(Some(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(300), socket.next()).await
    {}

    let projection = FuseProjection::open(store.clone(), &library.slug, false).await?;
    let current = String::from_utf8(store.get_document(&library.slug, "live.md").await?.content)
        .context("stored markdown should be valid UTF-8")?;
    overwrite_through_handle(
        &projection,
        "live.md",
        &current.replace("Bravo.", "Bravo, via FUSE during the session."),
    )
    .await
    .context("a flush during a session must not fail")?;

    // Durable immediately (the session-mode write checkpoints before ack)…
    let merged = String::from_utf8(store.get_document(&library.slug, "live.md").await?.content)
        .context("merged markdown should be valid UTF-8")?;
    assert_eq!(
        merged,
        "# Title\n\nAlpha.\n\nBravo, via FUSE during the session.\n"
    );
    assert_eq!(
        top_level_ids(&store.load_block_tree(&document_id).await?),
        ids_before
    );
    // …and the live doc broadcast the merge to the connected browser.
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .context("the session must broadcast the FUSE merge to subscribers")?
        .context("socket should remain open")??;
    assert!(frame.is_binary());

    socket.close(None).await.ok();
    server.abort();
    Ok(())
}

/// The editor atomic-save pattern (vim: write the buffer to a temp file,
/// rename it over the document) routes through the reconciler: the target
/// document id survives, sibling block ids and live anchors are preserved,
/// and the temp file's edit merges instead of replacing the projection.
#[tokio::test]
async fn atomic_save_rename_preserves_target_identity_block_ids_and_anchors() -> anyhow::Result<()>
{
    let store = test_store().await;
    let library = store.create_library("notes").await?;
    let document_id = import_markdown(
        &store,
        &library.slug,
        "doc.md",
        "# Title\n\nAlpha.\n\nBravo.\n",
    )
    .await?;
    let ids_before = top_level_ids(&store.load_block_tree(&document_id).await?);
    let anchor = store
        .put_block_review_item(quarry_storage::NewBlockReviewItem {
            document_id: document_id.to_string(),
            block_id: ids_before[1].clone(),
            kind: quarry_storage::BlockReviewKind::Comment,
            start_offset: 0,
            end_offset: 5,
            body: Some("anchored on alpha".to_string()),
            replacement: None,
            author: Some("Avery".to_string()),
            state: quarry_storage::BlockReviewState::Open,
            quote: None,
            context_before: None,
            context_after: None,
            parent_item_id: None,
        })
        .await?;

    let projection = FuseProjection::open(store.clone(), &library.slug, false).await?;
    // vim reads the document, edits one block, writes the buffer to a temp
    // file in the same directory…
    let current = String::from_utf8(store.get_document(&library.slug, "doc.md").await?.content)
        .context("stored markdown should be valid UTF-8")?;
    let edited = current.replace("Bravo.", "Bravo, atomically saved.");
    let handle = projection.create_file("doc.md.tmp").await?;
    projection
        .write_handle(handle, 0, edited.as_bytes())
        .await?;
    projection.release_handle(handle).await?;
    // …then renames it over the original.
    projection.rename("doc.md.tmp", "doc.md").await?;

    // The target document survived with its identity and projection intact.
    assert_eq!(
        store
            .head_document(&library.slug, "doc.md")
            .await
            .context("target document should survive")?
            .id,
        document_id
    );
    let rows = store.load_block_tree(&document_id).await?;
    assert_eq!(top_level_ids(&rows), ids_before, "sibling ids survive");
    assert_eq!(
        rows.iter()
            .find(|row| row.block_id == ids_before[2])
            .context("edited block should still exist")?
            .text,
        "Bravo, atomically saved."
    );
    let items = store.list_block_review_items(&document_id).await?;
    let kept = items
        .iter()
        .find(|item| item.id == anchor.id)
        .context("anchor review item should survive")?;
    assert_eq!(kept.state, quarry_storage::BlockReviewState::Open);
    assert_eq!(kept.block_id, ids_before[1]);
    // The temp document is gone.
    assert!(
        store
            .head_document(&library.slug, "doc.md.tmp")
            .await
            .is_err()
    );
    Ok(())
}
