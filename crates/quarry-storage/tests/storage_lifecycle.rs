use quarry_core::{
    DocumentSource, DocumentVersion, QuarryError, WritePrecondition, INLINE_CONTENT_THRESHOLD,
};
use quarry_storage::{
    group_version_history, BlockMutationCommit, BlockMutationOutcome, BlockReviewKind,
    BlockReviewState, NewBlockReviewItem, QuarryStore, StoreConfig, StoreEventKind,
    TransactionMetadata,
};
use std::time::Duration;

fn history_version(
    id: &str,
    document_id: &str,
    created_at: &str,
    session_id: Option<&str>,
    source: DocumentSource,
    checkpoint_reason: Option<&str>,
) -> DocumentVersion {
    let provenance = if let Some(reason) = checkpoint_reason {
        serde_json::json!({"history": {"kind": "checkpoint", "reason": reason}})
    } else if let Some(session_id) = session_id {
        serde_json::json!({"history": {"kind": "autosave", "reason": "typing", "session_id": session_id}})
    } else {
        serde_json::json!({"mode": "auto_commit"})
    };
    DocumentVersion {
        id: id.to_string(),
        document_id: document_id.to_string(),
        tx_id: format!("tx-{id}"),
        transaction_source: Some(source),
        transaction_actor: Some("browser".to_string()),
        transaction_message: Some("Autosaved edits".to_string()),
        transaction_provenance: Some(provenance),
        content_hash: None,
        inline_content: None,
        metadata: serde_json::json!({}),
        content_type: "text/markdown".to_string(),
        byte_size: 1,
        created_at: created_at.to_string(),
    }
}

#[test]
fn groups_autosave_history_and_splits_singletons() {
    let versions = vec![
        history_version(
            "v1",
            "doc",
            "2026-06-07T10:00:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v2",
            "doc",
            "2026-06-07T10:01:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v3",
            "doc",
            "2026-06-07T10:04:01Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v4",
            "doc",
            "2026-06-07T10:04:30Z",
            Some("s2"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v5",
            "doc",
            "2026-06-07T10:05:00Z",
            None,
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v6",
            "doc",
            "2026-06-07T10:05:30Z",
            Some("s2"),
            DocumentSource::Git,
            None,
        ),
        history_version(
            "v7",
            "doc",
            "2026-06-07T10:06:00Z",
            Some("s2"),
            DocumentSource::Rest,
            Some("restore"),
        ),
    ];

    let history = group_version_history(versions);

    assert_eq!(
        history
            .iter()
            .map(|entry| (
                entry.earliest_version_id.as_str(),
                entry.latest_version_id.as_str(),
                entry.raw_version_count
            ))
            .collect::<Vec<_>>(),
        vec![
            ("v7", "v7", 1),
            ("v6", "v6", 1),
            ("v5", "v5", 1),
            ("v4", "v4", 1),
            ("v3", "v3", 1),
            ("v1", "v2", 2),
        ]
    );
}

#[test]
fn autosave_groups_split_after_ten_minute_span() {
    let versions = vec![
        history_version(
            "v1",
            "doc",
            "2026-06-07T10:00:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v2",
            "doc",
            "2026-06-07T10:02:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v3",
            "doc",
            "2026-06-07T10:04:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v4",
            "doc",
            "2026-06-07T10:06:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v5",
            "doc",
            "2026-06-07T10:08:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v6",
            "doc",
            "2026-06-07T10:10:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
        history_version(
            "v7",
            "doc",
            "2026-06-07T10:12:00Z",
            Some("s1"),
            DocumentSource::Rest,
            None,
        ),
    ];

    let history = group_version_history(versions);

    assert_eq!(
        history
            .iter()
            .map(|entry| (
                entry.earliest_version_id.as_str(),
                entry.latest_version_id.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![("v7", "v7"), ("v1", "v6")]
    );
}

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

#[tokio::test]
async fn persists_collab_recovery_state_by_document_id_across_restart() {
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
    let library = store.create_library("collab").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "live.md",
            b"markdown head".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let state = store
        .put_collab_recovery_state(&written.document.id, None, vec![1, 2, 3, 4], true)
        .await
        .unwrap();
    assert_eq!(state.document_id, written.document.id);
    assert_eq!(state.base_version_id, Some(written.version.id.clone()));
    assert_eq!(state.update_v1, vec![1, 2, 3, 4]);
    assert!(state.dirty);

    drop(store);

    let reopened = QuarryStore::open(StoreConfig {
        db_path,
        cas_path,
        lock_path: None,
    })
    .await
    .unwrap();
    let state = reopened
        .collab_recovery_state(&written.document.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.base_version_id, Some(written.version.id.clone()));
    assert_eq!(state.update_v1, vec![1, 2, 3, 4]);
    assert!(state.dirty);
    assert_eq!(
        reopened
            .get_document(&library.slug, "live.md")
            .await
            .unwrap()
            .content,
        b"markdown head"
    );

    let clean = reopened
        .mark_collab_recovery_state_clean(&written.document.id, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(clean.base_version_id, Some(written.version.id));
    assert!(!clean.dirty);

    let error = reopened
        .put_collab_recovery_state("not-a-document", None, vec![9], true)
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::NotFound(_)));
}

#[tokio::test]
async fn manages_stateful_collab_invite_tokens_by_document_id() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("shares").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "live.md",
            b"markdown head".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let token = store
        .create_collab_invite_token(
            &library.slug,
            "live.md",
            "EDITOR",
            Some("Avery".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(token.document_id, written.document.id);
    assert_eq!(token.role, "editor");
    assert_eq!(token.by_hint.as_deref(), Some("Avery"));
    assert!(token.revoked_at.is_none());

    let tokens = store
        .collab_invite_tokens(&library.slug, "live.md")
        .await
        .unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].id, token.id);

    let revoked = store.revoke_collab_invite_token(&token.id).await.unwrap();
    assert_eq!(revoked.id, token.id);
    assert!(revoked.revoked_at.is_some());

    let error = store
        .create_collab_invite_token(&library.slug, "live.md", "owner", None)
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::InvalidPath(_)));
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
async fn put_after_delete_same_path_creates_new_document_identity_and_history() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("recreate").await.unwrap();

    let first = store
        .put_document(
            &library.slug,
            "notes/daily.md",
            b"old".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .delete_document(&library.slug, "notes/daily.md", DocumentSource::Rest)
        .await
        .unwrap();
    let second = store
        .put_document(
            &library.slug,
            "notes/daily.md",
            b"new".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    assert_ne!(first.document.id, second.document.id);
    assert_eq!(
        store
            .get_document(&library.slug, "notes/daily.md")
            .await
            .unwrap()
            .content,
        b"new"
    );
    let history = store
        .version_history(&library.slug, "notes/daily.md")
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].latest_version_id, second.version.id);
    assert!(store
        .document_version(&library.slug, "notes/daily.md", &first.version.id)
        .await
        .is_err());
}

