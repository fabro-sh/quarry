# Document TTL Expiry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add first-class TTL support so Quarry documents can expire at a chosen time and disappear from active document views.

**Architecture:** Store the current expiry timestamp on `documents.expires_at` for indexed visibility checks, and keep the same timestamp in document metadata as `expires_at` for version history and Git round-trip. Expiry is a soft delete: expired documents are hidden immediately by read-time filtering and later tombstoned by a system sweeper transaction.

**Tech Stack:** Rust 2021, Turso SQL storage, Axum REST, Clap CLI, Git import/export, FUSE projection, Tokio tests.

---

## Decisions

- Expiry means soft delete, not irreversible purge.
- The metadata key is exactly `expires_at`.
- Timestamp format is UTC RFC3339 with millisecond precision, matching `now_timestamp()`, for example `2026-05-28T15:30:00.000Z`.
- `expires_at: null` clears expiry metadata and stores `documents.expires_at = NULL`.
- Invalid `expires_at` values are rejected on storage writes, REST writes, metadata patches, transaction staging, Git import, and CLI writes.
- Expired documents are absent from normal `get`, `head`, `list`, precondition identity checks, FUSE projection, CLI reads/lists, and Git export/sync inputs.
- Historical versions remain available through `version_history`; CAS GC does not purge committed versions as part of TTL.

## File Responsibilities

- `crates/quarry-core/src/lib.rs`: add `InvalidMetadata` and expiry metadata constants/helpers that do not depend on storage.
- `crates/quarry-storage/src/lib.rs`: add schema migration, expiry normalization, visibility filtering, publish-time expiry mirroring, and the system sweeper.
- `crates/quarry-server/src/lib.rs`: accept expiry headers and map invalid metadata to HTTP 400.
- `crates/quarry-cli/src/lib.rs`: add `put --expires-at` and `put --ttl-seconds` convenience flags.
- `crates/quarry-git/src/lib.rs`: rely on storage validation and filtered lists; no separate expiry policy.
- `crates/quarry-fuse/src/lib.rs`: rely on filtered storage reads/lists; no separate expiry policy.
- Tests: extend storage, REST, CLI, Git, and FUSE suites with focused TTL scenarios.
- Docs: update `spec.md`, `docs/operations/rest-api.md`, `docs/operations/git-sync.md`, and `README.md` if the CLI examples need a TTL mention.

## Task 1: Core Metadata Validation And Schema

**Files:**
- Modify: `crates/quarry-core/src/lib.rs`
- Modify: `crates/quarry-storage/src/lib.rs`
- Test: `crates/quarry-storage/tests/storage_lifecycle.rs`

- [ ] **Step 1: Write the failing storage validation/schema test**

Add a test near `schema_indexes_metadata_hot_fields`:

```rust
#[tokio::test]
async fn schema_indexes_expiry_and_rejects_invalid_expires_at_metadata() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("quarry.db");
    let store = QuarryStore::open(StoreConfig {
        db_path: db_path.clone(),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("expiryvalidation").await.unwrap();

    let error = store
        .put_document(
            &library.slug,
            "notes/bad.md",
            b"bad".to_vec(),
            serde_json::json!({"content_type":"text/markdown","expires_at":"not-a-timestamp"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("invalid metadata"));
    assert!(error.to_string().contains("expires_at"));
    drop(store);

    let db = turso::Builder::new_local(db_path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let document_indexes = index_names(&conn, "documents").await;
    assert!(document_indexes.contains("idx_documents_expires_at"));
}
```

Run:

```sh
cargo test -p quarry-storage schema_indexes_expiry_and_rejects_invalid_expires_at_metadata
```

Expected: FAIL because `expires_at` validation and the index do not exist.

- [ ] **Step 2: Add core error and metadata constants**

In `QuarryError`, add:

```rust
#[error("invalid metadata: {0}")]
InvalidMetadata(String),
```

Add:

```rust
pub const EXPIRES_AT_METADATA_KEY: &str = "expires_at";
```

- [ ] **Step 3: Add storage-side expiry metadata normalization**

In `crates/quarry-storage/src/lib.rs`, import `EXPIRES_AT_METADATA_KEY`. Add helpers near `merge_json`:

```rust
fn normalize_document_metadata(mut metadata: JsonValue) -> Result<(JsonValue, Option<String>)> {
    let expires_at = match &mut metadata {
        JsonValue::Object(object) => match object.get(EXPIRES_AT_METADATA_KEY) {
            None => None,
            Some(JsonValue::Null) => {
                object.remove(EXPIRES_AT_METADATA_KEY);
                None
            }
            Some(JsonValue::String(value)) => {
                let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|_| {
                    QuarryError::InvalidMetadata(format!(
                        "{EXPIRES_AT_METADATA_KEY} must be an RFC3339 timestamp"
                    ))
                })?;
                let normalized = parsed
                    .with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                object.insert(
                    EXPIRES_AT_METADATA_KEY.to_string(),
                    JsonValue::String(normalized.clone()),
                );
                Some(normalized)
            }
            Some(_) => {
                return Err(QuarryError::InvalidMetadata(format!(
                    "{EXPIRES_AT_METADATA_KEY} must be a string timestamp or null"
                )));
            }
        },
        _ => None,
    };
    Ok((metadata, expires_at))
}

fn expires_at_from_metadata(metadata: &JsonValue) -> Result<Option<String>> {
    let (_, expires_at) = normalize_document_metadata(metadata.clone())?;
    Ok(expires_at)
}
```

- [ ] **Step 4: Add idempotent schema migration for existing databases**

Keep `CREATE TABLE IF NOT EXISTS documents` updated with:

```sql
expires_at TEXT,
```

Add:

```sql
CREATE INDEX IF NOT EXISTS idx_documents_expires_at ON documents(library_id, expires_at);
```

After `conn.execute_batch(SCHEMA)` in `migrate`, run an idempotent compatibility step that adds `documents.expires_at` to databases created before this feature. Use `PRAGMA table_info('documents')` to check for the column before executing:

```sql
ALTER TABLE documents ADD COLUMN expires_at TEXT
```

- [ ] **Step 5: Normalize metadata on version insert**

At the start of `insert_version_conn`, call `normalize_document_metadata(metadata)?` and use the returned metadata for `DocumentVersion` and `metadata_json`. Do not store invalid or unnormalized expiry metadata in `document_versions`.

- [ ] **Step 6: Verify task 1**

Run:

```sh
cargo test -p quarry-storage schema_indexes_expiry_and_rejects_invalid_expires_at_metadata
```

Expected: PASS.

## Task 2: Expiry Visibility And Publish Semantics

**Files:**
- Modify: `crates/quarry-storage/src/lib.rs`
- Test: `crates/quarry-storage/tests/storage_lifecycle.rs`

- [ ] **Step 1: Write the failing visibility test**

Add this storage test:

```rust
#[tokio::test]
async fn expired_documents_are_hidden_from_reads_lists_and_preconditions() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("expiryvisibility").await.unwrap();

    let expired = store
        .put_document(
            &library.slug,
            "notes/cache.md",
            b"expired".to_vec(),
            serde_json::json!({
                "content_type":"text/markdown",
                "expires_at":"2000-01-01T00:00:00.000Z"
            }),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    assert!(store.get_document(&library.slug, "notes/cache.md").await.is_err());
    assert!(store.head_document(&library.slug, "notes/cache.md").await.is_err());
    assert!(store
        .list_documents(&library.slug, Some("notes/"), Some(100))
        .await
        .unwrap()
        .is_empty());

    let replaced = store
        .put_document(
            &library.slug,
            "notes/cache.md",
            b"fresh".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfNoneMatch,
        )
        .await
        .unwrap();
    assert_ne!(expired.version.id, replaced.version.id);
    assert_eq!(
        store
            .get_document(&library.slug, "notes/cache.md")
            .await
            .unwrap()
            .content,
        b"fresh"
    );

    let history = store
        .version_history(&library.slug, "notes/cache.md")
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
}
```

Run:

```sh
cargo test -p quarry-storage expired_documents_are_hidden_from_reads_lists_and_preconditions
```

Expected: FAIL because expired documents are still visible.

- [ ] **Step 2: Mirror expiry when publishing document heads**

Change `publish_put_conn` to accept `expires_at: Option<&str>` and set both `deleted_at = NULL` and `expires_at = ?`:

```sql
UPDATE documents
SET head_version_id = ?1,
    deleted_at = NULL,
    expires_at = ?2,
    updated_at = ?3
WHERE id = ?4
```

For auto-commit puts and transaction commits, compute `expires_at` from the published version metadata with `expires_at_from_metadata(&version.metadata)?` or by loading and parsing the staged version metadata by ID.

- [ ] **Step 3: Use expiry-aware visibility predicates**

Every normal committed-document visibility query must include:

```sql
AND (d.expires_at IS NULL OR d.expires_at > ?)
```

Apply this to:

- `document_identity_conn`
- `document_entry_conn`
- `document_conn`
- both `list_documents` queries
- any helper that enforces visible target availability, such as move target checks

Use one `let visible_at = now_timestamp();` per public read/list/precondition operation and pass it as a bound parameter. Because all stored expiry values are normalized UTC RFC3339 with milliseconds, text comparison is stable.

- [ ] **Step 4: Preserve any-row helpers for history and reuse**

Do not add the expiry predicate to helpers that intentionally need tombstoned or expired rows:

- `document_any_identity_conn`
- `document_id_conn`
- `ensure_document_conn`
- `version_history`

This lets a path be recreated over an expired row while preserving historical versions.

- [ ] **Step 5: Clear expiry on non-expiring replacement**

When a document is replaced without `expires_at`, `publish_put_conn` must set `documents.expires_at = NULL`. This is required so a fresh replacement over an expired document does not inherit the old expiry.

- [ ] **Step 6: Verify task 2**

Run:

```sh
cargo test -p quarry-storage expired_documents_are_hidden_from_reads_lists_and_preconditions
```

Expected: PASS.

## Task 3: System Expiry Sweeper

**Files:**
- Modify: `crates/quarry-storage/src/lib.rs`
- Modify: `crates/quarry-server/src/lib.rs`
- Test: `crates/quarry-storage/tests/storage_lifecycle.rs`

- [ ] **Step 1: Write the failing sweeper test**

Add:

```rust
#[tokio::test]
async fn expire_due_documents_tombstones_due_heads_with_system_transactions() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("expirysweeper").await.unwrap();
    let mut events = store.subscribe_events();

    store
        .put_document(
            &library.slug,
            "notes/due.md",
            b"due".to_vec(),
            serde_json::json!({
                "content_type":"text/markdown",
                "expires_at":"2000-01-01T00:00:00.000Z"
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
            "notes/future.md",
            b"future".to_vec(),
            serde_json::json!({
                "content_type":"text/markdown",
                "expires_at":"2999-01-01T00:00:00.000Z"
            }),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let expired = store
        .expire_due_documents("2026-05-28T00:00:00.000Z", 100)
        .await
        .unwrap();
    assert_eq!(expired, 1);
    assert!(store.get_document(&library.slug, "notes/due.md").await.is_err());
    assert!(store.get_document(&library.slug, "notes/future.md").await.is_ok());

    let transactions = store.list_transactions(&library.slug).await.unwrap();
    assert!(transactions
        .iter()
        .any(|tx| tx.source == quarry_core::DocumentSource::System
            && tx.message.as_deref() == Some("expire documents")));

    let mut saw_delete = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == StoreEventKind::DocumentDelete
            && event.path.as_deref() == Some("notes/due.md")
        {
            saw_delete = true;
        }
    }
    assert!(saw_delete);
}
```

Run:

```sh
cargo test -p quarry-storage expire_due_documents_tombstones_due_heads_with_system_transactions
```

Expected: FAIL because `expire_due_documents` does not exist.

- [ ] **Step 2: Add `expire_due_documents`**

Add a public method on `QuarryStore`:

```rust
pub async fn expire_due_documents(&self, now: &str, limit: u64) -> Result<usize>
```

Behavior:

- Validate `now` with the same RFC3339 parser used for metadata.
- Clamp `limit` to `10_000`.
- Select due rows where `deleted_at IS NULL`, `head_version_id IS NOT NULL`, `expires_at IS NOT NULL`, and `expires_at <= now`.
- Group selected rows by `library_id` because transactions are library-scoped.
- For each library, create one `DocumentSource::System` transaction with message `expire documents` and provenance `{"mode":"ttl_expiry","expired_at": now}`.
- Insert a `ChangeType::Delete` row for each expired path using the current head version as `old_version_id`.
- Update each document with `deleted_at = now`, `updated_at = now`, and leave `expires_at` unchanged for auditability.
- Commit the system transaction.
- Emit one `DocumentDelete` event per expired document after commit.

