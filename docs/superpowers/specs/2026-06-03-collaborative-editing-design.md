# Conflict-free collaborative / concurrent editing — design
**Date:** 2026-06-03 **Status:** Proposed design (exploratory), pre-implementation **Repo area:** `ui/` (PlateJS editor) + `crates/quarry-server` (+ `crates/quarry-storage`)
## Goal
Let multiple **humans and agents edit the same document concurrently over a network** — humans in the browser, agents over an API — without clobbering each other. The guarantee is **scoped, not blanket**: browser↔browser editing is CRDT conflict-free; agents use optimistic concurrency (re-read on stale); silent merge of agent edits into a live human session is future work (§7). Markdown stays the durable at-rest truth so Git, CAS, versions, FUSE, and search keep working unchanged.

The design splits writers into **two populations with two transports**, which is the spine of everything below:

| Population | Transport | What they touch | Conflict model |
| --- | --- | --- | --- |
| **Humans** (browser) | Yjs over WebSocket | A live CRDT (`Y.Doc`) the server relays | CRDT merge (conflict-free) |
| **Agents** (API) | Proof-style HTTP block/ops API | Markdown via the existing `put_document` path | ETag optimistic concurrency (re-read on stale) |

The two layers meet only when an agent edits a document a human has open live; that intersection is mediated by the **existing ETag/**`If-Match` **backstop** (it may conflict — consistent with the git/FUSE boundary), and merging it _silently_ is explicit future work (§7).
## Locked decisions
(Locked by product direction; see Open questions for what remains.)

| Decision | Choice | Rationale |
|---|---|---|
| **In-scope concurrent writers** | Humans (browser) + agents (API), over a network | git / FUSE / CLI writers stay on today's conflict-generating path. |
| **Identity** | Self-provided, **unverified** | No user accounts. A per-doc session invite token *addresses* the join (not auth); `by:` records authorship; agent-id drives presence. |
| **Session invite tokens** | **Sharing/coordination convenience, not a security boundary** (security deferred) | Per-document, stateful (revocable), viewer/editor roles. Address/scope the join to a collab/agent session; the existing unauthenticated REST stays open. See §4. |
| **Human collab server** | **Rust-native `yrs` + Axum WebSocket** | Single binary, local-first, no Node sidecar. `yrs` is JS-wire-compatible. |
| **Server is Slate-blind** | Server relays + snapshots opaque Yjs bytes; never parses `Y.XmlText` structure | The Slate↔Yjs binding exists only in JS; keeping the server Slate-blind avoids porting it to Rust. |
| **At-rest truth** | Markdown, derived client-side | The whole quarry pipeline (Git/CAS/versions/FUSE/search) reads markdown. CRDT is the live session layer, not the durable format. |
| **How agents join** | **HTTP, not Yjs peers** | Agents are stateless HTTP clients (Proof model). No headless Yjs client, no Rust↔Slate bridge for v1. |
| **Agent edits land as** | Markdown writes through `put_document`, block-addressed | Reuses quarry's existing chokepoint and ETag concurrency; block refs are **snapshot-scoped** (no durable block ids exist). |
| **Agent review marks (`/ops`)** | **Scoped Rust RFM module** (markdown-level) | The server gains CriticMarkup + endmatter read/splice on at-rest markdown; it stays Yjs/Slate-blind. **Revises** the review-markup spec's "backend never learns about the review layer." See §2. |
| **Suggestion representation** | **At-rest RFM markdown** (inherited from the review-markup-layer spec) | Forced by Slate-blind server + no bridge: real-time Yjs marks would need §7. Not re-opened for v1. |
| **Silent agent↔live-human merge** | **Out of scope** (future work, §7) | Requires server-side Slate↔Yjs; v1 lets that intersection conflict at the markdown boundary. |
## Reference implementations
- **PlateJS** (`~/p/udecode/plate`, MIT): `@platejs/yjs` — `YjsPlugin`, `UnifiedProvider`, `registerProviderType`, the `init/destroy/connect/disconnect` API, deterministic seeding. Binding is a thin wrapper over `@slate-yjs/core` (`Y.XmlText` at key `"content"`).
  
- **Potion** (`~/p/fabro-sh/potion`, MIT): production client wiring of `@platejs/yjs` v52 — `plate-provider.tsx` (gating, `yjs.init`, `skipInitialization`), `remote-cursor-overlay.tsx`, and the snapshot-in-DB + derived-JSON persistence pattern. We reuse Potion's **client** wiring; its server (Node Hocuspocus + Postgres) does **not** transfer.
  
- **Proof Editor** (`proofeditor.ai`, ref: `github.com/EveryInc/proof-sdk`): the **agent API shape** we adopt — `/snapshot` (block refs + mutation-base token), `/edit/v2` (block ops), `/ops` (comments/suggestions/track-changes), `/presence`, `/events/stream` + `/events/pending` + `/ack`, and the `by:` / `X-Agent-Id` / bearer-token three-way identity split. We copy the _API shape_, not the server internals (Proof is JS top-to-bottom; its server does block-markdown↔Yjs natively, which quarry's Rust server can't).
  
- **Rust CRDT stack:** `yrs` 0.27 (`features = ["sync"]` → `yrs::sync`, the merged-in former `y-sync` crate); `yrs-warp` 0.9 `BroadcastGroup` as the broadcast-loop reference to vendor (it is framework-agnostic over `Sink<Vec<u8>>`/`Stream`).
  

**Key divergence from Potion/Proof:** both are JS servers that own the Yjs↔Slate conversion. Quarry keeps its **Rust server Yjs/Slate-blind** and pushes that conversion to the browser (humans), while agents write markdown — with a scoped, **markdown-level** Rust RFM module backing `/ops` (never Yjs). This is the load-bearing difference and the reason the two-population split exists.

* * *
## Section 1 — Human collaboration (browser ↔ browser)
**Core idea:** the browser does all Slate↔Yjs work via `@slate-yjs/core`; the Rust server is a dumb relay + opaque snapshot store. No server-side Slate knowledge.

**Client (extends** `PlateMarkdownEditor.tsx`**):**

- When collab is active: `usePlateEditor({ skipInitialization: true, value: undefined, plugins: [...] })`, then on mount `editor.getApi(YjsPlugin).yjs.init({ id: room, value, autoSelect: 'end', onReady })`; `yjs.destroy()` on unmount.
  
- **Custom provider:** `registerProviderType('rust-ws', RustWsWrapper)` where `RustWsWrapper` wraps `y-websocket`'s `WebsocketProvider` and satisfies v52's `UnifiedProvider` = `{ awareness, document, type, connect(), disconnect(), destroy(), isConnected, isSynced }`. v52 has **no** `on`**/**`off` on the provider — drive `isConnected`/`isSynced` from the `WebsocketProvider` `status`/`sync` events and call the plugin-injected `onConnect`/`onSyncChange` callbacks. Mirror the built-in `WebRTCProviderWrapper`. Connect URL: base `ws(s)://host/v1/collab`, room = the **stable document id** (see v1 invariants), which y-websocket appends as `/<room>`. The browser obtains `documentId` from the `getDocument`/`/snapshot` response before opening the WS (today it works in path terms only).
  
