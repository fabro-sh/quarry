# Tmp Capability Documents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert tmp documents into private-gist-style islands addressed only by an opaque share secret, with no tmp document listing and no separate tmp invite-token layer.

**Architecture:** Tmp documents keep their existing stable internal `document_id` for versions, blocks, sessions, and events, but clients address tmp documents only by a generated secret. For tmp-scope rows, the existing `documents.path` column becomes the opaque secret; the human filename/path concept moves out of identity and can remain only as metadata/title. REST, UI routes, agent prompts, and tmp websocket joins all use the same secret as both lookup key and bearer capability.

**Tech Stack:** Rust 2024 workspace with Axum/Turso storage, React/Vite/TypeScript UI, Plate/Yjs collaboration, Vitest/Playwright tests.

**Required Rust Style Guide:** Before editing Rust code, load and follow `/Users/bhelmkamp/p/brynary/rust-style-guide/SKILL.md`. The relevant rules for this plan are validated newtypes at boundaries, no secret logging, behavior-focused integration tests, and `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`.

---

## File Structure

- Modify `crates/quarry-storage/src/lib.rs`
  - Add a private `TmpDocumentSecret` newtype.
  - Generate secrets for new tmp docs.
  - Validate every tmp secret at storage boundaries.
  - Remove public tmp listing.
  - Stop accepting path-like tmp identifiers with `/`.
- Modify `crates/quarry-server/src/lib.rs`
  - Remove `GET /v1/tmp/documents`.
  - Keep `POST /v1/tmp/documents` but generate the secret server-side.
  - Remove tmp `/share` behavior and OpenAPI entries.
  - Add a tmp websocket route keyed by secret.
  - Update discovery/capability wording.
- Modify `crates/quarry-server/resources/agent-docs.md`
  - Document `/tmp/{share_secret}` as the tmp locator and capability.
  - Remove tmp share-token instructions.
- Modify `crates/quarry-server/resources/quarry.SKILL.md`
  - Document tmp capability URLs and direct REST usage.
- Modify `ui/src/api/client.ts`
  - Remove `listTmpDocuments` and `createTmpCollabInvite`.
  - Treat the tmp identifier argument as a secret, not a slash-separated path.
- Modify `ui/src/app/App.tsx`
  - Stop fetching tmp lists.
  - Hide tmp document tree/detail panes.
  - Create tmp docs without prompting for a path.
  - Navigate to `/tmp/{secret}` after creation.
  - Build tmp agent prompts without minting invite tokens.
  - Connect tmp collab through a secret-keyed websocket route.
- Modify `ui/src/app/agent-invite.ts`
  - Build tmp locator URLs without `?token=`.
  - Explain that the tmp URL secret is the capability.
- Modify `ui/src/features/collab/rust-ws-provider.ts`
  - Reuse existing `baseUrl` support for tmp secret websocket URLs.
- Modify `ui/src/features/editor/PlateMarkdownEditor.tsx`
  - Accept collab `baseUrl` and `roomName` from the app.
- Modify tests:
  - `crates/quarry-server/tests/feature_surface.rs`
  - `crates/quarry-server/tests/rest_api.rs`
  - `ui/src/api/client.test.ts`
  - `ui/src/app/agent-invite.test.ts`
  - `ui/src/app/workspace.test.tsx`
  - `ui/src/features/collab/rust-ws-provider.test.ts`
- Regenerate or update generated API artifacts:
  - `ui/src/api/generated/openapi.json`
  - `ui/src/api/generated/types.ts`

## Decisions Locked By This Plan

- Tmp docs are not enumerable. `GET /v1/tmp/documents` returns method-not-allowed and is absent from OpenAPI.
- `POST /v1/tmp/documents` is the only collection route. It returns a new document whose `document.path` is the generated secret.
- The tmp web route is `/tmp/{secret}`. `/tmp` without a secret is only an empty/create entry screen.
- Tmp REST route family is `/v1/tmp/documents/{secret}` and suffixes such as `/blocks`, `/transactions`, `/presence`, `/review`, `/versions`, and `/ttl`. Tmp handoff remains absent because no such route exists in the current server.
- Tmp websocket route is `/v1/tmp/collab/{secret}/{room}`. The server ignores `{room}` except for y-websocket compatibility and resolves `{secret}` to the internal document id.
- Agent identity remains `X-Agent-Id` and transaction `actor.id`. The tmp secret is not an agent identity.
- Secrets are high-entropy URL-safe strings generated server-side with `Uuid::new_v4().simple().to_string()`.
- Existing tmp docs with slash paths are not migrated. Tmp docs are temporary; old path-addressed tmp docs become inaccessible through the new validated API and expire normally.

