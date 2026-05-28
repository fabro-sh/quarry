# Quarry Browser Spec
## Purpose
Quarry Browser is the first full user-facing web workspace built on top of the Quarry daemon. It turns the phase-one document substrate into a browser-based, Obsidian-style knowledge workspace while preserving Quarry's core guarantees: documents are stored in the local Quarry server, REST is the canonical control surface, writes are versioned and conditional, Git/FUSE remain interoperable surfaces, and conflicts are explicit rather than hidden.

The browser is not a hosted notes product in this phase. It is a local-first web interface served by the same Quarry process, designed for one user working with one local daemon by default.
## Research Basis
This spec is based on:

- Existing phase-one repo behavior in `spec.md`, `README.md`, and `docs/operations/*.md`.
  
- Browser research in `.ai/research/browser/claude.md`, `.ai/research/browser/gemini.md`, and `.ai/research/browser/gpt.md`.
  
- Downloaded research resources in `.ai/research/browser/resources/`.
  

The research converges on a source-first markdown editor, not a WYSIWYG block editor. CodeMirror 6 with custom markdown decorations is the right foundation because the underlying document remains plain text and visual rendering is an overlay. That matches Obsidian Live Preview, SilverBullet, and Zettlr better than TipTap, Milkdown, Lexical, Slate, or BlockNote.
## Product Outcome
At the end of the full browser effort, a user can:

- Start `quarry serve` and open a local web workspace from the same daemon.
  
- Select a Library and browse its documents as a familiar folder tree derived from Quarry's path keys.
  
- Open, read, edit, rename, move, create, and delete markdown documents.
  
- Edit markdown in a source-first live-preview editor where syntax is hidden only when the cursor is outside the relevant span.
  
- Use `[[wikilinks]]`, markdown links, embeds, headings, tags, and backlinks as first-class navigation tools.
  
- Search the Library by path, title, and document body.
  
- See outgoing links, backlinks, unresolved links, and a graph view.
  
- Resolve save conflicts and Git sync conflicts without data loss.
  
- Inspect version history, compare versions, and restore an old version as a new head.
  
- See changes made through REST, FUSE, or Git sync appear in the browser without a full manual refresh.
  
- Preview images and common binary documents stored in Quarry's hybrid inline/CAS storage.
  
- Use keyboard-first navigation, command palette actions, and accessible core workflows.
  
## Design Center
- Local-first, single-user, same-daemon web workspace.
  
- Markdown source is the canonical editing representation.
  
- REST remains the source of truth for browser state and mutations.
  
- The browser never writes directly to TursoDB, CAS, Git, or the FUSE mount.
  
- Browser state is a cache or draft layer, never authoritative storage.
  
- Concurrency uses existing ETag and `If-Match` semantics.
  
- Conflicts are surfaced as workflows, not silent last-writer-wins behavior.
  
- Git sync remains explicit. The browser may trigger sync operations, but it must not make background Git sync ambient or surprising.
  
- The UI should feel like a quiet productivity tool: dense, fast, keyboardable, and direct. It is not a marketing surface.
  
## Product Decisions
These decisions are fixed for MVP and v1 unless a later implementation plan finds a concrete blocker:

- MVP uses explicit save plus local draft recovery. Debounced autosave is later polish, not required for MVP.
  
- Generated API client/types are committed under `ui/src/api/generated` and can be regenerated with a package script.
  
- Empty directories are not first-class browser objects in MVP. The tree is derived from document paths. A directory API can be added later if empty directories need to appear outside FUSE.
  
- Raw document HTML is disabled by default in browser preview. Mermaid, math, and rich code-fence rendering are optional later widgets, not required for MVP or v1 acceptance.
  
- Graph coordinates and layout preferences are stored in browser-local state for v1. Server-side graph layout metadata is deferred until sharing or multi-device workspace sync exists.
  
- Conflict sibling cleanup is not part of MVP or v1. Users can delete conflict sibling documents explicitly after resolving.
  
