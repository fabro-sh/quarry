use quarry_core::{DocumentSource, TransactionState, WritePrecondition};
use quarry_git::{export_worktree, import_worktree, push_peer, sync_peer, GitExportOptions};
use quarry_storage::{QuarryStore, StoreConfig};
use std::path::Path;

/// Opens a store with the Phase 4 reconciling markdown writer installed
/// (these tests play the owning process; the keepalive leaks for the test's
/// lifetime). Markdown writes would otherwise refuse to bypass the gateway.
async fn open_store(root: &Path) -> QuarryStore {
    let store = QuarryStore::open(StoreConfig {
        db_path: root.join("quarry.db"),
        cas_path: root.join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let state = quarry_server::app_state(store.clone());
    std::mem::forget(quarry_server::install_markdown_writer(&state));
    store
}

#[tokio::test]
async fn import_export_roundtrip_preserves_bytes_metadata_and_marker_safety() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("docs").await.unwrap();

    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source.path().join("notes")).unwrap();
    std::fs::write(
        source.path().join("notes/plan.md"),
        "---\ntitle: Plan\nrank: 1\n---\n# Plan\n",
    )
    .unwrap();
    std::fs::create_dir_all(source.path().join("assets")).unwrap();
    std::fs::write(source.path().join("assets/blob.bin"), [0, 1, 2, 3, 4]).unwrap();
    std::fs::write(
        source.path().join("assets/blob.bin.quarrymeta.yaml"),
        "content_type: application/custom\nowner: test\n",
    )
    .unwrap();

    let imported = import_worktree(&store, &library.slug, source.path())
        .await
        .unwrap();
    assert_eq!(imported.imported_paths.len(), 2);
    let plan = store
        .get_document(&library.slug, "notes/plan.md")
        .await
        .unwrap();
    // Markdown imports land via the Phase 4 reconciled write: the stored
    // content is the normalized text WITH frontmatter.
    assert_eq!(plan.content, b"---\nrank: 1\ntitle: Plan\n---\n# Plan\n");
    assert_eq!(plan.metadata["title"], "Plan");
    assert_eq!(plan.metadata["rank"], 1);
    let blob = store
        .get_document(&library.slug, "assets/blob.bin")
        .await
        .unwrap();
    assert_eq!(blob.content, vec![0, 1, 2, 3, 4]);
    assert_eq!(blob.metadata["owner"], "test");

    let output = tempfile::tempdir().unwrap();
    let exported = export_worktree(
        &store,
        &library.slug,
        output.path(),
        GitExportOptions {
            branch: "main".to_string(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await
    .unwrap();
    let commit_id = exported
        .commit_id
        .as_deref()
        .expect("export should create a commit id");
    git2::Oid::from_str(commit_id).expect("export commit id should parse as a Git OID");
    assert_eq!(
        std::fs::read_to_string(output.path().join(".quarry/marker.json")).unwrap(),
        format!(
            "{{\n  \"library_id\": \"{}\",\n  \"library_slug\": \"docs\"\n}}",
            library.id
        )
    );
    assert_eq!(
        std::fs::read_to_string(output.path().join("notes/plan.md")).unwrap(),
        "---\nrank: 1\ntitle: Plan\n---\n# Plan\n"
    );
    assert_eq!(
        std::fs::read(output.path().join("assets/blob.bin")).unwrap(),
        vec![0, 1, 2, 3, 4]
    );

    let other = store.create_library("other").await.unwrap();
    let error = export_worktree(
        &store,
        &other.slug,
        output.path(),
        GitExportOptions {
            branch: "main".to_string(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("marker"));
}

#[tokio::test]
async fn export_refuses_document_paths_reserved_for_git_sidecars() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("reservedgit").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/data.quarrymeta.yaml",
            b"not sidecar\n".to_vec(),
            serde_json::json!({"content_type":"application/x-yaml"}),
            "application/x-yaml",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let repo = tempfile::tempdir().unwrap();
    let error = export_worktree(
        &store,
        &library.slug,
        repo.path(),
        GitExportOptions {
            branch: "main".to_string(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("reserved for Git metadata"));
}

#[tokio::test]
async fn import_ignores_quarry_metadata_directory_and_orphan_sidecars() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("reservedimport").await.unwrap();

    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source.path().join(".quarry")).unwrap();
    std::fs::write(source.path().join(".quarry/marker.json"), b"reserved").unwrap();
    std::fs::write(
        source.path().join("unpaired.quarrymeta.yaml"),
        b"owner: ignored\n",
    )
    .unwrap();
    std::fs::write(source.path().join("kept.txt"), b"kept\n").unwrap();

    let imported = import_worktree(&store, &library.slug, source.path())
        .await
        .unwrap();

    assert_eq!(imported.imported_paths, vec!["kept.txt"]);
    assert_eq!(
        store
            .get_document(&library.slug, "kept.txt")
            .await
            .unwrap()
            .content,
        b"kept\n"
    );
    assert!(store
        .get_document(&library.slug, ".quarry/marker.json")
        .await
        .is_err());
    assert!(store
        .get_document(&library.slug, "unpaired.quarrymeta.yaml")
        .await
        .is_err());
}

#[tokio::test]
async fn import_preserves_case_distinct_git_paths_when_supported_by_filesystem() {
    let source = tempfile::tempdir().unwrap();
    if !filesystem_supports_case_distinct_paths(source.path()) {
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("casepaths").await.unwrap();

    std::fs::create_dir_all(source.path().join("Notes")).unwrap();
    std::fs::create_dir_all(source.path().join("notes")).unwrap();
    std::fs::write(source.path().join("Notes/Plan.md"), b"upper\n").unwrap();
    std::fs::write(source.path().join("notes/plan.md"), b"lower\n").unwrap();

    let imported = import_worktree(&store, &library.slug, source.path())
        .await
        .unwrap();

    assert_eq!(
        imported.imported_paths,
        vec!["Notes/Plan.md", "notes/plan.md"]
    );
    assert_eq!(
        store
            .get_document(&library.slug, "Notes/Plan.md")
            .await
            .unwrap()
            .content,
        b"upper\n"
    );
    assert_eq!(
        store
            .get_document(&library.slug, "notes/plan.md")
            .await
            .unwrap()
            .content,
        b"lower\n"
    );
}

#[tokio::test]
async fn failed_import_rolls_back_transaction_instead_of_leaving_it_open() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("failedimport").await.unwrap();

    let source = tempfile::tempdir().unwrap();
    // Raw files ride the staged multi-document transaction (markdown files
    // commit per document through the reconciled write and are NOT covered
    // by this rollback — a documented Phase 4 atomicity change).
    std::fs::write(source.path().join("valid.bin"), b"valid\n").unwrap();
    std::fs::write(source.path().join("bad\\path.bin"), b"bad\n").unwrap();

    let error = import_worktree(&store, &library.slug, source.path())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("bad\\path.bin"));
    let transactions = store.list_transactions(&library.slug).await.unwrap();
    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0].state, TransactionState::RolledBack);
    assert!(store
        .get_document(&library.slug, "valid.bin")
        .await
        .is_err());
}

#[tokio::test]
async fn sync_refuses_missing_or_mismatched_worktree_marker() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("markers").await.unwrap();

    let missing_marker_repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": missing_marker_repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    let error = sync_peer(&store, &library.slug, &peer.id)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("marker is missing"));

    let mismatched_marker_repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(mismatched_marker_repo.path().join(".quarry")).unwrap();
    std::fs::write(
        mismatched_marker_repo.path().join(".quarry/marker.json"),
        "{\n  \"library_id\": \"different\",\n  \"library_slug\": \"other\"\n}",
    )
    .unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": mismatched_marker_repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    let error = sync_peer(&store, &library.slug, &peer.id)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("belongs to library different"));
}

