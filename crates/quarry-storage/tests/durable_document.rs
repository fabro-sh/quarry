#![allow(
    clippy::unwrap_used,
    reason = "integration fixtures assert each setup step"
)]
use quarry_core::{DocumentSource, WritePrecondition};
use quarry_document::{Document, SeedBlock};
use quarry_storage::{
    BlockMutationCommit, BlockMutationOutcome, DocumentScopeRef, DurableDocumentCommit,
    QuarryStore, StoreConfig, document_projection,
};
use serde_json::json;

fn config(root: &std::path::Path) -> StoreConfig {
    StoreConfig {
        db_path: root.join("store.db"),
        cas_path: root.join("cas"),
        lock_path: None,
    }
}

async fn execute_sql(root: &std::path::Path, sql: &str) {
    let database = turso::Builder::new_local(root.join("store.db").to_str().unwrap())
        .build()
        .await
        .unwrap();
    let connection = database.connect().unwrap();
    connection.execute(sql, ()).await.unwrap();
}

#[tokio::test]
async fn startup_import_resumes_after_a_real_sql_failure_without_reimporting_completed_documents() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    store.create_library("upgrade").await.unwrap();
    let mut originals = Vec::new();
    for path in ["review.md", "second.md", "already-native.md"] {
        let outcome = store
            .put_document(quarry_storage::PutDocumentRequest {
                library: "upgrade".into(),
                path: path.into(),
                content: b"# Title\n\nSee {==TARGET==}{>>Keep<<}{#c} and {~~old~>new~~}{#s}.\n"
                    .to_vec(),
                metadata: json!({}),
                content_type: "text/markdown".into(),
                source: DocumentSource::Rest,
                precondition: WritePrecondition::None,
                origin_id: None,
                transaction: Default::default(),
            })
            .await
            .unwrap();
        let state = store
            .durable_document_for_scope(&DocumentScopeRef::library("upgrade"), path)
            .await
            .unwrap()
            .unwrap();
        originals.push((
            path,
            outcome.document.id.to_string(),
            state.bytes,
            store.load_block_tree(&outcome.document.id).await.unwrap(),
        ));
    }
    drop(store);
    let existing_id = &originals[2].1;
    // Simulate two documents awaiting the upgrade and one already completed.
    // Remove SQL review indexes to prove RFM comments do not depend on them.
    for table in [
        "document_states",
        "document_state_versions",
        "document_command_receipts",
        "block_review_items",
    ] {
        execute_sql(
            root.path(),
            &format!("DELETE FROM {table} WHERE document_id != '{existing_id}'"),
        )
        .await;
    }
    // First import commits. The second hits an actual SQL constraint error.
    execute_sql(root.path(), &format!("CREATE UNIQUE INDEX interrupt_import ON document_states(schema_version) WHERE document_id != '{existing_id}'")).await;
    let failure = QuarryStore::open(config(root.path()))
        .await
        .err()
        .expect("injected import must fail");
    assert!(
        failure.to_string().contains("UNIQUE"),
        "unexpected import error: {failure}"
    );
    let database = turso::Builder::new_local(root.path().join("store.db").to_str().unwrap())
        .build()
        .await
        .unwrap();
    let connection = database.connect().unwrap();
    let mut rows = connection
        .query(
            "SELECT document_id,state FROM document_states WHERE document_id != ?1",
            turso::params![existing_id.as_str()],
        )
        .await
        .unwrap();
    let completed = rows
        .next()
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("no import committed before error: {failure}"));
    let completed_id: String = completed.get(0).unwrap();
    let completed_bytes: Vec<u8> = completed.get(1).unwrap();
    assert!(rows.next().await.unwrap().is_none());
    drop(rows);
    drop(connection);
    drop(database);
    execute_sql(root.path(), "DROP INDEX interrupt_import").await;
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let mut migrated = Vec::new();
    for (path, id, bytes, blocks) in &originals {
        let state = store
            .durable_document_for_scope(&DocumentScopeRef::library("upgrade"), path)
            .await
            .unwrap()
            .unwrap();
        if id == existing_id {
            assert_eq!(&state.bytes, bytes);
        }
        if id == &completed_id {
            assert_eq!(state.bytes, completed_bytes);
        }
        assert_eq!(&store.load_block_tree(id).await.unwrap(), blocks);
        let document = Document::load(&state.bytes).unwrap();
        assert_eq!(
            document.comment_target("c").unwrap().attachments[0].quote,
            "TARGET"
        );
        assert_eq!(document.view().unwrap().proposals[0].text, "new");
        migrated.push((path.to_string(), state.bytes));
    }
    drop(store);
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    for (path, bytes) in migrated {
        assert_eq!(
            store
                .durable_document_for_scope(&DocumentScopeRef::library("upgrade"), &path)
                .await
                .unwrap()
                .unwrap()
                .bytes,
            bytes
        );
    }
}