- Non-loopback browser access remains warning-only in MVP and v1. A remote auth feature gate is later work.
  
- React Router and TanStack Router are both acceptable, but the implementation plan must choose exactly one before scaffolding the UI.
  
## Non-Goals
These are out of scope for MVP and v1 unless explicitly promoted later:

- Hosted multi-user service.
  
- Remote account system, teams, sharing, or ACLs.
  
- CRDT collaborative editing.
  
- Plugin API.
  
- AI-assisted note writing or agent operations.
  
- Browser-side direct database access.
  
- Replacing FUSE or Git as interoperability surfaces.
  
- Mobile-first editing UX.
  
- Full offline editing with a durable mutation outbox.
  
- Rich block database editing.
  
- Treating markdown serialization as secondary to a WYSIWYG tree.
  
## Phased Scope
### MVP
MVP proves the browser can operate as a real Quarry workspace over the current REST substrate with only the smallest backend additions needed for a usable web app.

MVP includes:

- React SPA shell reachable from the local Quarry daemon in dev or embedded mode.
  
- Library list and active Library selection.
  
- Document tree built from `GET /v1/libraries/{library}/documents`.
  
- Open/read document view.
  
- CodeMirror 6 markdown editor.
  
- Create, edit, save, rename/move, and delete documents.
  
- ETag-aware writes using `If-Match` and `If-None-Match: *`.
  
- Dirty/saving/saved/error/conflict state in the UI.
  
- Local draft persistence for unsaved editor content.
  
- Basic quick-open by path/title from the loaded document list.
  
- Basic document search if a backend endpoint exists; otherwise path/title search only.
  
- Backlinks/outgoing links panel if link indexing is present; otherwise a placeholder that makes the missing backend capability explicit.
  
- Conflict visibility for current phase-one conflict records.
  
- Binary/image preview using existing document content and content type.
  
- Generated TypeScript client from `/v1/openapi.json`.
  
- Browser tests covering core read/edit/save/conflict flows.
  

MVP does not require:

- Full-body ranked search.
  
- Live SSE updates.
  
- Graph view.
  
- Version history UI.
  
- Offline mutation outbox.
  
- Collaborative editing.
  
- Remote auth.
  

MVP exit criteria:

- `cargo run -p quarry -- serve --addr 127.0.0.1:7831` can serve the browser in embedded mode or proxy to the Vite dev server in development mode.
  
- Opening the browser shows Libraries from the live daemon.
  
- A user can open an existing markdown document, edit it, save it, reload the page, and see the saved content.
  
- A stale ETag save produces a conflict workflow instead of overwriting.
  
- Tree navigation remains responsive with at least 10,000 document paths in the Library.
  
- Keyboard-only users can select a Library, open a document, edit, save, and search by path.
  
### v1
v1 turns the MVP into a complete Obsidian-style local workspace.

v1 includes:

- Source-first live-preview markdown decorations in CodeMirror 6.
  
- `[[wikilink]]`, `![[embed]]`, `#tag`, heading, markdown link, image, blockquote, list, emphasis, code fence, and frontmatter handling.
  
- Server-side link extraction on every write path: REST, FUSE, Git import, Git pull/sync, and transaction commit.
  
- Backlinks, outgoing links, unresolved links, and hover previews.
  
- Server-side full-text search using Turso native FTS if supported by the repository-pinned Turso version, with a Tantivy fallback behind the same API if Turso FTS is not ready.
  
- Ranked full-body search, path search, and suggestion endpoints.
  
- SSE event stream for changes from REST, FUSE, Git sync, reindexing, and conflict creation/resolution.
  
- Version history list, specific-version fetch, diff, and restore.
  
- Conflict resolution UI for save conflicts and Git sync conflicts.
  
- Graph view using sigma.js, graphology, and worker-based layout.
  
- Command palette with quick open, search, create, rename, move, delete, sync, and settings actions.
  