---

### Task 1: Lock The Server Behavior With Failing Tests

**Files:**
- Modify: `crates/quarry-server/tests/feature_surface.rs`
- Modify: `crates/quarry-server/tests/rest_api.rs`

- [ ] **Step 1: Update the feature-surface test expectations**

In `crates/quarry-server/tests/feature_surface.rs`, update the OpenAPI assertions near the tmp path checks so `/v1/tmp/documents` exists only for `post`, tmp `/share` paths are absent, and a tmp collab websocket path is documented.

Use this assertion block:

```rust
if tmp_documents {
    assert!(openapi["paths"]["/v1/tmp/documents"]["post"].is_object());
    assert!(openapi["paths"]["/v1/tmp/documents"]["get"].is_null());
    assert!(openapi["paths"]["/v1/tmp/documents/{secret}/share"].is_null());
    assert!(openapi["paths"]["/v1/tmp/documents/{secret}/share/{token}/revoke"].is_null());
    assert!(openapi["paths"]["/v1/tmp/collab/{secret}/{room}"].is_object());
}
```

- [ ] **Step 2: Replace the tmp document lifecycle test body**

In `crates/quarry-server/tests/rest_api.rs`, replace the existing `rest_api_supports_tmp_documents_ttl_versions_and_promotion` test setup for tmp creation/read/update with a capability-secret version. Keep the later promote assertions only if the `lib-documents` feature is enabled in the test build; otherwise this test should focus on tmp behavior.

Use this test body shape:

```rust
let response = app
    .clone()
    .oneshot(json_request(
        Method::POST,
        "/v1/tmp/documents",
        serde_json::json!({
            "content": "draft one",
            "content_type": "text/plain",
            "metadata": {"title": "Scratch"}
        }),
    ))
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::CREATED);
let etag = response.headers()[header::ETAG].to_str().unwrap().to_string();
let created: Value = response_json(response).await;
let secret = created["document"]["path"].as_str().unwrap().to_string();
let document_id = created["document"]["id"].as_str().unwrap().to_string();
assert_eq!(secret.len(), 32);
assert!(secret.chars().all(|character| character.is_ascii_hexdigit()));
assert_eq!(created["document"]["library_id"], Value::Null);
assert!(created["document"]["expires_at"].as_str().is_some());

let response = app
    .clone()
    .oneshot(
        Request::builder()
            .method(Method::GET)
            .uri("/v1/tmp/documents")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

let response = app
    .clone()
    .oneshot(
        Request::builder()
            .method(Method::GET)
            .uri(format!("/v1/tmp/documents/{secret}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::OK);
assert_eq!(response.headers()[header::ETAG], etag);
assert_eq!(response.headers()["x-quarry-document-id"], document_id.as_str());
assert_eq!(to_bytes(response.into_body(), usize::MAX).await.unwrap(), "draft one");

let response = app
    .clone()
    .oneshot(
        Request::builder()
            .method(Method::GET)
            .uri("/v1/tmp/documents/scratch/note.txt")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::BAD_REQUEST);
```

- [ ] **Step 3: Add a tmp share removal assertion**

In the same server integration test, add this assertion after `secret` is known:

```rust
let response = app
    .clone()
    .oneshot(json_request(
        Method::POST,
        &format!("/v1/tmp/documents/{secret}/share"),
        serde_json::json!({"role": "editor"}),
    ))
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::NOT_FOUND);
```

- [ ] **Step 4: Run the server tests and confirm they fail for the expected reasons**

Run:

```bash
cargo test -p quarry-server --features tmp-documents rest_api_supports_tmp_documents_ttl_versions_and_promotion -- --nocapture
cargo test -p quarry-server --features tmp-documents tmp_documents_support_create_read_update_ttl_versions_and_delete -- --nocapture
```

Expected: tests fail because `POST /v1/tmp/documents` still uses the requested path/default path, `GET /v1/tmp/documents` still lists tmp docs, slash paths still work, and tmp `/share` still mints invite tokens.

