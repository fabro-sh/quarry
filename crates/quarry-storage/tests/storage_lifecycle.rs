#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for storage lifecycle fixtures"
)]

use anyhow::Context as _;
use quarry_core::{
    DocumentSource, DocumentVersion, INLINE_CONTENT_THRESHOLD, QuarryError, WritePrecondition,
};
use quarry_storage::{
    BlockMutationCommit, BlockMutationOutcome, BlockReviewItem, BlockReviewKind, BlockReviewState,
    DocumentScopeRef, NewBlockReviewItem, QuarryStore, StoreConfig, StoreEventKind, TmpTtl,
    TransactionMetadata, group_version_history,
};
use std::{io, time::Duration};

type TestResult = anyhow::Result<()>;

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
        id: id.to_string().into(),
        document_id: document_id.to_string().into(),
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
        created_at: created_at.to_string().into(),
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: alpha.slug.to_string(),
            path: ("notes/plan.md").to_string(),
            content: b"one".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown","topic":"plan"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    assert!(small.version.content_hash.is_none());
    assert_eq!(small.version.inline_content.as_deref(), Some(&b"one"[..]));

    let large_bytes = vec![b'x'; INLINE_CONTENT_THRESHOLD + 1];
    let large = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: alpha.slug.to_string(),
            path: ("assets/large.bin").to_string(),
            content: large_bytes.clone(),
            metadata: serde_json::json!({"content_type":"application/octet-stream"}),
            content_type: ("application/octet-stream").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let expected_large_hash = quarry_cas::DiskCas::hash(&large_bytes);
    assert_eq!(
        large.version.content_hash.as_deref(),
        Some(expected_large_hash.as_str())
    );
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: alpha.slug.to_string(),
            path: ("notes/plan.md").to_string(),
            content: b"two".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown","topic":"plan"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::IfMatch(small.version.id.to_string()),
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
    assert!(
        store
            .get_document(&alpha.slug, "notes/plan.md")
            .await
            .is_err()
    );
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
    assert!(
        store
            .get_document(&alpha.slug, "notes/renamed.md")
            .await
            .is_err()
    );

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
async fn legacy_database_with_collab_recovery_states_reopens_cleanly() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("quarry.db");

    // A database from before Phase 7 carries the recovery-state table.
    let db = turso::Builder::new_local(db_path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    conn.execute(
        "CREATE TABLE collab_recovery_states(
           document_id TEXT PRIMARY KEY,
           base_version_id TEXT,
           update_v1 BLOB NOT NULL,
           dirty INTEGER NOT NULL,
           updated_at TEXT NOT NULL
         )",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO collab_recovery_states VALUES ('doc-1', NULL, x'01', 1, 'now')",
        (),
    )
    .await
    .unwrap();
    drop(conn);
    drop(db);

    // Opening the store drops the table and the store works normally.
    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("legacy").await.unwrap();
    drop(store);

    let db = turso::Builder::new_local(db_path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'collab_recovery_states'",
            (),
        )
        .await
        .unwrap();
    assert!(
        rows.next().await.unwrap().is_none(),
        "table dropped at open"
    );
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("live.md").to_string(),
            content: b"markdown head".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
    let revoked_at = revoked
        .revoked_at
        .as_deref()
        .expect("revoked token should record a timestamp");
    chrono::DateTime::parse_from_rfc3339(revoked_at)
        .expect("revoked token timestamp should parse as RFC 3339");

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
                .put_document(quarry_storage::PutDocumentRequest {
                    library: library.to_string(),
                    path: format!("notes/{index}.md").to_string(),
                    content: format!("document {index}\n").into_bytes(),
                    metadata: serde_json::json!({"content_type":"text/markdown"}),
                    content_type: ("text/markdown").to_string(),
                    source: DocumentSource::Rest,
                    precondition: WritePrecondition::None,
                    origin_id: None,
                    transaction: quarry_storage::TransactionMetadata::default(),
                })
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/daily.md").to_string(),
            content: b"old".to_vec(),
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
        .delete_document(&library.slug, "notes/daily.md", DocumentSource::Rest)
        .await
        .unwrap();
    let second = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/daily.md").to_string(),
            content: b"new".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
    assert!(
        store
            .document_version(&library.slug, "notes/daily.md", &first.version.id)
            .await
            .is_err()
    );
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/daily.md").to_string(),
            content: b"old".to_vec(),
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
        provenance: Some(serde_json::json!({
            "history": {"kind": "autosave", "reason": "typing", "session_id": "browser:s1"}
        })),
    };

    let first = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/daily.md").to_string(),
            content: b"one".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: Some("browser:s1".to_string()),
            transaction: transaction(),
        })
        .await
        .unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/daily.md").to_string(),
            content: b"two".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::IfMatch(first.version.id.to_string()),
            origin_id: Some("browser:s1".to_string()),
            transaction: transaction(),
        })
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("target.md").to_string(),
            content: b"# Target Heading\n\nTarget body.\n".to_vec(),
            metadata: serde_json::json!({
                "content_type": "text/markdown",
                "aliases": ["Target Alias"]
            }),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();

    let source = store
        .put_document(quarry_storage::PutDocumentRequest {
library: library.slug.to_string(),
path: ("source.md").to_string(),
content: b"See [[Target Alias#Target Heading|alias]], ![[target]], [target](target.md), [[Missing]], and #tag.\n".to_vec(),
metadata: serde_json::json!({"content_type":"text/markdown"}),
content_type: ("text/markdown").to_string(),
source: DocumentSource::Rest,
precondition: WritePrecondition::None,
origin_id: None,
transaction: quarry_storage::TransactionMetadata::default(),
})
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
    assert!(
        outgoing
            .links
            .iter()
            .any(|link| link.target_kind == "tag" && link.target_text == "tag")
    );
    assert!(
        outgoing
            .links
            .iter()
            .any(|link| link.target_kind == "wiki_link"
                && link.target_text == "Missing"
                && !link.resolved)
    );

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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("raw.bin").to_string(),
            content: b"[[target]] should not be indexed from binary content".to_vec(),
            metadata: serde_json::json!({"content_type":"application/octet-stream"}),
            content_type: ("application/octet-stream").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    assert!(
        store
            .outgoing_links(&library.slug, "raw.bin")
            .await
            .unwrap()
            .links
            .is_empty()
    );
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
    assert!(
        focused_graph
            .nodes
            .iter()
            .any(|node| node.path == "target.md")
    );
    assert!(
        focused_graph
            .nodes
            .iter()
            .any(|node| node.path == "source.md")
    );
    assert!(
        !focused_graph
            .nodes
            .iter()
            .any(|node| node.path == "raw.bin")
    );

    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("source.md").to_string(),
            content: b"No links now.\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::IfMatch(source.version.id.to_string()),
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    assert!(
        store
            .outgoing_links(&library.slug, "source.md")
            .await
            .unwrap()
            .links
            .is_empty()
    );
    assert!(
        store
            .backlinks(&library.slug, "target.md")
            .await
            .unwrap()
            .links
            .is_empty()
    );

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

    assert!(
        store
            .backlinks(&library.slug, "target.md")
            .await
            .unwrap()
            .links
            .iter()
            .any(|link| link.src_path == "source.md")
    );
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("guide.md").to_string(),
            content: b"# Deep Section\n\nGuide body.\n".to_vec(),
            metadata: serde_json::json!({
                "content_type": "text/markdown",
                "title": "Guide",
                "aliases": ["Shortcut"]
            }),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();

    let alias_suggestions = serde_json::to_value(
        store
            .suggest_documents(&library.slug, "shortcut", Some(10))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        alias_suggestions
            .as_array()
            .unwrap()
            .iter()
            .any(|suggestion| {
                suggestion["path"] == "guide.md"
                    && suggestion["match_type"] == "alias"
                    && suggestion["matched_text"] == "Shortcut"
            })
    );

    let heading_suggestions = serde_json::to_value(
        store
            .suggest_documents(&library.slug, "deep", Some(10))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        heading_suggestions
            .as_array()
            .unwrap()
            .iter()
            .any(|suggestion| {
                suggestion["path"] == "guide.md"
                    && suggestion["match_type"] == "heading"
                    && suggestion["matched_text"] == "Deep Section"
                    && suggestion["target_anchor"] == "Deep Section"
            })
    );
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("guide.md").to_string(),
            content: b"---\naliases:\n  - Front Alias\n---\n# Guide\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
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
            path: ("source.md").to_string(),
            content: b"See [[Front Alias]].\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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

    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("alpha/target.md").to_string(),
            content: b"# Target\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
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
            path: ("omega/target.md").to_string(),
            content: b"# Target\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
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
            path: ("source.md").to_string(),
            content: b"See [[target]].\n".to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("source.md").to_string(),
            content: b"[site](https://example.com)\n\n[anchor](#section)\n\n[gone](gone.md)\n"
                .to_vec(),
            metadata: serde_json::json!({"content_type": "text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
async fn link_index_tracks_moves_and_deletes() -> TestResult {
    let root = tempfile::tempdir().context("create temp dir")?;
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .context("open store")?;
    let library = store
        .create_library("links")
        .await
        .context("create links library")?;

    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("target.md").to_string(),
            content: b"# Target\n".to_vec(),
            metadata: serde_json::json!({
                "content_type": "text/markdown",
                "aliases": ["target"]
            }),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("put target document")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("source.md").to_string(),
            content: b"See [[target]].\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("put source document")?;

    store
        .move_document(
            &library.slug,
            "target.md",
            "renamed.md",
            DocumentSource::Rest,
        )
        .await
        .context("move target document")?;
    let backlinks = store
        .backlinks(&library.slug, "renamed.md")
        .await
        .context("load backlinks after target move")?;
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
        .context("move source document")?;
    let backlinks = store
        .backlinks(&library.slug, "renamed.md")
        .await
        .context("load backlinks after source move")?;
    assert!(
        backlinks
            .links
            .iter()
            .any(|link| link.src_path == "folder/source.md")
    );

    store
        .delete_document(&library.slug, "folder/source.md", DocumentSource::Rest)
        .await
        .context("delete moved source document")?;
    assert!(
        store
            .backlinks(&library.slug, "renamed.md")
            .await
            .context("load backlinks after source delete")?
            .links
            .is_empty()
    );
    Ok(())
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("docs/a.md").to_string(),
            content: b"base".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown","topic":"old"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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

    assert!(
        store
            .get_document(&library.slug, "docs/new.md")
            .await
            .is_err()
    );
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
    assert!(
        store
            .get_document(&library.slug, "docs/a.md")
            .await
            .is_err()
    );
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
    assert!(
        store
            .get_document(&library.slug, "docs/rolled.bin")
            .await
            .is_err()
    );
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("docs/a.md").to_string(),
            content: b"base".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("docs/a.md").to_string(),
            content: b"newer".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::IfMatch(base.version.id.to_string()),
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("docs/source.md").to_string(),
            content: b"source".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("docs/target.md").to_string(),
            content: b"target winner".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
    assert!(
        store
            .get_document(&library.slug, "docs/staged.bin")
            .await
            .is_err()
    );
    drop(store);

    let reopened = QuarryStore::open(StoreConfig {
        db_path,
        cas_path,
        lock_path: None,
    })
    .await
    .unwrap();

    assert!(
        reopened
            .get_document(&library.slug, "docs/staged.bin")
            .await
            .is_err()
    );
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
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/share.md").to_string(),
            content: b"share".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    let block_document = store
        .import_block_document(
            &library.slug,
            "notes/blocks.md",
            "Review me.\n",
            serde_json::json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let block_id = store
        .load_block_tree(&block_document.document.id)
        .await
        .unwrap()[0]
        .block_id
        .clone();

    let guard = store.acquire_global_operation_lock().await;
    let writer_store = store.clone();
    let writer_library = library.slug.clone();
    let mut write = tokio::spawn(async move {
        writer_store
            .put_document(quarry_storage::PutDocumentRequest {
                library: writer_library.to_string(),
                path: ("notes/blocked.md").to_string(),
                content: b"blocked".to_vec(),
                metadata: serde_json::json!({"content_type":"text/markdown"}),
                content_type: ("text/markdown").to_string(),
                source: DocumentSource::Rest,
                precondition: WritePrecondition::None,
                origin_id: None,
                transaction: quarry_storage::TransactionMetadata::default(),
            })
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

    let guard = store.acquire_global_operation_lock().await;
    let invite_store = store.clone();
    let invite_library = library.slug.clone();
    let mut invite = tokio::spawn(async move {
        invite_store
            .create_collab_invite_token(&invite_library, "notes/share.md", "editor", None)
            .await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut invite)
            .await
            .is_err(),
        "invite write should wait while a global operation lock is held"
    );

    drop(guard);
    let token = tokio::time::timeout(Duration::from_secs(1), invite)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(token.role, "editor");

    let guard = store.acquire_global_operation_lock().await;
    let shadow_store = store.clone();
    let shadow_document_id = block_document.document.id.clone();
    let mut shadow = tokio::spawn(async move {
        shadow_store
            .put_block_shadow_base(
                "test",
                "peer:notes/blocks.md",
                &shadow_document_id,
                "Review me.\n",
                None,
            )
            .await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut shadow)
            .await
            .is_err(),
        "block shadow writes should wait while a global operation lock is held"
    );

    drop(guard);
    let shadow_base = tokio::time::timeout(Duration::from_secs(1), shadow)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(shadow_base.base_markdown, "Review me.\n");

    let guard = store.acquire_global_operation_lock().await;
    let tx_store = store.clone();
    let tx_document_id = block_document.document.id.clone();
    let mut block_tx = tokio::spawn(async move {
        tx_store
            .record_block_transaction(
                &tx_document_id,
                "lock-test",
                "agent",
                None,
                serde_json::json!([]),
                None,
            )
            .await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut block_tx)
            .await
            .is_err(),
        "block transaction writes should wait while a global operation lock is held"
    );

    drop(guard);
    let recorded = tokio::time::timeout(Duration::from_secs(1), block_tx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(recorded.client_tx_id, "lock-test");

    let guard = store.acquire_global_operation_lock().await;
    let review_store = store.clone();
    let review_document_id = block_document.document.id.clone();
    let mut review = tokio::spawn(async move {
        review_store
            .put_block_review_item(NewBlockReviewItem {
                document_id: review_document_id.to_string(),
                block_id,
                kind: BlockReviewKind::Comment,
                start_offset: 0,
                end_offset: 6,
                body: Some("note".to_string()),
                replacement: None,
                author: Some("agent".to_string()),
                state: BlockReviewState::Open,
                quote: None,
                context_before: None,
                context_after: None,
                parent_item_id: None,
            })
            .await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut review)
            .await
            .is_err(),
        "block review writes should wait while a global operation lock is held"
    );

    drop(guard);
    let review_item = tokio::time::timeout(Duration::from_secs(1), review)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(review_item.body.as_deref(), Some("note"));
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("/Notes/Plan.md").to_string(),
            content: b"upper".to_vec(),
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
            path: ("notes/plan.md").to_string(),
            content: b"lower".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: (".quarry/marker.json").to_string(),
            content: b"reserved".to_vec(),
            metadata: serde_json::json!({}),
            content_type: ("application/octet-stream").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
            .put_document(quarry_storage::PutDocumentRequest {
library: library.slug.to_string(),
path: format!("docs/{index:04}.bin").to_string(),
content,
metadata: serde_json::json!({"content_type":"application/octet-stream","index":index}),
content_type: ("application/octet-stream").to_string(),
source: DocumentSource::Rest,
precondition: WritePrecondition::None,
origin_id: None,
transaction: quarry_storage::TransactionMetadata::default(),
})
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
async fn visible_writes_emit_in_process_store_events() -> TestResult {
    let root = tempfile::tempdir().context("create temp dir")?;
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .context("open store")?;
    let library = store
        .create_library("events")
        .await
        .context("create events library")?;
    let mut events = store.subscribe_events();

    let write = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/a.md").to_string(),
            content: b"a".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("put document")?;
    let event = events.recv().await.context("receive document put event")?;
    assert_eq!(event.kind(), StoreEventKind::DocumentPut);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.path(), Some("notes/a.md"));
    assert_eq!(event.doc_id(), Some(write.document.id.as_str()));
    assert_eq!(event.version_id(), Some(write.version.id.as_str()));
    let event = events.recv().await.context("receive put links event")?;
    assert_eq!(event.kind(), StoreEventKind::LinksIndexed);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.path(), Some("notes/a.md"));

    store
        .move_document(
            &library.slug,
            "notes/a.md",
            "notes/b.md",
            DocumentSource::Rest,
        )
        .await
        .context("move document")?;
    let event = events.recv().await.context("receive document move event")?;
    assert_eq!(event.kind(), StoreEventKind::DocumentMove);
    assert_eq!(event.path(), Some("notes/a.md"));
    assert_eq!(event.new_path(), Some("notes/b.md"));
    assert_eq!(event.doc_id(), Some(write.document.id.as_str()));
    let event = events.recv().await.context("receive move links event")?;
    assert_eq!(event.kind(), StoreEventKind::LinksIndexed);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.path(), Some("notes/b.md"));

    store
        .delete_document(&library.slug, "notes/b.md", DocumentSource::Rest)
        .await
        .context("delete document")?;
    let event = events
        .recv()
        .await
        .context("receive document delete event")?;
    assert_eq!(event.kind(), StoreEventKind::DocumentDelete);
    assert_eq!(event.path(), Some("notes/b.md"));
    assert_eq!(event.doc_id(), Some(write.document.id.as_str()));
    let event = events.recv().await.context("receive delete links event")?;
    assert_eq!(event.kind(), StoreEventKind::LinksIndexed);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.path(), Some("notes/b.md"));

    let conflict = store
        .record_conflict(
            &library.slug,
            "notes/conflicted.md",
            Some("ours-version".to_string()),
            Some("theirs-version".to_string()),
        )
        .await
        .context("record conflict")?;
    let event = events
        .recv()
        .await
        .context("receive conflict created event")?;
    assert_eq!(event.kind(), StoreEventKind::ConflictCreated);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.path(), Some("notes/conflicted.md"));
    assert_eq!(event.conflict_id(), Some(conflict.id.as_str()));

    store
        .resolve_conflict(&conflict.id)
        .await
        .context("resolve conflict")?;
    let event = events
        .recv()
        .await
        .context("receive conflict resolved event")?;
    assert_eq!(event.kind(), StoreEventKind::ConflictResolved);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.path(), Some("notes/conflicted.md"));
    assert_eq!(event.conflict_id(), Some(conflict.id.as_str()));

    let report = store
        .reindex_library(&library.slug)
        .await
        .context("reindex library")?;
    assert!(report.ok);
    let event = events.recv().await.context("receive reindex event")?;
    assert_eq!(event.kind(), StoreEventKind::LibraryReindexed);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.path(), None);
    assert_eq!(event.conflict_id(), None);

    store
        .emit_git_sync_completed(&library.slug, "peer-1", 2, 1)
        .await
        .context("emit git sync completed")?;
    let event = events.recv().await.context("receive git sync event")?;
    assert_eq!(event.kind(), StoreEventKind::GitSyncCompleted);
    assert_eq!(event.library_id(), library.id.as_str());
    assert_eq!(event.peer_id(), Some("peer-1"));
    assert_eq!(event.applied(), Some(2));
    assert_eq!(event.conflicts(), Some(1));
    Ok(())
}