- Right pane with backlinks, outgoing links, properties, versions, conflicts, and local graph tabs.
  
- Multi-library switcher.
  
- Image preview and generic binary metadata/download view.
  
- Dark mode and persisted workspace layout.
  
- Accessibility target of WCAG 2.2 AA for core workflows.
  
- Performance budget and browser smoke suite.
  

v1 exit criteria:

- Full-text search returns ranked snippets from markdown document bodies.
  
- Saving a document updates link indexes and search indexes transactionally with the document version.
  
- A change written through FUSE while the browser is open invalidates the right browser state through SSE.
  
- Git sync conflicts appear in the browser and can be resolved without editing conflict marker text.
  
- A user can inspect version history, diff two versions, and restore one as a new head.
  
- The graph view can render at least 10,000 nodes or gracefully narrow to a focused neighborhood with an explicit UI message when a full graph would be too large.
  
- The production browser bundle is served by the Quarry binary with SPA fallback routing and sane cache headers.
  
### Later
Later work can build on the v1 contracts without changing the core browser architecture.

Candidates:

- Full offline editing with IndexedDB metadata cache and mutation outbox.
  
- CRDT collaboration with CodeMirror + Yjs if multi-user editing becomes a real product requirement.
  
- Remote auth using a same-origin Backend-for-Frontend and httpOnly cookies.
  
- Plugin API for custom commands, panels, markdown widgets, and graph filters.
  
- Published/read-only sharing surfaces.
  
- AI-assisted note operations and agent review workflows.
  
- Mobile-responsive editing refinements.
  
- Per-library themes and CSS overrides.
  
- Advanced search facets, saved searches, and task aggregation.
  
## Core Workflows
### Library Selection
When the browser opens, it fetches `GET /v1/libraries`. If there is exactly one Library, the browser opens it. If there are multiple Libraries, the browser shows a compact switcher with recent Libraries and a create-Library action.

The active Library is reflected in the route and persisted locally as a browser preference. If the persisted Library no longer exists, the browser falls back to the Library list.
### Tree Navigation
The browser builds a tree from document paths. Directories remain derived from path prefixes, matching Quarry's phase-one storage model. Empty directories only appear if the backend exposes directory metadata to the browser; otherwise MVP shows directories implied by documents.

Tree requirements:

- Virtualized rendering for large Libraries.
  
- Keyboard navigation for expand/collapse/select/rename.
  
- Context menu actions: new document, new document under a new folder prefix, rename/move, delete, reveal in graph, copy path.
  
- Drag-and-drop move in v1, implemented through REST move/transaction calls.
  
- Expansion state persisted per Library in local browser storage.
  
- No local tree mutation is considered committed until the REST mutation succeeds.
  
### Markdown Editing
The editor uses CodeMirror 6. The document text remains the canonical source. Live preview is implemented with decorations and widgets over the source text.

MVP editor requirements:

- Plain markdown editing.
  
- Syntax highlighting.
  
- Dirty state and save command.
  
- Local draft persistence keyed by Library and document path/version.
  
- Save with current ETag.
  
- Stale save handling.
  
- Large document guardrails for documents that should be opened read-only or with reduced decoration mode.
  

v1 editor requirements:

- Hide markdown syntax for links, images, emphasis, headings, lists, embeds, and wiki-links only when the cursor is outside the syntax span.
  
- Reveal source text when the cursor enters a decorated region.
  
- Render resolved wiki-links as navigable elements.
  
- Render unresolved wiki-links distinctly without relying on color alone.
  
- Support `[[Page]]`, `[[Page|Alias]]`, `[[Page#Heading]]`, `[[Page^block-id]]`, and `![[embed]]` syntax.
  
- Provide wiki-link autocomplete using path, title, alias, and heading data.
  
- Avoid contenteditable WYSIWYG models that make markdown a secondary serialization format.
  