- **Cursors:** `RemoteCursorOverlay` via `render.afterEditable`, `@slate-yjs/react`'s `useRemoteCursorOverlayPositions`; `cursors.data = { name, color }` from self-provided identity (§4).
  
- Shared root is the default `ydoc.get("content", Y.XmlText)` — do **not** override.
  

**Server (**`crates/quarry-server`**, new):**

- Route `/v1/collab/{documentId}` using `axum::extract::ws` (axum 0.8, `features = ["ws"]`); the room key is the document id, not the path (survives `doc.moved`).
  
- `yrs = { version = "0.27", features = ["sync"] }`. Per room: a `yrs::Doc`, `doc.get_or_insert_xml_text("content")` (must match slate-yjs's type+key or docs won't converge). Use `yrs::sync::DefaultProtocol` for SyncStep1/2/Update + awareness framing.
  
- **Broadcast:** vendor `yrs-warp`'s `BroadcastGroup` (operates on `Sink<Vec<u8>>`/`Stream<Item=Result<Vec<u8>,E>>`) and add ~30-line `AxumSink`/`AxumStream` adapters mapping `axum::ws::Message::Binary` ↔ `Vec<u8>`. Don't depend on `yrs-warp` (pins yrs 0.24) or `yrs-axum-ws` 0.1.0 (pins yrs 0.23) — vendoring keeps us on 0.27.
  
- **No Slate parsing.** The server treats updates as opaque bytes.
  

