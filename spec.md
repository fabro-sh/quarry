# Quarry Product and Engineering Spec

Status: draft v0  
Sources: `docs/deep-research/*.md`, mirrored corpus under `references/deep-research/`, and licensed Potion source under `references/potion/`

## 1. Product Definition

Quarry is a local-first collaborative storage substrate for humans and AI agents. It gives the same project state to rich editors, command-line agents, Git remotes, API/MCP clients, and memory systems without forcing every actor through the same interface.

The core product is a single `quarry` CLI. Running `quarry server` starts the local daemon, embedded web UI, HTTP/WebSocket API, Git bridge, and background jobs. Quarry is backed by immutable content objects, mutable refs, CRDT-backed structured documents, first-class annotations, Git import/export, REST/OpenAPI, and MCP tools.

Quarry is not "Git with collaboration bolted on." Git is an interoperability and history projection. The canonical source of truth is Quarry's local store: content-addressed blobs, op logs, snapshots, refs, provenance, and derived document views.

## 2. Goals

Quarry should deliver:

- A durable local-first object store for project files, rich documents, annotations, and binary pointer records.
- Real-time multi-actor editing between humans, agents, and Git imports.
- A two-way Git bridge that preserves normal Git workflows while keeping richer collaboration metadata outside Git.
- A web/editor surface for human review, comments, suggestions, provenance, and draft-to-publish workflows.
- An MCP and REST/WS API surface that agents can use directly without pretending every action is a file write.
- Git materialization and explicit exports that make published state inspectable by ordinary Git and shell tools without requiring a mounted filesystem.
- A substrate for memory products, not a monolithic memory app in v1.
- Browser, API, SDK, MCP, and Git access to project files, rich documents, annotations, and binary pointer records.
- Explicit provenance for every mutation: human, agent, Git import, or system.

## 3. Non-Goals

These are intentionally out of scope for the first production-quality release:

- Replacing Git as a global ecosystem. Quarry must interoperate with Git, not require the world to adopt Quarry remotes.
- Byte-level CRDT editing of arbitrary binary files. Binaries are immutable pointer records in v1; collaboration happens through whole-object comments and metadata.
- A fully hosted multi-tenant SaaS control plane.
- A general-purpose memory product competing with Supermemory, TrueMemory, Mem0, Zep, or AgentMemory.
- Perfect lossless round-tripping of rich documents through plain Git files. Git exports should be useful and stable, but some rich editor metadata will live in sidecars or Quarry-only metadata.
- FUSE mounts, sync-folder projections, SMB/Samba, or mounted filesystem views in v1.
- Inspecting, extracting, indexing, OCRing, or semantically understanding binary or opaque asset contents in v1.
- Built-in API/MCP search in v1. Search is a v1.1 feature.
- Authentication or authorization as a security boundary in v1. Actor identity is self-attested and used for provenance only.

## 4. Target Workflows

### 4.1 Agent Coding Session

An agent opens a Quarry workspace through REST/OpenAPI, MCP, the generated TS SDK, or a Git-materialized view, then edits a bounded set of files or documents. API and MCP writes implicitly create and commit transactions against the selected ref. Agents do not need a draft or collaboration session unless they explicitly enter a human-review workflow. Other actors see one coherent transaction instead of thousands of keystroke-level events.

### 4.2 Human Review of Agent Work

A human opens Quarry's editor UI, sees provenance rails showing what was produced by an agent, leaves inline comments or suggestions, and saves those comments as a draft ref. An agent can read the draft and produce a follow-up transaction. Publishing resolves the draft into a stable ref and optionally materializes it as a Git commit.

### 4.3 Two-Way Git Sync

Quarry materializes published refs into normal Git commits. If an external Git commit appears, Quarry ingests it as a transaction attributed to the Git author. If the import collides semantically with live work, Quarry opens a conflict draft rather than silently overwriting either side.

### 4.4 Binary Review

A binary is stored as an immutable pointer record, similar in spirit to Git LFS: Quarry tracks the hash, size, media type, and local path or external URL, but does not inspect the binary contents in v1. Comments can attach to the binary as a whole object or path-level artifact. The original binary remains unchanged.

### 4.5 Memory Layer on Top

Memory systems consume Quarry events, project metadata, versions, refs, transactions, and user/agent annotations. The first Quarry release should expose enough substrate for project memory, while leaving search and advanced memory pipelines as v1.1 or optional integration work.

