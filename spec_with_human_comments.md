# Quarry Product and Engineering Spec

Status: draft v0  
Sources: `docs/deep-research/*.md` and the mirrored corpus under `references/deep-research/`

## 1\. Product Definition

Quarry is a local-first collaborative storage substrate for humans and AI agents. It gives the same project state to rich editors, command-line agents, Git remotes, filesystem tools, and memory systems without forcing every actor through the same interface.

The core product is a daemon, `quarryd`, backed by immutable content objects, mutable refs, CRDT-backed structured documents, first-class annotations, Git import/export, filesystem projections, HTTP/WebSocket APIs, and MCP tools.

Quarry is not "Git with collaboration bolted on." Git is an interoperability and history projection. The canonical source of truth is Quarry's local store: content-addressed blobs, op logs, snapshots, refs, provenance, and derived searchable views.

## 2\. Goals

Quarry should deliver:

- A durable local-first object store for project files, rich documents, annotations, extracted text, and binary assets.  
- Real-time multi-actor editing between humans, agents, and Git imports.  
- A two-way Git bridge that preserves normal Git workflows while keeping richer collaboration metadata outside Git.  
- A filesystem projection that agents can use with ordinary tools like `ls`, `cat`, `rg`, LSPs, formatters, and test runners.  
- A web/editor surface for human review, comments, suggestions, provenance, and draft-to-publish workflows.  
- An MCP and REST/WS API surface that agents can use directly without pretending every action is a file write.  
- A substrate for memory products, not a monolithic memory app in v1.  
- Searchable text derivatives for binaries and rich documents so agent search works by default.  
- Explicit provenance for every mutation: human, agent, Git import, or system.

## 3\. Non-Goals

These are intentionally out of scope for the first production-quality release:

- Replacing Git as a global ecosystem. Quarry must interoperate with Git, not require the world to adopt Quarry remotes.  
- Byte-level CRDT editing of arbitrary binary files. Binaries are immutable blobs; collaboration happens in overlays and derived metadata.  
- A fully hosted multi-tenant SaaS control plane.  
- A general-purpose memory product competing with Supermemory, TrueMemory, Mem0, Zep, or AgentMemory.  
- Perfect lossless round-tripping of rich documents through plain Git files. Git exports should be useful and stable, but some rich editor metadata will live in sidecars or Quarry-only metadata.  
- Windows FUSE support in v1. Windows can get sync-folder export first.

## 4\. Target Workflows

### 4.1 Agent Coding Session

An agent mounts or opens a Quarry workspace, reads files through the filesystem projection or MCP, starts a transaction, edits a bounded set of files, commits the transaction, and publishes or leaves the work as a draft. Other actors see one coherent update instead of thousands of keystroke-level events.

Human Feedback: When docs are edited via API/MCP or filesystem mount, I think the commit is implicit/automatic (no draft, no collaboration) 

Transactions by default are good

### 4.2 Human Review of Agent Work

A human opens Quarry's editor UI, sees provenance rails showing what was produced by an agent, leaves inline comments or suggestions, and saves those comments as a draft ref. An agent can read the draft and produce a follow-up transaction. Publishing resolves the draft into a stable ref and optionally materializes it as a Git commit.

### 4.3 Two-Way Git Sync

Quarry materializes published refs into normal Git commits. If an external Git commit appears, Quarry ingests it as a transaction attributed to the Git author. If the import collides semantically with live work, Quarry opens a conflict draft rather than silently overwriting either side.

### 4.4 Binary Review

Binaries are stored as an immutable blob. Quarry does not extract text, outlines, page maps, thumbnails, and metadata into searchable derivatives. Comments and highlights are stored as annotation objects. The original blob remains unchanged.

### 4.5 Memory Layer on Top

Memory systems consume Quarry events, searchable derivatives, project metadata, and user/agent annotations. The first Quarry release should expose enough substrate for hybrid retrieval and project memory, while leaving advanced memory pipelines as optional packages or integrations.

## 5\. System Architecture

### 5.1 Process Shape

Quarry ships as a single local daemon:

- `quarryd`: owns lifecycle, config, local data directory, API listeners, background jobs, Git sync, and projection workers.  
- Local web UI: connects to `quarryd` over HTTP/WS. Embedded in the Rust binary  
- TS SDK: Generated from OpenAPI Spec  
- CLI: controls daemon lifecycle, workspace init, mount, sync, status, and diagnostics.