#[tokio::test]
async fn old_global_projection_keys_migrate_without_losing_rows() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let (_, commit) = setup(&store, "legacy").await;
    let original = store.load_block_tree(&commit.document_id).await.unwrap();
    drop(store);
    let database = turso::Builder::new_local(root.path().join("store.db").to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = database.connect().unwrap();
    // Recreate the actual pre-migration primary keys, retaining stored rows.
    for (table, key) in [("blocks", "block_id"), ("block_review_items", "id")] {
        let mut rows = conn
            .query(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                turso::params![table],
            )
            .await
            .unwrap();
        let schema: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
        drop(rows);
        let old_schema = schema
            .replace(
                &format!("CREATE TABLE {table}"),
                &format!("CREATE TABLE {table}_global"),
            )
            .replace(
                &format!("PRIMARY KEY(document_id,{key})"),
                &format!("PRIMARY KEY({key})"),
            );
        assert_ne!(schema, old_schema);
        conn.execute(old_schema, ()).await.unwrap();
        conn.execute(
            format!("INSERT INTO {table}_global SELECT * FROM {table}"),
            (),
        )
        .await
        .unwrap();
        conn.execute(format!("DROP TABLE {table}"), ())
            .await
            .unwrap();
        conn.execute(format!("ALTER TABLE {table}_global RENAME TO {table}"), ())
            .await
            .unwrap();
    }
    drop(conn);
    drop(database);
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    assert_eq!(
        store.load_block_tree(&commit.document_id).await.unwrap(),
        original
    );
    drop(store);
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    assert_eq!(
        store.load_block_tree(&commit.document_id).await.unwrap(),
        original
    );
}