## 5. System Architecture

### 5.1 Process Shape

Quarry ships as a single CLI binary with a daemon subcommand:

- `quarry server`: owns lifecycle, config, local data directory, API listeners, background jobs, and Git sync.
- Local web UI: embedded in the Rust binary and served by `quarry server`.
- TS SDK: generated from the OpenAPI spec.
- CLI: controls daemon lifecycle, workspace init, sync, export, status, and diagnostics.

Accepted v1 deployment: one `quarry server` process per developer machine. It may serve multiple local clients. Team synchronization should initially happen through Git and later through peer-to-peer or hosted sync.

### 5.2 Rust Workspace

The implementation should be split into small crates with clear ownership:

- `quarry-store`: filesystem and SQLite access only. Owns content-addressed blobs, metadata schema, op-log persistence, snapshot storage, and migrations.
- `quarry-crdt`: collaborative document abstraction, CRDT engine integration, op schema, transaction grouping, provenance, draft/publish semantics.
- `quarry-git`: bidirectional Git bridge using gitoxide. Owns `materialize` and `ingest`.
- `quarry-export`: Git materialization helpers, raw archive export, rich-document sidecars, binary pointer export, and deferred local-access adapters.
- `quarry-api`: Axum HTTP, WebSocket, SSE, MCP, git smart-HTTP, self-attested actor metadata, policy guardrails, and transaction coordination.
- `quarry-cli`: binary wiring, config, logging, background jobs, lifecycle, and CLI subcommands including `quarry server`.
- `quarry-sdk-ts`: generated browser/Node REST client from the OpenAPI spec, plus small hand-written WS helpers where OpenAPI does not apply.
- `quarry-ui`: Potion-derived web review/editor surface built as static assets and embedded into the Rust binary.

### 5.3 Storage Model

Quarry's canonical records:

- `Blob`: immutable text/small-object bytes addressed by content hash.
- `BinaryObject`: immutable pointer record for large or opaque binaries, storing hash, size, media type, local path, and optional external URL.
- `Tree`: directory/file view mapping logical paths to object IDs or refs.
- `StructuredDoc`: CRDT-backed rich or plain document state.
- `Annotation`: first-class comment, suggestion, highlight, critique, or provenance note attached to a blob, document, ref, path, range, or selector.
- `Ref`: mutable named pointer such as `published/main`, `draft/<id>`, `git/<branch>`, or `local/<user>`.
- `Transaction`: grouped mutation with actor, timestamps, affected paths/ranges, intent lock metadata, and optional Git identity.
- `Actor`: human, agent, Git import, system, or integration.
- `DerivedArtifact`: generated structured outputs for supported text/rich-document workflows, such as Markdown exports, AST summaries, normalized diffs, and future search indexes. Binary and opaque-asset extraction artifacts are out of scope for v1.
- `Event`: append-only audit and sync event.

The store uses Turso/libSQL-compatible SQLite with WAL mode for metadata and op indexes. Blob bytes live on local disk in a sharded content-addressed layout. Large or opaque binaries are represented by pointer records, not copied into Turso. Storage access must be behind traits so remote Turso, S3, or Postgres-backed variants can be added later.

## 6. CRDT and Collaboration

### 6.1 Accepted V1 CRDT Decision

V1 is Yjs/Yrs-first for structured documents because Potion is the licensed web-editor base and already uses PlateJS, `@platejs/yjs`, `@slate-yjs/*`, Hocuspocus-style collaboration, Yjs snapshots, comments, suggestions, and version history. The Rust core should expose a `CollabDoc` abstraction and use a Yjs-compatible path for v1. Loro remains a future backend option for non-editor-centric collaborative data once the v1 spine is working.

Potion should be treated as source material for the editor experience and data model, not as a server architecture to copy wholesale. Quarry should reuse or adapt Potion's PlateJS editor, comment/suggestion UI, version-history patterns, and Yjs document model while replacing the Next/tRPC/Prisma/auth/upload stack with Quarry's Rust server, Turso/libSQL store, OpenAPI REST API, self-attested actor identity, and binary pointer records.

### 6.2 Transaction Model

Every write must produce a transaction:

- `transaction_id`
- `actor_id`
- `actor_kind`: `human`, `agent`, `git_import`, `system`, `integration`
- `started_at`, `committed_at`
- affected paths and optional ranges
- operation group payload
- provenance relationships
- optional Git author/committer identity
- optional parent transaction or source draft

Agents should not broadcast keystroke-equivalent changes. Agent writes are coalesced into logical transactions such as "rewrite file", "apply patch", "resolve comments", or "import commit".

For API and MCP writes, transactions are implicit and auto-committed by default. Callers may pass a `client_transaction_id` or use an explicit transaction endpoint only when they need to group multiple operations.

### 6.3 Intent Locks

Agents and integrations can request soft intent locks for affected paths or ranges. Locks are advisory for humans and coordinating agents, but writes outside the owner should create conflict events. Locks expire automatically and must be visible in the UI/API.

### 6.4 Presence and Transient State

Presence, cursors, live typing, intent previews, and temporary review cues are transient. They must not be stored in durable document history unless explicitly committed as an annotation or transaction.

## 7. Git Bridge

### 7.1 Outbound Materialization

`quarry-git materialize` converts a Quarry ref into a Git commit:

- Walk the selected tree/ref.
- Export plain text/code/Markdown directly.
- Export rich documents as Markdown plus sidecar metadata.
- Export binaries through Git LFS policy when enabled.
- Preserve author identity from the originating transaction when possible.
- Attach Quarry transaction metadata in commit trailers, git notes, or sidecar refs.
- Produce normal Git commits that external tools can fetch, inspect, and merge.

### 7.2 Inbound Ingestion

`quarry-git ingest` imports external Git commits:

- Fetch or receive commits.
- Diff against the last materialized Quarry tree.
- Convert file changes into Quarry transactions.
- Attribute imported changes to the Git author with `actor_kind = git_import`.
- Preserve one Git commit as one transaction group to avoid op amplification.
- Reconcile against live edits through CRDT merge plus semantic conflict detection.
- Open conflict drafts when imported changes and live changes target the same semantic area.

### 7.3 Conflict Policy

V1 policy: preserve both sides and save both versions for human resolution. Do not silently let Git imports overwrite live work, and do not force-push local Quarry state over external Git without explicit policy. A future release can add LLM-assisted conflict resolution, but v1 should keep the conflict mechanics deterministic.

## 8. File Access and Export Surfaces

### 8.1 V1 Access Model

V1 does not include FUSE, sync-folder, SMB/Samba, or any mounted filesystem surface. The supported access paths are:

- web UI file tree and editor
- REST/OpenAPI endpoints
- generated TS SDK
- MCP resources and tools
- Git materialization for published refs
- explicit raw export for snapshots or review bundles

Agents should use REST/MCP/SDK for live reads and writes. When ordinary shell tooling is needed, Quarry should materialize a ref to Git or export a read-only snapshot rather than pretending the canonical store is a filesystem mount.

### 8.2 Agent-Readable Data

Anything agents are expected to act on in v1 must be available through API/MCP resources and the generated SDK. Quarry must expose:

- source files and directory trees
- structured document snapshots
- Markdown exports of rich documents when explicitly requested
- annotation and suggestion objects
- binary pointer records
- diagnostics through `GET /stats` and `quarry status`

Built-in API/MCP search is deferred to v1.1. In v1, Quarry does not provide a dedicated search endpoint, does not run OCR, and does not extract text from binary or opaque assets.

### 8.3 Ephemeral Workspaces

Ephemeral workspaces are cut from v1. Quarry should not define `ephemeral/<actor>` refs or scratch namespaces in the v1 storage model.

Local-only scratch behavior can be revisited after the core Git, web UI, and API/MCP loops are stable.

### 8.4 Deferred Local Projection Work

FUSE, sync-folder projection, SMB/Samba, and userspace network shares are post-v1. They should be designed only after the canonical store, API, Git bridge, and web UI have proven their transaction and conflict semantics.

## 9. API and MCP Surface

### 9.1 REST/WS Routes

The API should borrow Proof's separation of document state, snapshots, edits, ops, presence, comments, suggestions, and agent bridge routes.

The REST API must publish an OpenAPI spec, and the TS SDK is generated from that spec.

Minimum v1 REST/WS surface:

- `POST /workspaces`
- `GET /workspaces/:id/status`
- `GET /refs`
- `POST /refs`
- `GET /tree/:ref/*path`
- `GET /blobs/:hash`
- `GET /binary-objects/:id`
- `POST /documents`
- `GET /documents/:id/state`
- `GET /documents/:id/snapshot`
- `POST /documents/:id/transactions`
- `POST /documents/:id/ops`
- `POST /documents/:id/presence`
- `GET /documents/:id/events`
- `POST /annotations`
- `GET /annotations?target=...`
- `POST /git/materialize`
- `POST /git/ingest`
- `GET /stats`

Deferred to v1.1:

- `GET /search?q=...`
- search indexes over explicit text files and rich-document exports; binary text extraction remains out of scope unless a later ADR opts in

### 9.2 MCP Tools

Minimum MCP tools:

- `quarry_status`
- `quarry_list`
- `quarry_read`
- `quarry_write`
- `quarry_comment`
- `quarry_start_draft`
- `quarry_publish_draft`
- `quarry_git_sync`

Normal `quarry_write` calls auto-commit a transaction. Explicit start/commit transaction tools are deferred until a real agent workflow needs multi-operation grouping.

Minimum MCP resources:

- `quarry://status`
- `quarry://refs`
- `quarry://tree/{ref}/{path}`
- `quarry://document/{id}`
- `quarry://annotations/{target}`
- `quarry://stats`

V1 search decision: no built-in API/MCP search. Search returns in v1.1 after the web UI, REST API, MCP, and Git sync are working.

## 10. Human UI

### 10.1 Web Editor

The UI should deliver:

- document explorer
- rich text and Markdown editing
- live collaboration
- comments and threaded discussions
- suggestions / redlines
- provenance rails by actor
- intent lock indicators
- draft and publish controls
- conflict draft resolution
- transaction history and time-travel views

Potion is the v1 basis for this surface. Quarry should adapt Potion's PlateJS editor, comment kit, suggestion/redline UI, discussion side rail, version-history panel, Yjs snapshot handling, and document tree patterns. The release architecture should not require a separate Next.js app server: the built web UI should be embedded in and served by `quarry server`. During development, a Potion/Next dev server can remain a convenience, but production should be one `quarry` binary.

### 10.2 Agent Bridge UX

The UI should make agent work legible:

- show which agent changed what
- group edits by transaction
- show comments a human expects an agent to address
- show "regenerated from" or "based on" derivation chains instead of flattening all new text to the final actor
- let humans accept, reject, or request revision on agent transactions

## 11. Binary Assets and Annotation Model

### 11.1 Immutable Binary Pointers

Binaries are stored as immutable pointer records. Updating a binary creates a new pointer record and a ref update, not an in-place CRDT mutation. The record should track hash, size, media type, path or URL, and optional upload/provider metadata.

### 11.2 Binary Inspection

V1 does not inspect binary contents. Quarry does not extract PDF text, page maps, outlines, thumbnails, OCR, embeddings, or binary-derived search indexes in v1.

The web UI may preview a binary if the browser can render its URL or local served path, but the preview is not canonical state and does not imply Quarry understands the file contents.

### 11.3 Binary Comments

V1 binary comments attach to the binary object or logical path as a whole. Range, page, and region selectors for PDFs/images are deferred until binary inspection is in scope.

Text and rich-document annotations still use range/selector metadata, because those documents are part of Quarry's structured state.

## 12. Memory Substrate

Quarry should support memory systems by exposing clean inputs and durable source data:

- project event stream
- actor/transaction provenance
- comments, suggestions, decisions, and conflict resolutions
- stable object IDs for citation and recall

Optional post-core memory package:

- encoding gate for noise filtering
- fact extraction
- project/user profiles
- episodic summaries
- semantic facts
- hybrid lexical/vector retrieval
- reciprocal-rank fusion
- reranking
- MCP `remember`, `recall`, and `context`

V1 memory decision: memory is a layer above Quarry. Quarry should not sync arbitrary SQLite memory DB files as opaque CRDT state. It should provide first-class events, comments, versions, refs, transactions, and stable object IDs that memory systems can consume. Search, FTS indexes, embeddings, and memory-specific retrieval hooks are deferred to v1.1 or optional integrations.

## 13. Security, Policy, and Provenance

### 13.1 Actor Identity

Every write should have an actor. In v1 the actor is self-attested by the client; Quarry records it for provenance but does not authenticate it as a security boundary.