Default deployment is one `quarryd` per developer machine. It may serve multiple local clients and can optionally expose LAN access behind auth. Team synchronization should initially happen through Git and later through peer-to-peer or hosted sync.

### 5.2 Rust Workspace

The implementation should be split into small crates with clear ownership:

- `quarry-store`: filesystem and SQLite access only. Owns content-addressed blobs, metadata schema, op-log persistence, snapshot storage, and migrations.  
- `quarry-crdt`: collaborative document abstraction, CRDT engine integration, op schema, transaction grouping, provenance, draft/publish semantics.  
- `quarry-git`: bidirectional Git bridge using gitoxide. Owns `materialize` and `ingest`.  
- `quarry-projection`: derived read/write views, FUSE adapter, sync-folder export, raw export, searchable materializations.  
- `quarry-api`: Axum HTTP, WebSocket, SSE, MCP, git smart-HTTP, auth, ACLs, and transaction coordination.  
- `quarry-daemon`: binary wiring, config, logging, background jobs, and lifecycle.  
- `quarry-sdk-ts`: browser/Node client, WS subscriptions, typed API helpers, optional WASM bridge.  
- `quarry-ui`: web review/editor surface.

### 5.3 Storage Model

Quarry's canonical records:

- `Blob`: immutable bytes addressed by content hash.  
- `Tree`: directory/file view mapping logical paths to object IDs or refs.  
- `StructuredDoc`: CRDT-backed rich or plain document state.  
- `Annotation`: first-class comment, suggestion, highlight, critique, or provenance note attached to a blob, document, ref, path, range, or selector.  
- `Ref`: mutable named pointer such as `published/main`, `draft/<id>`, `git/<branch>`, `local/<user>`, or `ephemeral/<actor>`. (Human feedback: We are cutting ephemeral)  
- `Transaction`: grouped mutation with actor, timestamps, affected paths/ranges, intent lock metadata, and optional Git identity.  
- `Actor`: human, agent, Git import, system, or integration.  
- `DerivedArtifact`: extracted text, page maps, outlines, thumbnails, AST summaries, normalized diffs, and indexes.  
- `Event`: append-only audit and sync event.

The store uses Turso with WAL mode for metadata and op indexes. Blob bytes live on local disk in a sharded content-addressed layout. The binaries are basically like git lfs where we are just storing the pointer/path

## 6\. CRDT and Collaboration

### 6.1 Draft Default

Draft v0 assumes Loro as the core CRDT because Quarry is Rust-native, needs high-throughput agent transactions, benefits from moveable tree/rich text support, and can expose WASM/TS bindings. The implementation must keep a `CollabDoc` abstraction so Yjs/Yrs can be swapped in or used for editor compatibility if PlateJS integration becomes the dominant constraint.

Decision question: should the v1 core be Loro-first, or should we prioritize Yjs/Yrs because PlateJS and existing editor bindings are more important than Rust-native internals?  
Human feedback: look at the potion codebase and decide about this

### 6.2 Transaction Model

Every write must occur inside a transaction:

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

### 6.3 Intent Locks

Agents and integrations can request soft intent locks for affected paths or ranges. Locks are advisory for humans and coordinating agents, but writes outside the owner should create conflict events. Locks expire automatically and must be visible in the UI/API.

### 6.4 Presence and Ephemeral State

Presence, cursors, live typing, intent previews, and temporary review cues are transient. They must not be stored in durable document history unless explicitly committed as an annotation or transaction.

## 7\. Git Bridge

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

Draft default: preserve both sides and surface a conflict draft for human resolution. Do not silently let Git imports overwrite live work, and do not force-push local Quarry state over external Git without explicit policy.

Decision question: should severe Git import conflicts default to conflict drafts, imported-wins, live-wins, or repo-configurable policy?  
Human Feedback: v1 maybe it just saves both. Future could call an LLM to resolve

## 8\. Filesystem Projection

### 8.1 FUSE

`quarry-projection` exposes a POSIX-like view through FUSE on macOS and Linux:

- `read`: resolves current ref/path to a materialized byte view.  
- `write`: buffers the new content, computes a structured diff, and commits a transaction.  
- `rename`: updates path views without changing stable object identity.  
- `delete`: allowed only by policy; published/human-approved refs are protected by default.  
- `readdir` and `stat`: served from indexed metadata and caches.  
- `flush`/`fsync`: transaction boundary hooks.

