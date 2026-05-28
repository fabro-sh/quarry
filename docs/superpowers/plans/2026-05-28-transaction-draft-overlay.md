# Transaction Draft Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Quarry hold server-side drafts inside open transactions, supporting hidden read/write/read cycles before commit publishes the final draft state.

**Architecture:** Treat an open transaction as a private overlay on top of committed document heads. Normal document reads remain committed-only; transaction-scoped reads and lists resolve `transaction_changes` first and fall back to committed state. Repeated writes to the same draft path preserve the original base version and replace the staged version, so autosaves do not weaken stale-head protection or leak superseded drafts into committed history.

**Tech Stack:** Rust 2021, Turso, Axum, Utoipa, Tower tests, existing `quarry-storage`, `quarry-server`, and operation docs.

---

## File Structure

- Modify `crates/quarry-storage/src/lib.rs`
  - Add transaction-scoped draft read/list methods.
  - Add staged-change replacement that preserves the first `old_version_id`.
  - Add helpers for loading staged rows with `rowid`, loading versions by ID, and building document/list entries from explicit versions.
  - Filter `version_history` to committed transactions only.
- Modify `crates/quarry-storage/tests/storage_lifecycle.rs`
  - Add storage-level tests for hidden draft reads, repeated autosaves, stale-head protection, delete/move overlays, and committed-only history.
- Modify `crates/quarry-server/src/lib.rs`
  - Add REST `GET`/`HEAD` for transaction-scoped documents.
  - Add REST `GET` for transaction-scoped document lists.
  - Include the new handlers in OpenAPI.
- Modify `crates/quarry-server/tests/rest_api.rs`
  - Add REST tests for draft read/write/read, committed invisibility, draft list, draft move/delete behavior, publish, and library scoping.
- Modify `docs/operations/rest-api.md`
  - Document transaction-scoped draft read/list endpoints and clarify committed-only normal reads.

This plan intentionally does not add CLI draft aliases. The existing CLI only exposes `tx begin/commit/rollback`, not staged writes, so adding a friendly draft CLI is a separate UX feature.

---

### Task 1: Add Failing Storage Tests For Draft Overlay Semantics

**Files:**
- Modify: `crates/quarry-storage/tests/storage_lifecycle.rs`

- [ ] **Step 1: Add the hidden draft/autosave test**

Add this test after `explicit_transactions_publish_atomically_and_rollback_staged_cas`:

```rust
#[tokio::test]
async fn open_transaction_can_read_hidden_draft_and_publish_final_autosave() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("drafts").await.unwrap();

    let tx = store
        .begin_transaction(
            &library.slug,
            DocumentSource::Rest,
            Some("agent".to_string()),
            Some("draft autosaves".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    let first = store
        .stage_put(
            &tx.id,
            "docs/draft.md",
            b"draft one".to_vec(),
            serde_json::json!({"content_type":"text/markdown","revision":1}),
            "text/markdown",
        )
        .await
        .unwrap();
    assert!(store.get_document(&library.slug, "docs/draft.md").await.is_err());

    let visible_in_tx = store
        .get_document_in_transaction(&library.slug, &tx.id, "docs/draft.md")
        .await
        .unwrap();
    assert_eq!(visible_in_tx.content, b"draft one");
    assert_eq!(visible_in_tx.version.id, first.id);

    let second = store
        .stage_put(
            &tx.id,
            "docs/draft.md",
            b"draft two".to_vec(),
            serde_json::json!({"content_type":"text/markdown","revision":2}),
            "text/markdown",
        )
        .await
        .unwrap();
    assert_ne!(first.id, second.id);

    let visible_in_tx = store
        .get_document_in_transaction(&library.slug, &tx.id, "docs/draft.md")
        .await
        .unwrap();
    assert_eq!(visible_in_tx.content, b"draft two");
    assert_eq!(visible_in_tx.metadata["revision"], 2);
    assert_eq!(visible_in_tx.version.id, second.id);

    let draft_list = store
        .list_documents_in_transaction(&library.slug, &tx.id, Some("docs/"), Some(100))
        .await
        .unwrap();
    assert_eq!(draft_list.len(), 1);
    assert_eq!(draft_list[0].path, "docs/draft.md");
    assert_eq!(draft_list[0].head_version_id, second.id);

    assert_eq!(
        store
            .version_history(&library.slug, "docs/draft.md")
            .await
            .unwrap()
            .len(),
        0
    );

    store.commit_transaction(&tx.id).await.unwrap();

    let committed = store.get_document(&library.slug, "docs/draft.md").await.unwrap();
    assert_eq!(committed.content, b"draft two");
    assert_eq!(committed.version.id, second.id);
    assert_eq!(
        store
            .version_history(&library.slug, "docs/draft.md")
            .await
            .unwrap()
            .iter()
            .map(|version| version.id.clone())
            .collect::<Vec<_>>(),
        vec![second.id]
    );
}
```