#[tokio::test]
async fn import_parses_bom_and_crlf_markdown_frontmatter() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("windows").await.unwrap();

    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source.path().join("notes")).unwrap();
    std::fs::write(
        source.path().join("notes/crlf.md"),
        b"\xEF\xBB\xBF---\r\ntitle: Windows\r\n---\r\nBody\r\n",
    )
    .unwrap();

    import_worktree(&store, &library.slug, source.path())
        .await
        .unwrap();

    let document = store
        .get_document(&library.slug, "notes/crlf.md")
        .await
        .unwrap();
    assert_eq!(document.metadata["title"], "Windows");
    // Normalized by the reconciled write: BOM and CRLF gone, frontmatter
    // embedded in the stored text.
    assert_eq!(document.content, b"---\ntitle: Windows\n---\nBody\n");
}

#[tokio::test]
async fn export_refuses_large_git_blobs_unless_forced() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("largegit").await.unwrap();
    store
        .put_document(
            &library.slug,
            "assets/large.bin",
            vec![b'x'; quarry_core::GIT_BINARY_WARN_THRESHOLD + 1],
            serde_json::json!({"content_type":"application/octet-stream"}),
            "application/octet-stream",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let repo = tempfile::tempdir().unwrap();
    let error = export_worktree(
        &store,
        &library.slug,
        repo.path(),
        GitExportOptions {
            branch: "main".to_string(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("larger than the 5 MiB"));

    let result = export_worktree(
        &store,
        &library.slug,
        repo.path(),
        GitExportOptions {
            branch: "main".to_string(),
            force_large: true,
            frontmatter_markdown: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.exported_paths, vec!["assets/large.bin"]);
}

#[tokio::test]
async fn sync_preserves_both_sides_when_quarry_and_git_change_same_path() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("syncdocs").await.unwrap();
    let baseline = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();

    let first = push_peer(&store, &library.slug, &peer.id).await.unwrap();
    assert!(first.conflicts.is_empty());
    assert_eq!(
        store
            .sync_state(&peer.id, "notes/plan.md")
            .await
            .unwrap()
            .unwrap()
            .last_synced_doc_version_id,
        Some(baseline.version.id.clone())
    );

    let ours = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"ours\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(baseline.version.id.clone()),
        )
        .await
        .unwrap();
    std::fs::write(repo.path().join("notes/plan.md"), "theirs\n").unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();
    // Phase 4: markdown documents MERGE via diff3 against the peer's shadow
    // base — no sibling files, no legacy conflict records. The same block
    // changed on both sides, so the canonical side is retained and the
    // losing hunk becomes a conflict review item.
    let _ = ours;
    assert_eq!(result.conflicts.len(), 0);
    assert_eq!(result.conflict_paths.len(), 0);

    let document = store
        .get_document(&library.slug, "notes/plan.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"ours\n");
    let items = store.list_block_review_items(&document.id).await.unwrap();
    let conflicts: Vec<_> = items
        .iter()
        .filter(|item| item.kind == quarry_storage::BlockReviewKind::Conflict)
        .collect();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].state, quarry_storage::BlockReviewState::Open);
    assert_eq!(conflicts[0].body.as_deref(), Some("theirs\n"));
    assert_eq!(conflicts[0].quote.as_deref(), Some("ours\n"));
    assert_eq!(conflicts[0].context_before.as_deref(), Some("base\n"));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("notes/plan.md")).unwrap(),
        "ours\n"
    );
    let merged_head = store
        .head_document(&library.slug, "notes/plan.md")
        .await
        .unwrap()
        .head_version_id;
    assert_eq!(
        store
            .sync_state(&peer.id, "notes/plan.md")
            .await
            .unwrap()
            .unwrap()
            .last_synced_doc_version_id,
        Some(merged_head)
    );
}