Actor fields:

- actor ID
- display name
- kind
- optional avatar URL
- claimed auth method, if any
- Git identity mapping
- optional public key or token fingerprint when available

### 13.2 Auth

V1 auth decision:

- local CLI controls daemon with local OS permissions
- browser UI uses a local self-attested session identity
- agents may pass whatever actor identity they want
- no authorization enforcement in v1
- LAN mode is for trusted local networks only if enabled
- future team mode can add OAuth, OIDC, Tailscale identity, or mTLS

### 13.3 Policy

Default policies:

- published refs cannot be deleted by agents unless explicitly allowed
- agent writes require transactions
- bulk rewrites require intent locks
- Git imports cannot silently overwrite live work
- binary originals are immutable

V1 policy is a product guardrail, not a security boundary, because actor identity is not authenticated.

## 14. Observability and Diagnostics

Quarry must expose:

- structured logs with transaction IDs
- API `/stats`
- cache hit/miss rates
- CRDT op counts and snapshot sizes
- Git import/export status
- conflict counts
- selector resolution failures
- export/render failures
- background job status

The CLI should include:

- `quarry status`
- `quarry doctor`
- `quarry refs`
- `quarry sync`
- `quarry export`
- `quarry inspect transaction`
- `quarry compact`
- `quarry server`

## 15. Performance Targets

Draft targets for v1:

- Local API status call under 50 ms.
- File metadata listing for 10k files under 500 ms warm.
- Two local editor clients converge within 250 ms for ordinary human edits.
- Agent transaction containing 1k line edits commits as one transaction and broadcasts as one grouped update.
- Git outbound materialization for a medium project completes under 10 s warm.

V1 size policy: warn above 10 MB per structured text document and split or externalize above 50 MB. Binary records store pointers, hashes, and metadata rather than copying opaque bytes into Turso; large-file handling should follow a Git/LFS-style pointer policy. Agent transactions above 1k changed lines should be chunked or explicitly confirmed by policy.

## 16. Packaging

Deliverables:

- Rust workspace
- single `quarry` CLI binary with `quarry server`
- local web UI bundle
- generated TS SDK package from OpenAPI
- MCP server embedded in `quarry server`
- migration tooling
- import/export tooling
- test fixtures and demo workspace

## 17. Milestones

### M0: Repo and Design Skeleton

Deliver:

- Rust workspace scaffold.
- Crate boundaries.
- CLI skeleton.
- Storage traits.
- Basic config and local data directory.
- `quarry doctor`.
- Architecture docs and ADRs for CRDT, Git bridge, export/access surfaces, actor identity, and policy.

Acceptance:

- `cargo test --workspace` passes.
- CLI can initialize a workspace data directory.

### M1: Store Core

Deliver:

- Turso/libSQL-compatible SQLite metadata schema with migrations.
- Content-addressed blob store.
- Binary pointer records for large/opaque files.
- Ref store.
- Tree/path view records.
- Event log.
- Snapshot storage.
- In-memory implementation for tests.

Acceptance:

- Store tests cover create/read/update refs, blob dedupe, snapshot persistence, and migration round trips.

### M2: CRDT Document Core

Deliver:

- `CollabDoc` trait.
- Yjs/Yrs-compatible implementation aligned with Potion's PlateJS/Yjs model.
- Op schema v0.
- Transaction grouping.
- Actor/provenance metadata.
- Snapshot and restore.
- Intent lock model.

Acceptance:

- Two concurrent writers converge.
- A 1k-edit agent transaction applies as one group.
- Provenance survives snapshot/restore.

### M3: Local API and SDK

Deliver:

- Axum REST/WS API.
- OpenAPI spec.
- Generated TS SDK for state, transactions, subscriptions, and annotations.
- Minimal local web test client.
- Event streaming.

Acceptance:

- Two browser tabs edit the same document through `quarry server`.
- API and SDK tests cover transaction lifecycle and reconnect behavior.

### M4: Git Outbound

Deliver:

- Git materialization with gitoxide.
- Ref-to-commit export.
- Author preservation.
- Sidecar metadata export.
- Git LFS policy hook for large binaries.

Acceptance:

- Published ref becomes a normal Git commit.
- Commit author matches the originating human/agent/Git identity.
- External `git clone` can inspect the exported tree.