- [ ] **Step 2: Add the base-preservation test**

Add this test after `explicit_transaction_commit_rejects_stale_heads_without_overwriting_newer_writes`:

```rust
#[tokio::test]
async fn repeated_draft_write_preserves_original_base_for_commit_precondition() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("draft_race").await.unwrap();

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
            Some("draft race".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    store
        .stage_put(
            &tx.id,
            "docs/a.md",
            b"draft one".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();

    let newer = store
        .put_document(
            &library.slug,
            "docs/a.md",
            b"newer committed".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfMatch(base.version.id.clone()),
        )
        .await
        .unwrap();

    store
        .stage_put(
            &tx.id,
            "docs/a.md",
            b"draft two".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
        )
        .await
        .unwrap();

    let draft = store
        .get_document_in_transaction(&library.slug, &tx.id, "docs/a.md")
        .await
        .unwrap();
    assert_eq!(draft.content, b"draft two");

    let error = store.commit_transaction(&tx.id).await.unwrap_err();
    assert!(error.to_string().contains("precondition failed"));

    let committed = store.get_document(&library.slug, "docs/a.md").await.unwrap();
    assert_eq!(committed.content, b"newer committed");
    assert_eq!(committed.version.id, newer.version.id);
}
```

- [ ] **Step 3: Add the delete/move overlay test**

Add this test after the base-preservation test:

```rust
#[tokio::test]
async fn draft_overlay_reads_staged_delete_and_move_without_publishing() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("draft_moves").await.unwrap();

    store
        .put_document(
            &library.slug,
            "docs/delete.md",
            b"delete me".to_vec(),
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
            "docs/source.md",
            b"move me".to_vec(),
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
            None,
            Some("draft delete and move".to_string()),
            serde_json::json!({}),
        )
        .await
        .unwrap();

    store.stage_delete(&tx.id, "docs/delete.md").await.unwrap();
    store
        .stage_metadata(&tx.id, "docs/source.md", serde_json::json!({"drafted":true}))
        .await
        .unwrap();
    store
        .stage_move(&tx.id, "docs/source.md", "docs/moved.md")
        .await
        .unwrap();

    assert!(store
        .get_document_in_transaction(&library.slug, &tx.id, "docs/delete.md")
        .await
        .is_err());
    assert!(store
        .get_document_in_transaction(&library.slug, &tx.id, "docs/source.md")
        .await
        .is_err());

    let moved = store
        .get_document_in_transaction(&library.slug, &tx.id, "docs/moved.md")
        .await
        .unwrap();
    assert_eq!(moved.content, b"move me");
    assert_eq!(moved.metadata["drafted"], true);
    assert_eq!(moved.path, "docs/moved.md");

    assert!(store.get_document(&library.slug, "docs/moved.md").await.is_err());
    assert!(store.get_document(&library.slug, "docs/source.md").await.is_ok());
    assert!(store.get_document(&library.slug, "docs/delete.md").await.is_ok());

    let paths = store
        .list_documents_in_transaction(&library.slug, &tx.id, Some("docs/"), Some(100))
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["docs/moved.md"]);
}
```

- [ ] **Step 4: Run the storage tests and verify they fail**

Run:

```bash
cargo test -p quarry-storage draft
```

Expected: FAIL because `get_document_in_transaction` and `list_documents_in_transaction` do not exist.

- [ ] **Step 5: Commit the failing tests**

```bash
git add crates/quarry-storage/tests/storage_lifecycle.rs
git commit -m "test: specify transaction draft overlay behavior"
```

---

### Task 2: Preserve Draft Base Versions And Hide Uncommitted Versions From History

**Files:**
- Modify: `crates/quarry-storage/src/lib.rs`
- Test: `crates/quarry-storage/tests/storage_lifecycle.rs`

- [ ] **Step 1: Add row IDs to staged changes**

Change the private struct near the top of `crates/quarry-storage/src/lib.rs` to include `rowid`:

```rust
struct StagedChange {
    rowid: i64,
    path: String,
    change_type: String,
    old_version_id: Option<String>,
    new_version_id: Option<String>,
    new_path: Option<String>,
}
```

Update the commit query in `commit_transaction` to select `rowid`:

```rust
"SELECT rowid, path, change_type, old_version_id, new_version_id, new_path
 FROM transaction_changes
 WHERE tx_id = ?1 ORDER BY rowid"
```

Update row decoding in that loop:

```rust
changes.push(StagedChange {
    rowid: int(&row, 0)?,
    path: text(&row, 1)?,
    change_type: text(&row, 2)?,
    old_version_id: opt_text(&row, 3)?,
    new_version_id: opt_text(&row, 4)?,
    new_path: opt_text(&row, 5)?,
});
```

- [ ] **Step 2: Add a replacement helper that preserves the first base version**

Replace `delete_staged_change_conn` calls for `Put`, `Metadata`, and `Delete` paths with this helper. Add it near `insert_change_conn`:

```rust
async fn replace_staged_change_conn(
    conn: &Connection,
    tx_id: &str,
    path: &str,
    change_type: ChangeType,
    observed_old_version_id: Option<&str>,
    new_version_id: Option<&str>,
    new_path: Option<&str>,
) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT old_version_id, new_version_id
             FROM transaction_changes
             WHERE tx_id = ?1 AND path = ?2
             ORDER BY rowid DESC LIMIT 1",
            params![tx_id.to_string(), path.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    let existing = rows.next().await.map_err(map_turso_error)?;
    let preserved_old_version_id = existing
        .as_ref()
        .map(|row| opt_text(row, 0))
        .transpose()?
        .flatten()
        .or_else(|| observed_old_version_id.map(ToOwned::to_owned));
    let superseded_new_version_id = existing
        .as_ref()
        .map(|row| opt_text(row, 1))
        .transpose()?
        .flatten();
    drop(rows);

    delete_staged_change_conn(conn, tx_id, path).await?;
    insert_change_conn(
        conn,
        tx_id,
        path,
        change_type,
        preserved_old_version_id.as_deref(),
        new_version_id,
        new_path,
    )
    .await?;

    if let Some(version_id) = superseded_new_version_id {
        if Some(version_id.as_str()) != new_version_id {
            delete_unreferenced_open_version_conn(conn, tx_id, &version_id).await?;
        }
    }

    Ok(())
}
```

Add this helper below it:

```rust
async fn delete_unreferenced_open_version_conn(
    conn: &Connection,
    tx_id: &str,
    version_id: &str,
) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM transaction_changes WHERE new_version_id = ?1 LIMIT 1",
            params![version_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    if rows.next().await.map_err(map_turso_error)?.is_some() {
        return Ok(());
    }
    drop(rows);

    conn.execute(
        "DELETE FROM document_versions
         WHERE id = ?1
           AND tx_id = ?2
           AND EXISTS (
             SELECT 1 FROM transactions
             WHERE transactions.id = document_versions.tx_id
               AND transactions.state = 'open'
           )",
        params![version_id.to_string(), tx_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}
```

- [ ] **Step 3: Use replacement helper in staging methods**

In `stage_put`, replace:

```rust
delete_staged_change_conn(&conn, tx_id, &path).await?;
let version = self
    .insert_version_conn(&conn, &doc_id, tx_id, content, metadata, content_type)
    .await?;
insert_change_conn(
    &conn,
    tx_id,
    &path,
    ChangeType::Put,
    old_version_id.as_deref(),
    Some(&version.id),
    None,
)
.await?;
```

with:

```rust
let version = self
    .insert_version_conn(&conn, &doc_id, tx_id, content, metadata, content_type)
    .await?;
replace_staged_change_conn(
    &conn,
    tx_id,
    &path,
    ChangeType::Put,
    old_version_id.as_deref(),
    Some(&version.id),
    None,
)
.await?;
```

In `stage_delete`, replace the `delete_staged_change_conn` plus `insert_change_conn` pair with:

```rust
replace_staged_change_conn(
    &conn,
    tx_id,
    &path,
    ChangeType::Delete,
    old_version_id.as_deref(),
    None,
    None,
)
.await
```

In `stage_metadata`, replace the `delete_staged_change_conn` plus `insert_change_conn` pair with:

```rust
replace_staged_change_conn(
    &conn,
    tx_id,
    &path,
    ChangeType::Metadata,
    Some(&current.version.id),
    Some(&version.id),
    None,
)
.await?;
```

- [ ] **Step 4: Make version history committed-only**

Change the `version_history` query from:

```rust
"SELECT id, document_id, tx_id, content_hash, inline_content, metadata_json, content_type, byte_size, created_at
 FROM document_versions WHERE document_id = ?1 ORDER BY created_at, id"
```

to:

```rust
"SELECT dv.id, dv.document_id, dv.tx_id, dv.content_hash, dv.inline_content,
        dv.metadata_json, dv.content_type, dv.byte_size, dv.created_at
 FROM document_versions dv
 JOIN transactions t ON t.id = dv.tx_id
 WHERE dv.document_id = ?1 AND t.state = 'committed'
 ORDER BY dv.created_at, dv.id"
```

- [ ] **Step 5: Run the focused storage tests**

Run:

```bash
cargo test -p quarry-storage repeated_draft_write_preserves_original_base_for_commit_precondition
cargo test -p quarry-storage open_transaction_can_read_hidden_draft_and_publish_final_autosave
```

Expected after this task: the first command still FAILS because draft read APIs are missing, but the stale-head behavior will be ready for Task 3. The second command still FAILS because draft read/list APIs are missing.

- [ ] **Step 6: Commit base preservation and history filtering**

```bash
git add crates/quarry-storage/src/lib.rs
git commit -m "feat: preserve draft base versions"
```

---

### Task 3: Add Storage Draft Read And List APIs

**Files:**
- Modify: `crates/quarry-storage/src/lib.rs`
- Test: `crates/quarry-storage/tests/storage_lifecycle.rs`

- [ ] **Step 1: Add a private resolution enum**

Add near `StagedChange`:

```rust
enum DraftResolution {
    CommittedPath(String),
    Version {
        visible_path: String,
        version_id: String,
    },
    Deleted,
}
```

- [ ] **Step 2: Add public transaction-scoped read methods**

Add these methods inside `impl QuarryStore`, near `get_document` and `head_document`:

```rust
pub async fn get_document_in_transaction(
    &self,
    library: &str,
    tx_id: &str,
    path: &str,
) -> Result<Document> {
    let path = normalize_path(path)?;
    let conn = self.conn()?;
    let library = self.require_library_conn(&conn, library).await?;
    self.ensure_transaction_open_for_library_conn(&conn, &library.id, tx_id)
        .await?;

    match self
        .draft_resolution_conn(&conn, &library.id, tx_id, &path)
        .await?
    {
        DraftResolution::CommittedPath(path) => self.document_conn(&conn, &library.id, &path).await,
        DraftResolution::Version {
            visible_path,
            version_id,
        } => {
            self.document_from_version_conn(&conn, &library.id, &visible_path, &version_id)
                .await
        }
        DraftResolution::Deleted => Err(QuarryError::NotFound(path)),
    }
}

pub async fn head_document_in_transaction(
    &self,
    library: &str,
    tx_id: &str,
    path: &str,
) -> Result<DocumentListEntry> {
    let document = self
        .get_document_in_transaction(library, tx_id, path)
        .await?;
    Ok(DocumentListEntry {
        id: document.id,
        path: document.path,
        head_version_id: document.version.id,
        content_type: document.version.content_type,
        byte_size: document.version.byte_size,
        metadata: document.metadata,
        updated_at: document.updated_at,
    })
}
```

- [ ] **Step 3: Add public transaction-scoped list method**

Add near `list_documents`:

```rust
pub async fn list_documents_in_transaction(
    &self,
    library: &str,
    tx_id: &str,
    prefix: Option<&str>,
    limit: Option<u64>,
) -> Result<Vec<DocumentListEntry>> {
    let conn = self.conn()?;
    let library = self.require_library_conn(&conn, library).await?;
    self.ensure_transaction_open_for_library_conn(&conn, &library.id, tx_id)
        .await?;
    let normalized_prefix = match prefix {
        Some("") | None => None,
        Some(prefix) => Some(normalize_prefix(prefix)?),
    };
    let limit = limit.unwrap_or(1000).min(10_000) as usize;

    let mut entries = self
        .list_documents(&library.slug, None, Some(10_000))
        .await?
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();

    let changes = load_staged_changes_conn(&conn, tx_id).await?;
    for change in changes {
        match change.change_type.as_str() {
            "put" | "metadata" => {
                let version_id = change.new_version_id.ok_or_else(|| {
                    QuarryError::Storage("draft put missing version".to_string())
                })?;
                let entry = self
                    .document_entry_from_version_conn(&conn, &library.id, &change.path, &version_id)
                    .await?;
                entries.insert(change.path, entry);
            }
            "delete" => {
                entries.remove(&change.path);
            }
            "move" => {
                let new_path = change.new_path.ok_or_else(|| {
                    QuarryError::Storage("draft move missing new path".to_string())
                })?;
                let moved = if let Some(mut entry) = entries.remove(&change.path) {
                    entry.path = new_path.clone();
                    entry
                } else {
                    let document = self.document_conn(&conn, &library.id, &change.path).await?;
                    DocumentListEntry {
                        id: document.id,
                        path: new_path.clone(),
                        head_version_id: document.version.id,
                        content_type: document.version.content_type,
                        byte_size: document.version.byte_size,
                        metadata: document.metadata,
                        updated_at: document.updated_at,
                    }
                };
                entries.insert(new_path, moved);
            }
            other => return Err(QuarryError::Storage(format!("unknown change type {other}"))),
        }
    }

    Ok(entries
        .into_values()
        .filter(|entry| {
            normalized_prefix
                .as_ref()
                .map(|prefix| entry.path.starts_with(prefix))
                .unwrap_or(true)
        })
        .take(limit)
        .collect())
}
```

