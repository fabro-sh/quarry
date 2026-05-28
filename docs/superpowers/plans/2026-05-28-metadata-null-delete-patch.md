# Metadata Null Delete Patch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Change Quarry metadata patch semantics so object patch values of JSON `null` delete metadata keys instead of storing JSON null.

**Architecture:** Keep metadata stored as whole JSON snapshots on `document_versions`; only change the recursive merge helper used by auto-commit and explicit transaction metadata patches. REST and transaction endpoints already delegate to the storage merge path, so the behavior change belongs in `quarry-storage` with REST coverage proving API behavior.

**Tech Stack:** Rust 2021 workspace, Turso-backed storage, `serde_json::Value`, Axum REST tests, Tokio tests.

---

## File Structure

- Modify: `crates/quarry-storage/src/lib.rs`
  - Replace `merge_json` so object entries whose patch value is `JsonValue::Null` call `remove` on the target object.
  - Preserve current behavior for non-null recursive object patches and non-object root patches.
- Modify: `crates/quarry-storage/tests/storage_lifecycle.rs`
  - Add a focused storage test for auto-commit metadata patch deletion, nested deletion, recursive merge preservation, and version creation.
- Modify: `crates/quarry-server/tests/rest_api.rs`
  - Extend the existing REST metadata patch test so a `null` patch removes a key as observed through document listing metadata.
- Modify: `docs/operations/rest-api.md`
  - Document that metadata patch object keys with `null` values are removed.

---

### Task 1: Add Storage-Level Failing Test

**Files:**
- Modify: `crates/quarry-storage/tests/storage_lifecycle.rs`

- [ ] **Step 1: Insert the failing test after `explicit_transactions_publish_atomically_and_rollback_staged_cas`**

Add this complete test function after the existing explicit transaction test:

```rust
#[tokio::test]
async fn metadata_patch_null_deletes_object_keys() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();

    let library = store.create_library("metadata").await.unwrap();
    let first = store
        .put_document(
            &library.slug,
            "docs/a.md",
            b"body".to_vec(),
            serde_json::json!({
                "content_type": "text/markdown",
                "topic": "old",
                "obsolete": "remove",
                "nested": {
                    "keep": true,
                    "drop": "remove"
                }
            }),
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    let outcome = store
        .patch_metadata(
            &library.slug,
            "docs/a.md",
            serde_json::json!({
                "topic": "new",
                "obsolete": null,
                "nested": {
                    "drop": null,
                    "added": "yes"
                }
            }),
            DocumentSource::Rest,
            WritePrecondition::None,
        )
        .await
        .unwrap();

    assert_ne!(outcome.version.id, first.version.id);

    let document = store.get_document(&library.slug, "docs/a.md").await.unwrap();
    assert_eq!(document.content, b"body");
    assert_eq!(document.metadata["content_type"], "text/markdown");
    assert_eq!(document.metadata["topic"], "new");
    assert!(document.metadata.get("obsolete").is_none());
    assert_eq!(document.metadata["nested"]["keep"], true);
    assert_eq!(document.metadata["nested"]["added"], "yes");
    assert!(document.metadata["nested"].get("drop").is_none());

    let versions = store
        .version_history(&library.slug, "docs/a.md")
        .await
        .unwrap();
    assert_eq!(versions.len(), 2);
    assert!(versions[0].metadata.get("obsolete").is_some());
    assert!(versions[1].metadata.get("obsolete").is_none());
}
```

- [ ] **Step 2: Run the new storage test and verify it fails**

Run:

```bash
cargo test -p quarry-storage metadata_patch_null_deletes_object_keys
```

Expected: FAIL because `document.metadata.get("obsolete")` is still `Some(Value::Null)` under current merge behavior.

---

### Task 2: Add REST-Level Failing Test

**Files:**
- Modify: `crates/quarry-server/tests/rest_api.rs`

- [ ] **Step 1: Seed removable metadata in the existing REST test**

In `rest_api_supports_move_metadata_and_conflict_lookup_endpoints`, change the initial `put_document` metadata from:

```rust
serde_json::json!({"content_type":"text/markdown"}),
```

to:

```rust
serde_json::json!({"content_type":"text/markdown","stale":"remove"}),
```

- [ ] **Step 2: Insert API assertions after the existing successful metadata patch**