#[tokio::test]
async fn document_mutation_events_include_origin_and_document_identity() -> TestResult {
    let root = tempfile::tempdir().context("create temp dir")?;
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .context("open store")?;
    let library = store
        .create_library("origin-events")
        .await
        .context("create origin-events library")?;
    let mut events = store.subscribe_events();

    let write = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("notes/a.md").to_string(),
            content: b"a".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: Some("browser:origin-1".to_string()),
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("put document with origin")?;
    let event = events.recv().await.context("receive document put event")?;
    assert_eq!(event.kind(), StoreEventKind::DocumentPut);
    assert_eq!(event.doc_id(), Some(write.document.id.as_str()));
    assert_eq!(event.origin_id(), Some("browser:origin-1"));
    let _links = events.recv().await.context("receive put links event")?;

    store
        .move_document_with_origin(
            &library.slug,
            "notes/a.md",
            "notes/b.md",
            DocumentSource::Rest,
            Some("browser:origin-1".to_string()),
            None,
        )
        .await
        .context("move document with origin")?;
    let event = events.recv().await.context("receive document move event")?;
    assert_eq!(event.kind(), StoreEventKind::DocumentMove);
    assert_eq!(event.doc_id(), Some(write.document.id.as_str()));
    assert_eq!(event.origin_id(), Some("browser:origin-1"));
    let _links = events.recv().await.context("receive move links event")?;

    store
        .delete_document_with_origin(
            &library.slug,
            "notes/b.md",
            DocumentSource::Rest,
            Some("browser:origin-1".to_string()),
            None,
        )
        .await
        .context("delete document with origin")?;
    let event = events
        .recv()
        .await
        .context("receive document delete event")?;
    assert_eq!(event.kind(), StoreEventKind::DocumentDelete);
    assert_eq!(event.doc_id(), Some(write.document.id.as_str()));
    assert_eq!(event.origin_id(), Some("browser:origin-1"));
    Ok(())
}