### Save and Conflict Handling
The browser always saves through REST. The current document ETag is captured when the document is fetched. Writes use `If-Match` for existing documents and `If-None-Match: *` for creates.

Save states:

- `clean`: browser content matches last confirmed server version.
  
- `dirty`: editor content differs from the confirmed server version.
  
- `drafted`: dirty content is persisted locally but not saved to Quarry.
  
- `saving`: mutation in flight.
  
- `saved`: mutation completed and returned a new ETag.
  
- `stale`: server rejected the write due to an ETag/version mismatch.
  
- `failed`: mutation failed for a non-conflict reason.
  

When a save receives `412 Precondition Failed`, the browser must not retry with an unconditional write. It opens a conflict workflow that lets the user inspect:

- the local draft,
  
- the latest remote content,
  
- the last server version the browser edited from when available,
  
- metadata and paths involved.
  

MVP may present a two-way local/remote workflow. v1 should support a three-way base/local/remote workflow when the backend exposes the necessary version data.
### Git Sync Conflicts
Quarry already preserves Git conflicts without writing inline conflict markers into canonical documents. The browser should expose this clearly.

Conflict UI requirements:

- List open conflicts for the active Library.
  
- Show canonical path, conflict sibling path, discovered time, and status.
  
- Open ours/theirs documents side by side.
  
- Let the user choose ours, theirs, manual merged content, or deletion.
  
- Save the chosen resolution as a normal Quarry write.
  
- Mark the conflict resolved through the existing resolve endpoint.
  
- Do not delete conflict sibling documents automatically unless a future backend contract explicitly supports that operation.
  
### Search
Search has two layers:

- Quick open: instant path/title search over known document stubs.
  
- Full search: ranked search over document content, path, title, aliases, and tags.
  

MVP may ship only quick open if full-text indexing is not ready. v1 must provide server-side full-text search. Browser-only full-text indexing is a fallback for small local Libraries, not the authoritative v1 design.

Search requirements:

- Results include path, title, snippet, score, match type, and current ETag or version identifier when relevant.
  
- Search supports keyboard selection and preview.
  
- Search result clicks preserve unsaved editor state by opening in a new pane or asking before replacing a dirty document.
  
- Search endpoints support caching where practical.
  
- Full search must respect Library boundaries.
  
### Links and Backlinks
Quarry Browser should make links part of the durable substrate, not just a frontend rendering trick.

The backend should extract links synchronously on every document write inside the same transaction that advances the document head. Extracted links include:

- Obsidian-style wiki-links.
  
- Embeds.
  
- Standard markdown links.
  
- Heading anchors.
  
- Tags.
  
- Frontmatter aliases.
  

Resolution semantics should follow Obsidian by default:

- Case-insensitive filename match for wiki-link resolution.
  
- Shortest unique path wins.
  
- Ambiguous links remain unresolved and show ambiguity.
  
- Frontmatter aliases participate in resolution.
  
- Heading and block anchors are preserved.
  
- Renames should update doc-id-based edges rather than leaving path-only edges stale.
  

The browser should display:

- Backlinks to the current document.
  
- Outgoing links from the current document.
  
- Unresolved links from the current document.
  
- Hover preview for resolved links.
  
- Create-document flow from unresolved links.
  
### Graph View
The graph is an analytical navigation surface, not the default landing page.

v1 graph requirements:

- Use sigma.js, graphology, and worker-based layout.
  
- Start with a local graph around the current document.
  
- Provide filters by folder, tag, link kind, resolved/unresolved, and depth.
  
- Support full-library graph where practical.
  
- Show path/title on hover and navigate on click.
  
- Provide textual equivalents through backlinks/outgoing links for accessibility.
  
- Persist layout options per Library.
  
- Avoid DOM/SVG graph renderers for large Libraries.
  
### Version History
Version history turns Quarry's immutable version model into a browser workflow.

v1 requirements:

- List versions for a document, newest first.
  