#[tokio::test]
async fn sync_with_both_sides_unchanged_does_not_create_new_git_commit() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("unchanged").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/stable.md",
            b"stable\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();
    let head_before = git2::Repository::open(repo.path())
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    let document_id = store
        .head_document(&library.slug, "notes/stable.md")
        .await
        .unwrap()
        .id;
    let base_before = store
        .block_shadow_base("git", &peer.id, &document_id)
        .await
        .unwrap()
        .expect("export records the peer's shadow base");
    // Let the clock tick so an (incorrect) rewrite would change updated_at.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert!(result.imported_paths.is_empty());
    assert!(result.conflicts.is_empty());
    assert_eq!(result.commit_id, None);
    let head_after = git2::Repository::open(repo.path())
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    assert_eq!(head_after, head_before);
    // The shadow base already named the head, so the sync skipped both the
    // document-content load and the base rewrite.
    let base_after = store
        .block_shadow_base("git", &peer.id, &document_id)
        .await
        .unwrap()
        .expect("shadow base survives a no-op sync");
    assert_eq!(base_after, base_before);
}

#[tokio::test]
async fn sync_exports_quarry_only_content_change_to_git() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("quarrychange").await.unwrap();
    let baseline = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();
    let updated = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"from quarry\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(baseline.version.id),
        )
        .await
        .unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert!(result.imported_paths.is_empty());
    assert!(result.conflicts.is_empty());
    assert_eq!(result.exported_paths, vec!["notes/plan.md"]);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("notes/plan.md")).unwrap(),
        "from quarry\n"
    );
    assert_eq!(
        store
            .sync_state(&peer.id, "notes/plan.md")
            .await
            .unwrap()
            .unwrap()
            .last_synced_doc_version_id,
        Some(updated.version.id)
    );
}

#[tokio::test]
async fn sync_imports_git_only_content_change_to_quarry() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitchange").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();
    std::fs::write(repo.path().join("notes/plan.md"), "from git\n").unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert_eq!(result.imported_paths, vec!["notes/plan.md"]);
    assert!(result.conflicts.is_empty());
    assert_eq!(
        store
            .get_document(&library.slug, "notes/plan.md")
            .await
            .unwrap()
            .content,
        b"from git\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("notes/plan.md")).unwrap(),
        "from git\n"
    );
}