- [ ] **Step 4: Add transaction validation and staged-change helpers**

Add these private helpers in `impl QuarryStore` near `transaction_conn`:

```rust
async fn ensure_transaction_open_for_library_conn(
    &self,
    conn: &Connection,
    library_id: &str,
    tx_id: &str,
) -> Result<TransactionRecord> {
    let tx = self.transaction_conn(conn, tx_id).await?;
    if tx.library_id != library_id {
        return Err(QuarryError::NotFound(format!("transaction {tx_id}")));
    }
    ensure_open(&tx)?;
    Ok(tx)
}

async fn draft_resolution_conn(
    &self,
    conn: &Connection,
    library_id: &str,
    tx_id: &str,
    path: &str,
) -> Result<DraftResolution> {
    if let Some(change) = latest_staged_change_for_path_conn(conn, tx_id, path).await? {
        return match change.change_type.as_str() {
            "put" | "metadata" => Ok(DraftResolution::Version {
                visible_path: path.to_string(),
                version_id: change.new_version_id.ok_or_else(|| {
                    QuarryError::Storage("draft change missing new version".to_string())
                })?,
            }),
            "delete" | "move" => Ok(DraftResolution::Deleted),
            other => Err(QuarryError::Storage(format!("unknown change type {other}"))),
        };
    }

    if let Some(change) = latest_staged_move_to_path_conn(conn, tx_id, path).await? {
        if let Some(source_change) =
            latest_staged_change_for_path_conn(conn, tx_id, &change.path).await?
        {
            return match source_change.change_type.as_str() {
                "put" | "metadata" => Ok(DraftResolution::Version {
                    visible_path: path.to_string(),
                    version_id: source_change.new_version_id.ok_or_else(|| {
                        QuarryError::Storage("draft source change missing new version".to_string())
                    })?,
                }),
                "move" => Ok(DraftResolution::CommittedPath(change.path)),
                "delete" => Ok(DraftResolution::Deleted),
                other => Err(QuarryError::Storage(format!("unknown change type {other}"))),
            };
        }

        if self
            .document_identity_conn(conn, library_id, &change.path)
            .await?
            .is_some()
        {
            return Ok(DraftResolution::CommittedPath(change.path));
        }
        return Ok(DraftResolution::Deleted);
    }

    Ok(DraftResolution::CommittedPath(path.to_string()))
}
```

Add these free functions near `insert_change_conn`:

```rust
async fn load_staged_changes_conn(conn: &Connection, tx_id: &str) -> Result<Vec<StagedChange>> {
    let mut rows = conn
        .query(
            "SELECT rowid, path, change_type, old_version_id, new_version_id, new_path
             FROM transaction_changes
             WHERE tx_id = ?1 ORDER BY rowid",
            params![tx_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    let mut changes = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        changes.push(StagedChange {
            rowid: int(&row, 0)?,
            path: text(&row, 1)?,
            change_type: text(&row, 2)?,
            old_version_id: opt_text(&row, 3)?,
            new_version_id: opt_text(&row, 4)?,
            new_path: opt_text(&row, 5)?,
        });
    }
    Ok(changes)
}

async fn latest_staged_change_for_path_conn(
    conn: &Connection,
    tx_id: &str,
    path: &str,
) -> Result<Option<StagedChange>> {
    let mut rows = conn
        .query(
            "SELECT rowid, path, change_type, old_version_id, new_version_id, new_path
             FROM transaction_changes
             WHERE tx_id = ?1 AND path = ?2
             ORDER BY rowid DESC LIMIT 1",
            params![tx_id.to_string(), path.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    rows.next()
        .await
        .map_err(map_turso_error)?
        .map(|row| {
            Ok(StagedChange {
                rowid: int(&row, 0)?,
                path: text(&row, 1)?,
                change_type: text(&row, 2)?,
                old_version_id: opt_text(&row, 3)?,
                new_version_id: opt_text(&row, 4)?,
                new_path: opt_text(&row, 5)?,
            })
        })
        .transpose()
}

async fn latest_staged_move_to_path_conn(
    conn: &Connection,
    tx_id: &str,
    path: &str,
) -> Result<Option<StagedChange>> {
    let mut rows = conn
        .query(
            "SELECT rowid, path, change_type, old_version_id, new_version_id, new_path
             FROM transaction_changes
             WHERE tx_id = ?1 AND change_type = 'move' AND new_path = ?2
             ORDER BY rowid DESC LIMIT 1",
            params![tx_id.to_string(), path.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    rows.next()
        .await
        .map_err(map_turso_error)?
        .map(|row| {
            Ok(StagedChange {
                rowid: int(&row, 0)?,
                path: text(&row, 1)?,
                change_type: text(&row, 2)?,
                old_version_id: opt_text(&row, 3)?,
                new_version_id: opt_text(&row, 4)?,
                new_path: opt_text(&row, 5)?,
            })
        })
        .transpose()
}
```