The FUSE mount is a projection, not the canonical database.

### 8.2 Searchable Agent View

Anything agents are expected to find with plain `rg` must exist as visible text. Quarry must provide an agent-friendly view that includes:

- source files  
- Markdown exports of rich documents  
- extracted text for PDFs and other binaries  
- sidecar annotation text  
- generated outlines and summaries  
- `.quarry/stats/*` telemetry files

Ignored or hidden scratch areas must not be the only location where important agent-visible state exists.

### 8.3 Ephemeral Workspaces (Deferred)

### 8.4 Sync Folder and SMB

FUSE is the v1 local projection. A sync-folder projection should be offered for environments where FUSE is unavailable. SMB/Samba or a userspace SMB server is a later network-share deliverable for containers, VMs, and remote agents that cannot mount FUSE.

## 9\. API and MCP Surface

### 9.1 REST/WS Routes

The API should borrow Proof's separation of document state, snapshots, edits, ops, presence, comments, suggestions, and agent bridge routes.

Minimum REST/WS surface:

- `POST /workspaces`  
- `GET /workspaces/:id/status`  
- `GET /refs`  
- `POST /refs`  
- `GET /tree/:ref/*path`  
- `GET /blobs/:hash`  
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
- `GET /search?q=...`  
- `GET /stats`

### 9.2 MCP Tools

Minimum MCP tools:

- `quarry_status`  
- `quarry_list`  
- `quarry_read`  
- `quarry_write`  
- `quarry_search`  
- `quarry_start_transaction`  
- `quarry_commit_transaction`  
- `quarry_comment`  
- `quarry_start_draft`  
- `quarry_publish_draft`  
- `quarry_git_sync`

Minimum MCP resources:

- `quarry://status`  
- `quarry://refs`  
- `quarry://tree/{ref}/{path}`  
- `quarry://document/{id}`  
- `quarry://annotations/{target}`  
- `quarry://stats`

Decision question: should semantic search be part of MCP v1, or should v1 expose lexical/FTS search only and leave semantic search to the memory layer?  
Human feedback: Defer search to v1.1

## 10\. Human UI

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

PlateJS is the likely editor shell because the research points to mature comment, suggestion, Yjs, Markdown, and DOCX support. If Quarry chooses Loro as the core CRDT, the UI work must either bridge Loro to the editor model or isolate editor collaboration behind the `CollabDoc` interface.

Human feedback: Potion can form the basis for the web editor. Potion is PlatJS-based app that we licensed. 

### 10.2 Agent Bridge UX

The UI should make agent work legible:

- show which agent changed what  
- group edits by transaction  
- show comments a human expects an agent to address  
- show "regenerated from" or "based on" derivation chains instead of flattening all new text to the final actor  
- let humans accept, reject, or request revision on agent transactions

## 11\. Binary Assets and Annotation Model

### 11.1 Immutable Binary Blobs

Binaries are stored as immutable blobs. Updating a binary creates a new blob and a ref update, not an in-place CRDT mutation.

### 11.2 Derived Text and Metadata

For each supported binary type, Quarry should generate:

- extracted text  
- page or frame map  
- outline/table of contents when possible  
- thumbnails/previews  
- metadata  
- search index rows

PDF support should come first.

### 11.3 Annotation Selectors

Annotations must support both semantic and positional anchoring:

- text quote selector with prefix/suffix context  
- text position selector  
- range selector  
- page number plus rectangle for PDFs/images  
- SVG selector or normalized geometry for visual regions  
- target object/ref/path metadata

Decision question: should v1 support overlay-only annotation export, or must annotations round-trip back into PDFs immediately?

## 12\. Memory Substrate

Quarry should support memory systems by exposing clean inputs and durable source data:

- project event stream  
- actor/transaction provenance  
- extracted text and metadata  
- comments, suggestions, decisions, and conflict resolutions  
- FTS5 search indexes  
- optional embeddings table owned by a memory plugin  
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

Draft default: memory is a layer above Quarry for v1. Quarry should not sync arbitrary SQLite memory DB files as opaque CRDT state. It should provide first-class events and text artifacts that memory systems consume.

Decision question: do we want a built-in memory package in the first public release, or should v1 stop at the substrate and MCP/search hooks?

## 13\. Security, Policy, and Provenance

### 13.1 Actor Identity

Every write must have an actor. Anonymous writes are only allowed in explicitly local/test configurations.