### M5: Git Inbound

Deliver:

- Fetch/receive external commits.
- Commit-to-transaction ingestion.
- One commit maps to one transaction group.
- Semantic conflict detection hooks.
- Conflict draft creation.

Acceptance:

- External Git edit appears in live Quarry state.
- Concurrent live edit plus external import creates a conflict draft when appropriate.
- Identity is preserved on round trip.

### M6: Export and Agent Access

Deliver:

- Explicit raw snapshot export.
- Git-materialized tree inspection for published refs.
- API/SDK/MCP coverage for tree listing, file reads, file writes, annotations, binary pointer records, and stats.
- Rich-document Markdown sidecar export when requested.
- CLI commands for `quarry export`, `quarry refs`, `quarry status`, and `quarry inspect transaction`.

Acceptance:

- A user can export or Git-materialize a ref and inspect source files, rich-document exports, annotation sidecars, and binary pointer records with ordinary tools.
- Agent writes through REST/MCP produce Quarry transactions.
- Published-file deletion guardrails are visible as product policy failures.

### M7: Human Review UI

Deliver:

- Document explorer.
- Rich editor integration.
- Comments.
- Suggestions/redlines.
- Provenance rails.
- Draft/publish flow.
- Intent lock display.
- Conflict draft resolution.

Acceptance:

- Human can review agent work, leave comments, request revision, and publish.
- Agent can read and respond to review comments through API/MCP.

### M8: Binary Review

Deliver:

- Immutable binary pointer support.
- Binary metadata panel in the web UI.
- Whole-binary/path-level comments.
- Git export through pointer/LFS-style metadata.

Acceptance:

- UI can attach comments to a binary object/path.
- Original binary pointer/hash remains unchanged when comments are added.
- Quarry does not inspect binary contents in v1.

### M9: MCP and Agent Integration

Deliver:

- MCP resources and tools.
- Agent transaction workflow.
- Self-attested agent identity metadata.
- REST API integration proving agent/file writes.
- Browser editor integration proving the human review loop.
- Example MCP adapter after REST plus web UI are working.

Acceptance:

- Agent can list, read, write, comment, start draft, and publish through REST/MCP.
- MCP writes preserve actor and transaction metadata.
- REST API plus the browser editor can complete a full browse/edit/comment/draft/save/commit loop.

### M10: Memory Substrate

Deliver:

- Event feed for memory systems.
- Project profile resource.
- Example memory integration.

Acceptance:

- Memory integration can consume Quarry events and cite stable source object IDs.
- If an external memory integration performs retrieval, its results can point back to files, annotations, transactions, and refs.

### M11: Network and Team Mode

Deliver:

- Explicit LAN serving mode.
- Optional peer-to-peer or hosted sync design.
- Background compaction and backup tooling.

Acceptance:

- Multiple machines can connect to a shared daemon or sync through Git without losing provenance.
- Policy failures are logged and visible.

## 18. Validation Scenarios

These scenarios define what Quarry must prove end to end. They are written from the user's point of view and should become manual demos first, then automated tests where practical.

### 18.1 First Run and Workspace Discovery

User story:

- The user runs `quarry init` in an existing project or creates a new Quarry workspace from the UI.
- The user starts `quarry server` and opens the local web UI.
- The UI shows daemon status, active workspace, current ref, Git sync state, actor identity, and recent events.
- The file explorer shows the workspace tree with folders, source files, rich documents, binaries, annotations, and binary pointer metadata when present.

Validation:

- The user can tell which workspace is active without using the terminal.
- The UI and CLI agree on workspace ID, active ref, daemon status, and Git status.
- Restarting `quarry server` preserves workspace metadata and recent events.

### 18.2 Browse Files in the Web UI

User story:

- The user opens the web UI and selects a ref such as `published/main` or `draft/<id>`.
- The left pane shows a file tree.
- Clicking a text/code/Markdown file opens a readable preview/editor.
- Clicking a rich document opens the collaborative editor.
- Clicking a binary opens a preview when the browser can render it or a metadata panel when not.
- The details pane shows object ID, path, size, last transaction, actor provenance, annotations, and Git export status.

Validation:

- File tree navigation works entirely through the web UI and API.
- Large folders remain responsive.
- Hidden Quarry internals are not mixed into the normal project tree unless the user opens a diagnostics view.
- Rich-document exports and binary pointer records are visibly linked to their source objects.