- Show timestamp, transaction source, actor if available, byte size, content type, and message/provenance summary if available.
  
- Open a version read-only.
  
- Diff two versions.
  
- Diff current editor content against latest server content.
  
- Restore a selected version by writing it as a new document head.
  
- Never mutate or delete old versions as part of restore.
  
### Binary and Attachment Handling
Quarry stores arbitrary documents, not just markdown. The browser should handle this directly.

Requirements:

- Use `content_type` metadata to choose preview behavior.
  
- Render markdown and text in the editor/viewer.
  
- Render images inline.
  
- Show binary metadata: path, content type, byte size, hash if exposed, and download action.
  
- Do not attempt to edit unknown binary formats in CodeMirror.
  
- Disable raw HTML execution/rendering unless a future sanitized rendering mode is explicitly designed.
  
- Preserve binary documents in tree, search by path/title, and graph only when links or metadata justify it.
  
### Git Operations
The browser may expose existing Git operations as explicit commands:

- import,
  
- export,
  
- pull,
  
- push,
  
- sync,
  
- peer list/create.
  

Requirements:

- Git operations must show progress or pending state.
  
- Git operations must be explicit user actions.
  
- Conflicts created by Git sync must appear in the conflict UI.
  
- The browser must not run background Git sync without a future opt-in setting.
  
## Browser Architecture
### Frontend Stack
Use:

- Vite + React + TypeScript.
  
- React Router or TanStack Router for layout and document routes.
  
- TanStack Query for server state.
  
- Zustand for ephemeral workspace state.
  
- shadcn/ui + Tailwind + Radix primitives for UI.
  
- CodeMirror 6 for markdown editing.
  
- cmdk for command palette.
  
- react-resizable-panels for the workspace layout.
  
- react-arborist for tree rendering.
  
- sigma.js + graphology for graph view.
  
- lucide-react for icons.
  
- Shiki for code block highlighting where needed.
  
- Vitest, React Testing Library, Playwright, and Storybook.
  

Generated API client:

- Generate TypeScript types and client functions from `/v1/openapi.json`.
  
- Prefer `@hey-api/openapi-ts` if it integrates cleanly with the repo.
  
- `openapi-typescript` + a small hand-written fetch wrapper is acceptable if it keeps generated code smaller and easier to review.
  
- Generated code must be reproducible through a package script.
  
- Generated code is committed so UI development and review do not depend on a running daemon.
  
### Frontend Folder Shape
The expected UI root is `ui/`.

Representative structure:

```text
ui/
  src/
    api/
    app/
    components/
    features/
      command-palette/
      conflicts/
      documents/
      editor/
      graph/
      libraries/
      search/
      tree/
      versions/
    stores/
    workers/
    styles/
    test/
```

This is directional. Implementation plans may adjust names to match the tooling chosen at scaffold time.
### Workspace Layout
The default app is the workspace, not a landing page.

Layout:

- Top bar: Library switcher, command palette/search trigger, Git/sync actions, settings.
  
- Left pane: file tree with tabs for tree/search/conflicts if useful.
  
- Center pane: editor or viewer.
  
- Right pane: backlinks, outgoing links, properties, versions, conflicts, local graph.
  
- Bottom status strip: dirty/saving/saved state, current ETag/version, event connection status, last sync result.
  

Cards should be used only for repeated items, dialogs, and framed tools. The primary workspace should use panes, lists, tabs, menus, and toolbars.
### State Management
Server state belongs in TanStack Query:

- Libraries.
  
- Document stubs and document contents.
  
- Search results.
  
- Link/backlink results.
  
- Conflicts.
  
- Versions.
  
- Git peers and operation results.
  
- Graph subgraphs.
  

Zustand stores browser-only state:

- Active pane.
  
- Tree expansion.
  
- Command palette state.
  
- Selected right-pane tab.
  
- Unsaved local editor metadata.
  
- Recent documents.
  
- Layout sizes.
  