Actor fields:

- actor ID  
- display name  
- kind  
- auth method  
- Git identity mapping  
- allowed scopes  
- public key or token fingerprint when available

Human feedback: Add optional avatar URL when available

### 13.2 Auth

Draft default:

- local CLI controls daemon with local OS permissions  
- browser UI uses local session token  
- agents use scoped bearer tokens or local socket credentials  
- LAN mode requires explicit config and token auth  
- future team mode can add OAuth, OIDC, Tailscale identity, or mTLS

Decision question: for v1 LAN/team use, should auth be static bearer tokens, local OS/Tailscale identity, OAuth/OIDC, or mTLS?  
Human Feedback: No auth for now, you can pass whatever identity you want

### 13.3 Policy

Default policies:

- published refs cannot be deleted by agents unless explicitly allowed  
- agent writes require transactions  
- bulk rewrites require intent locks  
- Git imports cannot silently overwrite live work  
- scratch refs are excluded from Git export unless explicitly included  
- binary originals are immutable

## 14\. Observability and Diagnostics

Quarry must expose:

- structured logs with transaction IDs  
- `.quarry/stats` projection for agent-readable telemetry  
- API `/stats`  
- cache hit/miss rates  
- FUSE read/write latency  
- CRDT op counts and snapshot sizes  
- Git import/export status  
- conflict counts  
- selector resolution failures  
- search skip reasons  
- extraction failures  
- background job status

The CLI should include:

- `quarry status`  
- `quarry doctor`  
- `quarry mount`  
- `quarry unmount`  
- `quarry refs`  
- `quarry sync`  
- `quarry export`  
- `quarry inspect transaction`  
- `quarry compact`

## 15\. Performance Targets

Draft targets for v1:

- Local API status call under 50 ms.  
- File metadata listing for 10k files under 500 ms warm.  
- FUSE read of cached small files under 20 ms p95.  
- Two local editor clients converge within 250 ms for ordinary human edits.  
- Agent transaction containing 1k line edits commits as one transaction and broadcasts as one grouped update.  
- Search over visible text artifacts for a medium project returns within 1 s warm.  
- Git outbound materialization for a medium project completes under 10 s warm.

Decision question: what document and binary size ceilings should v1 enforce? Draft defaults: warn above 10 MB per structured text document, split or externalize above 50 MB, allow binary blobs up to 200 MB before requiring explicit large-file policy.

## 16\. Packaging

Deliverables:

- Rust workspace  
- `quarryd` binary  
- `quarry` CLI  
- local web UI bundle  
- TS SDK package  
- MCP server embedded in `quarryd`  
- FUSE projection for macOS/Linux  
- sync-folder projection fallback  
- migration tooling  
- import/export tooling  
- test fixtures and demo workspace

Human feedback: Combine quarryd and quarry binaries into a single CLI use “quarry server …” to replace quarryd

## 17\. Milestones

### M0: Repo and Design Skeleton

Deliver:

- Rust workspace scaffold.  
- Crate boundaries.  
- CLI skeleton.  
- Storage traits.  
- Basic config and local data directory.  
- `quarry doctor`.  
- Architecture docs and ADRs for CRDT, Git bridge, projection, and auth.

Acceptance:

- `cargo test --workspace` passes.  
- CLI can initialize a workspace data directory.

### M1: Store Core

Deliver:

- SQLite metadata schema with migrations.  
- Content-addressed blob store.  
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
- Loro-backed implementation, unless the CRDT decision changes.  
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
- TS SDK for state, transactions, subscriptions, and annotations.  
- Minimal local web test client.  
- Event streaming.

Acceptance:

- Two browser tabs edit the same document through `quarryd`.  
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

### M6: Filesystem Projection

Deliver:

- FUSE mount on macOS/Linux.  
- Read/write/rename/delete/stat/readdir support.  
- Agent-readable `.quarry/stats`.  
- Searchable agent view.  
- Local scratch namespace.  
- Sync-folder fallback.

Acceptance:

- `rg` finds ordinary files, rich-doc exports, annotations, and extracted PDF text.  
- Agent writes through the mount produce Quarry transactions.  
- Published files are protected from unauthorized agent deletion.

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

- Immutable PDF blob support.  
- Text extraction.  
- Page maps.  
- Annotation overlays.  
- Searchable derived text.  
- Optional annotated export artifact.

Acceptance:

- PDF text is searchable from agent view and API search.  
- UI can attach comments/highlights to PDF regions.  
- Original binary remains unchanged.

### M9: MCP and Agent Integration

Deliver:

- MCP resources and tools.  
- Agent transaction workflow.  
- Scoped agent tokens.  
- Example integrations for Codex/Claude-style agents.

Acceptance:

- Agent can list, read, search, write, comment, start draft, and publish through MCP.  
- MCP writes preserve actor and transaction metadata.

### M10: Memory Substrate

Deliver:

- Event feed for memory systems.  
- Optional local FTS5 index.  
- Optional embeddings plugin interface.  
- Project profile resource.  
- Example memory integration.

Acceptance:

- Memory integration can consume Quarry events and cite stable source object IDs.  
- Query results can point back to files, annotations, transactions, and refs.

### M11: Network and Team Mode

Deliver:

- Explicit LAN serving mode.  
- Hardened auth.  
- Team permission policies.  
- Optional SMB/userspace network share.  
- Background compaction and backup tooling.

Acceptance:

- Multiple machines can connect to a shared daemon or sync through Git without losing provenance.  
- Auth and policy failures are logged and visible.

## 18\. Validation Scenarios

These scenarios define what Quarry must prove end to end. They are written from the user's point of view and should become manual demos first, then automated tests where practical.

### 18.1 First Run and Workspace Discovery

User story:

- The user runs `quarry init` in an existing project or creates a new Quarry workspace from the UI.  
- The user starts `quarryd` and opens the local web UI.  
- The UI shows daemon status, active workspace, current ref, Git sync state, mount state, actor identity, and recent events.  
- The file explorer shows the workspace tree with folders, source files, rich documents, binaries, annotations, and generated/searchable derivatives when present.

Validation:

- The user can tell which workspace is active without using the terminal.  
- The UI and CLI agree on workspace ID, active ref, daemon status, and Git status.  
- Restarting `quarryd` preserves workspace metadata and recent events.

### 18.2 Browse Files in the Web UI

User story:

- The user opens the web UI and selects a ref such as `published/main` or `draft/<id>`.  
- The left pane shows a file tree.  
- Clicking a text/code/Markdown file opens a readable preview/editor.  
- Clicking a rich document opens the collaborative editor.  
- Clicking a binary opens a preview when supported or a metadata panel when not.  
- The details pane shows object ID, path, size, last transaction, actor provenance, annotations, and Git export status.

Validation:

- File tree navigation works without requiring a mounted filesystem.  
- Large folders remain responsive.  
- Hidden Quarry internals are not mixed into the normal project tree unless the user opens a diagnostics view.  
- Searchable derivatives are visibly linked to the source binary or rich document.

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

- An agent edits one or more files through MCP, REST, or the filesystem mount.  
- The UI shows the agent's transaction as a grouped change, not a flood of keystrokes.  
- The human opens the transaction, sees changed files, provenance rails, and any derivation links.  
- The human accepts, rejects, comments on, or requests revision for the transaction.  
- If accepted, the transaction can be published and materialized to Git.

Validation:

- Agent changes are attributable to the correct actor.  
- The UI can explain what changed at file and range level.  
- Rejection does not destroy the original transaction; it creates a new state/ref decision.  
- Requesting revision produces a task/comment the agent can discover through MCP.

### 18.6 Filesystem Mount and Agent Search

User story:

- The user runs `quarry mount ./quarry-mount`.  
- The mounted directory appears in Finder and the terminal.  
- The user can run `ls`, `cat`, `rg`, formatters, and test commands against the mount.  
- `rg` finds normal source text, rich-document exports, annotation sidecars, and extracted PDF text.  
- Editing a mounted text file creates a Quarry transaction visible in the web UI.

Validation:

- The mounted view is consistent with the active ref.  
- Important agent-readable state is visible to plain `rg` without requiring `rg -uuu`.  
- Published files reject unauthorized deletion by agents.  
- `.quarry/stats` exposes cache, projection, sync, and error telemetry as readable files.

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
- Conflict drafts are searchable and visible to agents.

### 18.9 Binary PDF Review

User story:

- The user adds a PDF through the UI or filesystem mount.  
- Quarry stores the original PDF as an immutable blob.  
- Quarry extracts text and page metadata.  
- The user opens the PDF in the web UI, highlights a region, and adds a comment.  
- An agent can search the extracted text and read the annotation target.  
- The user can export an annotated copy if that feature is enabled.