#[tokio::test]
async fn sync_does_not_advance_sync_state_when_export_fails() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("failedsync").await.unwrap();
    let baseline = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();
    std::fs::write(repo.path().join("notes/plan.md"), "from git\n").unwrap();
    store
        .put_document(
            &library.slug,
            "assets/too-large.bin",
            vec![b'x'; quarry_core::GIT_BINARY_WARN_THRESHOLD + 1],
            serde_json::json!({"content_type":"application/octet-stream"}),
            "application/octet-stream",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let error = sync_peer(&store, &library.slug, &peer.id)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("larger than the 5 MiB"));
    assert_eq!(
        store
            .sync_state(&peer.id, "notes/plan.md")
            .await
            .unwrap()
            .unwrap()
            .last_synced_doc_version_id,
        Some(baseline.version.id)
    );
}

#[tokio::test]
async fn sync_accepts_both_changed_to_same_content_without_conflict() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("samechange").await.unwrap();
    let baseline = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();
    let ours = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"same\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(baseline.version.id),
        )
        .await
        .unwrap();
    std::fs::write(repo.path().join("notes/plan.md"), "same\n").unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert!(result.conflicts.is_empty());
    assert!(result.conflict_paths.is_empty());
    assert_eq!(
        store
            .sync_state(&peer.id, "notes/plan.md")
            .await
            .unwrap()
            .unwrap()
            .last_synced_doc_version_id,
        Some(ours.version.id)
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("notes/plan.md")).unwrap(),
        "same\n"
    );
}

#[tokio::test]
async fn sync_preserves_both_sides_when_both_create_same_path_differently() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("bothcreate").await.unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();
    let ours = store
        .put_document(
            &library.slug,
            "notes/new.md",
            b"ours\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    std::fs::create_dir_all(repo.path().join("notes")).unwrap();
    std::fs::write(repo.path().join("notes/new.md"), "theirs\n").unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    // Phase 4: with no common ancestor (both sides created the path
    // independently), the merge uses an EMPTY base — every difference is a
    // conservative conflict review item, never a silent overwrite or a
    // sibling file.
    let _ = ours;
    assert_eq!(result.conflicts.len(), 0);
    assert_eq!(result.conflict_paths.len(), 0);
    let document = store
        .get_document(&library.slug, "notes/new.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"ours\n");
    let items = store.list_block_review_items(&document.id).await.unwrap();
    let conflicts: Vec<_> = items
        .iter()
        .filter(|item| item.kind == quarry_storage::BlockReviewKind::Conflict)
        .collect();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].body.as_deref(), Some("theirs\n"));
    assert_eq!(conflicts[0].quote.as_deref(), Some("ours\n"));
    assert_eq!(conflicts[0].context_before.as_deref(), Some(""));
}

#[tokio::test]
async fn sync_records_both_deleted_as_clean_state() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("bothdeleted").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/remove.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    store
        .delete_document(&library.slug, "notes/remove.md", DocumentSource::Rest)
        .await
        .unwrap();
    std::fs::remove_file(repo.path().join("notes/remove.md")).unwrap();
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert!(result.conflicts.is_empty());
    assert!(store
        .get_document(&library.slug, "notes/remove.md")
        .await
        .is_err());
    assert!(!repo.path().join("notes/remove.md").exists());
    let state = store
        .sync_state(&peer.id, "notes/remove.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.last_synced_doc_version_id, None);
    assert_eq!(state.last_synced_git_oid, None);
}

#[tokio::test]
async fn sync_applies_git_only_delete_to_quarry() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitdelete").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/remove.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    std::fs::remove_file(repo.path().join("notes/remove.md")).unwrap();
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert!(result.conflicts.is_empty());
    assert!(store
        .get_document(&library.slug, "notes/remove.md")
        .await
        .is_err());
    assert!(!repo.path().join("notes/remove.md").exists());
    let state = store
        .sync_state(&peer.id, "notes/remove.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.last_synced_doc_version_id, None);
    assert_eq!(state.last_synced_git_oid, None);
}

#[tokio::test]
async fn sync_applies_quarry_only_delete_to_git() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("quarrydelete").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/remove.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    store
        .delete_document(&library.slug, "notes/remove.md", DocumentSource::Rest)
        .await
        .unwrap();
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert!(result.conflicts.is_empty());
    assert!(!repo.path().join("notes/remove.md").exists());
    let state = store
        .sync_state(&peer.id, "notes/remove.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.last_synced_doc_version_id, None);
    assert_eq!(state.last_synced_git_oid, None);
}