- [ ] **Step 3: Run the sweeper on store open**

After schema migration in `QuarryStore::open`, call:

```rust
store.expire_due_documents(&now_timestamp(), 10_000).await?;
```

This makes CLI and one-shot operations converge stale expired rows without needing a long-lived daemon.

- [ ] **Step 4: Run a periodic server sweeper**

In `quarry_server::serve`, spawn a Tokio task before serving:

- Run immediately once.
- Then run every 60 seconds with batch size `10_000`.
- Log success at debug level and errors at warn level.
- Let the task end when the process exits; no shutdown plumbing is required for this local daemon.

- [ ] **Step 5: Verify task 3**

Run:

```sh
cargo test -p quarry-storage expire_due_documents_tombstones_due_heads_with_system_transactions
cargo test -p quarry-server
```

Expected: PASS.

## Task 4: REST And CLI Inputs

**Files:**
- Modify: `crates/quarry-server/src/lib.rs`
- Modify: `crates/quarry-cli/src/lib.rs`
- Test: `crates/quarry-server/tests/rest_api.rs`
- Test: `crates/quarry/tests/cli_smoke.rs`

- [ ] **Step 1: Write the failing REST test**

Add a REST test:

```rust
#[tokio::test]
async fn rest_api_accepts_document_expiry_headers_and_hides_expired_documents() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    assert_eq!(
        app.clone()
            .oneshot(json_request(
                Method::POST,
                "/v1/libraries",
                serde_json::json!({"slug":"restttl"}),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/restttl/documents/notes/expired.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("x-quarry-expires-at", "2000-01-01T00:00:00.000Z")
                .body(Body::from("expired"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/restttl/documents/notes/expired.md")
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
                .method(Method::PUT)
                .uri("/v1/libraries/restttl/documents/notes/bad.md")
                .header("x-quarry-expires-at", "bad")
                .body(Body::from("bad"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
```

Run:

```sh
cargo test -p quarry-server rest_api_accepts_document_expiry_headers_and_hides_expired_documents
```

Expected: FAIL because the headers are not parsed.

- [ ] **Step 2: Add REST expiry headers**

Extend `metadata_from_headers` to recognize:

- `x-quarry-expires-at: <rfc3339>`
- `x-quarry-ttl-seconds: <u64>`

Rules:

- Reject requests that provide both expiry headers.
- Reject requests that provide an expiry header and `x-quarry-metadata` already contains `expires_at`.
- `x-quarry-ttl-seconds` resolves to `chrono::Utc::now() + seconds`.
- Insert the resolved timestamp into metadata as `expires_at`; storage normalization remains the final validator.

- [ ] **Step 3: Map invalid metadata to HTTP 400**

Update `impl From<QuarryError> for ApiError` so `QuarryError::InvalidMetadata(_)` maps to `StatusCode::BAD_REQUEST`.

- [ ] **Step 4: Write the failing CLI test**

Add a CLI smoke test that writes a document with immediate TTL and verifies `get` fails:

```rust
#[test]
fn cli_put_ttl_seconds_expires_document() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input.md");
    std::fs::write(&input, "cache").unwrap();

    assert!(command()
        .arg("--root")
        .arg(root.path().join(".quarry"))
        .arg("put")
        .arg("notes")
        .arg("cache.md")
        .arg(&input)
        .arg("--ttl-seconds")
        .arg("0")
        .status()
        .unwrap()
        .success());

    let output = command()
        .arg("--root")
        .arg(root.path().join(".quarry"))
        .arg("get")
        .arg("notes")
        .arg("cache.md")
        .output()
        .unwrap();
    assert!(!output.status.success());
}
```

Use the existing command helper in `crates/quarry/tests/cli_smoke.rs`; keep the helper name used by that file.

- [ ] **Step 5: Add CLI flags**

Extend `PutCommand`:

```rust
#[arg(long)]
expires_at: Option<String>,

#[arg(long)]
ttl_seconds: Option<u64>,
```