Validation:

- The original binary hash does not change when comments are added.  
- Extracted text appears in search and the agent-readable projection.  
- PDF annotations have page/range/geometry metadata.  
- Overlay annotations remain readable after daemon restart.

### 18.10 Memory and Search Context

User story:

- The user searches from the UI or asks an agent to search through MCP.  
- Quarry returns files, documents, annotations, transactions, and extracted binary text.  
- A memory integration consumes Quarry events and builds project context.  
- Search results cite stable object IDs, refs, paths, and transaction IDs.

Validation:

- Lexical search works without a memory plugin.  
- Optional semantic search or memory plugins can enrich results without becoming required for core file access.  
- Results can be traced back to source objects and visible UI locations.

### 18.11 Offline, Restart, and Recovery

User story:

- The user edits documents while disconnected from a network.  
- The user restarts the browser and daemon.  
- Quarry restores drafts, refs, comments, transactions, and mount-visible state.  
- When network/Git access returns, Quarry resumes sync and reports conflicts explicitly.

Validation:

- Local-first editing does not require a remote service.  
- Restart does not lose committed transactions or draft refs.  
- Recovery status is visible in UI, CLI, and `.quarry/stats`.

### 18.12 Permissions and Unsafe Actions

User story:

- An agent attempts to delete or overwrite a published human-approved file.  
- Quarry denies the action by policy.  
- The user sees the denied action in the UI event log and diagnostics.  
- The user can explicitly grant a narrower permission or ask the agent to create a draft instead.

Validation:

- Policy failures are explainable and attributable.  
- The denied operation does not mutate canonical state.  
- The agent receives a useful error through its access path.

## 19\. Test Strategy

Required test layers:

- unit tests for store, CRDT, refs, transactions, and policies  
- property tests for concurrent edits and merge convergence  
- integration tests for API transaction lifecycle  
- Git round-trip tests using temporary repos  
- FUSE smoke tests gated by host support  
- browser tests for editor review flow  
- snapshot/restore tests  
- migration tests from every schema version  
- performance benchmarks for common agent operations

Critical fixtures:

- small code repo  
- Markdown-heavy repo  
- rich document with comments/suggestions  
- PDF with extracted text and annotations  
- conflicting Git import  
- high-volume agent rewrite

End-to-end validation should cover the scenarios in section 18 with a mix of Playwright browser tests, temp Git repos, API tests, MCP harnesses, and FUSE smoke tests gated by host support.

## 20\. Documentation Deliverables

Deliver:

- `README.md`: product overview and quick start.  
- `docs/architecture.md`: system architecture.  
- `docs/store.md`: storage model and schema.  
- `docs/crdt.md`: CRDT abstraction and op schema.  
- `docs/git-bridge.md`: import/export and conflict policy.  
- `docs/projections.md`: FUSE, sync folder, search view.  
- `docs/api.md`: REST/WS and MCP.  
- `docs/security.md`: auth, actor model, policy.  
- `docs/memory.md`: memory substrate and integration contract.  
- `docs/adr/`: decision records.

## 21\. Open Questions

Please answer these before implementation locks in:

1. Is the default deployment one daemon per developer synced through Git, or one team/LAN daemon that multiple developers connect to?  One daemon per developer synced through Git  
2. For severe Git import conflicts, should the default be conflict draft, imported-wins, live-wins, or repo-configurable? On conflict, keep both versions  
3. Should v1 include a built-in memory package, or only expose the substrate/events/search hooks memory systems need? Only expose substrate  
4. Should v1 semantic search be built into Quarry, or should v1 ship lexical/FTS search and leave semantic retrieval to the memory layer? No search in V1  
5. Should PDF annotations be overlay-only in v1, or must they write back into exported PDFs immediately? Quarry does not inspect binary data, so this is N/A.  
6. What auth model do you want for LAN/team use: static bearer tokens, local OS/Tailscale identity, OAuth/OIDC, or mTLS? No auth in V1.  
7. What size ceilings should v1 enforce for structured documents, binary blobs, and agent transactions? Use your judgement.  
8. Should SMB/Samba be part of v1, or is FUSE plus sync-folder enough for the first release? Defer SMB/Samba.  
9. Which client should be the first real integration target: Codex MCP, Claude Code, Cursor, a browser editor, or Git CLI workflows? REST API \+ web ui for browsing, draft/collaboration then save/commit. Two way git sync.