#[tokio::test]
async fn sync_records_conflict_when_quarry_changes_and_git_deletes() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("changeddelete").await.unwrap();
    let baseline = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    let ours = store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"ours\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(baseline.version.id),
        )
        .await
        .unwrap();
    std::fs::remove_file(repo.path().join("notes/plan.md")).unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].ours_version_id, Some(ours.version.id));
    assert_eq!(result.conflicts[0].theirs_version_id, None);
    assert_eq!(
        store
            .get_document(&library.slug, "notes/plan.md")
            .await
            .unwrap()
            .content,
        b"ours\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("notes/plan.md")).unwrap(),
        "ours\n"
    );
}

#[tokio::test]
async fn sync_records_conflict_when_quarry_deletes_and_git_changes() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("deletedchange").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/plan.md",
            b"base\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    store
        .delete_document(&library.slug, "notes/plan.md", DocumentSource::Rest)
        .await
        .unwrap();
    std::fs::write(repo.path().join("notes/plan.md"), "theirs\n").unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].ours_version_id, None);
    let theirs_version_id = result.conflicts[0]
        .theirs_version_id
        .as_deref()
        .expect("the git-side conflict should record the imported version id");
    assert!(store
        .get_document(&library.slug, "notes/plan.md")
        .await
        .is_err());
    let conflict_path = result.conflict_paths[0].clone();
    assert!(conflict_path.starts_with("notes/plan.md.conflict-git-"));
    let sibling = store
        .get_document(&library.slug, &conflict_path)
        .await
        .unwrap();
    assert_eq!(sibling.version.id.as_str(), theirs_version_id);
    assert_eq!(sibling.content, b"theirs\n");
    // Markdown siblings import through the block writer (Phase 7): they are
    // ordinary BlockDocuments with a projection, not raw bytes.
    assert!(
        !store.load_block_tree(&sibling.id).await.unwrap().is_empty(),
        "markdown conflict sibling carries block rows"
    );
    assert!(!repo.path().join("notes/plan.md").exists());
    assert_eq!(
        std::fs::read_to_string(repo.path().join(&conflict_path)).unwrap(),
        "theirs\n"
    );
}

#[tokio::test]
async fn sync_aborts_when_git_delete_batch_exceeds_configured_safety_limit() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("safety").await.unwrap();
    for index in 0..5 {
        store
            .put_document(
                &library.slug,
                &format!("notes/{index}.md"),
                format!("doc {index}\n").into_bytes(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap();
    }
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({
                "repo": repo.path(),
                "branch": "main",
                "max_delete_percent": 50
            }),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    for index in 0..4 {
        std::fs::remove_file(repo.path().join(format!("notes/{index}.md"))).unwrap();
    }

    let error = sync_peer(&store, &library.slug, &peer.id)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("delete safety"));
    for index in 0..5 {
        assert_eq!(
            store
                .get_document(&library.slug, &format!("notes/{index}.md"))
                .await
                .unwrap()
                .content,
            format!("doc {index}\n").as_bytes()
        );
    }
}

#[tokio::test]
async fn push_peer_updates_configured_remote_branch() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("pushremote").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/pushed.md",
            b"from quarry\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    git2::Repository::init_bare(remote.path()).unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({
                "repo": worktree.path(),
                "remote": remote.path(),
                "branch": "main"
            }),
        )
        .await
        .unwrap();

    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    let bare = git2::Repository::open_bare(remote.path()).unwrap();
    let commit = bare
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    let entry = commit
        .tree()
        .unwrap()
        .get_path(Path::new("notes/pushed.md"))
        .unwrap();
    let blob = bare.find_blob(entry.id()).unwrap();
    assert_eq!(blob.content(), b"from quarry\n");
}