Rules:

- Reject both flags together with `bail!("use either --expires-at or --ttl-seconds, not both")`.
- `--expires-at` inserts that value into metadata as `expires_at`.
- `--ttl-seconds` inserts `chrono::Utc::now() + seconds` normalized with millisecond precision.
- Keep `content_type` metadata unchanged.

- [ ] **Step 6: Verify task 4**

Run:

```sh
cargo test -p quarry-server rest_api_accepts_document_expiry_headers_and_hides_expired_documents
cargo test -p quarry cli_put_ttl_seconds_expires_document
```

Expected: PASS.

## Task 5: Git, FUSE, And Docs

**Files:**
- Modify: `crates/quarry-git/src/lib.rs` only if tests expose an unfiltered path
- Modify: `crates/quarry-fuse/src/lib.rs` only if tests expose an unfiltered path
- Modify: `docs/operations/rest-api.md`
- Modify: `docs/operations/git-sync.md`
- Modify: `spec.md`
- Test: `crates/quarry-git/tests/git_roundtrip.rs`
- Test: `crates/quarry-fuse/tests/projection.rs`

- [ ] **Step 1: Write Git expiry tests**

Add two tests:

- Import/export preserves future `expires_at` metadata in Markdown frontmatter or sidecar YAML.
- A document with past `expires_at` does not appear in `exported_paths` and is not written to the worktree.

Use existing helpers and patterns from `import_export_roundtrip_preserves_bytes_metadata_and_marker_safety`.

- [ ] **Step 2: Write FUSE projection expiry test**

Add a projection test:

- Put `docs/expired.md` with `expires_at` in the past.
- Open `FuseProjection`.
- Assert `list_dir("docs")` does not include `expired.md`.
- Assert `attr("docs/expired.md")` returns `QuarryError::NotFound`.

- [ ] **Step 3: Keep Git and FUSE policy in storage**

Do not add separate Git or FUSE expiry logic unless a test reveals an unfiltered storage helper. These layers should continue to call `list_documents`, `get_document`, and `head_document`.

- [ ] **Step 4: Update operations docs**

Document these REST examples in `docs/operations/rest-api.md`:

```sh
curl -X PUT \
  -H 'Content-Type: text/markdown' \
  -H 'X-Quarry-Expires-At: 2026-05-28T15:30:00.000Z' \
  --data-binary @note.md \
  http://127.0.0.1:7831/v1/libraries/notes/documents/cache/note.md

curl -X PUT \
  -H 'X-Quarry-TTL-Seconds: 3600' \
  --data-binary @artifact.bin \
  http://127.0.0.1:7831/v1/libraries/notes/documents/cache/artifact.bin
```

Document that `{"expires_at": null}` in metadata clears expiry.

- [ ] **Step 5: Update product spec and Git docs**

In `spec.md`, add TTL as a document metadata/indexed hot field and state that expiry is soft delete with retained history. In `docs/operations/git-sync.md`, state that `expires_at` round-trips as metadata and expired documents are omitted from export/sync inputs.

- [ ] **Step 6: Verify task 5**

Run:

```sh
cargo test -p quarry-git
cargo test -p quarry-fuse
```

Expected: PASS.

## Final Verification

- [ ] Run storage tests:

```sh
cargo test -p quarry-storage
```

- [ ] Run server tests:

```sh
cargo test -p quarry-server
```

- [ ] Run Git and FUSE tests:

```sh
cargo test -p quarry-git
cargo test -p quarry-fuse
```

- [ ] Run CLI smoke tests:

```sh
cargo test -p quarry
```

- [ ] Run workspace test suite:

```sh
cargo test --workspace
```

## Acceptance Criteria

- Documents with future `expires_at` behave like normal documents until the timestamp is due.
- Documents with past or due `expires_at` are hidden from normal reads and lists immediately.
- Due documents are eventually tombstoned by `DocumentSource::System` transactions.
- Replacing an expired document with `If-None-Match: *` succeeds and does not inherit the previous expiry.
- Metadata and Git round-trip preserve future expiry timestamps.
- Invalid expiry metadata returns a user-facing validation error instead of creating a broken document.
- No committed version or CAS blob is purged solely because a document expired.