#[tokio::test]
async fn inode_paths_are_lookupable_and_moves_keep_inode_identity() -> TestResult {
    let root = tempfile::tempdir().context("create temp dir")?;
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .context("open store")?;
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
        .context("put original document")?;
    let inode = store
        .inode_for_path(&library.slug, "plans/one.md")
        .await
        .context("load inode for original path")?;

    assert_eq!(
        store
            .path_for_inode(&library.slug, inode)
            .await
            .context("load path for inode before move")?,
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
        .context("move document")?;

    assert_eq!(
        store
            .inode_for_path(&library.slug, "archive/one.md")
            .await
            .context("load inode for moved path")?,
        inode
    );
    assert_eq!(
        store
            .path_for_inode(&library.slug, inode)
            .await
            .context("load path for inode after move")?,
        "archive/one.md"
    );
    assert!(
        store
            .inode_for_path(&library.slug, "plans/one.md")
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn move_document_can_reuse_deleted_target_path() -> TestResult {
    let root = tempfile::tempdir().context("create temp dir")?;
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .context("open store")?;
    let library = store
        .create_library("notes")
        .await
        .context("create notes library")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/source.md").to_string(),
            content: b"source\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("put source document")?;
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/target.md").to_string(),
            content: b"deleted\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .context("put target document")?;
    let source_inode = store
        .inode_for_path(&library.slug, "drafts/source.md")
        .await
        .context("load source inode")?;
    let source_document_id = store
        .get_document(&library.slug, "drafts/source.md")
        .await
        .context("load source document")?
        .id;

    store
        .delete_document(&library.slug, "drafts/target.md", DocumentSource::Rest)
        .await
        .context("delete target document")?;
    store
        .move_document(
            &library.slug,
            "drafts/source.md",
            "drafts/target.md",
            DocumentSource::Rest,
        )
        .await
        .context("move source over deleted target path")?;

    let document = store
        .get_document(&library.slug, "drafts/target.md")
        .await
        .context("load moved document")?;
    assert_eq!(document.content, b"source\n");
    assert_eq!(document.id, source_document_id);
    assert_eq!(
        store
            .inode_for_path(&library.slug, "drafts/target.md")
            .await
            .context("load moved document inode")?,
        source_inode
    );
    assert!(
        store
            .get_document(&library.slug, "drafts/source.md")
            .await
            .is_err()
    );
    Ok(())
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("drafts/source.md").to_string(),
            content: b"source\n".to_vec(),
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
            path: ("drafts/target.md").to_string(),
            content: b"deleted\n".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
    assert!(
        store
            .get_document(&library.slug, "drafts/source.md")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn opening_old_schema_migrates_documents_to_active_path_uniqueness() -> TestResult {
    let root = tempfile::tempdir()?;
    let db_path = root.path().join("quarry.db");
    {
        let db_path = db_path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "database path should be UTF-8")
        })?;
        let db = turso::Builder::new_local(db_path).build().await?;
        let conn = db.connect()?;
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
        .await?;
    }

    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await?;
    let library = store.create_library("migrated").await?;
    let first = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("same.md").to_string(),
            content: b"old".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await?;
    store
        .delete_document(&library.slug, "same.md", DocumentSource::Rest)
        .await?;
    let second = store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("same.md").to_string(),
            content: b"new".to_vec(),
            metadata: serde_json::json!({"content_type":"text/markdown"}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await?;

    assert_ne!(first.document.id, second.document.id);
    drop(store);

    let db_path = db_path.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "database path should be UTF-8")
    })?;
    let db = turso::Builder::new_local(db_path).build().await?;
    let conn = db.connect()?;
    let document_indexes = index_names(&conn, "documents").await;
    assert!(document_indexes.contains("idx_documents_active_library_path"));
    Ok(())
}