#[tokio::test]
async fn push_peer_does_not_advance_sync_state_when_remote_push_fails() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("pushfailure").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/pushed.md",
            b"from quarry\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let invalid_remote = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({
                "repo": worktree.path(),
                "remote": invalid_remote.path(),
                "branch": "main"
            }),
        )
        .await
        .unwrap();

    push_peer(&store, &library.slug, &peer.id)
        .await
        .unwrap_err();

    assert!(store
        .sync_state(&peer.id, "notes/pushed.md")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn pull_peer_fetches_configured_remote_branch_before_importing() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("pullremote").await.unwrap();
    let remote = tempfile::tempdir().unwrap();
    git2::Repository::init_bare(remote.path()).unwrap();
    seed_remote(
        remote.path(),
        &[
            (
                ".quarry/marker.json",
                format!(
                    "{{\n  \"library_id\": \"{}\",\n  \"library_slug\": \"pullremote\"\n}}",
                    library.id
                )
                .as_bytes()
                .to_vec(),
            ),
            ("notes/pulled.md", b"from remote\n".to_vec()),
        ],
    );

    let worktree = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({
                "repo": worktree.path(),
                "remote": remote.path(),
                "branch": "main"
            }),
        )
        .await
        .unwrap();

    let result = quarry_git::pull_peer(&store, &library.slug, &peer.id)
        .await
        .unwrap();

    assert_eq!(result.imported_paths, vec!["notes/pulled.md"]);
    assert_eq!(
        store
            .get_document(&library.slug, "notes/pulled.md")
            .await
            .unwrap()
            .content,
        b"from remote\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.path().join("notes/pulled.md")).unwrap(),
        "from remote\n"
    );
}

#[tokio::test]
async fn sync_peer_fetches_remote_branch_and_pushes_merged_tree() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("syncremote").await.unwrap();
    store
        .put_document(
            &library.slug,
            "notes/local.md",
            b"from quarry\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let remote = tempfile::tempdir().unwrap();
    git2::Repository::init_bare(remote.path()).unwrap();
    seed_remote(
        remote.path(),
        &[
            (
                ".quarry/marker.json",
                format!(
                    "{{\n  \"library_id\": \"{}\",\n  \"library_slug\": \"syncremote\"\n}}",
                    library.id
                )
                .as_bytes()
                .to_vec(),
            ),
            ("notes/remote.md", b"from remote\n".to_vec()),
        ],
    );

    let worktree = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({
                "repo": worktree.path(),
                "remote": remote.path(),
                "branch": "main"
            }),
        )
        .await
        .unwrap();

    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    assert!(result.conflicts.is_empty());
    assert_eq!(result.imported_paths, vec!["notes/remote.md"]);
    assert_eq!(
        store
            .get_document(&library.slug, "notes/remote.md")
            .await
            .unwrap()
            .content,
        b"from remote\n"
    );

    let bare = git2::Repository::open_bare(remote.path()).unwrap();
    let commit = bare
        .find_reference("refs/heads/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    let tree = commit.tree().unwrap();
    let local = tree.get_path(Path::new("notes/local.md")).unwrap();
    let remote_doc = tree.get_path(Path::new("notes/remote.md")).unwrap();
    assert_eq!(
        bare.find_blob(local.id()).unwrap().content(),
        b"from quarry\n"
    );
    assert_eq!(
        bare.find_blob(remote_doc.id()).unwrap().content(),
        b"from remote\n"
    );
}

fn seed_remote(remote_path: &Path, files: &[(&str, Vec<u8>)]) {
    let seed = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(seed.path()).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    for (path, content) in files {
        let path = seed.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("Quarry Test", "quarry@test.local").unwrap();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "seed remote",
        &tree,
        &[],
    )
    .unwrap();
    let mut remote = repo
        .remote("origin", &remote_path.to_string_lossy())
        .unwrap();
    remote
        .push(&["refs/heads/main:refs/heads/main"], None)
        .unwrap();
}

fn filesystem_supports_case_distinct_paths(root: &Path) -> bool {
    // Probe inside a throwaway subdirectory: on a case-sensitive filesystem
    // the probe files would otherwise linger in `root` and get imported.
    let probe = tempfile::tempdir_in(root).unwrap();
    let upper = probe.path().join("CaseProbe");
    let lower = probe.path().join("caseprobe");
    std::fs::write(&upper, b"upper").unwrap();
    std::fs::write(&lower, b"lower").unwrap();
    std::fs::read(&upper).unwrap() == b"upper" && std::fs::read(&lower).unwrap() == b"lower"
}

// ---------------------------------------------------------------------------
// Phase 4: git markdown sync reconciles via diff3 with per-peer shadow bases.
// ---------------------------------------------------------------------------