- [ ] **Step 5: Commit the failing tests**

```bash
git add crates/quarry-server/tests/feature_surface.rs crates/quarry-server/tests/rest_api.rs
git commit -m "test: lock tmp capability document behavior"
```

---

### Task 2: Add Tmp Secret Generation And Storage Validation

**Files:**
- Modify: `crates/quarry-storage/src/lib.rs`

- [ ] **Step 1: Add the private tmp secret newtype**

Add this type near the other storage-domain helpers in `crates/quarry-storage/src/lib.rs`:

```rust
const TMP_DOCUMENT_SECRET_LEN: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
struct TmpDocumentSecret(String);

impl TmpDocumentSecret {
    fn generate() -> Self {
        Self(Uuid::new_v4().simple().to_string())
    }

    fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        let valid = value.len() == TMP_DOCUMENT_SECRET_LEN
            && value.chars().all(|character| character.is_ascii_hexdigit());
        if !valid {
            return Err(QuarryError::InvalidPath("invalid tmp document secret".to_string()));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}
```

- [ ] **Step 2: Add focused unit tests for the secret validator**

Add a `#[cfg(test)]` test module near the helper or extend the existing bottom-of-file tests:

```rust
#[cfg(test)]
mod tmp_secret_tests {
    use super::*;

    #[test]
    fn generated_tmp_secret_is_url_safe_hex() {
        let secret = TmpDocumentSecret::generate();

        assert_eq!(secret.as_str().len(), TMP_DOCUMENT_SECRET_LEN);
        assert!(secret.as_str().chars().all(|character| character.is_ascii_hexdigit()));
        assert!(!secret.as_str().contains('/'));
    }

    #[test]
    fn tmp_secret_rejects_path_like_values() {
        let error = TmpDocumentSecret::parse("scratch/note.md")
            .expect_err("path-like tmp identifiers should be rejected");

        assert!(matches!(error, QuarryError::InvalidPath(message) if message == "invalid tmp document secret"));
    }

    #[test]
    fn tmp_secret_normalizes_uppercase_hex() -> Result<()> {
        let secret = TmpDocumentSecret::parse("ABCDEF0123456789ABCDEF0123456789")?;

        assert_eq!(secret.as_str(), "abcdef0123456789abcdef0123456789");
        Ok(())
    }
}
```

- [ ] **Step 3: Replace tmp path normalization with secret validation**

In these public tmp storage methods, replace `let path = normalize_path(path)?;` with `let secret = TmpDocumentSecret::parse(path)?;` and pass `secret.as_str()` into all existing helper calls and SQL parameters:

```rust
put_tmp_document
get_tmp_document
head_tmp_document
raw_tmp_version_history
tmp_version_history
tmp_document_version
delete_tmp_document
set_tmp_document_ttl
promote_tmp_document
```

Keep function signatures unchanged for this task so callers can be migrated incrementally. Within each function, rename local variables from `path` to `secret` when the value is the capability string.

- [ ] **Step 4: Add a storage create method that owns secret generation**

Add this public method beside `put_tmp_document`:

```rust
pub async fn create_tmp_document(
    &self,
    content: Vec<u8>,
    metadata: JsonValue,
    content_type: &str,
    ttl: TmpTtl,
) -> Result<WriteOutcome> {
    let secret = TmpDocumentSecret::generate();
    self.put_tmp_document(
        secret.as_str(),
        content,
        metadata,
        content_type,
        ttl,
        WritePrecondition::IfNoneMatch,
    )
    .await
}
```

- [ ] **Step 5: Delete the public tmp listing method**

Remove the entire `pub async fn list_tmp_documents` function from `crates/quarry-storage/src/lib.rs`. Leave the `idx_documents_scope_path` index in place because direct tmp lookup still uses `document_scope, path`.

- [ ] **Step 6: Delete tmp invite-token storage helpers**

Delete the tmp-only storage methods `create_tmp_collab_invite_token` and `tmp_collab_invite_tokens`. Keep library-document invite-token methods unchanged.

- [ ] **Step 7: Run focused storage tests**

Run:

```bash
cargo test -p quarry-storage tmp_secret -- --nocapture
```

Expected: the new `tmp_secret_tests` pass, no storage method accepts slash-delimited tmp paths, and there are no references to deleted tmp invite-token methods.

- [ ] **Step 8: Commit storage changes**