### 18.3 Edit a Text File in the Web UI

User story:

- The user opens a Markdown or source file.
- The user edits text directly in the browser.
- Quarry autosaves into a draft transaction.
- The transaction appears in the timeline with the user's actor identity.
- A second browser tab connected to the same document sees the update.
- The user can publish the change to `published/main`.

Validation:

- The edit survives browser refresh and daemon restart.
- The timeline groups the edit as one understandable transaction.
- Publishing updates the selected ref without losing draft history.
- Git materialization can turn the published ref into a normal commit.

### 18.4 Comment and Suggest Changes

User story:

- The user highlights a text range in a document.
- The user adds a comment such as "tighten this wording" or creates a suggested replacement.
- The comment appears in a side rail and in the document marker.
- The thread can be replied to, resolved, reopened, or converted into an agent task.
- An agent using MCP can list the unresolved comments, read the selected source range, and submit a revision transaction that references the comment.

Validation:

- Comments and suggestions survive concurrent edits near the selected range.
- Selectors remain anchored using semantic and positional fallbacks.
- The web UI shows which comments are unresolved, resolved, or awaiting agent response.
- The MCP/API representation contains enough context for an agent to act without scraping UI state.

### 18.5 Review Agent Work

User story:

- An agent edits one or more files through MCP, REST, or the generated SDK.
- The UI shows the agent's transaction as a grouped change, not a flood of keystrokes.
- The human opens the transaction, sees changed files, provenance rails, and any derivation links.
- The human accepts, rejects, comments on, or requests revision for the transaction.
- If accepted, the transaction can be published and materialized to Git.

Validation:

- Agent changes are attributable to the correct actor.
- The UI can explain what changed at file and range level.
- Rejection does not destroy the original transaction; it creates a new state/ref decision.
- Requesting revision produces a task/comment the agent can discover through MCP.

### 18.6 API, SDK, and Git File Access

User story:

- The user or agent lists a ref through REST/MCP/SDK.
- The user opens source files, rich-document snapshots, annotations, stats, and binary pointer records through API resources.
- The user materializes a published ref to Git or exports a raw snapshot when ordinary shell tools are needed.
- The exported snapshot includes normal source text, requested rich-document Markdown exports, annotation sidecars, and binary pointer metadata.
- Editing through REST/MCP creates a Quarry transaction visible in the web UI.

Validation:

- API, SDK, MCP, Git materialization, and raw export agree on the selected ref.
- Important v1 text state is available through explicit API resources and export outputs.
- Published-file deletion guardrails are visible as policy failures.
- `GET /stats` and `quarry status` expose cache, sync, and error telemetry.

### 18.7 Draft, Publish, and Git Export

User story:

- The user creates a draft from `published/main`.
- The user or agent makes edits in the draft.
- The user compares draft against published state.
- The user publishes the draft.
- Quarry creates or updates a Git branch/commit for the published ref.
- The user can inspect the result with ordinary Git commands outside Quarry.

Validation:

- Draft refs are visible in UI, CLI, API, and MCP.
- Publish is atomic from the user's perspective.
- Git commit author and metadata preserve the originating actor where policy allows.
- Rich document sidecars and binary pointers are exported consistently.

### 18.8 External Git Import and Conflict Draft

User story:

- An external Git commit modifies a file Quarry already tracks.
- Quarry imports the commit as a `git_import` transaction.
- If there is no semantic collision, the UI shows the imported change as part of the live state.
- If live edits collide with the import, the UI opens a conflict draft with both sides preserved.
- The user resolves the conflict and publishes the result.

Validation:

- One external Git commit maps to one Quarry transaction.
- Imported author identity is preserved.
- Quarry does not silently discard live edits or external Git changes.
- Conflict drafts are visible to agents through API/MCP/SDK and the web UI.

### 18.9 Binary Object Review

User story:

- The user adds a binary through the UI or API.
- Quarry stores an immutable pointer record with hash, size, media type, and path/URL.
- The user opens the binary metadata panel or browser-supported preview.
- The user adds a whole-file comment.
- An agent can read the binary pointer metadata and associated comments.

Validation:

- The original binary hash does not change when comments are added.
- Quarry does not extract text, page maps, thumbnails, OCR, or embeddings from the binary in v1.
- Binary comments remain readable after daemon restart.