/// Editing one block in the worktree preserves the sibling `block_id`s and
/// live review anchors: the sync reconciles instead of replacing the
/// projection.
#[tokio::test]
async fn git_sync_edit_preserves_sibling_block_ids_and_live_anchors() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitblocks").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "notes/doc.md",
            "# Title\n\nAlpha.\n\nBravo.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let document_id = outcome.document.id.clone();
    let ids_before: Vec<String> = store
        .load_block_tree(&document_id)
        .await
        .unwrap()
        .iter()
        .filter(|row| row.parent_block_id.is_none())
        .map(|row| row.block_id.clone())
        .collect();
    let anchor = store
        .put_block_review_item(quarry_storage::NewBlockReviewItem {
            document_id: document_id.clone(),
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
        .await
        .unwrap();

    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    // The push recorded this peer's diff3 shadow base.
    let shadow = store
        .block_shadow_base("git", &peer.id, &document_id)
        .await
        .unwrap()
        .expect("export records the peer's shadow base");
    assert_eq!(shadow.base_markdown, "# Title\n\nAlpha.\n\nBravo.\n");

    let file = repo.path().join("notes/doc.md");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("Bravo.", "Bravo, from git.")).unwrap();
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();
    assert!(result.conflicts.is_empty());

    let rows = store.load_block_tree(&document_id).await.unwrap();
    let ids_after: Vec<String> = rows
        .iter()
        .filter(|row| row.parent_block_id.is_none())
        .map(|row| row.block_id.clone())
        .collect();
    assert_eq!(ids_after, ids_before, "sibling ids survive the git sync");
    assert_eq!(
        rows.iter()
            .find(|row| row.block_id == ids_before[2])
            .unwrap()
            .text,
        "Bravo, from git."
    );
    let items = store.list_block_review_items(&document_id).await.unwrap();
    let kept = items.iter().find(|item| item.id == anchor.id).unwrap();
    assert_eq!(kept.state, quarry_storage::BlockReviewState::Open);
    assert_eq!(kept.block_id, ids_before[1]);
    assert_eq!((kept.start_offset, kept.end_offset), (0, 5));

    // …and the sync advanced the shadow base to the merged canonical text.
    let shadow = store
        .block_shadow_base("git", &peer.id, &document_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        shadow.base_markdown,
        "# Title\n\nAlpha.\n\nBravo, from git.\n"
    );
}

/// RawDocument bypass: binary files round-trip exactly through sync and
/// never touch the block tables.
#[tokio::test]
async fn git_sync_raw_documents_bypass_the_block_model() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitraw").await.unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    let bytes: Vec<u8> = vec![0, 159, 146, 150, 13, 10, 0];
    std::fs::write(repo.path().join("blob.bin"), &bytes).unwrap();
    sync_peer(&store, &library.slug, &peer.id).await.unwrap();

    let document = store.get_document(&library.slug, "blob.bin").await.unwrap();
    assert_eq!(document.content, bytes);
    assert_eq!(
        store.load_block_tree(&document.id).await.unwrap(),
        Vec::<quarry_storage::BlockRow>::new()
    );
}

/// A pure git-side rename (identical bytes at a new path) pairs the delete
/// and the create into an identity-preserving document move: the document
/// id, block ids, review anchors, and the peer's shadow base all survive.
#[tokio::test]
async fn sync_pairs_pure_git_renames_into_identity_preserving_moves() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitrename").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "notes/old.md",
            "# Title\n\nAlpha.\n\nBravo.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let document_id = outcome.document.id.clone();
    let ids_before: Vec<String> = store
        .load_block_tree(&document_id)
        .await
        .unwrap()
        .iter()
        .map(|row| row.block_id.clone())
        .collect();
    let anchor = store
        .put_block_review_item(quarry_storage::NewBlockReviewItem {
            document_id: document_id.clone(),
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
        .await
        .unwrap();

    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    std::fs::rename(
        repo.path().join("notes/old.md"),
        repo.path().join("notes/new.md"),
    )
    .unwrap();
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();
    assert!(result.conflicts.is_empty());

    let moved = store
        .get_document(&library.slug, "notes/new.md")
        .await
        .unwrap();
    assert_eq!(
        moved.id, document_id,
        "the rename preserves the document id"
    );
    assert!(store
        .get_document(&library.slug, "notes/old.md")
        .await
        .is_err());
    let ids_after: Vec<String> = store
        .load_block_tree(&document_id)
        .await
        .unwrap()
        .iter()
        .map(|row| row.block_id.clone())
        .collect();
    assert_eq!(ids_after, ids_before, "block ids survive the rename");
    let items = store.list_block_review_items(&document_id).await.unwrap();
    let kept = items.iter().find(|item| item.id == anchor.id).unwrap();
    assert_eq!(kept.state, quarry_storage::BlockReviewState::Open);
    assert_eq!(kept.block_id, ids_before[1]);
    let shadow = store
        .block_shadow_base("git", &peer.id, &document_id)
        .await
        .unwrap()
        .expect("the peer's shadow base rides the document id");
    assert_eq!(shadow.base_markdown, "# Title\n\nAlpha.\n\nBravo.\n");

    assert!(!repo.path().join("notes/old.md").exists());
    assert!(repo.path().join("notes/new.md").exists());
    let old_state = store
        .sync_state(&peer.id, "notes/old.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_state.last_synced_doc_version_id, None);
    assert_eq!(old_state.last_synced_git_oid, None);
    let new_state = store
        .sync_state(&peer.id, "notes/new.md")
        .await
        .unwrap()
        .expect("the new path carries the sync state");
    assert_eq!(
        new_state.last_synced_doc_version_id.as_deref(),
        Some(moved.version.id.as_str())
    );
}

/// Rename-and-edit between syncs does not byte-match, so it stays the
/// conservative delete + create — fresh identity, no guessed pairing.
#[tokio::test]
async fn sync_treats_renamed_and_edited_files_as_delete_plus_create() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitrenameedit").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "notes/old.md",
            "Alpha.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main"}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    let text = std::fs::read_to_string(repo.path().join("notes/old.md")).unwrap();
    std::fs::remove_file(repo.path().join("notes/old.md")).unwrap();
    std::fs::write(
        repo.path().join("notes/new.md"),
        text.replace("Alpha.", "Alpha, edited."),
    )
    .unwrap();
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();
    assert!(result.conflicts.is_empty());

    let created = store
        .get_document(&library.slug, "notes/new.md")
        .await
        .unwrap();
    assert_ne!(
        created.id, outcome.document.id,
        "no pairing without a byte match"
    );
    assert!(store
        .get_document(&library.slug, "notes/old.md")
        .await
        .is_err());
}