#[tokio::test]
async fn explicit_transaction_recreate_same_path_uses_new_document_identity() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("txrecreate").await.unwrap();

    let first = store
        .put_document(
            &library.slug,
            "notes/daily.md",
            b"old".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .delete_document(&library.slug, "notes/daily.md", DocumentSource::Rest)
        .await
        .unwrap();

    let tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("agent".to_string()),
            Some("recreate".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let staged = store
        .stage_put(
            &tx.id,
            "notes/daily.md",
            b"new".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();

    assert_ne!(staged.document_id, first.document.id);
    store.commit_transaction(&tx.id).await.unwrap();
    let recreated = store
        .get_document(&library.slug, "notes/daily.md")
        .await
        .unwrap();
    assert_eq!(recreated.id, staged.document_id);
    assert_eq!(recreated.content, b"new");
    assert_eq!(
        store
            .version_history(&library.slug, "notes/daily.md")
            .await
            .unwrap()
            .iter()
            .map(|version| version.latest_version_id.as_str())
            .collect::<Vec<_>>(),
        vec![staged.id.as_str()]
    );
}

#[tokio::test]
async fn autosave_tagged_writes_keep_raw_versions_but_group_history() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("autosavehistory").await.unwrap();
    let transaction = || TransactionMetadata {
        actor: Some("browser".to_string()),
        message: Some("Autosaved edits".to_string()),
        provenance: serde_json::json!({
            "history": {"kind": "autosave", "reason": "typing", "session_id": "browser:s1"}
        }),
    };

    let first = store
        .put_document_with_transaction(
            &library.slug,
            "notes/daily.md",
            b"one".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
            Some("browser:s1".to_string()),
            transaction(),
        )
        .await
        .unwrap();
    store
        .put_document_with_transaction(
            &library.slug,
            "notes/daily.md",
            b"two".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(first.version.id),
            Some("browser:s1".to_string()),
            transaction(),
        )
        .await
        .unwrap();

    let raw = store
        .raw_version_history(&library.slug, "notes/daily.md")
        .await
        .unwrap();
    let grouped = store
        .version_history(&library.slug, "notes/daily.md")
        .await
        .unwrap();

    assert_eq!(raw.len(), 2);
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped[0].raw_version_count, 2);
    assert_eq!(grouped[0].latest_version_id, raw[0].id);
    assert_eq!(grouped[0].earliest_version_id, raw[1].id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_staged_creates_same_path_publish_by_staged_document_id() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("stagedcreates").await.unwrap();

    let tx1 = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("agent-a".to_string()),
            Some("create a".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let tx2 = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("agent-b".to_string()),
            Some("create b".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let staged1 = store
        .stage_put(
            &tx1.id,
            "notes/race.md",
            b"one".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();
    let staged2 = store
        .stage_put(
            &tx2.id,
            "notes/race.md",
            b"two".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();

    assert_ne!(staged1.document_id, staged2.document_id);
    store.commit_transaction(&tx2.id).await.unwrap();
    let visible = store
        .get_document(&library.slug, "notes/race.md")
        .await
        .unwrap();
    assert_eq!(visible.id, staged2.document_id);
    assert_eq!(visible.version.id, staged2.id);
    assert_eq!(visible.content, b"two");

    let error = store.commit_transaction(&tx1.id).await.unwrap_err();
    assert!(error.to_string().contains("precondition failed"));
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
async fn markdown_links_without_document_targets_are_external() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("externallinks").await.unwrap();

    store
        .put_document(
            &library.slug,
            "source.md",
            b"[site](https://example.com)\n\n[anchor](#section)\n\n[gone](gone.md)\n".to_vec(),
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

    let external_url = outgoing
        .links
        .iter()
        .find(|link| {
            link.target_kind == "markdown_link" && link.target_text == "https://example.com"
        })
        .unwrap();
    assert!(!external_url.resolved);
    assert_eq!(
        serde_json::to_value(external_url).unwrap()["resolution_status"],
        "external"
    );

    let fragment = outgoing
        .links
        .iter()
        .find(|link| {
            link.target_kind == "markdown_link" && link.target_anchor.as_deref() == Some("section")
        })
        .unwrap();
    assert!(!fragment.resolved);
    assert_eq!(
        serde_json::to_value(fragment).unwrap()["resolution_status"],
        "external"
    );

    // A link to a missing document is broken, not external — it intended a document target.
    let broken = outgoing
        .links
        .iter()
        .find(|link| link.target_kind == "markdown_link" && link.target_text == "gone.md")
        .unwrap();
    assert!(!broken.resolved);
    assert_eq!(
        serde_json::to_value(broken).unwrap()["resolution_status"],
        "unresolved"
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
async fn stale_empty_lock_file_does_not_block_store_open_and_is_removed_on_drop() {
    let root = tempfile::tempdir().unwrap();
    let lock_path = root.path().join("quarry.lock");
    std::fs::write(&lock_path, "").unwrap();
    let config = StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    };

    let store = QuarryStore::open(config).await.unwrap();
    assert!(lock_path.exists());

    drop(store);
    assert!(!lock_path.exists());
}

#[tokio::test]
async fn dropped_store_removes_lock_file() {
    let root = tempfile::tempdir().unwrap();
    let lock_path = root.path().join("quarry.lock");
    let config = StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    };

    let store = QuarryStore::open(config).await.unwrap();
    assert!(lock_path.exists());

    drop(store);
    assert!(!lock_path.exists());
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
    assert_eq!(event.doc_id.as_deref(), Some(write.document.id.as_str()));
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
    assert_eq!(event.doc_id.as_deref(), Some(write.document.id.as_str()));
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
async fn document_mutation_events_include_origin_and_document_identity() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("origin-events").await.unwrap();
    let mut events = store.subscribe_events();

    let write = store
        .put_document_with_origin(
            &library.slug,
            "notes/a.md",
            b"a".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
            Some("browser:origin-1".to_string()),
        )
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::DocumentPut);
    assert_eq!(event.doc_id.as_deref(), Some(write.document.id.as_str()));
    assert_eq!(event.origin_id.as_deref(), Some("browser:origin-1"));
    let _links = events.recv().await.unwrap();

    store
        .move_document_with_origin(
            &library.slug,
            "notes/a.md",
            "notes/b.md",
            DocumentSource::Rest,
            Some("browser:origin-1".to_string()),
        )
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::DocumentMove);
    assert_eq!(event.doc_id.as_deref(), Some(write.document.id.as_str()));
    assert_eq!(event.origin_id.as_deref(), Some("browser:origin-1"));
    let _links = events.recv().await.unwrap();

    store
        .delete_document_with_origin(
            &library.slug,
            "notes/b.md",
            DocumentSource::Rest,
            Some("browser:origin-1".to_string()),
        )
        .await
        .unwrap();
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, StoreEventKind::DocumentDelete);
    assert_eq!(event.doc_id.as_deref(), Some(write.document.id.as_str()));
    assert_eq!(event.origin_id.as_deref(), Some("browser:origin-1"));
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
    let source_document_id = store
        .get_document(&library.slug, "drafts/source.md")
        .await
        .unwrap()
        .id;

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
    assert_eq!(document.id, source_document_id);
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
    let source_document_id = store
        .get_document(&library.slug, "drafts/source.md")
        .await
        .unwrap()
        .id;
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
    assert_eq!(document.id, source_document_id);
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
async fn opening_old_schema_migrates_documents_to_active_path_uniqueness() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("quarry.db");
    {
        let db = turso::Builder::new_local(db_path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE documents(
              id TEXT PRIMARY KEY,
              library_id TEXT NOT NULL,
              path TEXT NOT NULL,
              head_version_id TEXT,
              deleted_at TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              UNIQUE(library_id, path)
            );
            "#,
        )
        .await
        .unwrap();
    }

    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("migrated").await.unwrap();
    let first = store
        .put_document(
            &library.slug,
            "same.md",
            b"old".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .delete_document(&library.slug, "same.md", DocumentSource::Rest)
        .await
        .unwrap();
    let second = store
        .put_document(
            &library.slug,
            "same.md",
            b"new".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    assert_ne!(first.document.id, second.document.id);
    drop(store);

    let db = turso::Builder::new_local(db_path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let document_indexes = index_names(&conn, "documents").await;
    assert!(document_indexes.contains("idx_documents_active_library_path"));
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

    assert!(document_indexes.contains("idx_documents_active_library_path"));
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

// ---------------------------------------------------------------------------
// Canonical block rows (Phase 1 of the session-scoped collaboration rewrite).
// ---------------------------------------------------------------------------

async fn open_block_store(root: &std::path::Path) -> QuarryStore {
    QuarryStore::open(StoreConfig {
        db_path: root.join("quarry.db"),
        cas_path: root.join("cas"),
        lock_path: None,
    })
    .await
    .unwrap()
}

const BLOCK_FIXTURE: &str = "\
---
title: Plan
tags:
  - alpha
  - beta
---
# Heading

Body with **bold** text and a [link](https://example.test/docs).

- item one
    - nested item

```rust
fn main() {}
```

<div>
opaque html
</div>
";

const NORMALIZED_BLOCK_FIXTURE: &str = "\
---
tags:
- alpha
- beta
title: Plan
---
# Heading

Body with **bold** text and a [link](https://example.test/docs).

- item one
    - nested item

```rust
fn main() {}
```

<div>
opaque html
</div>
";

#[tokio::test]
async fn imports_block_document_and_exports_stably_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("blocks").await.unwrap();

    let outcome = store
        .import_block_document(
            &library.slug,
            "notes/plan.md",
            BLOCK_FIXTURE,
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    // Frontmatter lands in document metadata (the existing mechanism)...
    assert_eq!(
        outcome.version.metadata,
        serde_json::json!({"title": "Plan", "tags": ["alpha", "beta"]})
    );
    // ...and the normalized export is the stored version content.
    let stored = String::from_utf8(outcome.version.inline_content.clone().unwrap()).unwrap();
    assert_eq!(stored, NORMALIZED_BLOCK_FIXTURE);
    let exported = store
        .export_block_document(&outcome.document.id)
        .await
        .unwrap();
    assert_eq!(exported, NORMALIZED_BLOCK_FIXTURE);

    let tree = store.load_block_tree(&outcome.document.id).await.unwrap();
    let shape: Vec<(&str, bool)> = tree
        .iter()
        .map(|row| (row.block_type.as_str(), row.parent_block_id.is_some()))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("h1", false),
            ("p", false),
            ("p", false),
            ("p", false),
            ("code_block", false),
            ("code_line", true),
            ("raw_markdown", false),
        ]
    );

    // Re-importing the export is byte-stable (one-time normalization done).
    let reimported = store
        .import_block_document(
            &library.slug,
            "notes/plan.md",
            &exported,
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(outcome.version.id.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(reimported.version.inline_content.clone().unwrap()).unwrap(),
        NORMALIZED_BLOCK_FIXTURE
    );

    drop(store);

    let reopened = open_block_store(root.path()).await;
    let restarted_tree = reopened
        .load_block_tree(&outcome.document.id)
        .await
        .unwrap();
    assert_eq!(restarted_tree.len(), tree.len());
    assert_eq!(
        reopened
            .export_block_document(&outcome.document.id)
            .await
            .unwrap(),
        NORMALIZED_BLOCK_FIXTURE
    );
    assert_eq!(
        reopened
            .get_document(&library.slug, "notes/plan.md")
            .await
            .unwrap()
            .content,
        NORMALIZED_BLOCK_FIXTURE.as_bytes()
    );
}

#[tokio::test]
async fn replace_block_tree_swaps_the_whole_row_set_transactionally() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("blocks").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "swap.md",
            "Original paragraph.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let mut next = 0u32;
    let replacement = quarry_collab_codec::markdown_to_block_rows("# New\n\nReplaced.\n", || {
        next += 1;
        format!("swap-{next}")
    })
    .unwrap();
    store
        .replace_block_tree(&outcome.document.id, &replacement)
        .await
        .unwrap();

    let tree = store.load_block_tree(&outcome.document.id).await.unwrap();
    assert_eq!(tree, replacement);
    assert_eq!(
        store
            .export_block_document(&outcome.document.id)
            .await
            .unwrap(),
        "# New\n\nReplaced.\n"
    );

    let missing = store
        .replace_block_tree("not-a-document", &replacement)
        .await
        .unwrap_err();
    assert!(matches!(missing, QuarryError::NotFound(_)));
}

#[tokio::test]
async fn block_review_anchors_validate_utf16_boundaries_and_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("anchors").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "anchors.md",
            "Anchor target 👍 emoji.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let tree = store.load_block_tree(&outcome.document.id).await.unwrap();
    let block_id = tree[0].block_id.clone();
    // "Anchor target " (14) + 👍 (2, surrogate pair) + " emoji." (7) = 23.
    assert_eq!(tree[0].text, "Anchor target 👍 emoji.");
    let item = |start, end, state| NewBlockReviewItem {
        document_id: outcome.document.id.clone(),
        block_id: block_id.clone(),
        kind: BlockReviewKind::Comment,
        start_offset: start,
        end_offset: end,
        body: Some("note".to_string()),
        replacement: None,
        author: Some("user".to_string()),
        state,
        quote: None,
        context_before: None,
        context_after: None,
        parent_item_id: None,
    };

    // Anchors at the exact block boundaries are valid.
    let full = store
        .put_block_review_item(item(0, 23, BlockReviewState::Open))
        .await
        .unwrap();
    // The emoji's surrogate pair is two UTF-16 units: [14, 16) is exact...
    let emoji = store
        .put_block_review_item(item(14, 16, BlockReviewState::Open))
        .await
        .unwrap();
    // ...and an offset inside the pair is rejected.
    let split_pair = store
        .put_block_review_item(item(14, 15, BlockReviewState::Open))
        .await
        .unwrap_err();
    assert!(matches!(split_pair, QuarryError::InvalidInput(_)));
    let past_end = store
        .put_block_review_item(item(0, 24, BlockReviewState::Open))
        .await
        .unwrap_err();
    assert!(matches!(past_end, QuarryError::InvalidInput(_)));
    let inverted = store
        .put_block_review_item(item(9, 4, BlockReviewState::Open))
        .await
        .unwrap_err();
    assert!(matches!(inverted, QuarryError::InvalidInput(_)));
    // A collapsed range means orphaned at the row layer: open is rejected,
    // orphaned is stored.
    let collapsed_open = store
        .put_block_review_item(item(5, 5, BlockReviewState::Open))
        .await
        .unwrap_err();
    assert!(matches!(collapsed_open, QuarryError::InvalidInput(_)));
    let collapsed_orphaned = store
        .put_block_review_item(item(5, 5, BlockReviewState::Orphaned))
        .await
        .unwrap();
    let unknown_block = store
        .put_block_review_item(NewBlockReviewItem {
            block_id: "missing-block".to_string(),
            ..item(0, 1, BlockReviewState::Open)
        })
        .await
        .unwrap_err();
    assert!(matches!(unknown_block, QuarryError::NotFound(_)));

    drop(store);

    let reopened = open_block_store(root.path()).await;
    let items = reopened
        .list_block_review_items(&outcome.document.id)
        .await
        .unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.contains(&full));
    assert!(items.contains(&emoji));
    assert!(items.contains(&collapsed_orphaned));
}

#[tokio::test]
async fn raw_documents_keep_the_byte_path_untouched() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("raw").await.unwrap();
    let bytes = vec![0u8, 159, 146, 150, 255, 0, 13, 10];
    let outcome = store
        .put_document(
            &library.slug,
            "assets/data.bin",
            bytes.clone(),
            serde_json::json!({"content_type": "application/octet-stream"}),
            "application/octet-stream",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    assert_eq!(
        quarry_storage::document_kind("assets/data.bin", "application/octet-stream"),
        quarry_storage::DocumentKind::RawDocument
    );
    assert_eq!(
        quarry_storage::document_kind("notes/plan.md", "text/markdown"),
        quarry_storage::DocumentKind::BlockDocument
    );
    assert_eq!(
        quarry_storage::document_kind("upper/CASE.MD", "application/octet-stream"),
        quarry_storage::DocumentKind::BlockDocument
    );

    let refused = store
        .import_block_document(
            &library.slug,
            "assets/data.bin",
            "# not markdown\n",
            serde_json::json!({}),
            "application/octet-stream",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap_err();
    assert!(matches!(refused, QuarryError::Unsupported(_)));
    let export_refused = store
        .export_block_document(&outcome.document.id)
        .await
        .unwrap_err();
    assert!(matches!(export_refused, QuarryError::Unsupported(_)));

    assert!(store
        .load_block_tree(&outcome.document.id)
        .await
        .unwrap()
        .is_empty());

    drop(store);

    let reopened = open_block_store(root.path()).await;
    assert_eq!(
        reopened
            .get_document(&library.slug, "assets/data.bin")
            .await
            .unwrap()
            .content,
        bytes
    );
    assert!(reopened
        .load_block_tree(&outcome.document.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn import_surfaces_the_codecs_typed_unsupported_error() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("typed").await.unwrap();

    let error = store
        .import_block_document(
            &library.slug,
            "critic.md",
            "Edited {==this==}{>>why<<} text.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap_err();

    let QuarryError::UnsupportedMarkdown(inner) = error else {
        panic!("expected the codec's typed Unsupported error, got {error:?}");
    };
    assert_eq!(
        inner,
        quarry_collab_codec::Unsupported::new("critic markup")
    );
    // The rejected import left no document behind.
    assert!(store
        .get_document(&library.slug, "critic.md")
        .await
        .is_err());
}

#[tokio::test]
async fn block_shadow_bases_and_block_transactions_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("bases").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "doc.md",
            "Shadow me.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let document_id = outcome.document.id.clone();

    let base = store
        .put_block_shadow_base(
            "git",
            "peer-1:doc.md",
            &document_id,
            "Shadow me.\n",
            Some(outcome.version.id.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .block_shadow_base("git", "peer-1:doc.md", &document_id)
            .await
            .unwrap(),
        Some(base)
    );
    // Upsert replaces the base for the same scope.
    store
        .put_block_shadow_base("git", "peer-1:doc.md", &document_id, "Updated.\n", None)
        .await
        .unwrap();
    let updated = store
        .block_shadow_base("git", "peer-1:doc.md", &document_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.base_markdown, "Updated.\n");
    assert_eq!(updated.base_version_id, None);
    assert_eq!(
        store
            .block_shadow_base("fuse", "peer-1:doc.md", &document_id)
            .await
            .unwrap(),
        None
    );

    let ops = serde_json::json!([{"op": "replace_block_content", "block_id": "b1"}]);
    let recorded = store
        .record_block_transaction(&document_id, "ctx-1", "agent", None, ops.clone(), None)
        .await
        .unwrap();
    assert_eq!(
        store
            .block_transaction(&document_id, "ctx-1")
            .await
            .unwrap(),
        Some(recorded)
    );
    // client_tx_id is unique per document: duplicates conflict (idempotent
    // replay answers from the stored record in Phase 2).
    let duplicate = store
        .record_block_transaction(&document_id, "ctx-1", "agent", None, ops, None)
        .await
        .unwrap_err();
    assert!(matches!(duplicate, QuarryError::Conflict(_)));
}

#[tokio::test]
async fn legacy_put_clears_the_block_projection_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("legacy").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "doc.md",
            "Imported paragraph.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let document_id = outcome.document.id.clone();
    let block_id = store.load_block_tree(&document_id).await.unwrap()[0]
        .block_id
        .clone();
    store
        .put_block_review_item(NewBlockReviewItem {
            document_id: document_id.clone(),
            block_id,
            kind: BlockReviewKind::Comment,
            start_offset: 0,
            end_offset: 8,
            body: Some("note".to_string()),
            replacement: None,
            author: None,
            state: BlockReviewState::Open,
            quote: None,
            context_before: None,
            context_after: None,
            parent_item_id: None,
        })
        .await
        .unwrap();

    // A legacy put bypasses the import path...
    store
        .put_document(
            &library.slug,
            "doc.md",
            b"Rewritten outside the block path.\n".to_vec(),
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    // ...so the block projection is dropped rather than serving stale rows.
    assert!(store
        .load_block_tree(&document_id)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .list_block_review_items(&document_id)
        .await
        .unwrap()
        .is_empty());
    let stale = store.export_block_document(&document_id).await.unwrap_err();
    assert!(matches!(stale, QuarryError::NotFound(_)));
    // The byte path still serves the legacy write.
    assert_eq!(
        store
            .get_document(&library.slug, "doc.md")
            .await
            .unwrap()
            .content,
        b"Rewritten outside the block path.\n"
    );

    // Re-importing restores the projection.
    store
        .import_block_document(
            &library.slug,
            "doc.md",
            "Imported again.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    assert_eq!(
        store.export_block_document(&document_id).await.unwrap(),
        "Imported again.\n"
    );
}

#[tokio::test]
async fn delete_document_removes_the_block_projection() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("deleting").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "gone.md",
            "Doomed paragraph.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let document_id = outcome.document.id.clone();
    let block_id = store.load_block_tree(&document_id).await.unwrap()[0]
        .block_id
        .clone();
    store
        .put_block_review_item(NewBlockReviewItem {
            document_id: document_id.clone(),
            block_id,
            kind: BlockReviewKind::Suggestion,
            start_offset: 0,
            end_offset: 6,
            body: None,
            replacement: Some("Saved".to_string()),
            author: None,
            state: BlockReviewState::Open,
            quote: None,
            context_before: None,
            context_after: None,
            parent_item_id: None,
        })
        .await
        .unwrap();

    store
        .delete_document(&library.slug, "gone.md", DocumentSource::Rest)
        .await
        .unwrap();

    assert!(store
        .load_block_tree(&document_id)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .list_block_review_items(&document_id)
        .await
        .unwrap()
        .is_empty());
    let exported = store.export_block_document(&document_id).await.unwrap_err();
    assert!(matches!(exported, QuarryError::NotFound(_)));
}

#[tokio::test]
async fn empty_body_import_canonicalizes_to_one_empty_paragraph_row() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("empty").await.unwrap();
    let outcome = store
        .import_block_document(
            &library.slug,
            "stub.md",
            "---\ntitle: Stub\n---\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let tree = store.load_block_tree(&outcome.document.id).await.unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].block_type, "p");
    assert_eq!(tree[0].text, "");
    assert_eq!(
        store
            .export_block_document(&outcome.document.id)
            .await
            .unwrap(),
        "---\ntitle: Stub\n---\n"
    );
}

#[tokio::test]
async fn block_mutation_commit_applies_rows_version_history_and_replays_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("mutate").await.unwrap();
    let imported = store
        .import_block_document(
            &library.slug,
            "doc.md",
            "First paragraph.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let state = store
        .block_mutation_state(&library.slug, "doc.md", "ctx-1")
        .await
        .unwrap();
    assert_eq!(state.document_id, imported.document.id);
    assert_eq!(state.head_version_id, imported.version.id);
    assert!(!state.projection_missing);
    assert!(state.replay.is_none());
    assert!(state.version_ids.contains(&imported.version.id));

    let mut rows = state.rows.clone();
    rows[0].text = "Rewritten paragraph.".to_string();
    let commit = BlockMutationCommit {
        document_id: state.document_id.clone(),
        expected_head_version_id: state.head_version_id.clone(),
        client_tx_id: "ctx-1".to_string(),
        actor_kind: "agent".to_string(),
        actor_id: Some("agent-7".to_string()),
        transaction_actor: Some("Agent Seven".to_string()),
        source: DocumentSource::Rest,
        recorded_ops: serde_json::json!({"ops": [], "ack": {"status": "committed"}}),
        metadata: state.metadata.clone(),
        content_type: state.content_type.clone(),
        rows: rows.clone(),
        review_items: state.review_items.clone(),
        normalized_markdown: "Rewritten paragraph.\n".to_string(),
    };
    let BlockMutationOutcome::Applied { outcome, record } = store
        .commit_block_mutation(&library.slug, commit.clone())
        .await
        .unwrap()
    else {
        panic!("first commit must apply");
    };
    assert_eq!(
        record.resulting_version_id,
        Some(outcome.version.id.clone())
    );
    assert_eq!(
        store
            .export_block_document(&state.document_id)
            .await
            .unwrap(),
        "Rewritten paragraph.\n"
    );
    let document = store.get_document(&library.slug, "doc.md").await.unwrap();
    assert_eq!(document.version.id, outcome.version.id);
    assert_eq!(
        String::from_utf8(document.content).unwrap(),
        "Rewritten paragraph.\n"
    );

    // Duplicate client_tx_id replays the stored record without re-applying.
    let BlockMutationOutcome::Replayed(replayed) = store
        .commit_block_mutation(&library.slug, commit)
        .await
        .unwrap()
    else {
        panic!("duplicate commit must replay");
    };
    assert_eq!(replayed, record);
    let after_replay = store.get_document(&library.slug, "doc.md").await.unwrap();
    assert_eq!(after_replay.version.id, outcome.version.id);
}

#[tokio::test]
async fn block_mutation_commit_rejects_a_moved_head() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("stale").await.unwrap();
    let imported = store
        .import_block_document(
            &library.slug,
            "doc.md",
            "Original.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let state = store
        .block_mutation_state(&library.slug, "doc.md", "ctx-1")
        .await
        .unwrap();
    // Another write moves the head between load and commit.
    store
        .import_block_document(
            &library.slug,
            "doc.md",
            "Moved on.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let error = store
        .commit_block_mutation(
            &library.slug,
            BlockMutationCommit {
                document_id: state.document_id.clone(),
                expected_head_version_id: state.head_version_id.clone(),
                client_tx_id: "ctx-1".to_string(),
                actor_kind: "agent".to_string(),
                actor_id: None,
                transaction_actor: None,
                source: DocumentSource::Rest,
                recorded_ops: serde_json::json!({}),
                metadata: state.metadata.clone(),
                content_type: state.content_type.clone(),
                rows: state.rows.clone(),
                review_items: vec![],
                normalized_markdown: "Original.\n".to_string(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::PreconditionFailed(_)));
    let _ = imported;
}

#[tokio::test]
async fn block_mutation_state_materializes_rows_for_legacy_written_documents() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("legacy-state").await.unwrap();
    // A legacy put creates a markdown document with no block projection.
    store
        .put_document(
            &library.slug,
            "legacy.md",
            b"# Title\n\nBody text.\n".to_vec(),
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let state = store
        .block_mutation_state(&library.slug, "legacy.md", "ctx-1")
        .await
        .unwrap();
    assert!(state.projection_missing);
    let shape: Vec<&str> = state
        .rows
        .iter()
        .map(|row| row.block_type.as_str())
        .collect();
    assert_eq!(shape, vec!["h1", "p"]);
    assert_eq!(state.rows[1].text, "Body text.");
    // Nothing was persisted by the read.
    assert!(store
        .load_block_tree(&state.document_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn block_mutation_commit_rejects_open_review_items_with_dead_anchors() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store.create_library("anchors-guard").await.unwrap();
    store
        .import_block_document(
            &library.slug,
            "doc.md",
            "Anchored text.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let state = store
        .block_mutation_state(&library.slug, "doc.md", "ctx-1")
        .await
        .unwrap();
    store
        .put_block_review_item(NewBlockReviewItem {
            document_id: state.document_id.clone(),
            block_id: state.rows[0].block_id.clone(),
            kind: BlockReviewKind::Comment,
            start_offset: 0,
            end_offset: 8,
            body: Some("note".to_string()),
            replacement: None,
            author: None,
            state: BlockReviewState::Open,
            quote: None,
            context_before: None,
            context_after: None,
            parent_item_id: None,
        })
        .await
        .unwrap();
    let state = store
        .block_mutation_state(&library.slug, "doc.md", "ctx-1")
        .await
        .unwrap();

    // Shrinking the text below the anchor without adjusting the open anchor
    // must be rejected: the commit validates the final review set.
    let mut rows = state.rows.clone();
    rows[0].text = "Tiny".to_string();
    let error = store
        .commit_block_mutation(
            &library.slug,
            BlockMutationCommit {
                document_id: state.document_id.clone(),
                expected_head_version_id: state.head_version_id.clone(),
                client_tx_id: "ctx-2".to_string(),
                actor_kind: "agent".to_string(),
                actor_id: None,
                transaction_actor: None,
                source: DocumentSource::Rest,
                recorded_ops: serde_json::json!({}),
                metadata: state.metadata.clone(),
                content_type: state.content_type.clone(),
                rows,
                review_items: state.review_items.clone(),
                normalized_markdown: "Tiny\n".to_string(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::InvalidInput(_)));
}