### 18.10 Memory Context

User story:

- Quarry exposes files, documents, annotations, transactions, refs, versions, and binary pointer records.
- The memory integration can cite stable object IDs, refs, paths, and transaction IDs in its own retrieval results.

Validation:

- Memory integrations can trace facts back to source objects and visible UI locations.
- Search and retrieval are not required for core v1 file access.
- API/MCP search is deferred to v1.1.

### 18.11 Offline, Restart, and Recovery

User story:

- The user edits documents while disconnected from a network.
- The user restarts the browser and daemon.
- Quarry restores drafts, refs, comments, transactions, and exported/materialized state metadata.
- When network/Git access returns, Quarry resumes sync and reports conflicts explicitly.

Validation:

- Local-first editing does not require a remote service.
- Restart does not lose committed transactions or draft refs.
- Recovery status is visible in UI, CLI, and `GET /stats`.

### 18.12 Permissions and Unsafe Actions

User story:

- An agent attempts to delete or overwrite a published file.
- Quarry applies its product guardrail and saves/blocks according to the selected ref policy.
- The user sees the denied action in the UI event log and diagnostics.
- The user can explicitly grant a narrower permission or ask the agent to create a draft instead.

Validation:

- Policy failures are explainable and attributable to the self-attested actor.
- The denied operation does not mutate canonical state.
- The agent receives a useful error through its access path.

## 19. Test Strategy

Required test layers:

- unit tests for store, CRDT, refs, transactions, and policies
- property tests for concurrent edits and merge convergence
- integration tests for API transaction lifecycle
- Git round-trip tests using temporary repos
- browser tests for editor review flow
- snapshot/restore tests
- migration tests from every schema version
- performance benchmarks for common agent operations

Critical fixtures:

- small code repo
- Markdown-heavy repo
- rich document with comments/suggestions
- binary pointer with whole-file comments
- conflicting Git import
- high-volume agent rewrite

End-to-end validation should cover the scenarios in section 18 with a mix of Playwright browser tests, temp Git repos, API tests, MCP harnesses, SDK tests, and export checks.

## 20. Documentation Deliverables

Deliver:

- `README.md`: product overview and quick start.
- `docs/architecture.md`: system architecture.
- `docs/store.md`: storage model and schema.
- `docs/crdt.md`: CRDT abstraction and op schema.
- `docs/git-bridge.md`: import/export and conflict policy.
- `docs/export-and-access.md`: API/MCP/SDK access, Git materialization, raw export, and deferred projection notes.
- `docs/api.md`: REST/WS and MCP.
- `docs/provenance-policy.md`: self-attested actor model and policy guardrails.
- `docs/memory.md`: memory substrate and integration contract.
- `docs/adr/`: decision records.

## 21. Accepted Defaults

The implementation can proceed with these defaults:

1. V1 is Yjs/Yrs-first for structured documents because Potion is the licensed PlateJS/Yjs editor base. Keep `CollabDoc` so Loro can be evaluated later.
2. V1 runs one `quarry server` process per developer machine and syncs through Git first.
3. Severe Git import conflicts preserve both versions for human resolution.
4. V1 ships Quarry as a substrate, not as a built-in memory product.
5. V1 does not include built-in API/MCP search. Search returns in v1.1.
6. V1 treats binaries and opaque assets as pointer records. No OCR, PDF/text extraction, region annotations, binary-derived text indexes, or annotated binary export in v1.
7. V1 has no auth or authorization boundary. Clients may pass self-attested actor identity, including optional avatar URL.
8. Structured documents warn above 10 MB and split or externalize above 50 MB. Binary pointer records can reference large files; actual large-file storage policy is Git/LFS-style pointer based.
9. Ephemeral refs/workspaces are cut from v1.
10. FUSE, sync-folder projection, SMB/Samba, and mounted filesystem views are deferred. V1 uses REST/OpenAPI, MCP, the generated SDK, the web UI, Git materialization, and raw export.
11. First integration targets are the REST/OpenAPI API and web UI for browsing, draft/collaboration, save/commit, and two-way Git sync. MCP adapters follow after that loop works.

No remaining question blocks the first implementation pass. Later ADRs can revisit Loro, hosted/team mode, search, binary inspection, SMB, and real auth once the v1 spine is working.
