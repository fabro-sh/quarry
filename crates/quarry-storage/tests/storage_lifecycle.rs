use quarry_core::{DocumentSource, WritePrecondition, INLINE_CONTENT_THRESHOLD};
use quarry_storage::{QuarryStore, StoreConfig, StoreEventKind};
use std::time::Duration;

#[tokio::test]
async fn stores_multiple_libraries_versions_cas_restart_and_gc() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("quarry.db");
    let cas_path = root.path().join("cas");

    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: cas_path.clone(),
        lock_path: None,
    })
    .await
    .unwrap();

    let alpha = store.create_library("alpha").await.unwrap();
    let beta = store.create_library("beta").await.unwrap();
    assert_ne!(alpha.id, beta.id);

    let small = store
        .put_document(
            &alpha.slug,
            "notes/plan.md",
            b"one".to_vec(),
            serde_json::json!({"content_type":"text/markdown","topic":"plan"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    assert!(small.version.content_hash.is_none());
    assert!(small.version.inline_content.is_some());

    let large_bytes = vec![b'x'; INLINE_CONTENT_THRESHOLD + 1];
    let large = store
        .put_document(
            &alpha.slug,
            "assets/large.bin",
            large_bytes.clone(),
            serde_json::json!({"content_type":"application/octet-stream"}),
            "application/octet-stream",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    assert!(large.version.content_hash.is_some());
    assert!(large.version.inline_content.is_none());
    let listed_large = store
        .list_documents(&alpha.slug, Some("assets/"), None)
        .await
        .unwrap()
        .into_iter()
        .find(|document| document.path == "assets/large.bin")
        .unwrap();
    assert_eq!(listed_large.content_hash, large.version.content_hash);

    let second = store
        .put_document(
            &alpha.slug,
            "notes/plan.md",
            b"two".to_vec(),
            serde_json::json!({"content_type":"text/markdown","topic":"plan"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(small.version.id.clone()),
        )
        .await
        .unwrap();
    assert_ne!(small.version.id, second.version.id);
    assert_eq!(
        store
            .version_history(&alpha.slug, "notes/plan.md")
            .await
            .unwrap()
            .len(),
        2
    );

    store
        .move_document(
            &alpha.slug,
            "notes/plan.md",
            "notes/renamed.md",
            DocumentSource::Rest,
        )
        .await
        .unwrap();
    assert!(store
        .get_document(&alpha.slug, "notes/plan.md")
        .await
        .is_err());
    assert_eq!(
        store
            .get_document(&alpha.slug, "notes/renamed.md")
            .await
            .unwrap()
            .content,
        b"two"
    );

    store
        .delete_document(&alpha.slug, "notes/renamed.md", DocumentSource::Rest)
        .await
        .unwrap();
    assert!(store
        .get_document(&alpha.slug, "notes/renamed.md")
        .await
        .is_err());

    drop(store);

    let reopened = QuarryStore::open(StoreConfig {
        db_path,
        cas_path,
        lock_path: None,
    })
    .await
    .unwrap();
    assert_eq!(
        reopened
            .get_document(&alpha.slug, "assets/large.bin")
            .await
            .unwrap()
            .content,
        large_bytes
    );

    let gc = reopened.gc().await.unwrap();
    assert_eq!(gc.removed, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_auto_commit_writes_publish_without_lost_documents() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("concurrent").await.unwrap();

    let mut handles = Vec::new();
    for index in 0..32 {
        let store = store.clone();
        let library = library.slug.clone();
        handles.push(tokio::spawn(async move {
            store
                .put_document(
                    &library,
                    &format!("notes/{index}.md"),
                    format!("document {index}\n").into_bytes(),
                    serde_json::json!({"content_type":"text/markdown"}),
                    "text/markdown",
                    DocumentSource::Rest,
                    WritePrecondition::None,
                )
                .await
                .unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let documents = store
        .list_documents(&library.slug, Some("notes/"), Some(100))
        .await
        .unwrap();
    assert_eq!(documents.len(), 32);
    for index in 0..32 {
        assert_eq!(
            store
                .get_document(&library.slug, &format!("notes/{index}.md"))
                .await
                .unwrap()
                .content,
            format!("document {index}\n").as_bytes()
        );
    }
}

#[tokio::test]
async fn link_index_updates_from_markdown_writes_and_ignores_binary_content() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("links").await.unwrap();

    store
        .put_document(
            &library.slug,
            "target.md",
            b"# Target Heading\n\nTarget body.\n".to_vec(),
            serde_json::json!({
                "content_type": "text/markdown",
                "aliases": ["Target Alias"]
            }),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let source = store
        .put_document(
            &library.slug,
            "source.md",
            b"See [[Target Alias#Target Heading|alias]], ![[target]], [target](target.md), [[Missing]], and #tag.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let outgoing = store
        .outgoing_links(&library.slug, "source.md")
        .await
        .unwrap();
    assert!(outgoing.links.iter().any(|link| {
        link.target_kind == "wiki_link"
            && link.target_path.as_deref() == Some("target.md")
            && link.target_anchor.as_deref() == Some("Target Heading")
            && link.alias.as_deref() == Some("alias")
            && link.resolved
    }));
    assert!(outgoing.links.iter().any(
        |link| link.target_kind == "embed" && link.target_path.as_deref() == Some("target.md")
    ));
    assert!(outgoing
        .links
        .iter()
        .any(|link| link.target_kind == "tag" && link.target_text == "tag"));
    assert!(outgoing
        .links
        .iter()
        .any(|link| link.target_kind == "wiki_link"
            && link.target_text == "Missing"
            && !link.resolved));

    let target_links = store
        .outgoing_links(&library.slug, "target.md")
        .await
        .unwrap();
    assert!(target_links.links.iter().any(|link| {
        link.target_kind == "heading"
            && link.target_text == "Target Heading"
            && link.target_anchor.as_deref() == Some("target-heading")
            && link.target_path.as_deref() == Some("target.md")
            && link.resolved
    }));

    store
        .put_document(
            &library.slug,
            "raw.bin",
            b"[[target]] should not be indexed from binary content".to_vec(),
            serde_json::json!({"content_type":"application/octet-stream"}),
            "application/octet-stream",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    assert!(store
        .outgoing_links(&library.slug, "raw.bin")
        .await
        .unwrap()
        .links
        .is_empty());
    let focused_graph = store
        .graph(
            &library.slug,
            Some("target.md"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(focused_graph
        .nodes
        .iter()
        .any(|node| node.path == "target.md"));
    assert!(focused_graph
        .nodes
        .iter()
        .any(|node| node.path == "source.md"));
    assert!(!focused_graph
        .nodes
        .iter()
        .any(|node| node.path == "raw.bin"));

    store
        .put_document(
            &library.slug,
            "source.md",
            b"No links now.\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(source.version.id.clone()),
        )
        .await
        .unwrap();
    assert!(store
        .outgoing_links(&library.slug, "source.md")
        .await
        .unwrap()
        .links
        .is_empty());
    assert!(store
        .backlinks(&library.slug, "target.md")
        .await
        .unwrap()
        .links
        .is_empty());

    let tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            None,
            Some("restore source link".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    store
        .stage_put(
            &tx.id,
            "source.md",
            b"Transaction link to [[target]].\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();
    store.commit_transaction(&tx.id).await.unwrap();

    assert!(store
        .backlinks(&library.slug, "target.md")
        .await
        .unwrap()
        .links
        .iter()
        .any(|link| link.src_path == "source.md"));
}

#[tokio::test]
async fn suggestions_include_aliases_and_headings_for_wikilink_completion() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("suggestions").await.unwrap();

    store
        .put_document(
            &library.slug,
            "guide.md",
            b"# Deep Section\n\nGuide body.\n".to_vec(),
            serde_json::json!({
                "content_type": "text/markdown",
                "title": "Guide",
                "aliases": ["Shortcut"]
            }),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let alias_suggestions = serde_json::to_value(
        store
            .suggest_documents(&library.slug, "shortcut", Some(10))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(alias_suggestions
        .as_array()
        .unwrap()
        .iter()
        .any(|suggestion| {
            suggestion["path"] == "guide.md"
                && suggestion["match_type"] == "alias"
                && suggestion["matched_text"] == "Shortcut"
        }));

    let heading_suggestions = serde_json::to_value(
        store
            .suggest_documents(&library.slug, "deep", Some(10))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(heading_suggestions
        .as_array()
        .unwrap()
        .iter()
        .any(|suggestion| {
            suggestion["path"] == "guide.md"
                && suggestion["match_type"] == "heading"
                && suggestion["matched_text"] == "Deep Section"
                && suggestion["target_anchor"] == "Deep Section"
        }));
}

#[tokio::test]
async fn markdown_frontmatter_aliases_participate_in_link_resolution() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("frontmatterlinks").await.unwrap();

    store
        .put_document(
            &library.slug,
            "guide.md",
            b"---\naliases:\n  - Front Alias\n---\n# Guide\n".to_vec(),
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "source.md",
            b"See [[Front Alias]].\n".to_vec(),
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let guide = store.get_document(&library.slug, "guide.md").await.unwrap();
    assert_eq!(
        String::from_utf8_lossy(&guide.content),
        "---\naliases:\n  - Front Alias\n---\n# Guide\n"
    );
    assert_eq!(guide.version.metadata["aliases"][0], "Front Alias");

    let outgoing = store
        .outgoing_links(&library.slug, "source.md")
        .await
        .unwrap();
    assert!(outgoing.links.iter().any(|link| {
        link.target_kind == "wiki_link"
            && link.target_text == "Front Alias"
            && link.target_path.as_deref() == Some("guide.md")
            && link.resolved
    }));
}

#[tokio::test]
async fn ambiguous_short_wikilinks_remain_unresolved() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("ambiguouslinks").await.unwrap();

    for path in ["alpha/target.md", "omega/target.md"] {
        store
            .put_document(
                &library.slug,
                path,
                b"# Target\n".to_vec(),
                serde_json::json!({"content_type": "text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap();
    }
    store
        .put_document(
            &library.slug,
            "source.md",
            b"See [[target]].\n".to_vec(),
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let outgoing = store
        .outgoing_links(&library.slug, "source.md")
        .await
        .unwrap();
    let link = outgoing
        .links
        .iter()
        .find(|link| link.target_kind == "wiki_link" && link.target_text == "target")
        .unwrap();
    assert_eq!(link.target_path, None);
    assert!(!link.resolved);
    assert_eq!(
        serde_json::to_value(link).unwrap()["resolution_status"],
        "ambiguous"
    );
}

#[tokio::test]
async fn link_index_tracks_moves_and_deletes() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("links").await.unwrap();

    store
        .put_document(
            &library.slug,
            "target.md",
            b"# Target\n".to_vec(),
            serde_json::json!({
                "content_type": "text/markdown",
                "aliases": ["target"]
            }),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "source.md",
            b"See [[target]].\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    store
        .move_document(
            &library.slug,
            "target.md",
            "renamed.md",
            DocumentSource::Rest,
        )
        .await
        .unwrap();
    let backlinks = store.backlinks(&library.slug, "renamed.md").await.unwrap();
    assert!(backlinks.links.iter().any(|link| {
        link.src_path == "source.md" && link.target_path.as_deref() == Some("renamed.md")
    }));

    store
        .move_document(
            &library.slug,
            "source.md",
            "folder/source.md",
            DocumentSource::Rest,
        )
        .await
        .unwrap();
    let backlinks = store.backlinks(&library.slug, "renamed.md").await.unwrap();
    assert!(backlinks
        .links
        .iter()
        .any(|link| link.src_path == "folder/source.md"));

    store
        .delete_document(&library.slug, "folder/source.md", DocumentSource::Rest)
        .await
        .unwrap();
    assert!(store
        .backlinks(&library.slug, "renamed.md")
        .await
        .unwrap()
        .links
        .is_empty());
}

#[tokio::test]
async fn explicit_transactions_publish_atomically_and_rollback_staged_cas() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();

    let library = store.create_library("txlib").await.unwrap();
    let base = store
        .put_document(
            &library.slug,
            "docs/a.md",
            b"base".to_vec(),
            serde_json::json!({"content_type":"text/markdown","topic":"old"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("codex".to_string()),
            Some("multi-file edit".to_string()),
            serde_json::json!({"test": true}),
        )
        .await
        .unwrap();
    store
        .stage_put(
            &tx.id,
            "docs/new.md",
            b"new".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();
    store
        .stage_metadata(&tx.id, "docs/a.md", serde_json::json!({"topic":"new"}))
        .await
        .unwrap();
    store
        .stage_move(&tx.id, "docs/a.md", "docs/b.md")
        .await
        .unwrap();

    assert!(store
        .get_document(&library.slug, "docs/new.md")
        .await
        .is_err());
    let still_visible = store
        .get_document(&library.slug, "docs/a.md")
        .await
        .unwrap();
    assert_eq!(still_visible.version.id, base.version.id);
    assert_eq!(still_visible.metadata["topic"], "old");

    store.commit_transaction(&tx.id).await.unwrap();
    assert_eq!(
        store
            .get_document(&library.slug, "docs/new.md")
            .await
            .unwrap()
            .content,
        b"new"
    );
    assert!(store
        .get_document(&library.slug, "docs/a.md")
        .await
        .is_err());
    let moved = store
        .get_document(&library.slug, "docs/b.md")
        .await
        .unwrap();
    assert_eq!(moved.content, b"base");
    assert_eq!(moved.metadata["topic"], "new");

    let rollback_tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            None,
            Some("rollback large".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    store
        .stage_put(
            &rollback_tx.id,
            "docs/rolled.bin",
            vec![b'z'; INLINE_CONTENT_THRESHOLD + 10],
            serde_json::json!({"content_type":"application/octet-stream"}),
            "application/octet-stream",
        )
        .await
        .unwrap();
    assert_eq!(store.gc().await.unwrap().removed, 0);
    store.rollback_transaction(&rollback_tx.id).await.unwrap();
    assert!(store
        .get_document(&library.slug, "docs/rolled.bin")
        .await
        .is_err());
    assert_eq!(store.gc().await.unwrap().removed, 1);
}

#[tokio::test]
async fn explicit_transaction_commit_rejects_stale_heads_without_overwriting_newer_writes() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("txraces").await.unwrap();

    let base = store
        .put_document(
            &library.slug,
            "docs/a.md",
            b"base".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("agent".to_string()),
            Some("stale edit".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    store
        .stage_put(
            &tx.id,
            "docs/a.md",
            b"staged".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();
    let newer = store
        .put_document(
            &library.slug,
            "docs/a.md",
            b"newer".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(base.version.id),
        )
        .await
        .unwrap();

    let error = store.commit_transaction(&tx.id).await.unwrap_err();

    assert!(error.to_string().contains("precondition failed"));
    let visible = store
        .get_document(&library.slug, "docs/a.md")
        .await
        .unwrap();
    assert_eq!(visible.content, b"newer");
    assert_eq!(visible.version.id, newer.version.id);

    store.rollback_transaction(&tx.id).await.unwrap();

    store
        .put_document(
            &library.slug,
            "docs/source.md",
            b"source".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let move_tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("agent".to_string()),
            Some("stale move".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    store
        .stage_move(&move_tx.id, "docs/source.md", "docs/target.md")
        .await
        .unwrap();
    let target = store
        .put_document(
            &library.slug,
            "docs/target.md",
            b"target winner".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let error = store.commit_transaction(&move_tx.id).await.unwrap_err();

    assert!(error.to_string().contains("precondition failed"));
    assert_eq!(
        store
            .get_document(&library.slug, "docs/source.md")
            .await
            .unwrap()
            .content,
        b"source"
    );
    let visible_target = store
        .get_document(&library.slug, "docs/target.md")
        .await
        .unwrap();
    assert_eq!(visible_target.content, b"target winner");
    assert_eq!(visible_target.version.id, target.version.id);
}

#[tokio::test]
async fn open_transaction_survives_restart_without_publishing_staged_cas() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("quarry.db");
    let cas_path = root.path().join("cas");
    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: cas_path.clone(),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("restarttx").await.unwrap();
    let tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("agent".to_string()),
            Some("restart staged write".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    store
        .stage_put(
            &tx.id,
            "docs/staged.bin",
            vec![b'z'; INLINE_CONTENT_THRESHOLD + 32],
            serde_json::json!({"content_type":"application/octet-stream"}),
            "application/octet-stream",
        )
        .await
        .unwrap();
    assert!(store
        .get_document(&library.slug, "docs/staged.bin")
        .await
        .is_err());
    drop(store);

    let reopened = QuarryStore::open(StoreConfig {
        db_path,
        cas_path,
        lock_path: None,
    })
    .await
    .unwrap();

    assert!(reopened
        .get_document(&library.slug, "docs/staged.bin")
        .await
        .is_err());
    let transactions = reopened.list_transactions(&library.slug).await.unwrap();
    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0].state, quarry_core::TransactionState::Open);
    assert_eq!(reopened.gc().await.unwrap().removed, 0);

    reopened.rollback_transaction(&tx.id).await.unwrap();
    assert_eq!(reopened.gc().await.unwrap().removed, 1);
}

#[tokio::test]
async fn global_operation_lock_blocks_normal_writes_until_released() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("locked").await.unwrap();

    let guard = store.acquire_global_operation_lock().await;
    let writer_store = store.clone();
    let writer_library = library.slug.clone();
    let mut write = tokio::spawn(async move {
        writer_store
            .put_document(
                &writer_library,
                "notes/blocked.md",
                b"blocked".to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut write)
            .await
            .is_err(),
        "write should wait while a global operation lock is held"
    );

    drop(guard);
    let outcome = tokio::time::timeout(Duration::from_secs(1), write)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(outcome.document.path, "notes/blocked.md");
}

#[tokio::test]
async fn second_store_owner_is_rejected_by_lock_file() {
    let root = tempfile::tempdir().unwrap();
    let config = StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    };
    let _owner = QuarryStore::open(config.clone()).await.unwrap();

    let error = match QuarryStore::open(config).await {
        Ok(_) => panic!("second store owner should be rejected"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("another Quarry daemon"));
}

#[tokio::test]
async fn paths_are_normalized_reserved_paths_rejected_and_keys_case_sensitive() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("paths").await.unwrap();

    store
        .put_document(
            &library.slug,
            "/Notes/Plan.md",
            b"upper".to_vec(),
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
            "notes/plan.md",
            b"lower".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .get_document(&library.slug, "Notes/Plan.md")
            .await
            .unwrap()
            .content,
        b"upper"
    );
    assert_eq!(
        store
            .get_document(&library.slug, "notes/plan.md")
            .await
            .unwrap()
            .content,
        b"lower"
    );
    let error = store
        .put_document(
            &library.slug,
            ".quarry/marker.json",
            b"reserved".to_vec(),
            serde_json::json!({}),
            "application/octet-stream",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("reserved"));
}

#[tokio::test]
async fn stores_lists_and_reopens_one_thousand_mixed_size_documents() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("quarry.db");
    let cas_path = root.path().join("cas");
    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: cas_path.clone(),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("bulk").await.unwrap();

    for index in 0..1000 {
        let content = if index % 100 == 0 {
            vec![b'x'; INLINE_CONTENT_THRESHOLD + index + 1]
        } else {
            format!("doc {index}\n").into_bytes()
        };
        store
            .put_document(
                &library.slug,
                &format!("docs/{index:04}.bin"),
                content,
                serde_json::json!({"content_type":"application/octet-stream","index":index}),
                "application/octet-stream",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap();
    }

    let listed = store
        .list_documents(&library.slug, Some("docs/"), Some(10_000))
        .await
        .unwrap();
    assert_eq!(listed.len(), 1000);
    assert!(listed[0].path < listed[999].path);
    drop(store);

    let reopened = QuarryStore::open(StoreConfig {
        db_path,
        cas_path,
        lock_path: None,
    })
    .await
    .unwrap();
    assert_eq!(
        reopened
            .get_document(&library.slug, "docs/0001.bin")
            .await
            .unwrap()
            .content,
        b"doc 1\n"
    );
    assert_eq!(
        reopened
            .get_document(&library.slug, "docs/0900.bin")
            .await
            .unwrap()
            .content
            .len(),
        INLINE_CONTENT_THRESHOLD + 901
    );
}

#[tokio::test]
async fn visible_writes_emit_in_process_store_events() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("events").await.unwrap();
    let mut events = store.subscribe_events();

    let write = store
        .put_document(
            &library.slug,
            "notes/a.md",
            b"a".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::DocumentPut);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.path.as_deref(), Some("notes/a.md"));
    assert_eq!(event.doc_id.as_deref(), Some(write.document.id.as_str()));
    assert_eq!(event.version_id.as_deref(), Some(write.version.id.as_str()));
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::LinksIndexed);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.path.as_deref(), Some("notes/a.md"));

    store
        .move_document(
            &library.slug,
            "notes/a.md",
            "notes/b.md",
            DocumentSource::Rest,
        )
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::DocumentMove);
    assert_eq!(event.path.as_deref(), Some("notes/a.md"));
    assert_eq!(event.new_path.as_deref(), Some("notes/b.md"));
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::LinksIndexed);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.path.as_deref(), Some("notes/b.md"));

    store
        .delete_document(&library.slug, "notes/b.md", DocumentSource::Rest)
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::DocumentDelete);
    assert_eq!(event.path.as_deref(), Some("notes/b.md"));
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::LinksIndexed);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.path.as_deref(), Some("notes/b.md"));

    let conflict = store
        .record_conflict(
            &library.slug,
            "notes/conflicted.md",
            Some("ours-version".to_string()),
            Some("theirs-version".to_string()),
        )
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::ConflictCreated);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.path.as_deref(), Some("notes/conflicted.md"));
    assert_eq!(event.conflict_id.as_deref(), Some(conflict.id.as_str()));

    store.resolve_conflict(&conflict.id).await.unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::ConflictResolved);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.path.as_deref(), Some("notes/conflicted.md"));
    assert_eq!(event.conflict_id.as_deref(), Some(conflict.id.as_str()));

    let report = store.reindex_library(&library.slug).await.unwrap();
    assert!(report.ok);
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::LibraryReindexed);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.path, None);
    assert_eq!(event.conflict_id, None);

    store
        .emit_git_sync_completed(&library.slug, "peer-1", 2, 1)
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::GitSyncCompleted);
    assert_eq!(event.library_id, library.id);
    assert_eq!(event.peer_id.as_deref(), Some("peer-1"));
    assert_eq!(event.applied, Some(2));
    assert_eq!(event.conflicts, Some(1));
}

#[tokio::test]
async fn inode_paths_are_lookupable_and_moves_keep_inode_identity() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
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
    let inode = store
        .inode_for_path(&library.slug, "plans/one.md")
        .await
        .unwrap();

    assert_eq!(
        store.path_for_inode(&library.slug, inode).await.unwrap(),
        "plans/one.md"
    );

    store
        .move_document(
            &library.slug,
            "plans/one.md",
            "archive/one.md",
            DocumentSource::Rest,
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .inode_for_path(&library.slug, "archive/one.md")
            .await
            .unwrap(),
        inode
    );
    assert_eq!(
        store.path_for_inode(&library.slug, inode).await.unwrap(),
        "archive/one.md"
    );
    assert!(store
        .inode_for_path(&library.slug, "plans/one.md")
        .await
        .is_err());
}

#[tokio::test]
async fn move_document_can_reuse_deleted_target_path() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/source.md",
            b"source\n".to_vec(),
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
            "drafts/target.md",
            b"deleted\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let source_inode = store
        .inode_for_path(&library.slug, "drafts/source.md")
        .await
        .unwrap();

    store
        .delete_document(&library.slug, "drafts/target.md", DocumentSource::Rest)
        .await
        .unwrap();
    store
        .move_document(
            &library.slug,
            "drafts/source.md",
            "drafts/target.md",
            DocumentSource::Rest,
        )
        .await
        .unwrap();

    let document = store
        .get_document(&library.slug, "drafts/target.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"source\n");
    assert_eq!(
        store
            .inode_for_path(&library.slug, "drafts/target.md")
            .await
            .unwrap(),
        source_inode
    );
    assert!(store
        .get_document(&library.slug, "drafts/source.md")
        .await
        .is_err());
}

#[tokio::test]
async fn committed_transaction_move_can_reuse_deleted_target_path() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("notes").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/source.md",
            b"source\n".to_vec(),
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
            "drafts/target.md",
            b"deleted\n".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let source_inode = store
        .inode_for_path(&library.slug, "drafts/source.md")
        .await
        .unwrap();
    store
        .delete_document(&library.slug, "drafts/target.md", DocumentSource::Rest)
        .await
        .unwrap();

    let tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            None,
            Some("move over tombstone".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    store
        .stage_move(&tx.id, "drafts/source.md", "drafts/target.md")
        .await
        .unwrap();
    store.commit_transaction(&tx.id).await.unwrap();

    let document = store
        .get_document(&library.slug, "drafts/target.md")
        .await
        .unwrap();
    assert_eq!(document.content, b"source\n");
    assert_eq!(
        store
            .inode_for_path(&library.slug, "drafts/target.md")
            .await
            .unwrap(),
        source_inode
    );
    assert!(store
        .get_document(&library.slug, "drafts/source.md")
        .await
        .is_err());
}

#[tokio::test]
async fn schema_indexes_metadata_hot_fields() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("quarry.db");
    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    drop(store);

    let db = turso::Builder::new_local(db_path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let document_indexes = index_names(&conn, "documents").await;
    let version_indexes = index_names(&conn, "document_versions").await;

    assert!(document_indexes.contains("idx_documents_created_at"));
    assert!(document_indexes.contains("idx_documents_updated_at"));
    assert!(version_indexes.contains("idx_versions_content_type"));
    assert!(version_indexes.contains("idx_versions_created_at"));
}

async fn index_names(conn: &turso::Connection, table: &str) -> std::collections::HashSet<String> {
    let mut rows = conn
        .query(format!("PRAGMA index_list('{table}')"), ())
        .await
        .unwrap();
    let mut names = std::collections::HashSet::new();
    while let Some(row) = rows.next().await.unwrap() {
        names.insert(row.get::<String>(1).unwrap());
    }
    names
}