```bash
git add crates/quarry-storage/src/lib.rs
git commit -m "feat(storage): address tmp docs by capability secret"
```

---

### Task 3: Convert Tmp Server Routes To Capability Access

**Files:**
- Modify: `crates/quarry-server/src/lib.rs`

- [ ] **Step 1: Remove the tmp collection GET route**

Change `install_tmp_document_routes` from:

```rust
router
    .route(
        "/v1/tmp/documents",
        get(list_tmp_documents).post(create_tmp_document),
    )
    .route("/v1/tmp/documents/{*path}", tmp_document_route)
```

to:

```rust
router
    .route("/v1/tmp/documents", post(create_tmp_document))
    .route("/v1/tmp/collab/{secret}/{room}", get(tmp_collab_websocket))
    .route("/v1/tmp/documents/{*path}", tmp_document_route)
```

- [ ] **Step 2: Delete `list_tmp_documents` server handler and OpenAPI registration**

Remove the `list_tmp_documents` function and remove it from the OpenAPI path list. Keep `create_tmp_document`.

- [ ] **Step 3: Generate tmp secrets in `create_tmp_document`**

Replace the current requested-path logic in `create_tmp_document` with:

```rust
let content_type = request
    .content_type
    .as_deref()
    .unwrap_or("text/markdown")
    .to_string();
let mut metadata = request.metadata.unwrap_or_else(|| serde_json::json!({}));
if let JsonValue::Object(object) = &mut metadata {
    object
        .entry("content_type")
        .or_insert_with(|| JsonValue::String(content_type.clone()));
}
let ttl = request
    .expires_at
    .map(quarry_storage::TmpTtl::ExpiresAt)
    .unwrap_or(quarry_storage::TmpTtl::Default);
let outcome = state
    .store
    .create_tmp_document(
        request.content.unwrap_or_default().into_bytes(),
        metadata,
        &content_type,
        ttl,
    )
    .await?;
json_with_etag(StatusCode::CREATED, &outcome, &outcome.version.id)
```

Then remove `path` from `CreateTmpDocumentRequest`:

```rust
pub struct CreateTmpDocumentRequest {
    pub content: Option<String>,
    pub metadata: Option<JsonValue>,
    pub content_type: Option<String>,
    pub expires_at: Option<String>,
}
```

- [ ] **Step 4: Remove tmp `/share` branches**

In `get_tmp_document`, delete:

```rust
if let Some(path) = path.strip_suffix("/share") {
    return json_response(
        StatusCode::OK,
        &state.store.tmp_collab_invite_tokens(path).await?,
    );
}
```

In `post_tmp_document_action`, delete:

```rust
if let Some(path) = path.strip_suffix("/share") {
    let request: CreateCollabInviteRequest = serde_json::from_value(request)
        .map_err(|error| QuarryError::InvalidPath(format!("invalid share request: {error}")))?;
    let token = state
        .store
        .create_tmp_collab_invite_token(path, &request.role, request.by_hint)
        .await?;
    return json_response(StatusCode::CREATED, &token);
}
```

- [ ] **Step 5: Add tmp secret websocket route**

Add this handler near `collab_websocket`:

```rust
#[utoipa::path(
    get,
    path = "/v1/tmp/collab/{secret}/{room}",
    params(("secret" = String, Path), ("room" = String, Path)),
    responses((status = 101, description = "Yjs collaboration websocket for tmp capability documents"))
)]
#[allow(dead_code)]
async fn tmp_collab_websocket_openapi() {}

async fn tmp_collab_websocket(
    State(state): State<AppState>,
    Path((secret, _room)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let document = state.store.head_tmp_document(&secret).await?;
    let shutdown = state.shutdown_token();
    Ok(ws
        .on_upgrade(move |socket| async move {
            state
                .sessions
                .serve_socket(document.id, socket, shutdown)
                .await;
        })
        .into_response())
}
```

Register `tmp_collab_websocket_openapi` in the OpenAPI path list.

- [ ] **Step 6: Update discovery and capabilities**

Change tmp capabilities from:

```rust
capabilities.extend(["tmp_documents", "share"]);
```

to:

```rust
capabilities.extend(["tmp_documents", "capability_urls"]);
```

Update discovery auth text so tmp docs are described as capability URLs:

```rust
auth_note:
    "Tmp document URLs are bearer capabilities: anyone with /tmp/{secret} can access that tmp document. Library REST APIs remain trusted-localhost for now.",
```

Keep library document auth language unchanged where it still applies.

- [ ] **Step 7: Remove tmp share OpenAPI functions**

Delete `tmp_document_share_openapi` and `tmp_document_share_create_openapi` and remove their path registrations.

- [ ] **Step 8: Run focused server tests**

Run:

```bash
cargo test -p quarry-server --features tmp-documents rest_api_supports_tmp_documents_ttl_versions_and_promotion -- --nocapture
cargo test -p quarry-server --features tmp-documents tmp_documents_support_create_read_update_ttl_versions_and_delete -- --nocapture
cargo test -p quarry-server --features tmp-documents feature_surface -- --nocapture
```

Expected: the server tests pass after updating any remaining assertions that still use old path-like tmp identifiers.

- [ ] **Step 9: Commit server route changes**

```bash
git add crates/quarry-server/src/lib.rs crates/quarry-server/tests/rest_api.rs crates/quarry-server/tests/feature_surface.rs
git commit -m "feat(server): make tmp docs capability addressed"
```

---

### Task 4: Update The TypeScript API Client And Agent Prompt Helpers

**Files:**
- Modify: `ui/src/api/client.ts`
- Modify: `ui/src/api/client.test.ts`
- Modify: `ui/src/app/agent-invite.ts`
- Modify: `ui/src/app/agent-invite.test.ts`

- [ ] **Step 1: Remove tmp list and tmp invite helpers**

In `ui/src/api/client.ts`, delete:

```ts
export const listTmpDocuments = () => jsonRequest<DocumentListEntry[]>('/v1/tmp/documents');
```

and delete:

```ts
export const createTmpCollabInvite = (
  path: string,
  request: { byHint?: string; role?: 'editor' | 'viewer' } = {}
) =>
  jsonRequest<CollabInviteToken>(`/v1/tmp/documents/${pathSegments(path)}/share`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ byHint: request.byHint, role: request.role ?? 'editor' }),
  });
```

- [ ] **Step 2: Rename tmp API parameters to `secret`**

Change tmp client helpers to accept `secret: string` and use `segment(secret)` instead of `pathSegments(path)`.

Use this pattern:

```ts
export async function getTmpDocument(secret: string): Promise<LoadedDocument> {
  const response = await fetch(tmpDocumentHref(secret));
  await assertOk(response);
  const contentType = response.headers.get('content-type') ?? 'application/octet-stream';
  return {
    documentId: response.headers.get('x-quarry-document-id') ?? '',
    path: secret,
    content: isTextContentType(contentType) ? await response.text() : '',
    contentType,
    etag: response.headers.get('etag') ?? '',
    expiresAt: response.headers.get('x-quarry-expires-at') ?? undefined,
  };
}

export function tmpDocumentHref(secret: string) {
  return `/v1/tmp/documents/${segment(secret)}`;
}
```

Apply the same `segment(secret)` pattern to tmp versions, TTL, promote, delete, review, blocks, transactions, and presence helpers.

- [ ] **Step 3: Remove `path` from tmp create request**

Change the TypeScript request type:

```ts
export interface CreateTmpDocumentRequest {
  content?: string;
  metadata?: Record<string, unknown>;
  contentType?: string;
  expiresAt?: string;
}
```

Change `createTmpDocument` body construction:

```ts
body: JSON.stringify({
  content: request.content,
  content_type: request.contentType,
  metadata: request.metadata,
  expires_at: request.expiresAt,
}),
```

- [ ] **Step 4: Update tmp agent URL helpers**

In `ui/src/app/agent-invite.ts`, make tmp URLs tokenless:

```ts
export function tmpWorkspaceRouteForDocument(secret: string) {
  if (!secret) return '/tmp';
  return `/tmp/${encodeURIComponent(secret)}`;
}
```

Change `buildTokenizedDocumentUrl` so `scope === 'tmp'` ignores `token` and returns `/tmp/{secret}`:

```ts
if (scope === 'tmp') {
  return new URL(tmpWorkspaceRouteForDocument(path), normalizedOrigin(origin)).toString();
}
```

Keep the library branch using `?token=` for now.

- [ ] **Step 5: Update tmp agent prompt text**

For tmp scope, build prompts with this wording:

```ts
const tmpCapabilityNotice =
  'Tmp document URLs are bearer capabilities. Anyone with this URL can access this tmp document; do not treat the secret as an agent identity.';
```

Include `tmpCapabilityNotice` in the generated prompt only for tmp docs. Keep `X-Agent-Id` instructions unchanged.

- [ ] **Step 6: Update client and prompt tests**

Update `ui/src/api/client.test.ts` so there is no `/v1/tmp/documents` GET expectation and no tmp `/share` expectation. Update `ui/src/app/agent-invite.test.ts` so tmp URL assertions expect:

```ts
expect(tmpWorkspaceRouteForDocument('72cb58585aa73e35758bc1141f79e32e')).toBe(
  '/tmp/72cb58585aa73e35758bc1141f79e32e'
);
expect(tokenizedDocUrl).toBe(
  'http://127.0.0.1:5173/tmp/72cb58585aa73e35758bc1141f79e32e'
);
expect(prompt).toContain('Tmp document URLs are bearer capabilities');
expect(prompt).toContain(
  'POST http://127.0.0.1:5173/v1/tmp/documents/72cb58585aa73e35758bc1141f79e32e/presence'
);
```

- [ ] **Step 7: Run focused UI unit tests**

Run:

```bash
cd ui
bun test src/api/client.test.ts src/app/agent-invite.test.ts
```

Expected: the updated client and prompt tests pass.

- [ ] **Step 8: Commit TypeScript client changes**

```bash
git add ui/src/api/client.ts ui/src/api/client.test.ts ui/src/app/agent-invite.ts ui/src/app/agent-invite.test.ts
git commit -m "feat(ui): treat tmp document ids as capability secrets"
```

---

### Task 5: Update Tmp Workspace UI And Tmp Websocket Join

**Files:**
- Modify: `ui/src/app/App.tsx`
- Modify: `ui/src/app/workspace.test.tsx`
- Modify: `ui/src/features/collab/rust-ws-provider.ts`
- Modify: `ui/src/features/collab/rust-ws-provider.test.ts`
- Modify: `ui/src/features/editor/PlateMarkdownEditor.tsx`

- [ ] **Step 1: Remove tmp list fetching from the workspace**

In `ui/src/app/App.tsx`, remove the tmp list SWR call:

```ts
const { data: tmpDocuments = [] } = useSWR(
  tmpDocumentsEnabled ? ['/v1/tmp-documents'] : null,
  listTmpDocuments
);
const documents = isTmpDocument ? tmpDocuments : libraryDocuments;
```

Replace it with:

```ts
const documents = isTmpDocument ? [] : libraryDocuments;
```

Remove `listTmpDocuments` from the imports.

- [ ] **Step 2: Hide navigation/detail panes for tmp documents**

Replace pane visibility checks that use `tmpOnlyMode` with tmp-scope checks:

```ts
const tmpIslandMode = isTmpDocument;
```

Use `tmpIslandMode` where the UI decides whether to render the document tree, right pane, search, graph, and details. The tmp page should render only the document body for `/tmp/{secret}` and the empty/create state for `/tmp`.

- [ ] **Step 3: Make tmp creation pathless**

Replace `createNewTmpDocument` with:

```ts
async function createNewTmpDocument() {
  if (!tmpDocumentsEnabled) return;
  setDocumentScope('tmp');
  const initialContent = '# Untitled\n';
  const initialContentType = 'text/markdown';
  const created = await createTmpDocument({
    content: initialContent,
    contentType: initialContentType,
    metadata: { title: 'Untitled' },
  });
  const secret = created.outcome.document?.path ?? '';
  if (!secret) throw new Error('tmp document creation did not return a secret');
  const createdEtag = created.etag || `"${created.outcome.version.id}"`;
  await Promise.all([
    mutate(
      ['/v1/tmp-document', secret],
      {
        content: initialContent,
        contentType: initialContentType,
        documentId: created.outcome.document?.id ?? '',
        etag: createdEtag,
        path: secret,
      },
      { revalidate: false }
    ),
    mutate(['/v1/tmp-versions', secret], [historyEntryFromVersion(created.outcome.version)], {
      revalidate: false,
    }),
  ]);
  setSelectedPath(secret);
}
```

Update `createVisibleDocument` so it does not pass a path default into `createNewTmpDocument`.

- [ ] **Step 4: Parse tmp routes as one secret segment**