/// Duplicate content on either side makes pairing ambiguous; the sync
/// refuses to guess and falls back to delete + create for all of them.
#[tokio::test]
async fn sync_refuses_to_pair_renames_with_duplicate_content() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitrenamedup").await.unwrap();
    let first = store
        .import_block_document(
            &library.slug,
            "notes/one.md",
            "Same body.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let second = store
        .import_block_document(
            &library.slug,
            "notes/two.md",
            "Same body.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main", "max_delete_percent": 100}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    std::fs::rename(
        repo.path().join("notes/one.md"),
        repo.path().join("notes/uno.md"),
    )
    .unwrap();
    std::fs::rename(
        repo.path().join("notes/two.md"),
        repo.path().join("notes/dos.md"),
    )
    .unwrap();
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();
    assert!(result.conflicts.is_empty());

    let uno = store
        .get_document(&library.slug, "notes/uno.md")
        .await
        .unwrap();
    let dos = store
        .get_document(&library.slug, "notes/dos.md")
        .await
        .unwrap();
    assert_ne!(uno.id, first.document.id);
    assert_ne!(uno.id, second.document.id);
    assert_ne!(dos.id, first.document.id);
    assert_ne!(dos.id, second.document.id);
}

/// A bulk folder rename used to look like a mass deletion and trip the
/// delete-safety abort; paired renames no longer count as deletions.
#[tokio::test]
async fn sync_bulk_renames_pass_the_delete_safety_limit() {
    let root = tempfile::tempdir().unwrap();
    let store = open_store(root.path()).await;
    let library = store.create_library("gitrenamebulk").await.unwrap();
    let alpha = store
        .import_block_document(
            &library.slug,
            "notes/alpha.md",
            "Alpha body.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let bravo = store
        .import_block_document(
            &library.slug,
            "notes/bravo.md",
            "Bravo body.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let repo = tempfile::tempdir().unwrap();
    let peer = store
        .create_git_peer(
            &library.slug,
            serde_json::json!({"repo": repo.path(), "branch": "main", "max_delete_percent": 50}),
        )
        .await
        .unwrap();
    push_peer(&store, &library.slug, &peer.id).await.unwrap();

    std::fs::create_dir_all(repo.path().join("archive")).unwrap();
    std::fs::rename(
        repo.path().join("notes/alpha.md"),
        repo.path().join("archive/alpha.md"),
    )
    .unwrap();
    std::fs::rename(
        repo.path().join("notes/bravo.md"),
        repo.path().join("archive/bravo.md"),
    )
    .unwrap();

    // 2 of 2 tracked paths gone would exceed the 50% delete cap; paired
    // renames must not count as deletions.
    let result = sync_peer(&store, &library.slug, &peer.id).await.unwrap();
    assert!(result.conflicts.is_empty());
    assert_eq!(
        store
            .get_document(&library.slug, "archive/alpha.md")
            .await
            .unwrap()
            .id,
        alpha.document.id
    );
    assert_eq!(
        store
            .get_document(&library.slug, "archive/bravo.md")
            .await
            .unwrap()
            .id,
        bravo.document.id
    );
}