- [ ] **Step 5: Add version-backed document builders**

Add these private methods near `document_conn`:

```rust
async fn document_from_version_conn(
    &self,
    conn: &Connection,
    library_id: &str,
    visible_path: &str,
    version_id: &str,
) -> Result<Document> {
    let mut rows = conn
        .query(
            "SELECT d.id, d.library_id, d.path, d.created_at, d.updated_at,
                    v.id, v.document_id, v.tx_id, v.content_hash, v.inline_content,
                    v.metadata_json, v.content_type, v.byte_size, v.created_at
             FROM document_versions v
             JOIN documents d ON d.id = v.document_id
             WHERE d.library_id = ?1 AND v.id = ?2
             LIMIT 1",
            params![library_id.to_string(), version_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    let row = rows
        .next()
        .await
        .map_err(map_turso_error)?
        .ok_or_else(|| QuarryError::NotFound(format!("version {version_id}")))?;
    let version = DocumentVersion {
        id: text(&row, 5)?,
        document_id: text(&row, 6)?,
        tx_id: text(&row, 7)?,
        content_hash: opt_text(&row, 8)?,
        inline_content: opt_blob(&row, 9)?,
        metadata: serde_json::from_str(&text(&row, 10)?)?,
        content_type: text(&row, 11)?,
        byte_size: int(&row, 12)? as u64,
        created_at: text(&row, 13)?,
    };
    let content = match (&version.inline_content, &version.content_hash) {
        (Some(bytes), None) => bytes.clone(),
        (None, Some(hash)) => self.cas.read(hash)?,
        _ => {
            return Err(QuarryError::Storage(format!(
                "version {} violates inline/CAS invariant",
                version.id
            )))
        }
    };
    Ok(Document {
        id: text(&row, 0)?,
        library_id: text(&row, 1)?,
        path: visible_path.to_string(),
        metadata: version.metadata.clone(),
        version,
        content,
        created_at: text(&row, 3)?,
        updated_at: text(&row, 4)?,
    })
}

async fn document_entry_from_version_conn(
    &self,
    conn: &Connection,
    library_id: &str,
    visible_path: &str,
    version_id: &str,
) -> Result<DocumentListEntry> {
    let document = self
        .document_from_version_conn(conn, library_id, visible_path, version_id)
        .await?;
    Ok(DocumentListEntry {
        id: document.id,
        path: document.path,
        head_version_id: document.version.id,
        content_type: document.version.content_type,
        byte_size: document.version.byte_size,
        metadata: document.metadata,
        updated_at: document.updated_at,
    })
}
```

- [ ] **Step 6: Reuse the staged-change loader in commit**

In `commit_transaction`, replace the local query/loop that builds `changes` with:

```rust
let changes = load_staged_changes_conn(&conn, tx_id).await?;
```

Keep the validation and publishing loops as they are.

- [ ] **Step 7: Run focused storage tests**

Run:

```bash
cargo test -p quarry-storage open_transaction_can_read_hidden_draft_and_publish_final_autosave
cargo test -p quarry-storage repeated_draft_write_preserves_original_base_for_commit_precondition
cargo test -p quarry-storage draft_overlay_reads_staged_delete_and_move_without_publishing
```

Expected: PASS.

- [ ] **Step 8: Run the full storage crate tests**

Run:

```bash
cargo test -p quarry-storage
```

Expected: PASS.

- [ ] **Step 9: Commit storage draft APIs**

```bash
git add crates/quarry-storage/src/lib.rs crates/quarry-storage/tests/storage_lifecycle.rs
git commit -m "feat: add transaction draft reads"
```

---

### Task 4: Add REST Draft Read And List Endpoints

**Files:**
- Modify: `crates/quarry-server/src/lib.rs`
- Modify: `crates/quarry-server/tests/rest_api.rs`

- [ ] **Step 1: Add failing REST test for draft read/write/read**

Add this test after `rest_api_supports_transaction_metadata_patch_and_move`:

```rust
#[tokio::test]
async fn rest_api_supports_transaction_draft_reads_and_lists() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("draftapi").await.unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/draftapi/transactions",
            serde_json::json!({"message":"draft"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/draftapi/transactions/{tx}/documents/docs/a.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("x-quarry-metadata", r#"{"draft":1}"#)
                .body(Body::from("draft one"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first_etag = response.headers()[header::ETAG].to_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/draftapi/documents/docs/a.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/draftapi/transactions/{tx}/documents/docs/a.md"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], first_etag);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "draft one"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/draftapi/transactions/{tx}/documents/docs/a.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("x-quarry-metadata", r#"{"draft":2}"#)
                .body(Body::from("draft two"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let second_etag = response.headers()[header::ETAG].to_str().unwrap().to_string();
    assert_ne!(first_etag, second_etag);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri(format!(
                    "/v1/libraries/draftapi/transactions/{tx}/documents/docs/a.md"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], second_etag);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/draftapi/transactions/{tx}/documents?prefix=docs/"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["path"], "docs/a.md");
    assert_eq!(body[0]["head_version_id"], second_etag.trim_matches('"'));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/draftapi/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/draftapi/documents/docs/a.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "draft two"
    );
}
```

- [ ] **Step 2: Add failing REST test for scoped transaction draft reads**

Add this assertion block to `rest_api_scopes_transaction_routes_to_the_url_library`, after the existing wrong-library `DELETE` assertion and before wrong-library commit:

```rust
let response = app
    .clone()
    .oneshot(
        Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/v1/libraries/other/transactions/{tx}/documents/drafts/a.md"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::NOT_FOUND);

let response = app
    .clone()
    .oneshot(
        Request::builder()
            .method(Method::HEAD)
            .uri(format!(
                "/v1/libraries/other/transactions/{tx}/documents/drafts/a.md"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::NOT_FOUND);

let response = app
    .clone()
    .oneshot(
        Request::builder()
            .method(Method::GET)
            .uri(format!("/v1/libraries/other/transactions/{tx}/documents"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::NOT_FOUND);
```

- [ ] **Step 3: Run REST tests and verify they fail**

Run:

```bash
cargo test -p quarry-server rest_api_supports_transaction_draft_reads_and_lists
```

Expected: FAIL with `405 Method Not Allowed` or missing route for transaction document `GET`.

- [ ] **Step 4: Add transaction document routes**

In `router`, add a collection route before the wildcard transaction document route:

```rust
.route(
    "/v1/libraries/{library}/transactions/{tx}/documents",
    get(list_transaction_documents),
)
```

Change the wildcard transaction document route from:

```rust
put(stage_put_document)
    .post(post_transaction_document_action)
    .patch(patch_transaction_document_metadata)
    .delete(stage_delete_document),
```

to:

```rust
get(get_transaction_document)
    .head(head_transaction_document)
    .put(stage_put_document)
    .post(post_transaction_document_action)
    .patch(patch_transaction_document_metadata)
    .delete(stage_delete_document),
```

- [ ] **Step 5: Add handlers**

Add these handlers near the existing transaction handlers:

```rust
#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/transactions/{tx}/documents",
    params(("library" = String, Path), ("tx" = String, Path), ListQuery),
    responses((status = 200, body = [DocumentListEntry]), (status = 404, body = ErrorResponse))
)]
async fn list_transaction_documents(
    State(state): State<AppState>,
    Path((library, tx)): Path<(String, String)>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<DocumentListEntry>>, ApiError> {
    scoped_transaction(&state.store, &library, &tx).await?;
    Ok(Json(
        state
            .store
            .list_documents_in_transaction(&library, &tx, query.prefix.as_deref(), query.limit)
            .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/transactions/{tx}/documents/{path}",
    params(("library" = String, Path), ("tx" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = String), (status = 404, body = ErrorResponse))
)]
async fn get_transaction_document(
    State(state): State<AppState>,
    Path((library, tx, path)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    scoped_transaction(&state.store, &library, &tx).await?;
    let document = state
        .store
        .get_document_in_transaction(&library, &tx, &path)
        .await?;
    bytes_response(
        StatusCode::OK,
        document.content,
        &document.version.content_type,
        &document.version.id,
    )
}

#[utoipa::path(
    head,
    path = "/v1/libraries/{library}/transactions/{tx}/documents/{path}",
    params(("library" = String, Path), ("tx" = String, Path), ("path" = String, Path)),
    responses((status = 200), (status = 404, body = ErrorResponse))
)]
async fn head_transaction_document(
    State(state): State<AppState>,
    Path((library, tx, path)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    scoped_transaction(&state.store, &library, &tx).await?;
    let document = state
        .store
        .head_document_in_transaction(&library, &tx, &path)
        .await?;
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(&document.head_version_id)).unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&document.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    Ok(response)
}
```

- [ ] **Step 6: Register handlers in OpenAPI**

Add these names to the `paths` list in the `OpenApi` derive:

```rust
list_transaction_documents,
get_transaction_document,
head_transaction_document,
```

- [ ] **Step 7: Assert OpenAPI exposes draft read routes**