**At-rest markdown (the bridge back to quarry's pipeline):**

- **Flusher election (leader lease).** Exactly one peer at a time holds a *flusher lease* advertised in awareness; it serializes `Y.Doc → Plate → markdown` with the **existing** `plateValueToMarkdown` / `reviewToMarkdown` and PUTs via the existing REST path, **debounced + on last-peer-disconnect** (cf. Potion's `debounce`/`maxDebounce`). If the leader drops, a surviving peer claims the lease. If **no** peer remains, the server keeps the dirty snapshot (below) and the next opener flushes it — markdown lags, nothing is lost. (Flush runs in a browser because the server is Slate-blind.)
  
- **Flush conflict (412).** The flush PUT keeps `If-Match: <baseEtag>`. If a concurrent git/FUSE/CLI/agent write bumped the version → `412`. The flusher **never force-overwrites and never discards the unflushed `Y.Doc`**: it pauses flushing, marks the session **contended**, surfaces the external change via the existing stale/conflict UI, the human reconciles, the `Y.Doc` is **re-based** to the merged head, then flushing resumes against the new `baseEtag`. This is the git/agent↔live-human conflict surfacing as a flush-412 — the same handler as the SSE external-change path (§6).
  
- **Flush echo suppression (provenance guard) — required, or every flush self-conflicts.** A *successful* flush itself emits `doc.changed` for the active doc, so contention can't key off "any `doc.changed`." The `doc.changed` payload **already carries `version_id` + `etag`** (server `lib.rs:1484-1489`). On flush 200 the leader records the returned ETag and broadcasts it to peers over awareness (a `collabSessionId` stamped on the write + echoed in the event is the race-free belt-and-suspenders). A `doc.changed` for the active doc is then treated as **our own flush echo — ignored, not a conflict — iff** its `version_id` is in the session's acked-flush set (or its `collabSessionId` matches the room). **Only** a `doc.changed` from a different/absent session (git/FUSE/CLI/agent/other) moves the session to `contended`.
  
- **Re-seed:** when no live session exists, a fresh room is seeded from at-rest markdown (`markdownToReview` → `slateToDeterministicYjsState` → `Y.applyUpdate`); deterministic seeding means peers that independently seed produce identical bytes (no duplicate content).
  
- **Collab recovery state (durable Yjs snapshot).** The server persists the opaque `encode_state_as_update_v1` snapshot per room (CAS blob or a `documents.yjs_snapshot` column), tagged with the base markdown version (ETag) it last reconciled from + a **dirty** flag. **Markdown is the only at-rest source of truth.** While dirty, this snapshot — the *collab recovery state* — is the authoritative input for **reconstructing the live session** on reconnect/crash; it is **not** an at-rest source of truth and is never served to external readers. It holds the only copy of edits not yet flushed, so it must be durably persisted on its own cadence (below). Reconnect/late-join/recovery: a dirty recovery state is loaded, reconciled, and flushed to markdown on next open. The flush is _acknowledged_ — the recovery state is marked clean only after the `If-Match` PUT returns 200. Keep `yrs` GC **on**.
  
- **Recovery-state persistence cadence (independent of markdown flush).** The server debounces Yjs-update persistence to CAS/Turso *as updates arrive* — **not** only on disconnect — so a server crash loses at most the persist-debounce window, not the whole session. Persistence is acked; on persist error the server signals peers and refuses to silently advance. v1 may persist a full `encode_state_as_update_v1` snapshot per debounce (simplest) or an append log with periodic compaction (cheaper writes); either way it is separate from the browser-driven markdown flush (which the Slate-blind server can't perform itself).
  
- **External reads are bounded-eventually-consistent.** git export, FUSE, search, version history, and the agent `/snapshot` all read **last-flushed markdown** — never the recovery state, which is **collab-recovery only**. In steady state the leader flushes (debounced) and synchronously on graceful last-disconnect, so the lag is at most the flush window; the only truly-stale case is a crash with no peer left to flush (markdown catches up on the next open). The server is Yjs/Slate-blind, so it can't serialize on demand anyway — external reads are never blocked, and the staleness window is stated, not hidden.
  
- **Move / delete during a live session.** The room key is the document id, but the flush is a *path-based* PUT, so the flusher resolves the **current path from the id at flush time** and follows `doc.moved` (the existing `App.tsx:385-392` handler retargets `selectedPath`). A `doc.moved` is **not** a conflict — the session continues on the new path; a flush that races the move and lands on the old path → treat as `contended` and retry against the new path. On `doc.deleted` of an active dirty session, **do not auto-tear-down** (today `App.tsx:385-388` clears the selection): enter `deleted`, surface "deleted externally," and let the human **discard** (end session) or **resurrect** (flush re-creates via the existing tombstone-clearing `put_document`).

### Collab state machine
Per live document (room). Markdown is the only at-rest source of truth; the durable Yjs snapshot is the *collab recovery state*.

| State | Meaning |
|---|---|
| `recovering` | On open/reconnect: load the recovery state (if dirty) or seed from markdown; reconcile before going live. |
| `clean` | Y.Doc == last-flushed markdown; no unflushed edits. |
| `dirty` | Unflushed edits exist; recovery state ahead of markdown (persisted on its own debounce). |
| `flushing` | A flush PUT (`If-Match: baseEtag`) is in flight. |
| `contended` | An external change (flush 412, SSE `doc.changed`, or a `doc.moved` race) needs reconcile; flushing paused. |
| `deleted` | The doc was deleted externally during a live session; awaiting discard-or-resurrect. |

| Transition | From → To | Notes |
|---|---|---|
| open / reconnect | → `recovering` → `clean` | dirty recovery state wins, is flushed, then clean; else seed from markdown. |
| local/remote update | `clean` → `dirty` | also schedules a debounced recovery-state persist (independent of flush). |
| flush start (debounce / last-disconnect) | `dirty` → `flushing` | leader-lease holder only; runs in a browser. |
| flush 200 | `flushing` → `clean` | recovery state marked clean; `baseEtag` advanced. |
| flush 412 | `flushing` → `contended` | never force-overwrite; surface stale/conflict UI; re-base to merged head. |
| SSE `doc.changed` — **our flush echo** | `flushing`/`dirty` → (no change) | matched by acked `version_id` (or `collabSessionId`); ignored, never a conflict. |
| SSE `doc.changed` — **external** (active doc) | `*` → `contended` | only if **not** our echo (provenance guard); suppress auto-reset; banner; pause flush. |
| reconcile done | `contended` → `dirty` → `flushing` (or `clean`) | re-base Y.Doc to merged head, resume. |
| `doc.moved` | (state unchanged) | retarget flush to the new path; not a conflict. |
| `doc.deleted` | `*` → `deleted` | no auto-teardown; human discards or resurrects. |
| server / last-browser crash | (next open) → `recovering` | loads last *persisted* recovery state (bounded by the persist debounce). |
  

* * *
## Section 2 — Agent collaboration (API)
**Core idea (the Proof lesson):** agents are **plain HTTP clients**, not Yjs peers. They edit **markdown through** `put_document` using **block-addressed operations** + a **mutation-base token**, which avoids whole-document diffing and reuses primitives quarry already has. No Yjs and no Rust↔Slate (Yjs) bridge; `/ops` adds a scoped **markdown-level** RFM module, which is **not** that bridge.

**Quarry already has the two primitives Proof's edit API is built on:**

| Proof primitive | Quarry equivalent (already exists) |
|---|---|
| `mutationBase.token` (revision-level optimistic concurrency) | **ETag = head version id** (`If-Match`, `412` on stale) — already wired end-to-end |
| `blocks[].ref` (opaque per-block address) | **snapshot-scoped ref** `{baseToken, ordinal, contentHash}` — quarry has nanoid ids for *review marks* only, **not** content blocks, so refs are valid only against the snapshot's `baseToken` and rejected on stale base. Durable block IDs deferred (the RFM codec deliberately scoped nanoids to marks). |
| events as wake-signal → re-read | **SSE `doc.changed` → SWR revalidate** — already the model (incl. `stream.lagged`) |
| `/ops` track-changes (comment/suggestion/accept/reject, `by:`) | **the RFM review format** — CriticMarkup + endmatter, `by:` labels. v1 adds a **scoped Rust RFM module** to read/splice these server-side (the TS codec is UI-only); see below. |

**Endpoint mapping (new under the existing document routes):**

```
GET  /v1/libraries/{lib}/documents/{*path}/snapshot   → { documentId, baseToken: <etag>, blocks:[{ref:{ordinal,contentHash}, markdown}] }
POST /v1/libraries/{lib}/documents/{*path}/edit       → { baseToken, operations:[{op, ref, block:{markdown}}] }
POST /v1/libraries/{lib}/documents/{*path}/ops        → comment.add | suggestion.add|accept|reject (→ RFM marks)
POST /v1/libraries/{lib}/documents/{*path}/presence   → { agentId, status }            (awareness)
GET  /v1/.../events/stream  +  /events/pending?after= + /events/ack                     (reuse SSE + polling fallback)
```

- `/snapshot` parses at-rest markdown → top-level blocks; `ref` is **snapshot-scoped** (`{ordinal, contentHash}`, valid only against this `baseToken`), `baseToken` = current head ETag, `documentId` = the collab room key.
  
- `/edit` ops, **v1 = top-level block ops only** (`replace_block`, `insert_before`, `insert_after`, `delete_block`), each with **AST-backed validation** (the addressed block must parse and the spliced markdown must round-trip). The server splices the addressed block into the document body and writes a new version via `put_document` (`DocumentSource::Rest`, `WritePrecondition::IfMatch(baseToken)`). Stale → `412`/`STALE_BASE`, agent re-reads `/snapshot`. Carry an `Idempotency-Key`; support `?dryRun=1`. **Deferred:** text-range ops (`replace_range`, `find_replace_in_block`) — they need UTF-16 indexing + ambiguity rules (§7).
  
- `/ops` (`comment.add` / `suggestion.add` / `accept` / `reject`) is backed by a **scoped Rust RFM module** that parses the at-rest markdown, splices the CriticMarkup marker into the anchor block, **merges the `{by, at, re?}` entry into the YAML endmatter** (which the block view strips, so plain `/edit` can't reach it), and writes the result via `put_document` with `by:` attribution. The server gains **markdown-level** RFM awareness only — it stays Yjs/Slate-blind. This **revises** the review-markup spec's "backend never learns about the review layer"; the module is the "Approach C standalone RFM module" that spec deferred, and it must stay behavior-compatible with the TS codec (drift risk — see Risks). Still the most differentiated win: agents proposing **reviewable** changes into a human's doc.
  
- **`/ops` anchoring contract (v1 = block ref + exact quote).** Review marks are inherently anchored, so even though `/edit` text-range ops are deferred, `/ops` needs an anchor: `{ ref, quote, body|content, by }`. The `quote` is matched **within the addressed block** and must be **unique there** (else `AMBIGUOUS_ANCHOR`; not found → `ANCHOR_NOT_FOUND`); omit `quote` to anchor the whole block. **No character offsets** (consistent with deferring range ops) — matching Proof's `quote` param and the Roughdraft `{==…==}` convention agents already produce. The module wraps the matched span as `{==quote==}{>>body<<}{#id}` (comment) or `{++…++}` / `{--…--}` / `{~~old~>new~~}{#id}` (suggestion) and adds the endmatter entry.
  
- **`accept` / `reject` semantics (id-keyed CriticMarkup transforms).** The module looks up the mark by `{#id}`, applies the deterministic transform, then prunes the endmatter entry: **insert** — accept keeps the text + drops markers/`{#id}`, reject removes the text; **delete** — accept removes the text, reject keeps it; **substitution** `{~~old~>new~~}` — accept keeps `new`, reject keeps `old`; **comment.resolve** — set endmatter `status: resolved`, no body change. Endmatter cleanup is order-preserving (idempotent re-serialize), and orphan-pruning follows the review-markup spec (only live `{#id}`s keep entries). "Byte-compatible with the TS codec" means *these exact transforms*, asserted against the shared Roughdraft conformance fixtures.
  
- `rewrite.apply` (full-doc replace) is the bluntest path; treat as last resort and reject/conflict it against a live session via `If-Match` (cf. Proof blocking it during live collab).
  
- `/presence` **+ events:** reuse quarry's SSE as the wake signal; add a polling fallback (`/events/pending?after=<id>`) and an ack cursor for agents that can't hold a stream. Contract (from Proof): **events are sparse signals carrying revisions/hashes, not content — refresh state before acting.** quarry's SSE already behaves this way.
  

**Why HTTP-not-Yjs for agents:** it removes the headless-Yjs-client requirement _and_ the Rust↔Slate bridge. Agents and humans share markdown-as-truth; the ETag backstop mediates the rare overlap. (The alternative — agents as Yjs peers — was considered and rejected: worse agent ergonomics, and it still needs JS-side Slate conversion the agents would have to carry.)

* * *
## Section 3 — Boundary with git / FUSE / CLI (unchanged)
These writers stay exactly as today; the collab layer sits _above_ the single `QuarryStore::put_document` chokepoint (differentiated by `DocumentSource` + `WritePrecondition`):

| Writer | `DocumentSource` | Precondition | On contention |
|---|---|---|---|
| Browser flush (live session) | `Rest` | `IfMatch(baseEtag)` | `412` → existing stale/merge dialog |
| Agent `/edit` / `/ops` | `Rest` | `IfMatch(baseToken)` | `412`/`STALE_BASE` → re-read snapshot |
| CLI `quarry put` | `Cli` | `None` | last-writer-wins (unchanged) |
| FUSE mount | `Fuse` | `IfMatch`/`IfNoneMatch` | `PreconditionFailed` → EIO/EEXIST (unchanged) |
| git-peer sync | `Git` | mixed | **`ConflictRecord` + `.conflict-git-<ts>` sibling** (unchanged) |

git-peer sync remains the only producer of `ConflictRecord`s. A git pull / FUSE / CLI write landing on a doc with a live session trips the flush's `If-Match` → the existing conflict machinery — i.e. it "may still conflict," as scoped.

* * *
## Section 4 — Identity & access (self-provided, unverified)
Three separate concerns (Proof's split, which fits quarry's "no accounts" posture):

| Concern | Mechanism | Notes |
|---|---|---|
| **Join** | Per-doc **session invite token** (`?token=` / `Authorization: Bearer`) | A sharing/coordination handle (see below), not a security boundary and not authentication. The one net-new subsystem. |
| **Authorship** | `by:` label on every write (`"ai:<name>"`, `"user"`) | Maps 1:1 to RFM's `by`. Lets the existing `identity.ts` constant `"user"` be replaced by a real self-provided name. |
| **Presence** | `X-Agent-Id` (agents) / `cursors.data {name,color}` (humans) | Self-declared; not verified. Humans get cursors via Yjs awareness; agents get semantic status (`reading|thinking|acting|waiting|completed|error`). |
### Session invite tokens (v1 = sharing convenience; security deferred)
**Framing (locked):** a session invite token is a **coordination/sharing handle** that lets a human or agent _find and join_ a document's collaborative session — the Proof "here's a tokenized URL, join immediately" ritual. It is **not** an access-control boundary and does **not authenticate** anyone. quarry's existing `/v1` REST API is unauthenticated, so on a non-loopback bind anyone who can reach the port can already write directly; the token only addresses/scopes the _new_ collab/agent session and **does not claim confidentiality the design doesn't provide**. A real security boundary (auth across the whole mutating surface) is deferred — its own project. Extend `warn_if_non_loopback` to cover the collab/agent routes.

**Model (defaulted from the framing):**

| Aspect | v1 choice | Notes |
|---|---|---|
| **Granularity** | Per-document | Bound to the **stable document id**, not the path string, so a move/rename doesn't break the token. Token's doc id = the collab room key. |
| **State** | Stateful — a Turso `tokens` table | `{ id, document_id, role, by_hint?, created_at, revoked_at? }`. Gives revocation + listing + audit via the existing events bus. (Stateless signing rejected: its only win is "no DB lookup," which we don't need.) |
| **Roles** | `viewer` / `editor` | viewer = read + presence + events; editor = `+ /edit` and `/ops`. `commenter` (`/ops` only) deferred — trivial to add since `/ops` exists. |
| **Mint / revoke** | `quarry share` CLI **and** UI "Share" action **and** a REST endpoint | Agent-native parity (any UI action is also an API). The **loopback operator is the implicit owner/admin**; no separate Proof-style `ownerSecret` in v1. |
| **Expiry / TTL** | None in v1 | Revocation covers it; TTL can reuse the document-ttl-expiry patterns later. |

**Bootstrapping ritual (Proof-shaped):** mint → hand out `…/d/<doc>?token=<token>` (or `Authorization: Bearer`) → the joiner reads `/state`/`/snapshot`, announces `/presence`, then works. The token _addresses/scopes_ the join (not authentication); `by:` records authorship; `X-Agent-Id`/`cursors.data` drive presence — all three decoupled, all self-declared.

* * *
## v1 invariants
The non-negotiables that keep the implementation safe; everything in Phasing must preserve these:

1. **Room key = document id** (not path) — survives `doc.moved`; the browser resolves `documentId` from `getDocument`/`/snapshot` before opening the WS.
  
2. **Only browser↔browser is CRDT conflict-free.** Agents use optimistic concurrency; agent↔live-human silent merge is future work (§7).
  
3. **Markdown is the only at-rest source of truth.** The durable Yjs snapshot is the *collab recovery state* — authoritative only for reconstructing a live session, never an at-rest source of truth and never served externally. Markdown becomes authoritative only after an acked flush; no peer/leader teardown may drop unflushed state; a flush 412 never force-overwrites — it re-bases to the merged head (§1).
  
4. **Agent block refs are snapshot-scoped** (`{baseToken, ordinal, contentHash}`, rejected on stale base) unless/until durable block IDs are added.
  
5. **SSE self-stomp is gated, with a flush-echo provenance guard.** A client in a live session **suppresses the automatic reload/`resetPlateEditor`** for its own active doc. Contention is keyed only on **external** `doc.changed` — an event matching our own acknowledged flush (`version_id`/`collabSessionId`) is recognized as the echo and ignored, so a normal flush 200 never self-conflicts. External writes still **surface a banner + pause flush** → the §1 reconcile handler. Gate the auto-reset, not the signal. (phase-1 prerequisite)
  
6. **Session invite tokens are convenience-only** — not a security boundary; the existing mutating REST surface remains unprotected by design.
  
7. **External reads are bounded-eventually-consistent** — git/FUSE/search/versions/`/snapshot` read last-flushed markdown; the collab recovery state is never served to non-collab readers. Yjs updates are persisted to durable storage on their own debounce (independent of markdown flush), so a crash loses at most that window.
  

* * *
## Section 5 — Phasing
1. **Browser↔browser multiplayer.** Rust `yrs` relay + durable snapshot; custom WS provider; remote cursors; self-provided identity; leader-lease flusher → _acknowledged_ markdown PUT. **Blocking prerequisite — gate the SSE self-stomp:** a client in a live session must **suppress the automatic `doc.changed`-driven reload/`resetPlateEditor`** for its own active doc (instead: surface an external-change banner + pause flush), or the first co-edit corrupts the session. **Also expose the document id** (`X-Quarry-Document-Id` on `GET`/`HEAD`, `documentId` in `/snapshot` + `LoadedDocument`) so the browser can key the WS room. (No Rust Slate knowledge.)
  
2. **Persistence hardening.** Dirty-snapshot reconnect/late-join + recovery; deterministic re-seed from markdown; flush cadence + version-churn control. (SSE self-stomp gating moved to phase 1.)
  
3. **Agent HTTP API.** `/snapshot` + `/edit` (block ops on markdown) + `/presence` + events polling fallback. Session invite tokens. `Idempotency-Key`, `?dryRun`.
  
4. **Agent** `/ops` **= review layer.** Build the **scoped Rust RFM module** (CriticMarkup + endmatter splice/merge, byte-compatible with the TS codec) and expose `comment.add` / `suggestion.add` / `accept` / `reject`.
  

* * *
## Section 6 — Risks & edge cases
- **Slate↔Yjs is JS-only** → server stays Slate-blind (drives the whole design). Any future server-side Slate work (§7) needs a Rust port or embedded JS engine.
  
- **SSE self-stomp** during live sessions (**phase-1 prerequisite**) — verified live: `App.tsx:394` revalidates the active doc on `doc.changed` and `PlateMarkdownEditor.tsx:346` calls `resetPlateEditor` on `content` change, so an unguarded co-edit stomps the `Y.Doc` immediately. Fix = suppress the auto-reset, surface the change + pause flush, **and** apply the flush-echo provenance guard (§1) so a flush's own `doc.changed` isn't mistaken for an external write.
  
- **Version churn** from flushing → debounce + last-disconnect policy; each flush is a new version (history preserved, but don't flush per keystroke).
  
- **markdown↔Y.Doc determinism** incl. RFM CriticMarkup + the `SuggestionPlugin` layer; round-trip must be idempotent (the review-markup spec already requires this for review regions).
  
- **Suggestion representation (locked for v1):** suggestions stay at-rest RFM markdown (an agent's suggestion appears in a live session only after re-read/re-seed, via the SSE wake signal). This is inherited from the review-markup-layer spec and not re-opened here — real-time suggestion marks in the `Y.Doc` are a _post-v1_ direction requiring §7.
  
- **Rust RFM module ↔ TS codec drift** — the scoped server-side RFM module (§2) and the UI codec must stay behavior-compatible (same CriticMarkup/endmatter semantics, idempotent round-trip), or agent `/ops` writes and human edits disagree. Shared conformance fixtures (the review-markup spec's Roughdraft fixtures) are the guard.
  
- **UTF-16 indexing** for any server-side text op (relevant only if §7 is pursued).
  
- **Axum+yrs glue is not first-party** — ~200 vendored lines (`BroadcastGroup` + adapters).
  
- **Session invite tokens are convenience, not security** (§4) — the existing unauthenticated REST stays open on the bound interface; don't represent the collab layer as confidential. Flag on non-loopback bind.
  

* * *
## Section 7 — Future work (explicitly out of scope)
**Silent merge of agent edits into a live human Yjs session.**

Today's boundary (this design): when an agent edits a document a human currently has open in a live Yjs session, the agent's write goes through `put_document` and bumps the version; the human's eventual flush hits `If-Match` and **conflicts at the markdown boundary** (existing stale/merge UI). The agent's change is _not_ injected into the live `Y.Doc` in real time — it surfaces to the human via the SSE wake signal as an external change (re-read / re-seed), exactly like a git pull does.

The future upgrade is to make that intersection **conflict-free**: an agent's block edit (or suggestion) merges _silently and instantly_ into the live `Y.Doc` and appears in the human's editor with the same CRDT guarantees as another human's keystrokes — no `412`, no re-read, no markdown-boundary conflict.

What it requires (and why it's deferred):

- **Server-side Slate↔Yjs conversion.** To turn an agent's per-block markdown into a `Y.XmlText` mutation on the live doc, the server needs `slateNodesToInsertDelta` / `yTextToSlateElement` semantics — which exist **only in JavaScript**. There is no Rust port. Options, all costly:
  
  - **Port the Slate↔Yjs delta logic to Rust** (large; must stay in lockstep with the TS codec — duplication risk).
    
  - **Embed a JS engine in the Rust server** (`rquickjs` / `deno_core`) running the existing TS codecs (heavier binary, but reuses canonical code).
    
  - **A narrow JS "collab worker"** the Rust server calls for block-markdown↔Yjs-delta only (re-introduces a second runtime the v1 design deliberately avoids).
    
- **Suggestions as Yjs marks**, not at-rest RFM, so agent-proposed changes render in real time during a live session (the suggestion-representation fork in §6 resolves toward the CRDT).
  
- **UTF-16-correct, block-scoped diff/merge** so a server-applied edit places ops correctly without corrupting block boundaries (Proof solves this with block-addressed ops; we'd need the same precision server-side).
  

Until a product requirement justifies that cost, agent↔live-human concurrency stays on the ETag backstop. The v1 primitives (snapshot-scoped block refs, mutation-base tokens, deterministic seeding, opaque snapshots) are chosen so this upgrade is **not foreclosed** — it's an additive server capability, not a rewrite.

* * *
## Dependencies & integration checks
- **New UI deps** (match Plate 52, per repo ≥24h policy): `@platejs/yjs@52.0.13`, `@slate-yjs/core@1.0.2`, `@slate-yjs/react@1.1.0`, `yjs@13.6.x`, `y-websocket@^3`, `y-protocols`.
  
- **New Cargo deps:** `yrs = { version = "0.27", features = ["sync"] }`, `axum` `features += ["ws"]`; vendor `yrs-warp` 0.9 `broadcast.rs` (don't depend on it directly).
  
- **API change (phase 1):** expose the stable document id — add `X-Quarry-Document-Id` to the `GET`/`HEAD` document response, include `documentId` in `/snapshot`, and add it to `LoadedDocument` (`client.ts:14`, today `path/content/contentType/etag` only). The WS room key depends on it.
  
- **Event-payload provenance (phase 1):** `doc.changed` already carries `version_id`/`etag` (`lib.rs:1484-1489`) — surface them in the client `BrowserEventPayload`, and add a `collabSessionId` (stamped on the collab flush write, echoed in the event) so a live client can tell its own flush echo from an external write (§1 provenance guard).
  
- **Verify (not change):** the designated-flusher PUT path reuses `put_document` + `precondition_from_headers` unchanged; the agent `/edit` path reuses the same chokepoint with `DocumentSource::Rest`.
  
- **Net-new subsystems:** (1) per-doc **session invite tokens** — a stateful Turso `tokens` table (`document_id`, `role` viewer/editor, revocable) + mint/revoke via CLI/UI/REST; a sharing-convenience handle, _not_ an auth boundary (§4). (2) a **scoped Rust RFM module** (CriticMarkup + endmatter parse/splice/merge on markdown) backing `/ops`, behavior-compatible with the TS codec.
