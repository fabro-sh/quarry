# First-Run Onboarding Modal & Change Attribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a first-run modal that collects the user's name and record that name as the transaction actor on every change the UI makes — REST mutations and live-session checkpoints.

**Architecture:** The name lives in the existing `quarry:author` localStorage key. REST mutations carry it via the existing `X-Quarry-Transaction-Actor` header (percent-encoded UTF-8; the server gains decoding plus actor plumbing on delete/move/restore). Live typing is attributed by the server's session checkpointer reading participant names from Yjs awareness cursor data instead of hardcoding `"browser"`.

**Tech Stack:** Rust (axum, yrs, turso) for `quarry-server`/`quarry-storage`; React + TypeScript + Tailwind (bun, vitest) for `ui/`.

**Spec:** `docs/superpowers/specs/2026-06-12-first-run-onboarding-design.md`

**Branch:** work directly on local `main` (per user instruction). Run UI commands with bun from `ui/`; Rust from the repo root.

---

### Task 1: Server percent-decodes the transaction actor header

The UI will send `X-Quarry-Transaction-Actor: Jos%C3%A9` because browser `fetch` rejects non-Latin-1 header values. The server must decode it. Plain ASCII without `%` decodes to itself, so existing senders are unaffected.

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/quarry-server/Cargo.toml`
- Modify: `crates/quarry-server/src/lib.rs` (`transaction_metadata_from_headers`, ~line 3368)
- Test: `crates/quarry-server/tests/rest_api.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/quarry-server/tests/rest_api.rs`, next to `put_document_rejects_invalid_transaction_provenance_header` (~line 2101). The `/versions` endpoint exposes the actor as `"actor"` (see the existing `assert_eq!(body[0]["actor"], "Avery")` at ~line 2095).

```rust
#[tokio::test]
async fn put_document_decodes_percent_encoded_transaction_actor_header() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("actorheader").await.unwrap();
    let app = router(store);

    // Percent-encoded UTF-8 name decodes before storage.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/actorheader/documents/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("x-quarry-transaction-actor", "Jos%C3%A9")
                .body(Body::from("# A\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Plain ASCII passes through unchanged.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/actorheader/documents/b.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::from("# B\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // No header records no actor.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/actorheader/documents/c.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("# C\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        latest_version_actor(&app, "actorheader", "a.md").await,
        serde_json::json!("José")
    );
    assert_eq!(
        latest_version_actor(&app, "actorheader", "b.md").await,
        serde_json::json!("Avery")
    );
    assert_eq!(
        latest_version_actor(&app, "actorheader", "c.md").await,
        serde_json::json!(null)
    );
}

/// The `"actor"` of the newest version of `path`, via GET `/versions`.
async fn latest_version_actor(app: &axum::Router, library: &str, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/{library}/documents/{path}/versions"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    body[0]["actor"].clone()
}
```

Note: `.md` PUTs route through the block-document gateway, which threads the same `TransactionMetadata` — this test covers the markdown path the UI actually uses.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p quarry-server --test rest_api put_document_decodes_percent_encoded_transaction_actor_header`
Expected: FAIL — `a.md` records the literal `Jos%C3%A9` instead of `José`.

- [ ] **Step 3: Add the percent-encoding dependency**

In the root `Cargo.toml` under `[workspace.dependencies]` (alphabetical order):

```toml
percent-encoding = "2.3"
```

In `crates/quarry-server/Cargo.toml` under `[dependencies]` (alphabetical order):

```toml
percent-encoding.workspace = true
```

(`percent-encoding 2.3.2` is already in `Cargo.lock` as a transitive dep — no new code enters the tree.)

- [ ] **Step 4: Decode the header value**

In `crates/quarry-server/src/lib.rs`, add to the imports:

```rust
use percent_encoding::percent_decode_str;
```

In `transaction_metadata_from_headers` (~line 3368), change the `actor` field:

```rust
        actor: optional_header(headers, "x-quarry-transaction-actor")?
            .map(|value| percent_decode_str(&value).decode_utf8_lossy().into_owned()),
```

(`decode_utf8_lossy`: attribution metadata must never fail a write over a bad escape sequence.)

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p quarry-server --test rest_api put_document_decodes_percent_encoded_transaction_actor_header`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/quarry-server/Cargo.toml crates/quarry-server/src/lib.rs crates/quarry-server/tests/rest_api.rs
git commit -m "feat(server): percent-decode the transaction actor header"
```

---

### Task 2: Actor on delete, move, and restore

Only `put_document` honors the actor header today. `delete_document`, move, and restore hardcode `None` in storage. Thread an `actor: Option<String>` through.

**Files:**
- Modify: `crates/quarry-storage/src/lib.rs` (`restore_document_version_with_origin` ~1636, `delete_document` wrapper ~1676, `delete_document_with_origin` ~1686, `move_document` wrapper ~1758, `move_document_with_origin` ~1769)
- Modify: `crates/quarry-server/src/lib.rs` (`post_document_action` ~2086, `delete_document` ~2603)
- Modify: `crates/quarry-server/src/markdown_write.rs` (`restore_block_document_version` ~148)
- Modify: `crates/quarry-storage/tests/storage_lifecycle.rs` (two `*_with_origin` call sites ~1949, ~1965 gain a `None` arg)
- Test: `crates/quarry-server/tests/rest_api.rs`

- [ ] **Step 1: Write the failing test**

DELETE and move return a `TransactionRecord` whose JSON carries `"actor"`; restore returns a `WriteOutcome` (assert via `/versions`).

```rust
#[tokio::test]
async fn delete_move_and_restore_record_transaction_actor_header() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("actorops").await.unwrap();
    let app = router(store);

    put_actorops_doc(&app, "keep.md").await;
    put_actorops_doc(&app, "doomed.md").await;

    // Move records the actor on its transaction.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/libraries/actorops/documents/keep.md/move")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::from(r#"{"to_path":"kept.md"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["actor"], "Avery");

    // Delete records the actor on its transaction.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/v1/libraries/actorops/documents/doomed.md")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["actor"], "Avery");

    // Restore records the actor on the restored version.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/actorops/documents/kept.md/versions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let versions: Value = response_json(response).await;
    let version_id = versions[0]["id"].as_str().unwrap().to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/v1/libraries/actorops/documents/kept.md/versions/{version_id}/restore"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-quarry-transaction-actor", "Avery")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/actorops/documents/kept.md/versions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let versions: Value = response_json(response).await;
    assert_eq!(versions[0]["actor"], "Avery");
}

/// Seeds one markdown document in the `actorops` library.
async fn put_actorops_doc(app: &axum::Router, path: &str) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/libraries/actorops/documents/{path}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("# Doc\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p quarry-server --test rest_api delete_move_and_restore_record_transaction_actor_header`
Expected: FAIL — `body["actor"]` is null for move/delete.

- [ ] **Step 3: Thread actor through the storage methods**

In `crates/quarry-storage/src/lib.rs`:

`restore_document_version_with_origin` (~1636): add a final parameter `actor: Option<String>`, and in its `TransactionMetadata` literal replace `actor: None,` with `actor,`.

`delete_document_with_origin` (~1686): add a final parameter `actor: Option<String>`, and change its `insert_transaction_conn` call from

```rust
            let tx = insert_transaction_conn(
                &conn,
                &library.id,
                source,
                None,
                None,
                serde_json::json!({ "mode": "auto_commit" }),
            )
```

to

```rust
            let tx = insert_transaction_conn(
                &conn,
                &library.id,
                source,
                actor,
                None,
                serde_json::json!({ "mode": "auto_commit" }),
            )
```