#[tokio::test]
async fn tmp_documents_are_versioned_live_until_expiry_and_promotable() -> TestResult {
    let root = tempfile::tempdir()?;
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await?;

    let tmp = store
        .create_tmp_document(
            b"draft one".to_vec(),
            serde_json::json!({"title":"Scratch"}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await?;
    let secret = tmp.document.path.clone();
    assert_eq!(secret.len(), 32);
    assert!(
        secret
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    let expires_at = tmp.document.expires_at.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "tmp document should record an expiry timestamp",
        )
    })?;
    chrono::DateTime::parse_from_rfc3339(expires_at)?;
    assert_eq!(tmp.document.library_id, None);

    let updated = store
        .put_tmp_document(
            &secret,
            b"draft two".to_vec(),
            serde_json::json!({"title":"Scratch"}),
            "text/markdown",
            TmpTtl::Unchanged,
            WritePrecondition::IfMatch(tmp.version.id.to_string()),
        )
        .await?;
    assert_eq!(updated.document.id, tmp.document.id);
    assert_ne!(updated.version.id, tmp.version.id);

    let raw_versions = store.raw_tmp_version_history(&secret).await?;
    assert_eq!(raw_versions.len(), 2);
    let first_version_id = tmp.version.id.clone();
    let first = store
        .tmp_document_version(&secret, &first_version_id)
        .await?;
    assert_eq!(first.content, "draft one");

    let library = store.create_library("promoted").await?;
    store
        .promote_tmp_document(
            &secret,
            &library.slug,
            "notes/scratch.md",
            WritePrecondition::IfMatch(updated.version.id.to_string()),
        )
        .await?;

    assert!(store.get_tmp_document(&secret).await.is_err());
    let promoted = store
        .get_document(&library.slug, "notes/scratch.md")
        .await?;
    assert_eq!(promoted.id, tmp.document.id);
    assert_eq!(promoted.content, b"draft two");
    assert_eq!(
        store
            .raw_version_history(&library.slug, "notes/scratch.md")
            .await?
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn expired_documents_are_gone_and_excluded_from_live_queries() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();

    let expired = store
        .create_tmp_document(
            b"old".to_vec(),
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap();
    let expired_secret = expired.document.path.clone();
    store
        .set_tmp_document_ttl(&expired_secret, Some("2000-01-01T00:00:00Z".to_string()))
        .await
        .unwrap();
    let err = store.get_tmp_document(&expired_secret).await.unwrap_err();
    assert!(matches!(err, QuarryError::Gone(_)));

    let library = store.create_library("ttl").await.unwrap();
    store
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("gone.md").to_string(),
            content: b"old".to_vec(),
            metadata: serde_json::json!({}),
            content_type: ("text/plain").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();
    store
        .set_document_ttl(
            &library.slug,
            "gone.md",
            Some("2000-01-01T00:00:00Z".to_string()),
        )
        .await
        .unwrap();
    let err = store
        .get_document(&library.slug, "gone.md")
        .await
        .unwrap_err();
    assert!(matches!(err, QuarryError::Gone(_)));
    assert!(
        store
            .list_documents(&library.slug, None, None)
            .await
            .unwrap()
            .is_empty()
    );

    store
        .set_document_ttl(&library.slug, "gone.md", None)
        .await
        .unwrap();
    assert_eq!(
        store
            .get_document(&library.slug, "gone.md")
            .await
            .unwrap()
            .content,
        b"old"
    );

    let err = store
        .set_tmp_document_ttl("expired.md", None)
        .await
        .unwrap_err();
    assert!(matches!(err, QuarryError::InvalidInput(_)));
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

#[tokio::test]
async fn tmp_documents_accept_markdown_media_types_and_normalize_parameters() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;

    let outcome = store
        .create_tmp_document(
            b"# Scratch\n".to_vec(),
            serde_json::json!({}),
            "text/x-markdown; charset=utf-8",
            TmpTtl::Default,
        )
        .await
        .unwrap();

    assert_eq!(outcome.version.content_type, "text/x-markdown");
    assert_eq!(outcome.version.metadata["content_type"], "text/x-markdown");
    assert_eq!(
        store
            .get_tmp_document(&outcome.document.path)
            .await
            .unwrap()
            .content,
        b"# Scratch\n"
    );

    let scalar_metadata = store
        .create_tmp_document(
            b"Scalar metadata still gets content type.\n".to_vec(),
            serde_json::json!("ignored for tmp documents"),
            "application/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap();
    assert_eq!(
        scalar_metadata.version.metadata,
        serde_json::json!({"content_type": "application/markdown"})
    );
}

#[tokio::test]
async fn tmp_documents_reject_non_markdown_media_types_on_create_and_update() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;

    let error = store
        .create_tmp_document(
            b"not markdown".to_vec(),
            serde_json::json!({}),
            "text/plain",
            TmpTtl::Default,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuarryError::UnsupportedMediaType(_)),
        "text/plain should be rejected, got {error:?}"
    );
    let error = store
        .create_tmp_document(
            b"not markdown".to_vec(),
            serde_json::json!({}),
            "application/json",
            TmpTtl::Default,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuarryError::UnsupportedMediaType(_)),
        "application/json should be rejected, got {error:?}"
    );
    let error = store
        .create_tmp_document(
            b"not markdown".to_vec(),
            serde_json::json!({}),
            "image/png",
            TmpTtl::Default,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuarryError::UnsupportedMediaType(_)),
        "image/png should be rejected, got {error:?}"
    );

    let valid = store
        .create_tmp_document(
            b"still markdown".to_vec(),
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap();

    let error = store
        .put_tmp_document(
            &valid.document.path,
            b"replacement".to_vec(),
            serde_json::json!({}),
            "text/plain",
            TmpTtl::Unchanged,
            WritePrecondition::IfMatch(valid.version.id.to_string()),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuarryError::UnsupportedMediaType(_)),
        "text/plain should be rejected, got {error:?}"
    );
    let error = store
        .put_tmp_document(
            &valid.document.path,
            b"replacement".to_vec(),
            serde_json::json!({}),
            "application/json",
            TmpTtl::Unchanged,
            WritePrecondition::IfMatch(valid.version.id.to_string()),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuarryError::UnsupportedMediaType(_)),
        "application/json should be rejected, got {error:?}"
    );
    let error = store
        .put_tmp_document(
            &valid.document.path,
            b"replacement".to_vec(),
            serde_json::json!({}),
            "image/png",
            TmpTtl::Unchanged,
            WritePrecondition::IfMatch(valid.version.id.to_string()),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, QuarryError::UnsupportedMediaType(_)),
        "image/png should be rejected, got {error:?}"
    );

    let head = store.head_tmp_document(&valid.document.path).await.unwrap();
    assert_eq!(head.head_version_id, valid.version.id);
}