- Theme preference.
  

IndexedDB or browser storage may hold:

- unsaved drafts,
  
- query persistence if enabled,
  
- recent documents,
  
- tree expansion,
  
- graph layout preferences.
  

Local browser storage must not be treated as authoritative Quarry content.
## Backend Additions
### Static Browser Serving
The Quarry server should serve the SPA in production builds.

Requirements:

- Feature-gated browser bundling, for example `bundle_ui`.
  
- Vite production build outputs to `ui/dist`.
  
- Axum serves static assets from the binary.
  
- Client-routed non-API paths fall back to `index.html`.
  
- Hashed assets get long-lived immutable cache headers.
  
- `index.html` gets no-cache or short-cache headers.
  
- API routes remain under `/v1`.
  
- Dev mode supports Vite proxying `/v1` to the Quarry daemon.
  

Use `rust-embed`, `static-serve`, or an equivalent approach. If precompressed assets are easy to support, prefer gzip/brotli at build time.
### Search API
Add:

```text
GET /v1/libraries/{library}/search?q=&limit=&cursor=
GET /v1/libraries/{library}/search/suggest?q=&limit=
POST /v1/libraries/{library}/reindex
```

Search response fields:

- document id,
  
- path,
  
- title/display name,
  
- content type,
  
- score,
  
- snippet,
  
- matched fields,
  
- current version or ETag if useful for cache invalidation.
  

Search implementation:

- Prefer Turso native FTS if the repository-pinned Turso version supports the required index/query behavior.
  
- Keep FTS updates transactional with document writes.
  
- Do not use FTS predicates for DML if the pinned Turso version still has that limitation.
  
- If Turso native FTS is not ready, implement a Tantivy-backed search module behind the same REST contract.
  
- Keep search scoped by Library.
  
### Link Graph API
Add:

```text
GET /v1/libraries/{library}/documents/{path}/backlinks
GET /v1/libraries/{library}/documents/{path}/outgoing-links
GET /v1/libraries/{library}/graph?root=&depth=&limit=
```

Potential schema additions:

```sql
links(
  library_id TEXT NOT NULL,
  src_doc_id TEXT NOT NULL,
  src_version_id TEXT NOT NULL,
  target_kind TEXT NOT NULL,
  target_text TEXT NOT NULL,
  target_doc_id TEXT,
  target_anchor TEXT,
  start_offset INTEGER NOT NULL,
  end_offset INTEGER NOT NULL,
  alias TEXT,
  PRIMARY KEY (library_id, src_doc_id, src_version_id, start_offset)
);

aliases(
  library_id TEXT NOT NULL,
  doc_id TEXT NOT NULL,
  alias TEXT NOT NULL,
  source TEXT NOT NULL,
  PRIMARY KEY (library_id, alias, doc_id)
);
```

Exact table names and keys should follow existing storage conventions. The important requirement is doc-id-based edges so renames do not destroy link identity.
### Version API
Add:

```text
GET /v1/libraries/{library}/documents/{path}/versions?limit=&cursor=
GET /v1/libraries/{library}/documents/{path}/versions/{version}
GET /v1/libraries/{library}/documents/{path}/versions/{version}/diff?against=
POST /v1/libraries/{library}/documents/{path}/versions/{version}/restore
```

Restore writes a new version and advances the document head. It never mutates historical versions.
### SSE Events API
Add:

```text
GET /v1/events?library={library}
```

Event types:

```json
{ "type": "doc.changed", "library": "notes", "path": "Daily.md", "doc_id": "...", "version_id": "...", "etag": "..." }
{ "type": "doc.deleted", "library": "notes", "path": "Old.md" }
{ "type": "doc.moved", "library": "notes", "from": "A.md", "to": "B.md" }
{ "type": "links.indexed", "library": "notes", "path": "Daily.md" }
{ "type": "library.reindexed", "library": "notes" }
{ "type": "conflict.created", "library": "notes", "conflict_id": "..." }
{ "type": "conflict.resolved", "library": "notes", "conflict_id": "..." }
{ "type": "git.sync.completed", "library": "notes", "peer_id": "...", "applied": 12, "conflicts": 1 }
```