`move_document_with_origin` (~1769): same change — add the `actor: Option<String>` parameter and pass `actor` as the fourth argument of its `insert_transaction_conn` call (identical shape to delete's).

The plain wrappers pass `None`:

```rust
    pub async fn delete_document(
        &self,
        library: &str,
        path: &str,
        source: DocumentSource,
    ) -> Result<TransactionRecord> {
        self.delete_document_with_origin(library, path, source, None, None)
            .await
    }
```

```rust
    pub async fn move_document(
        &self,
        library: &str,
        from_path: &str,
        to_path: &str,
        source: DocumentSource,
    ) -> Result<TransactionRecord> {
        self.move_document_with_origin(library, from_path, to_path, source, None, None)
            .await
    }
```

If `insert_transaction_conn`'s `actor` parameter is `Option<&str>` rather than `Option<String>` at these call sites, pass `actor.as_deref()` instead — match the existing call's type.

- [ ] **Step 4: Pass the decoded header from the REST handlers**

In `crates/quarry-server/src/lib.rs`:

`delete_document` (~2603):

```rust
async fn delete_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
) -> Result<Json<TransactionRecord>, ApiError> {
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    let actor = transaction_metadata_from_headers(&headers)?.actor;
    Ok(Json(
        state
            .store
            .delete_document_with_origin(&library, &path, DocumentSource::Rest, origin_id, actor)
            .await?,
    ))
}
```

`post_document_action` (~2086): after the existing `let origin_id = ...` line add

```rust
    let actor = transaction_metadata_from_headers(&headers)?.actor;
```

then pass `actor.clone()` as the new final argument to the three store/gateway calls in this handler:

- `markdown_write::restore_block_document_version(&state, &library, path, &target, origin_id.clone(), actor.clone())`
- `state.store.restore_document_version_with_origin(&library, path, version, origin_id.clone(), actor.clone())`
- `state.store.move_document_with_origin(&library, from_path, to_path, DocumentSource::Rest, origin_id.clone(), actor.clone())`

In `crates/quarry-server/src/markdown_write.rs`, `restore_block_document_version` (~148): add a final parameter `actor: Option<String>` and in its `TransactionMetadata` literal replace `actor: None,` with `actor,`.

- [ ] **Step 5: Fix the storage lifecycle test call sites**

In `crates/quarry-storage/tests/storage_lifecycle.rs` (~1949 and ~1965), the `move_document_with_origin` and `delete_document_with_origin` calls each gain a trailing `None` argument.

- [ ] **Step 6: Verify the workspace compiles and tests pass**

Run: `cargo check --workspace` then `cargo test -p quarry-server --test rest_api delete_move_and_restore_record_transaction_actor_header && cargo test -p quarry-storage`
Expected: check clean; tests PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/quarry-storage/src/lib.rs crates/quarry-storage/tests/storage_lifecycle.rs crates/quarry-server/src/lib.rs crates/quarry-server/src/markdown_write.rs crates/quarry-server/tests/rest_api.rs
git commit -m "feat(server): record transaction actor on delete, move, and restore"
```

---

### Task 3: Session checkpoints attribute the awareness author

Typing never touches REST: the server checkpoints live Yjs sessions with `transaction_actor: Some("browser")` hardcoded (`session.rs` ~581). The Plate editor already publishes the author into awareness cursor data (`{ data: { color, name } }` per client state — slate-yjs `cursorDataField` defaults to `"data"`). Derive the checkpoint actor from awareness, cache the last non-empty value (the final checkpoint can run after the socket closes and awareness empties), and fall back to `"browser"`.

**Files:**
- Modify: `crates/quarry-server/src/session.rs`
- Test: `crates/quarry-server/tests/rest_api.rs`

- [ ] **Step 1: Write the failing test**

Add next to `multiple_typed_updates_coalesce_into_one_debounced_checkpoint` (~line 4731). First a helper near `send_local_edit_unechoed`:

```rust
/// Publishes a slate-yjs-style awareness state carrying the author name,
/// exactly as the Plate editor's cursor data does.
async fn send_awareness_name(socket: &mut WsSocket, doc: &Doc, name: &str) {
    use yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
    let update = AwarenessUpdate {
        clients: std::collections::HashMap::from([(
            doc.client_id(),
            AwarenessUpdateEntry {
                clock: 1,
                json: format!(r#"{{"data":{{"name":"{name}","color":"#8be9fd"}}}}"#).into(),
            },
        )]),
    };
    socket
        .send(TungsteniteMessage::Binary(
            YMessage::Awareness(update).encode_v1().into(),
        ))
        .await
        .unwrap();
}
```

Then the test:

```rust
#[tokio::test]
async fn session_checkpoint_attributes_awareness_author() {
    let (_root, addr, app, store, server) = spawn_session_server().await;
    put_block_markdown(&app, "live.md", "Attribution target.\n").await;
    let document_id = document_id_of(&store, "live.md").await;

    let (mut socket, doc) = connect_session(addr, &document_id).await;
    send_awareness_name(&mut socket, &doc, "Avery").await;
    send_local_edit(&mut socket, &doc, |txn, _root| {
        let block = nth_block_text_in(txn, 0);
        block.insert(txn, 19, " Signed.");
    })
    .await;

    let markdown = wait_for_markdown_containing(&app, "live.md", "Signed.").await;
    assert_eq!(markdown, "Attribution target. Signed.\n");
    let versions = raw_versions(&app, "live.md").await;
    assert_eq!(
        versions.as_array().unwrap()[0]["transaction_actor"],
        "Avery"
    );
    server.abort();
}
```

(Adjust the `AwarenessUpdate` import path or `ClientID` conversion if the compiler complains — the goal is a y-sync v1 Awareness message whose client state JSON is `{"data":{"name":"Avery","color":"#8be9fd"}}`.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p quarry-server --test rest_api session_checkpoint_attributes_awareness_author`
Expected: FAIL — `transaction_actor` is `"browser"`.

- [ ] **Step 3: Derive the checkpoint actor from awareness**

In `crates/quarry-server/src/session.rs`:

Add a free function near the bottom of the file (module level):

```rust
/// The attribution label for a checkpoint: every connected client's
/// slate-yjs cursor-data name (`{ data: { name } }`), deduped and joined.
/// Multiple participants produce "Avery, Blake".
fn awareness_actor(awareness: &Awareness) -> Option<String> {
    let mut names: Vec<String> = awareness
        .iter()
        .filter_map(|(_, state)| {
            let raw = state.data?;
            let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
            let name = json.get("data")?.get("name")?.as_str()?.trim().to_owned();
            (!name.is_empty()).then_some(name)
        })
        .collect();
    names.sort();
    names.dedup();
    (!names.is_empty()).then(|| names.join(", "))
}
```

Add a field to the `LiveSession` struct (~line 317, alongside `items`):

```rust
    /// Last non-empty awareness author label, kept so the final checkpoint
    /// (which can run after the socket closed and awareness emptied) still
    /// attributes correctly.
    live_actor: Mutex<Option<String>>,
```

and initialize it where the struct is constructed (same place `items` is initialized):

```rust
            live_actor: Mutex::new(None),
```

(Use the same `Mutex` type the struct already uses for `items` — `std::sync::Mutex`.)

In `checkpoint_locked`, replace the hardcoded actor. Before the `let commit = BlockMutationCommit {` literal (~line 575) add:

```rust
        if let Some(actor) = awareness_actor(awareness) {
            *self.live_actor.lock().unwrap() = Some(actor);
        }
        let transaction_actor = self
            .live_actor
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "browser".to_string());
```

and change the commit field from

```rust
                transaction_actor: Some("browser".to_string()),
```

to

```rust
                transaction_actor: Some(transaction_actor.clone()),
```

(`checkpoint_locked` retries in a loop; cloning per attempt is fine.)

- [ ] **Step 4: Run the new and existing session tests**

Run: `cargo test -p quarry-server --test rest_api session_checkpoint_attributes_awareness_author multiple_typed_updates_coalesce_into_one_debounced_checkpoint final_checkpoint_persists_typing_despite_unknown_inline_marks`
Expected: all PASS — the nameless-session tests still checkpoint as `"browser"`.

- [ ] **Step 5: Commit**

```bash
git add crates/quarry-server/src/session.rs crates/quarry-server/tests/rest_api.rs
git commit -m "feat(server): attribute session checkpoints to the awareness author"
```

---

### Task 4: UI client percent-encodes the actor header

**Files:**
- Modify: `ui/src/api/client.ts` (`mutationHeaders`, ~line 424)
- Test: `ui/src/api/client.test.ts`

- [ ] **Step 1: Write the failing test**

Add to `ui/src/api/client.test.ts`, next to the existing "stamps existing document saves with the mutation origin id" test (~line 59):

```ts
it('percent-encodes the transaction actor header on document saves', async () => {
  const fetch = vi.fn(async () =>
    new Response(JSON.stringify({ version: { id: 'v2' } }), {
      headers: { ETag: '"v2"', 'content-type': 'application/json' },
    })
  );
  vi.stubGlobal('fetch', fetch);

  await putDocument('notes', 'a.md', 'next', '"v1"', 'text/markdown', {
    transactionActor: 'José Avery',
  });

  expect(fetch).toHaveBeenCalledWith(
    '/v1/libraries/notes/documents/a.md',
    expect.objectContaining({
      headers: expect.objectContaining({
        'X-Quarry-Transaction-Actor': 'Jos%C3%A9%20Avery',
      }),
    })
  );
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd ui && bun run test src/api/client.test.ts`
Expected: FAIL — header is the raw `José Avery`.

- [ ] **Step 3: Encode in `mutationHeaders`**

In `ui/src/api/client.ts` (~line 430), change:

```ts
  if (options.transactionActor) next['X-Quarry-Transaction-Actor'] = options.transactionActor;
```

to:

```ts
  if (options.transactionActor) {
    // fetch rejects non-Latin-1 header values; the server percent-decodes.
    next['X-Quarry-Transaction-Actor'] = encodeURIComponent(options.transactionActor);
  }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd ui && bun run test src/api/client.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add ui/src/api/client.ts ui/src/api/client.test.ts
git commit -m "feat(ui): percent-encode the transaction actor header"
```

---

### Task 5: Identity helpers for first-run detection

`loadAuthor()` masks the unset case behind the `'user'` default. First-run detection needs the raw key.

**Files:**
- Modify: `ui/src/features/review/identity.ts`
- Test: `ui/src/features/review/identity.test.ts`

- [ ] **Step 1: Write the failing tests**

Add to `ui/src/features/review/identity.test.ts` (inside the existing `describe`, which already clears localStorage in `beforeEach`):

```ts
it('reports whether an author was explicitly stored', () => {
  expect(hasStoredAuthor()).toBe(false);
  saveAuthor('Avery');
  expect(hasStoredAuthor()).toBe(true);
  saveAuthor('user');
  expect(hasStoredAuthor()).toBe(false);
});
```

Update the import line to include the new exports:

```ts
import {
  currentAuthor,
  DEFAULT_AUTHOR,
  hasStoredAuthor,
  loadAuthor,
  normalizeAuthor,
  saveAuthor,
} from './identity';
```

Also assert the exported default in the existing "defaults" test:

```ts
  it('defaults to "user"', () => {
    expect(currentAuthor()).toBe('user');
    expect(DEFAULT_AUTHOR).toBe('user');
  });
```

- [ ] **Step 2: Run to verify failure**

Run: `cd ui && bun run test src/features/review/identity.test.ts`
Expected: FAIL — no export named `hasStoredAuthor` / `DEFAULT_AUTHOR`.

- [ ] **Step 3: Implement**

In `ui/src/features/review/identity.ts`, export the existing constant:

```ts
export const DEFAULT_AUTHOR = 'user';
```

and add:

```ts
/// True when the user explicitly chose a name (the raw key exists).
/// `loadAuthor()` cannot distinguish "never asked" from "chose the default".
export function hasStoredAuthor(storage?: Storage): boolean {
  const target = storage ?? (typeof window === 'undefined' ? undefined : window.localStorage);
  return Boolean(target?.getItem(AUTHOR_STORAGE_KEY)?.trim());
}
```

(Keep `AUTHOR_STORAGE_KEY` private; just change `const DEFAULT_AUTHOR` to `export const DEFAULT_AUTHOR`. TS uses `//` comments — write the doc comment with `//`.)

- [ ] **Step 4: Run to verify pass**

Run: `cd ui && bun run test src/features/review/identity.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add ui/src/features/review/identity.ts ui/src/features/review/identity.test.ts
git commit -m "feat(ui): expose explicit-author detection from identity"
```

---

### Task 6: Wire the author into every UI mutation

`browserMutationOptions()` already feeds every document mutation call site. Add the actor there, and route the conflict dialog's two inline call sites through the same options.

**Files:**
- Modify: `ui/src/app/App.tsx` (`browserMutationOptions` ~335, `ConflictMergeDialog` props ~1893 and call sites within, render site ~1300, identity import ~133)
- Test: `ui/src/app/workspace.test.tsx` (extend the conflict-resolution test ~1254)

- [ ] **Step 1: Extend the failing test**

In `ui/src/app/workspace.test.tsx`, in `it('resolves a conflict by saving the chosen ours content before marking resolved', ...)` (~line 1254):

After the `const fetch = vi.fn(...)` / `vi.stubGlobal('fetch', fetch);` lines and **before** `renderApp();`, add:

```ts
    localStorage.setItem('quarry:author', 'Avery');
```

Inside the fetch mock's PUT branch, extend the header assertion:

```ts
      if (url === '/v1/libraries/merge-lib/documents/conflict.md' && init?.method === 'PUT') {
        expect(init.headers).toMatchObject({
          'If-Match': '"head"',
          'X-Quarry-Transaction-Actor': 'Avery',
        });
        expect(init.body).toBe('# Ours');
        return json({ version: { id: 'merged' } }, { ETag: '"merged"' });
      }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd ui && bun run test src/app/workspace.test.tsx -t 'resolves a conflict'`
Expected: FAIL — PUT has no `X-Quarry-Transaction-Actor` header.

- [ ] **Step 3: Implement the wiring**

In `ui/src/app/App.tsx`:

Update the identity import (~line 133):

```ts
import { DEFAULT_AUTHOR, hasStoredAuthor, loadAuthor, saveAuthor } from '../features/review/identity';
```

(`hasStoredAuthor` is consumed in Task 7; importing now avoids churn. If the linter rejects the unused import, add it in Task 7 instead.)

`browserMutationOptions` (~line 335):

```ts
  function browserMutationOptions() {
    return {
      originId: collabSessionIdRef.current,
      transactionActor: author === DEFAULT_AUTHOR ? undefined : author,
    };
  }
```

`ConflictMergeDialog` props (~line 1893): replace the `originId: string` prop with `mutationOptions: DocumentMutationOptions`:

```ts
function ConflictMergeDialog({
  activeLibrary,
  conflict,
  mutationOptions,
  onClose,
}: {
  activeLibrary: string;
  conflict: ConflictRecord;
  mutationOptions: DocumentMutationOptions;
  onClose: () => void;
}) {
```

Import the type from the client module (App.tsx already imports values from `'../api/client'` — add `type DocumentMutationOptions` to that import).

Inside the dialog, `resolveWith` passes the options through:

```ts
      await putDocument(
        activeLibrary,
        conflict.path,
        content,
        head.etag,
        head.contentType,
        mutationOptions
      );
```

and `resolveWithDelete`:

```ts
      await deleteDocument(activeLibrary, conflict.path, mutationOptions);
```

Render site (~line 1300):

```tsx
      {mergeConflict ? (
        <ConflictMergeDialog
          activeLibrary={activeLibrary}
          conflict={mergeConflict}
          mutationOptions={browserMutationOptions()}
          onClose={() => setMergeConflictId(null)}
        />
      ) : null}
```

- [ ] **Step 4: Run the workspace tests**

Run: `cd ui && bun run test src/app/workspace.test.tsx`
Expected: all PASS (tests with no stored author send no actor header, so untouched tests stay green).

- [ ] **Step 5: Commit**

```bash
git add ui/src/app/App.tsx ui/src/app/workspace.test.tsx
git commit -m "feat(ui): stamp document mutations with the stored author"
```

---

### Task 7: First-run onboarding modal

Shown when `quarry:author` was never stored. Cannot be dismissed without a name. Existing workspace tests run with cleared localStorage, so `renderApp` seeds an author by default; onboarding tests opt out.

**Files:**
- Modify: `ui/src/app/App.tsx` (new `OnboardingDialog` component next to `SettingsDialog` ~1774; state + render in `Workspace`)
- Test: `ui/src/app/workspace.test.tsx`

- [ ] **Step 1: Seed the author in `renderApp`**

In `ui/src/app/workspace.test.tsx` (~line 1836):

```ts
function renderApp({ seedAuthor = true }: { seedAuthor?: boolean } = {}) {
  // Most tests predate onboarding; a stored author keeps the modal away.
  if (seedAuthor) localStorage.setItem('quarry:author', 'Tester');
  return render(
    <SWRConfig value={{ provider: () => new Map() }}>
      <App />
    </SWRConfig>
  );
}
```

The conflict test from Task 6 sets `quarry:author` to `Avery` before `renderApp()`; the default seeding would overwrite it with `Tester`. Change that test's call to `renderApp({ seedAuthor: false })` and keep its `localStorage.setItem('quarry:author', 'Avery')` line.

- [ ] **Step 2: Run the suite to confirm nothing broke**

Run: `cd ui && bun run test src/app/workspace.test.tsx`
Expected: all PASS (seeding is inert until the modal exists).

- [ ] **Step 3: Write the failing onboarding tests**

Add to `ui/src/app/workspace.test.tsx`. The fetch mock can be minimal — the modal renders before any document interaction:

```ts
  it('requires a name on first run and stamps it into the author store', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-1', slug: 'notes', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/notes/documents') return json([]);
      if (url === '/v1/libraries/notes/conflicts') return json([]);
      if (url === '/v1/libraries/notes/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/notes/search')) return json({ results: [], cursor: null });
      if (url.startsWith('/v1/libraries/notes/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp({ seedAuthor: false });

    const dialog = await screen.findByRole('dialog', { name: 'Welcome to Quarry' });
    const getStarted = within(dialog).getByRole('button', { name: 'Get started' });
    expect(getStarted).toBeDisabled();

    // Whitespace does not count as a name.
    await userEvent.type(within(dialog).getByLabelText('Your name'), '   ');
    expect(getStarted).toBeDisabled();

    await userEvent.clear(within(dialog).getByLabelText('Your name'));
    await userEvent.type(within(dialog).getByLabelText('Your name'), '  Avery  ');
    expect(getStarted).toBeEnabled();
    await userEvent.click(getStarted);

    expect(screen.queryByRole('dialog', { name: 'Welcome to Quarry' })).not.toBeInTheDocument();
    expect(localStorage.getItem('quarry:author')).toBe('Avery');
  });

  it('does not show onboarding when an author is already stored', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-1', slug: 'notes', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/notes/documents') return json([]);
      if (url === '/v1/libraries/notes/conflicts') return json([]);
      if (url === '/v1/libraries/notes/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/notes/search')) return json({ results: [], cursor: null });
      if (url.startsWith('/v1/libraries/notes/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await waitFor(() => expect(fetch).toHaveBeenCalled());
    expect(screen.queryByRole('dialog', { name: 'Welcome to Quarry' })).not.toBeInTheDocument();
  });
```

- [ ] **Step 4: Run to verify failure**

Run: `cd ui && bun run test src/app/workspace.test.tsx -t 'onboarding|first run'`
Expected: FAIL — no dialog named "Welcome to Quarry".

- [ ] **Step 5: Implement the modal**

In `ui/src/app/App.tsx`, add the component next to `SettingsDialog` (~line 1774):

```tsx
function OnboardingDialog({
  open,
  onSubmit,
}: {
  open: boolean;
  onSubmit: (name: string) => void;
}) {
  // Required modal: Escape and click-outside are deliberately inert.
  const dialogRef = useDialogFocusTrap(open, () => {});
  const [draftName, setDraftName] = useState('');
  const name = draftName.trim();

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 bg-black/20 p-4">
      <div
        aria-label="Welcome to Quarry"
        aria-modal="true"
        className="mx-auto mt-[12vh] w-full max-w-md overflow-hidden rounded-md border border-line-strong bg-surface shadow-xl"
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="space-y-4 p-6">
          <h2 className="text-lg font-semibold text-ink">Welcome to Quarry</h2>
          <p className="text-sm text-body">
            Quarry is a local-first workspace for versioned documents. Every change you make is
            kept with full history, alongside edits from agents and Git.
          </p>
          <label className="grid gap-1 text-sm">
            <span className="text-muted">Your name</span>
            <input
              autoFocus
              className="h-9 rounded-md border border-line bg-raised px-3 text-sm text-body outline-none focus:border-accent-line focus:ring-2 focus:ring-accent-ring"
              maxLength={120}
              onChange={(event) => setDraftName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && name) onSubmit(name);
              }}
              value={draftName}
            />
            <span className="text-xs text-muted">
              Quarry records your name on every change you make, so history shows who did what.
            </span>
          </label>
          <button
            className={primaryButton}
            disabled={!name}
            onClick={() => onSubmit(name)}
            type="button"
          >
            Get started
          </button>
        </div>
      </div>
    </div>
  );
}
```

In `Workspace`, next to the `author` state (~line 232):

```ts
  const [showOnboarding, setShowOnboarding] = useState(() => !hasStoredAuthor());
```

(Ensure `hasStoredAuthor` is in the identity import from Task 6.)

Render it directly above the `<SettingsDialog ... />` element (~line 1270):

```tsx
      <OnboardingDialog
        open={showOnboarding}
        onSubmit={(name) => {
          changeAuthor(name);
          setShowOnboarding(false);
        }}
      />
```

- [ ] **Step 6: Run the full workspace suite**

Run: `cd ui && bun run test src/app/workspace.test.tsx`
Expected: all PASS, including the two new onboarding tests.

- [ ] **Step 7: Commit**

```bash
git add ui/src/app/App.tsx ui/src/app/workspace.test.tsx
git commit -m "feat(ui): first-run onboarding modal collects the author name"
```

---

### Task 8: Full verification

**Files:** none new.

- [ ] **Step 1: Rust workspace**

Run: `cargo test --workspace`
Expected: PASS. (`quarry-fuse` is Linux-only; if its tests are skipped on macOS that is the existing behavior.)

- [ ] **Step 2: UI typecheck and tests**

Run: `cd ui && bun run typecheck && bun run test`
Expected: both clean.

- [ ] **Step 3: Eyeball it live (optional but recommended)**

Run the backend (`cargo run -p quarry -- serve --addr 127.0.0.1:7831`) and `cd ui && bun run dev`, open `http://127.0.0.1:5173` in a private window (empty localStorage): the modal appears, requires a name, and after typing in a document the Versions pane shows the name on the checkpoint.

- [ ] **Step 4: Commit any stragglers**

```bash
git status --short
```

Expected: clean (every task committed as it went).