Update `parseWorkspaceRoute` tmp branch:

```ts
if (segments[0] === 'tmp') {
  return {
    scope: 'tmp' as DocumentScope,
    library: null,
    path: segments[1] ? safeDecodeSegment(segments[1]) : '',
  };
}
```

Update `tmpWorkspaceRoute`:

```ts
function tmpWorkspaceRoute(secret: string) {
  if (!secret) return '/tmp';
  return `/tmp/${encodeURIComponent(secret)}`;
}
```

- [ ] **Step 5: Build tmp agent prompts without minting a token**

In `openAddAgentModal`, keep library documents on `createCollabInvite`, but for tmp documents set:

```ts
const tokenizedDocUrl = buildTokenizedDocumentUrl({
  origin: window.location.origin,
  scope,
  library,
  path,
  token: '',
});
```

Do not call `createTmpCollabInvite` for tmp docs. The tmp `path` variable is the secret.

- [ ] **Step 6: Add tmp websocket URL support**

In `ui/src/features/collab/rust-ws-provider.ts`, add:

```ts
export function tmpCollabWebSocketBaseUrl(
  secret: string,
  location: Pick<Location, 'host' | 'protocol'> = window.location
) {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${location.host}/v1/tmp/collab/${encodeURIComponent(secret)}`;
}
```

In `ui/src/features/editor/PlateMarkdownEditor.tsx`, extend `CollabEditorConfig`:

```ts
export interface CollabEditorConfig {
  baseUrl?: string;
  documentId: string;
  onSaveStateChange?: (state: CollabSaveState) => void;
  roomName?: string;
  sessionId: string;
  token?: string;
}
```

Pass those options to the provider:

```ts
options: {
  baseUrl: collab.baseUrl,
  roomName: collab.roomName ?? collabDocumentId,
  token: collabToken,
},
```

In `App.tsx`, for tmp docs set `collabBaseUrl` and `roomName`:

```ts
const collabBaseUrl = isTmpDocument && selectedPath ? tmpCollabWebSocketBaseUrl(selectedPath) : undefined;
const collabRoomName = isTmpDocument ? 'content' : undefined;
```

Pass both into `DocumentBody` and then into the `collab` config.

- [ ] **Step 7: Update workspace tests**

Update tmp tests in `ui/src/app/workspace.test.tsx` so they seed routes like `/tmp/72cb58585aa73e35758bc1141f79e32e`, never mock `/v1/tmp/documents`, and assert the tree is hidden for tmp scope:

```ts
window.history.pushState({}, '', '/tmp/72cb58585aa73e35758bc1141f79e32e');
render(<App />);
await screen.findByText('Scratch');
expect(fetch).not.toHaveBeenCalledWith('/v1/tmp/documents', undefined);
expect(screen.queryByLabelText('Document tree')).not.toBeInTheDocument();
expect(window.location.pathname).toBe('/tmp/72cb58585aa73e35758bc1141f79e32e');
```

- [ ] **Step 8: Update websocket provider tests**

In `ui/src/features/collab/rust-ws-provider.test.ts`, add:

```ts
expect(tmpCollabWebSocketBaseUrl('72cb58585aa73e35758bc1141f79e32e', {
  protocol: 'http:',
  host: '127.0.0.1:5173',
})).toBe('ws://127.0.0.1:5173/v1/tmp/collab/72cb58585aa73e35758bc1141f79e32e');
```

Add a provider factory assertion that `baseUrl` is forwarded when present:

```ts
expect(factory).toHaveBeenCalledWith(
  'ws://127.0.0.1:5173/v1/tmp/collab/72cb58585aa73e35758bc1141f79e32e',
  'content',
  expect.anything(),
  expect.objectContaining({ params: {} })
);
```

- [ ] **Step 9: Run focused UI tests**

Run:

```bash
cd ui
bun test src/app/workspace.test.tsx src/features/collab/rust-ws-provider.test.ts
```

Expected: the tmp workspace tests pass without any call to `GET /v1/tmp/documents`.

- [ ] **Step 10: Commit UI workspace changes**

```bash
git add ui/src/app/App.tsx ui/src/app/workspace.test.tsx ui/src/features/collab/rust-ws-provider.ts ui/src/features/collab/rust-ws-provider.test.ts ui/src/features/editor/PlateMarkdownEditor.tsx
git commit -m "feat(ui): open tmp docs as isolated capability islands"
```

---

### Task 6: Update Docs, Generated API Artifacts, And Final Verification

**Files:**
- Modify: `crates/quarry-server/resources/agent-docs.md`
- Modify: `crates/quarry-server/resources/quarry.SKILL.md`
- Modify: `ui/src/api/generated/openapi.json`
- Modify: `ui/src/api/generated/types.ts`

- [ ] **Step 1: Update agent docs**

In `crates/quarry-server/resources/agent-docs.md`, replace the tmp invite section with:

````markdown
Tmp document links look like this:

```text
http://127.0.0.1:5173/tmp/72cb58585aa73e35758bc1141f79e32e
```

For tmp documents, the segment after `/tmp/` is the share secret. It is both
the document locator and the bearer capability. Anyone with this URL can access
the tmp document.

Build the document API from `/v1/tmp/documents/$SECRET`:

```sh
ORIGIN="http://127.0.0.1:5173"
SECRET="72cb58585aa73e35758bc1141f79e32e"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/tmp/documents/$SECRET"
```
````

Remove statements that tmp docs expose `$DOC/share`.

- [ ] **Step 2: Update Quarry skill docs**

In `crates/quarry-server/resources/quarry.SKILL.md`, update the locator examples so tmp docs use:

```text
http://127.0.0.1:5173/tmp/72cb58585aa73e35758bc1141f79e32e
```

Add:

```markdown
For tmp documents, the segment after `/tmp/` is the document identifier and capability.
Do not send a separate bearer token for tmp docs. Use `X-Agent-Id` to identify
your agent.
```

- [ ] **Step 3: Regenerate OpenAPI JSON**

Start the server if it is not already running, then run:

```bash
cd ui
bun run generate:api
```

Expected: `ui/src/api/generated/openapi.json` no longer contains tmp `get` list or tmp `/share` paths, and contains `/v1/tmp/collab/{secret}/{room}`.

- [ ] **Step 4: Update generated TypeScript types**

Manually update `ui/src/api/generated/types.ts` so `CreateTmpDocumentRequest` has this shape:

```ts
export interface CreateTmpDocumentRequest {
  content?: string | null;
  content_type?: string | null;
  expires_at?: string | null;
  metadata?: unknown;
}
```

Remove tmp share operation-related generated types only if they are no longer referenced. Keep `CollabInviteToken` if library documents still use it.

- [ ] **Step 5: Run full Rust verification**

Run:

```bash
cargo test --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