Implementation requirements:

- Use Axum SSE and a Tokio broadcast channel or equivalent.
  
- All write surfaces publish events after commit.
  
- Events are best-effort invalidation hints, not the source of truth.
  
- Browser refetches through REST after receiving relevant events.
  
- Event stream includes keepalives.
  
- Browser falls back to polling when SSE is unavailable.
  
### Conflict API Enhancements
The existing conflict endpoints are enough for MVP visibility. v1 should make browser resolution smoother by exposing enough data to avoid extra ad hoc fetches.

Potential enhancement:

```text
GET /v1/libraries/{library}/conflicts/{conflict}
```

Response should include:

- conflict id,
  
- canonical path,
  
- conflict sibling path if present,
  
- ours version id,
  
- theirs version id,
  
- status,
  
- discovered/resolved timestamps,
  
- enough document metadata to render a useful side-by-side view.
  

Resolution should remain explicit and scoped to the Library.
## Data Integrity Requirements
- Browser writes must use REST mutation endpoints.
  
- Browser saves must preserve ETag precondition behavior.
  
- Search and link indexes must update inside the same transaction as the document write when implemented server-side.
  
- Reindex must be idempotent.
  
- Reindex must not alter document content or heads.
  
- SSE events must be emitted only for committed state.
  
- Restoring an old version creates a new version.
  
- Conflict resolution must not discard conflict sibling content unless the user explicitly deletes it.
  
- Browser local drafts must be clearly distinguishable from committed Quarry content.
  
## Security Requirements
MVP assumes the phase-one local server model:

- Bind to `127.0.0.1` by default.
  
- Warn on non-loopback binds as phase one already does.
  
- No remote auth in MVP or v1 unless separately specified.
  

Browser-specific security:

- Treat markdown rendering as an XSS boundary.
  
- Sanitize rendered HTML or disable raw HTML rendering by default.
  
- Do not execute scripts from documents.
  
- Do not allow arbitrary document HTML to escape the editor/preview container.
  
- Do not store sensitive tokens in `localStorage`.
  
- If remote access/auth is later added, use same-origin BFF-style session handling with httpOnly cookies, CSRF protection, and locked-down CORS.
  
## Accessibility Requirements
Target WCAG 2.2 AA for core workflows by v1.

Requirements:

- Keyboard-only Library selection, tree navigation, document open, edit, save, search, command palette, and conflict resolution.
  
- Visible focus states.
  
- Screen-reader labels for icon buttons and status indicators.
  
- Non-color-only indicators for unresolved links, dirty state, errors, and conflicts.
  
- Dialog focus trapping and restoration.
  
- Motion reduction for graph animations and transitions.
  
- Textual alternatives for graph information through backlinks/outgoing links.
  
## Performance Requirements
MVP:

- Tree remains usable with at least 10,000 document paths.
  
- Opening an ordinary markdown document on localhost feels immediate.
  
- Save feedback appears within one interaction frame.
  
- Large documents degrade gracefully if decorations are expensive.
  

v1:

- Code split graph, versions/diff, workers, and optional heavy preview features.
  
- Run graph layout in a worker.
  
- Use virtualized trees and virtualized long result lists.
  
- Avoid re-parsing entire Libraries in the browser when server indexes exist.
  
- Keep production browser assets cacheable and reasonably small.
  
- Avoid rendering the full graph by default on every note open.
  
## Observability
The browser and server should expose enough information to debug local issues.

Track or log:

- route changes,
  
- document fetch latency,
  
- save latency and result,
  
- ETag conflict frequency,
  
- search latency,
  
- reindex duration,
  
- SSE connection state,
  
- Git operation result,
  