In `rest_api_supports_documents_transactions_etags_and_openapi`, add after the transaction metadata assertion:

```rust
assert!(openapi["paths"]["/v1/libraries/{library}/transactions/{tx}/documents"]["get"].is_object());
assert!(
    openapi["paths"]["/v1/libraries/{library}/transactions/{tx}/documents/{path}"]["get"]
        .is_object()
);
assert!(
    openapi["paths"]["/v1/libraries/{library}/transactions/{tx}/documents/{path}"]["head"]
        .is_object()
);
```

- [ ] **Step 8: Run focused REST tests**

Run:

```bash
cargo test -p quarry-server rest_api_supports_transaction_draft_reads_and_lists
cargo test -p quarry-server rest_api_scopes_transaction_routes_to_the_url_library
cargo test -p quarry-server rest_api_supports_documents_transactions_etags_and_openapi
```

Expected: PASS.

- [ ] **Step 9: Commit REST support**

```bash
git add crates/quarry-server/src/lib.rs crates/quarry-server/tests/rest_api.rs
git commit -m "feat: expose transaction draft reads over REST"
```

---

### Task 5: Document Draft Read Semantics

**Files:**
- Modify: `docs/operations/rest-api.md`

- [ ] **Step 1: Update endpoint list**

In `docs/operations/rest-api.md`, add this endpoint after `POST /v1/libraries/{library}/transactions`:

```markdown
- `GET /v1/libraries/{library}/transactions/{tx}/documents?prefix=&limit=`
```

Add these endpoints alongside the existing transaction document mutation endpoints:

```markdown
- `GET /v1/libraries/{library}/transactions/{tx}/documents/{path}`
- `HEAD /v1/libraries/{library}/transactions/{tx}/documents/{path}`
```

- [ ] **Step 2: Clarify committed versus draft reads**

Replace the final paragraph with:

```markdown
Normal document reads return only committed document heads and use an `ETag` based on the visible committed version. Writes support `If-Match` and `If-None-Match: *`.

Open transactions act as private draft overlays. Transaction-scoped reads and lists resolve staged changes first, then fall back to committed documents. Staged `PUT` and metadata changes are visible through the transaction routes, staged deletes return `404` through the transaction route, and staged moves hide the old path while exposing the new path. Explicit transaction commits return `412 Precondition Failed` if any staged document head changed before commit, leaving the newer committed document visible.
```

- [ ] **Step 3: Run docs-adjacent checks**

Run:

```bash
cargo test -p quarry-server rest_api_supports_documents_transactions_etags_and_openapi
```

Expected: PASS.

- [ ] **Step 4: Commit docs**

```bash
git add docs/operations/rest-api.md
git commit -m "docs: describe transaction draft reads"
```

---

### Task 6: Final Verification

**Files:**
- All files changed in Tasks 1-5.

- [ ] **Step 1: Run focused affected tests**

Run:

```bash
cargo test -p quarry-storage draft
cargo test -p quarry-storage transaction
cargo test -p quarry-server transaction
cargo test -p quarry-server rest_api_supports_transaction_draft_reads_and_lists
```

Expected: PASS.

- [ ] **Step 2: Run full workspace tests**

Run:

```bash
cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Run workspace check**

Run:

```bash
cargo check --workspace
```

Expected: PASS.

- [ ] **Step 4: Inspect the final diff for scope**

Run:

```bash
git diff --stat HEAD~4..HEAD
git diff HEAD~4..HEAD -- crates/quarry-storage/src/lib.rs crates/quarry-server/src/lib.rs docs/operations/rest-api.md
```

Expected: changes are limited to transaction draft overlay behavior, REST exposure, tests, and docs.

- [ ] **Step 5: Commit any final fixups**

If verification required small corrections, commit them:

```bash
git add crates/quarry-storage/src/lib.rs crates/quarry-storage/tests/storage_lifecycle.rs crates/quarry-server/src/lib.rs crates/quarry-server/tests/rest_api.rs docs/operations/rest-api.md
git commit -m "fix: complete transaction draft overlay"
```

If no corrections were needed, do not create an empty commit.

---

## Self-Review

- Spec coverage: The plan covers hidden server-side draft reads, repeated writes before commit, final publish, stale-head protection, REST read/list exposure, committed-only normal reads/history, and documentation.
- Red-flag scan: No incomplete-work markers are present. Each task has concrete file paths, code snippets, commands, and expected outcomes.
- Type consistency: Public storage methods consistently use `get_document_in_transaction`, `head_document_in_transaction`, and `list_documents_in_transaction`. REST handlers call those exact methods. Draft visible versions are exposed through existing `Document`, `DocumentListEntry`, and ETag behavior.