Immediately after the block that patches `{"reviewed":true}` and asserts `StatusCode::OK`, insert:

```rust
    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/actions/documents/b.md/metadata",
            serde_json::json!({"stale":null}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/actions/documents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["path"], "b.md");
    assert_eq!(body[0]["metadata"]["reviewed"], true);
    assert!(body[0]["metadata"].get("stale").is_none());
```

- [ ] **Step 3: Run the REST test and verify it fails**

Run:

```bash
cargo test -p quarry-server rest_api_supports_move_metadata_and_conflict_lookup_endpoints
```

Expected: FAIL because the REST-visible list metadata still contains `"stale": null`.

---

### Task 3: Implement Null-as-Delete Merge Semantics

**Files:**
- Modify: `crates/quarry-storage/src/lib.rs`

- [ ] **Step 1: Replace `merge_json` with key-removal behavior for object null values**

Replace the existing `merge_json` function with:

```rust
fn merge_json(target: &mut JsonValue, patch: JsonValue) {
    match (target, patch) {
        (JsonValue::Object(target), JsonValue::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    target.remove(&key);
                } else {
                    merge_json(target.entry(key).or_insert(JsonValue::Null), value);
                }
            }
        }
        (target, value) => *target = value,
    }
}
```

This preserves these existing behaviors:

- A non-object patch at the root replaces the entire metadata value.
- A non-null object value still merges recursively.
- A non-object nested value still replaces the target key.

- [ ] **Step 2: Run both focused tests and verify they pass**

Run:

```bash
cargo test -p quarry-storage metadata_patch_null_deletes_object_keys
cargo test -p quarry-server rest_api_supports_move_metadata_and_conflict_lookup_endpoints
```

Expected: both PASS.

---

### Task 4: Document REST Metadata Patch Semantics

**Files:**
- Modify: `docs/operations/rest-api.md`

- [ ] **Step 1: Add a metadata patch semantics paragraph after the ETag paragraph**

After the paragraph currently ending with “leaving the newer committed document visible.”, add:

```markdown
Metadata patch endpoints recursively merge JSON object patches into the current metadata snapshot. Object keys whose patch value is `null` are removed. Non-null object values merge recursively, and non-object patch values replace the target metadata value.
```

- [ ] **Step 2: Review the rendered Markdown wording**

Run:

```bash
sed -n '38,50p' docs/operations/rest-api.md
```

Expected: the endpoint list remains intact, followed by the ETag paragraph and the new metadata patch semantics paragraph.

---

### Task 5: Final Verification

**Files:**
- Verify: workspace tests touched by this change

- [ ] **Step 1: Run targeted package tests**

Run:

```bash
cargo test -p quarry-storage
cargo test -p quarry-server
```

Expected: both packages pass.

- [ ] **Step 2: Run workspace tests if time allows**

Run:

```bash
cargo test --workspace
```

Expected: the full workspace passes.

- [ ] **Step 3: Inspect the final diff**

Run:

```bash
git diff -- crates/quarry-storage/src/lib.rs crates/quarry-storage/tests/storage_lifecycle.rs crates/quarry-server/tests/rest_api.rs docs/operations/rest-api.md
```

Expected: diff is limited to null-as-delete metadata merge behavior, focused tests, and REST API documentation.

- [ ] **Step 4: Commit the completed change when requested**

Run:

```bash
git add crates/quarry-storage/src/lib.rs crates/quarry-storage/tests/storage_lifecycle.rs crates/quarry-server/tests/rest_api.rs docs/operations/rest-api.md
git commit -m "Change metadata null patches to delete keys"
```

Expected: one focused commit containing the behavior change, tests, and docs.

---

## Self-Review

**Spec coverage:** The plan changes object patch values of `null` from “store JSON null” to “delete key” in the single shared storage merge helper. It covers both direct storage usage and REST auto-commit metadata patches. Explicit transaction metadata patches use the same `merge_json` path through `stage_metadata`, so the storage test covers their core semantics indirectly.

**Placeholder scan:** The plan contains exact file paths, exact Rust snippets, exact shell commands, and expected outcomes. It does not leave implementation details open.

**Type consistency:** All snippets use existing imports already present in the target test files: `DocumentSource`, `WritePrecondition`, `QuarryStore`, `StoreConfig`, `Method`, `Request`, `Body`, `StatusCode`, and `Value`.