- conflict creation/resolution,
  
- frontend uncaught errors.
  

For local-only MVP, browser console logging plus server tracing is acceptable. For remote deployments, structured metrics and request IDs should be added.
## Testing Strategy
Backend tests:

- Search indexing on create/update/delete/rename.
  
- Link extraction for wiki-links, embeds, markdown links, headings, aliases, and unresolved links.
  
- Link index updates from REST, explicit transactions, FUSE writes, Git import, and Git sync.
  
- Reindex idempotence.
  
- Version list/fetch/diff/restore.
  
- SSE event emission after commit and no emission after rollback.
  
- Conflict API Library scoping.
  

Frontend unit/component tests:

- Tree path-to-node transformation.
  
- Document route loading.
  
- Editor dirty/save state transitions.
  
- ETag conflict modal behavior.
  
- Draft persistence and recovery.
  
- Search result keyboard navigation.
  
- Backlinks/outgoing links panels.
  
- Version diff and restore dialogs.
  
- Conflict resolution UI.
  

End-to-end tests with Playwright:

- Open browser and select Library.
  
- Create document, edit, save, reload.
  
- Rename/move document from tree.
  
- Simulate stale ETag save and verify conflict workflow.
  
- Search and open result.
  
- Navigate through wiki-link/backlink.
  
- Trigger backend change and verify SSE invalidation.
  
- Resolve a Git conflict record.
  
- Restore an older version.
  

Accessibility tests:

- Keyboard-only smoke for the main workflow.
  
- Axe or equivalent checks for shell, dialogs, command palette, tree, and conflict UI.
  
- Storybook states for empty, loading, dirty, saving, stale, error, and conflict states.
  
## Acceptance Checklist
MVP is accepted when:

- The browser is reachable from the local Quarry daemon.
  
- The browser uses generated types from the live OpenAPI spec.
  
- A Library can be browsed as a tree.
  
- Markdown documents can be opened and edited.
  
- Saves use ETag preconditions.
  
- Stale saves do not overwrite remote state.
  
- Unsaved drafts survive reload.
  
- Core workflows are keyboard accessible.
  
- Playwright covers create/edit/save/stale-save.
  

v1 is accepted when:

- Live preview editor behavior matches the source-first model.
  
- Search is ranked and server-backed.
  
- Link graph data is indexed transactionally.
  
- Backlinks/outgoing links work for wiki-links and markdown links.
  
- SSE updates browser state after external REST/FUSE/Git changes.
  
- Version history, diff, and restore work.
  
- Conflict resolution is usable from the browser.
  
- Graph view renders focused and full-library graph modes.
  
- Binary/image preview works.
  
- Accessibility and performance budgets have automated checks or documented manual verification.
  
## Open Questions
These are not blockers for this spec, but they should be decided before or during implementation planning:

- Does the repository-pinned Turso version support the exact native FTS behavior v1 needs, or should v1 use Tantivy behind the same API?
  
- What exact large-document thresholds should disable or reduce live-preview decorations?
  
- Which package manager should `ui/` standardize on: pnpm, npm, bun, or another tool already preferred by the project?
  
- Which router should the implementation plan choose: React Router or TanStack Router?
  
- Should v1 include Mermaid/math widgets, or keep them explicitly post-v1?
  
## Initial Implementation Sequence
This is not a detailed implementation plan, but the product should be built in this order:

1. Browser shell, build glue, generated client, Library list, and document tree.
  
2. Markdown editor, read/edit/save/delete/move, ETag conflict workflow, and draft recovery.
  
3. Static asset embedding and production serving from the Quarry daemon.
  
4. Link extraction, backlinks/outgoing links, and wiki-link navigation.
  
5. Search API and browser search UI.
  
6. SSE event stream and browser invalidation.
  
7. Version history, diff, restore, and conflict resolution UI.
  
8. Graph view, binary previews, command palette depth, accessibility polish, and performance hardening.
