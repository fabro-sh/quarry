use quarry_core::{DocumentSource, WritePrecondition};
use quarry_fuse::{FuseNodeKind, FuseProjection};
use quarry_storage::{QuarryStore, StoreConfig};

async fn test_store() -> QuarryStore {
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
async fn projection_lists_virtual_directories_and_reads_committed_documents() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(
            &library.slug,
            "plans/one.md",
            b"one\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "plans/two.md",
            b"two\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let projection = FuseProjection::open(store.clone(), &library.slug, true)
        .await
        .unwrap();

    let root_entries = projection.list_dir("").await.unwrap();
    assert_eq!(root_entries.len(), 1);
    assert_eq!(root_entries[0].name, "plans");
    assert_eq!(root_entries[0].kind, FuseNodeKind::Directory);

    let plan_entries = projection.list_dir("plans").await.unwrap();
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
        projection.read_file("plans/one.md", 1, 2).await.unwrap(),
        b"ne"
    );
}

#[tokio::test]
async fn projection_coalesces_writes_and_auto_commits_on_release() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    projection.mkdir("drafts").await.unwrap();
    let handle = projection.create_file("drafts/new.md").await.unwrap();
    projection.write_handle(handle, 0, b"hello").await.unwrap();
    projection.write_handle(handle, 5, b"\n").await.unwrap();
    projection.release_handle(handle).await.unwrap();

    let document = store
        .get_document(&library.slug, "drafts/new.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"hello\n");
    assert_eq!(document.version.content_type, "text/markdown");
}

#[tokio::test]
async fn projection_truncate_open_replaces_existing_content_on_release() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/existing.md",
            b"old content that should be removed".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();

    let handle = projection
        .open_file_for_write_truncating("drafts/existing.md")
        .await
        .unwrap();
    projection.write_handle(handle, 0, b"new").await.unwrap();
    projection.release_handle(handle).await.unwrap();

    let document = store
        .get_document(&library.slug, "drafts/existing.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"new");
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
        .put_document(
            &library.slug,
            "drafts/old.md",
            b"old\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
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

    assert!(store
        .get_document(&library.slug, "drafts/new.md")
        .await
        .is_err());
    assert!(projection.list_dir("").await.unwrap().is_empty());
}

#[tokio::test]
async fn projection_rename_file_over_existing_file_replaces_target() {
    let store = test_store().await;
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/current.md",
            b"old\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/.current.md.tmp",
            b"new\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let projection = FuseProjection::open(store.clone(), &library.slug, false)
        .await
        .unwrap();
    let source_inode = projection
        .attr("drafts/.current.md.tmp")
        .await
        .unwrap()
        .inode;

    projection
        .rename("drafts/.current.md.tmp", "drafts/current.md")
        .await
        .unwrap();

    let document = store
        .get_document(&library.slug, "drafts/current.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"new\n");
    assert_eq!(
        projection.attr("drafts/current.md").await.unwrap().inode,
        source_inode
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
        .put_document(
            &library.slug,
            "plans/event.md",
            b"event\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
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
        .put_document(
            &library.slug,
            "plans/one.md",
            b"one\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
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
        .put_document(
            &library.slug,
            "plans/one.md",
            b"one\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
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
        .put_document(
            &library.slug,
            "drafts/one.md",
            b"one\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
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