#[tokio::test]
async fn tmp_documents_reject_invalid_utf8_on_create_and_update() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;

    let error = store
        .create_tmp_document(
            vec![0xff],
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::InvalidInput(_)));

    let valid = store
        .create_tmp_document(
            b"valid markdown".to_vec(),
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap();
    let error = store
        .put_tmp_document(
            &valid.document.path,
            vec![0xff],
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Unchanged,
            WritePrecondition::IfMatch(valid.version.id.to_string()),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::InvalidInput(_)));

    let head = store.head_tmp_document(&valid.document.path).await.unwrap();
    assert_eq!(head.head_version_id, valid.version.id);
}

#[tokio::test]
async fn tmp_documents_enforce_one_mib_canonical_markdown_limit() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;

    let exact = vec![b'a'; quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES];
    let outcome = store
        .create_tmp_document(
            exact,
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.version.byte_size,
        quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES as u64
    );

    let too_large = vec![b'a'; quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 1];
    let error = store
        .create_tmp_document(
            too_large,
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, QuarryError::PayloadTooLarge(_)));
}

#[tokio::test]
async fn tmp_block_mutation_rejects_oversized_normalized_markdown_without_moving_head() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let created = store
        .create_tmp_document(
            b"Original.\n".to_vec(),
            serde_json::json!({}),
            "text/markdown",
            TmpTtl::Default,
        )
        .await
        .unwrap();
    let secret = created.document.path.clone();
    let imported = store
        .import_tmp_block_document(
            &secret,
            "Original.\n",
            serde_json::json!({}),
            "text/markdown",
            WritePrecondition::IfMatch(created.version.id.to_string()),
        )
        .await
        .unwrap();
    let state = store
        .block_mutation_state_for_scope(&DocumentScopeRef::Tmp, &secret, "oversized-tx")
        .await
        .unwrap();
    let oversized = "a".repeat(quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 1);

    let error = store
        .commit_block_mutation_for_scope(
            &DocumentScopeRef::Tmp,
            BlockMutationCommit {
                document_id: state.document_id.clone(),
                expected_head_version_id: state.head_version_id.clone(),
                client_tx_id: "oversized-tx".to_string(),
                actor_kind: "agent".to_string(),
                actor_id: None,
                transaction_actor: None,
                transaction_message: None,
                transaction_provenance: None,
                origin_id: None,
                source: DocumentSource::Rest,
                recorded_ops: serde_json::json!({ "ops": [] }),
                metadata: state.metadata.clone(),
                content_type: state.content_type.clone(),
                rows: state.rows.clone(),
                review_items: state.review_items.clone(),
                normalized_markdown: oversized,
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(error, QuarryError::PayloadTooLarge(_)));
    let head = store.head_tmp_document(&secret).await.unwrap();
    assert_eq!(head.head_version_id, imported.version.id);
    assert_eq!(
        store.load_block_tree(&state.document_id).await.unwrap(),
        state.rows
    );
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
            WritePrecondition::IfMatch(outcome.version.id.to_string()),
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
async fn tmp_block_import_rejects_path_like_identifiers() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;

    let error = store
        .import_tmp_block_document(
            "scratch/note.md",
            "# Tmp\n",
            serde_json::json!({}),
            "text/markdown",
            WritePrecondition::None,
        )
        .await
        .expect_err("tmp block imports should require capability secrets");

    assert!(matches!(
        error,
        QuarryError::InvalidPath(message) if message == "invalid tmp document secret"
    ));
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
        document_id: outcome.document.id.to_string(),
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("assets/data.bin").to_string(),
            content: bytes.clone(),
            metadata: serde_json::json!({"content_type": "application/octet-stream"}),
            content_type: ("application/octet-stream").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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

    assert!(
        store
            .load_block_tree(&outcome.document.id)
            .await
            .unwrap()
            .is_empty()
    );

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
    assert!(
        reopened
            .load_block_tree(&outcome.document.id)
            .await
            .unwrap()
            .is_empty()
    );
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
    assert!(
        store
            .get_document(&library.slug, "critic.md")
            .await
            .is_err()
    );
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
            Some(outcome.version.id.to_string()),
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
            document_id: document_id.to_string(),
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("doc.md").to_string(),
            content: b"Rewritten outside the block path.\n".to_vec(),
            metadata: serde_json::json!({}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
        .await
        .unwrap();

    // ...so the block projection is dropped rather than serving stale rows.
    assert!(
        store
            .load_block_tree(&document_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_block_review_items(&document_id)
            .await
            .unwrap()
            .is_empty()
    );
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
            document_id: document_id.to_string(),
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

    assert!(
        store
            .load_block_tree(&document_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_block_review_items(&document_id)
            .await
            .unwrap()
            .is_empty()
    );
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
    assert!(state.version_ids.contains(imported.version.id.as_str()));

    let mut rows = state.rows.clone();
    rows[0].text = "Rewritten paragraph.".to_string();
    let commit = BlockMutationCommit {
        document_id: state.document_id.clone(),
        expected_head_version_id: state.head_version_id.clone(),
        client_tx_id: "ctx-1".to_string(),
        actor_kind: "agent".to_string(),
        actor_id: Some("agent-7".to_string()),
        transaction_actor: Some("Agent Seven".to_string()),
        transaction_message: None,
        transaction_provenance: None,
        origin_id: None,
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
        Some(outcome.version.id.to_string())
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
                transaction_message: None,
                transaction_provenance: None,
                origin_id: None,
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
        .put_document(quarry_storage::PutDocumentRequest {
            library: library.slug.to_string(),
            path: ("legacy.md").to_string(),
            content: b"# Title\n\nBody text.\n".to_vec(),
            metadata: serde_json::json!({}),
            content_type: ("text/markdown").to_string(),
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id: None,
            transaction: quarry_storage::TransactionMetadata::default(),
        })
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
    assert!(
        store
            .load_block_tree(&state.document_id)
            .await
            .unwrap()
            .is_empty()
    );
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
                transaction_message: None,
                transaction_provenance: None,
                origin_id: None,
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

#[tokio::test]
async fn block_mutation_commit_accepts_replies_to_collapsed_insertion_suggestions() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    let library = store
        .create_library("insertion-reply-anchor")
        .await
        .unwrap();
    store
        .import_block_document(
            &library.slug,
            "doc.md",
            "Type here.\n",
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
    let block_id = state.rows[0].block_id.clone();
    let now = "2026-06-16T00:00:00.000Z".to_string();
    let insertion_suggestion = BlockReviewItem {
        id: "s1".to_string(),
        document_id: state.document_id.clone(),
        block_id: block_id.clone(),
        kind: BlockReviewKind::Suggestion,
        start_offset: 4,
        end_offset: 4,
        body: None,
        replacement: Some(" inserted".to_string()),
        author: Some("agent".to_string()),
        state: BlockReviewState::Open,
        quote: Some(String::new()),
        context_before: None,
        context_after: None,
        parent_item_id: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let reply = BlockReviewItem {
        id: "r1".to_string(),
        document_id: state.document_id.clone(),
        block_id,
        kind: BlockReviewKind::Comment,
        start_offset: 4,
        end_offset: 4,
        body: Some("Why this insertion?".to_string()),
        replacement: None,
        author: Some("reviewer".to_string()),
        state: BlockReviewState::Open,
        quote: Some(String::new()),
        context_before: None,
        context_after: None,
        parent_item_id: Some("s1".to_string()),
        created_at: now.clone(),
        updated_at: now,
    };

    let outcome = store
        .commit_block_mutation(
            &library.slug,
            BlockMutationCommit {
                document_id: state.document_id.clone(),
                expected_head_version_id: state.head_version_id.clone(),
                client_tx_id: "ctx-2".to_string(),
                actor_kind: "browser_session".to_string(),
                actor_id: None,
                transaction_actor: Some("browser".to_string()),
                transaction_message: Some("Live session edits".to_string()),
                transaction_provenance: None,
                origin_id: None,
                source: DocumentSource::Rest,
                recorded_ops: serde_json::json!({}),
                metadata: state.metadata.clone(),
                content_type: state.content_type.clone(),
                rows: state.rows.clone(),
                review_items: vec![insertion_suggestion, reply],
                normalized_markdown: "Type here.\n".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(outcome, BlockMutationOutcome::Applied { .. }));

    let items = store
        .list_block_review_items(&state.document_id)
        .await
        .unwrap();
    assert!(items.iter().any(|item| item.id == "r1"));
}

/// `put_block_review_item` accepts the gateway's conflict shape (Phase 4):
/// `block_id` holds the attachment point ("" = document start), the range is
/// a collapsed open placement, and no text anchor exists to validate.
#[tokio::test]
async fn put_block_review_item_accepts_the_conflict_shape() {
    let root = tempfile::tempdir().unwrap();
    let store = open_block_store(root.path()).await;
    store.create_library("conflicts").await.unwrap();
    let outcome = store
        .import_block_document(
            "conflicts",
            "doc.md",
            "Alpha.\n",
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let stored = store
        .put_block_review_item(NewBlockReviewItem {
            document_id: outcome.document.id.to_string(),
            block_id: String::new(),
            kind: BlockReviewKind::Conflict,
            start_offset: 0,
            end_offset: 0,
            body: Some("Incoming hunk.\n".to_string()),
            replacement: None,
            author: Some("git".to_string()),
            state: BlockReviewState::Open,
            quote: Some("Canonical side.\n".to_string()),
            context_before: Some("Base context.\n".to_string()),
            context_after: None,
            parent_item_id: None,
        })
        .await
        .unwrap();

    let items = store
        .list_block_review_items(&outcome.document.id)
        .await
        .unwrap();
    let kept = items.iter().find(|item| item.id == stored.id).unwrap();
    assert_eq!(kept.kind, BlockReviewKind::Conflict);
    assert_eq!(kept.block_id, "");
    assert_eq!(kept.state, BlockReviewState::Open);
    assert_eq!(kept.body.as_deref(), Some("Incoming hunk.\n"));
}