Expected: all Rust tests and Clippy pass. If Clippy reports variable names like `path` that now mean a secret, rename them rather than suppressing the lint.

- [ ] **Step 6: Run full UI verification**

Run:

```bash
cd ui
bun test
bunx playwright test
```

Expected: all UI unit and browser tests pass. If browser tests assume tmp list behavior, update them to create tmp docs through `POST /v1/tmp/documents` and follow the returned secret URL.

- [ ] **Step 7: Search for stale tmp token/list language**

Run:

```bash
rg -n "listTmpDocuments|createTmpCollabInvite|/v1/tmp/documents\\\"|/v1/tmp/documents'|tmp.*share|locator token|tmp invite|/tmp/.+\\?token" ui crates docs spec-browser.md
```

Expected: no stale tmp-list or tmp-token references remain. Library share-token references may remain.

- [ ] **Step 8: Final commit**

```bash
git add crates/quarry-server/resources/agent-docs.md crates/quarry-server/resources/quarry.SKILL.md ui/src/api/generated/openapi.json ui/src/api/generated/types.ts
git commit -m "docs: describe tmp capability document URLs"
```

---

## Self-Review Checklist

- The plan removes `GET /v1/tmp/documents` from server routing, UI calls, tests, and OpenAPI.
- The plan removes tmp `/share` from server routing, UI calls, tests, and docs.
- The plan uses one tmp secret for web, REST, agent prompts, events, blocks, transactions, and websocket joins.
- The plan preserves internal `document_id` for storage, versions, blocks, and session state.
- The plan keeps agent identity separate through `X-Agent-Id` and transaction `actor.id`.
- The plan explicitly follows `/Users/bhelmkamp/p/brynary/rust-style-guide/SKILL.md`.
- The plan does not add ownership, accounts, per-agent tokens, or tmp listing.