async fn setup(store: &QuarryStore, library: &str) -> (Document, BlockMutationCommit) {
    store.create_library(library).await.unwrap();
    store
        .import_block_document(
            library,
            "doc.md",
            "See TARGET here.\n",
            json!({}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();
    let state = store
        .block_mutation_state(library, "doc.md", "first")
        .await
        .unwrap();
    let saved = store
        .durable_document_for_scope(&DocumentScopeRef::library(library), "doc.md")
        .await
        .unwrap()
        .unwrap();
    let document = Document::load(&saved.bytes).unwrap();
    let commit = BlockMutationCommit {
        document_id: state.document_id,
        expected_head_version_id: state.head_version_id,
        client_tx_id: "first".into(),
        actor_kind: "agent".into(),
        actor_id: Some("test".into()),
        transaction_actor: Some("Test".into()),
        transaction_message: None,
        transaction_provenance: None,
        origin_id: None,
        source: DocumentSource::Rest,
        recorded_ops: json!({"ack":{"result":"committed"}}),
        metadata: state.metadata,
        content_type: state.content_type,
        rows: document_projection(&document).unwrap(),
        review_items: Vec::new(),
        normalized_markdown: "See TARGET here.\n".into(),
    };
    (document, commit)
}

#[tokio::test]
async fn canonical_bytes_and_exact_request_receipt_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let (document, commit) = setup(&store, "one").await;
    let scope = DocumentScopeRef::library("one");
    let durable = DurableDocumentCommit {
        bytes: document.save(),
        request_hash: "request-hash".into(),
    };
    let result = store
        .commit_durable_document_for_scope(&scope, commit.clone(), durable.clone())
        .await
        .unwrap();
    let BlockMutationOutcome::Applied { record, .. } = result else {
        panic!("First request must apply")
    };
    drop(store);
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let saved = store
        .durable_document_for_scope(&scope, "doc.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.version_id,
        record.resulting_version_id.clone().unwrap()
    );
    assert_eq!(
        Document::load(&saved.bytes).unwrap().heads(),
        document.heads()
    );
    let result = store
        .commit_durable_document_for_scope(&scope, commit.clone(), durable.clone())
        .await
        .unwrap();
    let BlockMutationOutcome::Replayed(replayed) = result else {
        panic!("Retry must replay")
    };
    assert_eq!(record, replayed);
    let wrong = DurableDocumentCommit {
        request_hash: "different-request".into(),
        ..durable
    };
    assert!(
        store
            .commit_durable_document_for_scope(&scope, commit.clone(), wrong)
            .await
            .is_err()
    );
    let saved_again = store
        .durable_document_for_scope(&scope, "doc.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.bytes, saved_again.bytes);
    store.create_library("other").await.unwrap();
    assert!(
        store
            .commit_durable_document_for_scope(
                &DocumentScopeRef::library("other"),
                commit,
                DurableDocumentCommit {
                    bytes: document.save(),
                    request_hash: "request-hash".into()
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_sql_commit_rolls_back_version_rows_state_receipt_and_events() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let (mut document, commit) = setup(&store, "one").await;
    let scope = DocumentScopeRef::library("one");
    store
        .commit_durable_document_for_scope(
            &scope,
            commit.clone(),
            DurableDocumentCommit {
                bytes: document.save(),
                request_hash: "first".into(),
            },
        )
        .await
        .unwrap();
    let prior = store
        .durable_document_for_scope(&scope, "doc.md")
        .await
        .unwrap()
        .unwrap();
    drop(store);
    // Inject a real SQL constraint failure at the final native-head write.
    // Exclude the initial import, leaving one indexed version per document.
    execute_sql(
        root.path(),
        &format!("CREATE UNIQUE INDEX injected_commit_failure ON document_state_versions(document_id) WHERE version_id != '{}'", commit.expected_head_version_id),
    )
    .await;
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let mut events = store.subscribe_events();
    document
        .insert_block(SeedBlock {
            id: "candidate".into(),
            kind: "p".into(),
            parent: None,
            position: 1,
            attrs: Default::default(),
            text: "Candidate".into(),
        })
        .unwrap();
    let failed = BlockMutationCommit {
        expected_head_version_id: prior.version_id.clone(),
        client_tx_id: "retryable".into(),
        rows: document_projection(&document).unwrap(),
        normalized_markdown: "See TARGET here.\n\nCandidate\n".into(),
        ..commit.clone()
    };
    assert!(
        store
            .commit_durable_document_for_scope(
                &scope,
                failed,
                DurableDocumentCommit {
                    bytes: document.save(),
                    request_hash: "failed".into()
                }
            )
            .await
            .is_err()
    );
    let after = store
        .durable_document_for_scope(&scope, "doc.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prior.version_id, after.version_id);
    assert_eq!(prior.bytes, after.bytes);
    assert!(events.try_recv().is_err());
    drop(store);
    execute_sql(root.path(), "DROP INDEX injected_commit_failure").await;
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let mut retry = Document::load(&prior.bytes).unwrap();
    let id = retry.blocks().unwrap()[0].id.clone();
    let point = retry.point(&id, 4).unwrap();
    retry.insert_text(&point, "new ").unwrap();
    let corrected = BlockMutationCommit {
        expected_head_version_id: prior.version_id,
        client_tx_id: "retryable".into(),
        rows: document_projection(&retry).unwrap(),
        normalized_markdown: "See new TARGET here.\n".into(),
        ..commit
    };
    assert!(matches!(
        store
            .commit_durable_document_for_scope(
                &scope,
                corrected,
                DurableDocumentCommit {
                    bytes: retry.save(),
                    request_hash: "corrected".into()
                }
            )
            .await
            .unwrap(),
        BlockMutationOutcome::Applied { .. }
    ));
}

#[tokio::test]
async fn stale_and_legacy_writers_cannot_replace_canonical_state() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let (document, commit) = setup(&store, "one").await;
    let scope = DocumentScopeRef::library("one");
    store
        .commit_durable_document_for_scope(
            &scope,
            commit.clone(),
            DurableDocumentCommit {
                bytes: document.save(),
                request_hash: "first".into(),
            },
        )
        .await
        .unwrap();
    let prior = store
        .durable_document_for_scope(&scope, "doc.md")
        .await
        .unwrap()
        .unwrap();
    let stale = BlockMutationCommit {
        client_tx_id: "stale".into(),
        ..commit.clone()
    };
    assert!(
        store
            .commit_durable_document_for_scope(
                &scope,
                stale,
                DurableDocumentCommit {
                    bytes: document.save(),
                    request_hash: "stale".into()
                }
            )
            .await
            .is_err()
    );
    let reset = Document::with_id(&commit.document_id).unwrap();
    let reset_commit = BlockMutationCommit {
        expected_head_version_id: prior.version_id.clone(),
        client_tx_id: "reset-history".into(),
        rows: Vec::new(),
        review_items: Vec::new(),
        normalized_markdown: String::new(),
        ..commit
    };
    assert!(
        store
            .commit_durable_document_for_scope(
                &scope,
                reset_commit,
                DurableDocumentCommit {
                    bytes: reset.save(),
                    request_hash: "reset-history".into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .durable_document_for_scope(&scope, "doc.md")
            .await
            .unwrap()
            .unwrap()
            .bytes,
        prior.bytes
    );
}

#[tokio::test]
async fn archive_import_is_create_only_and_retains_review_metadata_and_history_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    store.create_library("archive").await.unwrap();
    store.import_block_document("archive", "original.md", "---\ntitle: Review\ntags: [one, two]\n---\nSee {==TARGET==}{>>Keep<<}{#c} and {~~old~>new~~}{#s}.\n", json!({}), "text/markdown", DocumentSource::Rest, WritePrecondition::None).await.unwrap();
    let scope = DocumentScopeRef::library("archive");
    let original = store
        .export_document_archive(&scope, "original.md")
        .await
        .unwrap();
    let before = Document::load(&original.bytes).unwrap();
    let original_json = serde_json::to_value(&original).unwrap();
    let imported = store
        .import_document_archive(&scope, "copy.md", original)
        .await
        .unwrap();
    assert_ne!(imported.document.id.as_str(), before.id().unwrap());
    let state = store
        .durable_document_for_scope(&scope, "copy.md")
        .await
        .unwrap()
        .unwrap();
    let copy = Document::load(&state.bytes).unwrap();
    assert_eq!(copy.view().unwrap().blocks, before.view().unwrap().blocks);
    assert_eq!(
        copy.view().unwrap().comments,
        before.view().unwrap().comments
    );
    assert_eq!(
        copy.view().unwrap().proposals,
        before.view().unwrap().proposals
    );
    assert_eq!(
        copy.fork_at(&before.heads()).unwrap().view().unwrap(),
        before.view().unwrap()
    );
    assert_eq!(imported.version.metadata["title"], "Review");
    assert_eq!(imported.version.metadata["tags"], json!(["one", "two"]));
    assert!(
        store
            .import_document_archive(
                &scope,
                "copy.md",
                serde_json::from_value(original_json.clone()).unwrap()
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .durable_document_for_scope(&scope, "copy.md")
            .await
            .unwrap()
            .unwrap()
            .bytes,
        state.bytes
    );
    let mut invalid = original_json.clone();
    invalid["version"] = json!(999);
    assert!(
        store
            .import_document_archive(
                &scope,
                "invalid.md",
                serde_json::from_value(invalid).unwrap()
            )
            .await
            .is_err()
    );
    assert!(store.get_document("archive", "invalid.md").await.is_err());
    let mut invalid = original_json;
    invalid["bytes"] = json!([0, 1, 2, 3]);
    assert!(
        store
            .import_document_archive(
                &scope,
                "invalid.md",
                serde_json::from_value(invalid).unwrap()
            )
            .await
            .is_err()
    );
    drop(store);
    let store = QuarryStore::open(config(root.path())).await.unwrap();
    let archived = store
        .export_document_archive(&scope, "copy.md")
        .await
        .unwrap();
    assert_eq!(archived.bytes, state.bytes);
    assert_eq!(archived.metadata["title"], "Review");
}

#[tokio::test]
async fn concurrent_reads_never_mix_native_bytes_and_published_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(QuarryStore::open(config(root.path())).await.unwrap());
    store.create_library("reading").await.unwrap();
    let put = |revision| quarry_storage::PutDocumentRequest {
        library: "reading".into(),
        path: "doc.md".into(),
        content: format!("Revision {revision}\n").into_bytes(),
        metadata: json!({"revision":revision}),
        content_type: "text/markdown".into(),
        source: DocumentSource::Rest,
        precondition: WritePrecondition::None,
        origin_id: None,
        transaction: Default::default(),
    };
    store.put_document(put(0)).await.unwrap();
    let writer = store.clone();
    let write = async move {
        for revision in 1..50 {
            writer.put_document(put(revision)).await.unwrap();
            tokio::task::yield_now().await;
        }
    };
    let read = async {
        for _ in 0..200 {
            let saved = store
                .durable_document_for_scope(&DocumentScopeRef::library("reading"), "doc.md")
                .await
                .unwrap()
                .unwrap();
            let native = Document::load(&saved.bytes).unwrap();
            assert_eq!(
                native.view().unwrap().blocks[0].text,
                format!("Revision {}", saved.metadata["revision"])
            );
            assert_eq!(native.id().unwrap(), saved.document_id);
            tokio::task::yield_now().await;
        }
    };
    tokio::join!(write, read);
}
