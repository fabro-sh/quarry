# Session-Scoped Collaboration Rewrite Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each phase is independently executable; detail a phase into bite-sized TDD steps when you pick it up, grounded in the actual code at that time.

**Goal:** Replace quarry-2's autosave/draft/injection-gate collaboration with canonical block rows in SQL, ephemeral per-session Yjs documents, a semantic mutation gateway that writes into live sessions as a collaborator, and diff3-based Markdown reconciliation for Git/FUSE/CLI.

**Architecture:** Canonical state is a plain block tree in SQL (no durable CRDT state). A Yjs document exists only while browsers are connected: seeded from rows, checkpointed back on a debounce, discarded when the last browser leaves. A per-document mode switch routes semantic transactions either directly to rows (no session) or into the live session as another collaborator (session active) — there is no flush/reseed bridge and no injection gate. Whole-file Markdown writes reconcile via diff3 against a stored shadow base; true conflicts become review items, never write failures.

**Tech Stack:** Rust, Axum, Turso (quarry-storage), Yrs/Yjs (session-scoped only), TypeScript, React, Plate, slate-yjs.

**Design spec:** `docs/superpowers/specs/2026-06-09-session-scoped-collab-design.md` — requirements, accepted constraints (online-only browsers, crash loss acceptable, single server), and revisit triggers live there. Read it before executing any phase.

---

## Summary

quarry-2 currently has three competing sources of truth for an editable document: Markdown autosave drafts, the live Yjs room, and agent injection through the gate in `crates/quarry-server/src/collab.rs`. This plan removes two of them. After this plan, block rows are the only durable truth, the live session is the only transient truth, and exactly one of them is write-authoritative for a document at any moment.

This is an alternative to the CRDT block-tree rewrite executed in the sibling `quarry` repo. It deliberately does not build: durable Yrs canonical state, the browser-checkpoint/text-delta storage policy, the live-room flush/reseed bridge, the similarity-matching Markdown reconciler, offline draft persistence, or `/edit`-`/ops`-style facades over a second mutation vocabulary.

## Settled Scope

- [x] Live character-level human-human co-editing is browser-only and session-scoped. No offline browser editing; a disconnected browser is read-only until reseeded.
- [x] Crash loss of un-checkpointed session edits is accepted. No session WAL.
- [x] Canonical state for `BlockDocument`s is relational block rows. No durable CRDT bytes anywhere.
- [x] `POST /transactions` is the single public mutation contract for agents, CLI, Git, FUSE, and imports. No `/edit` or `/ops` facades.
- [x] Browser edits (text and structural) flow through the Yjs session, never through `POST /transactions`. Browsers always have a session while editing.
- [x] Semantic transactions during a live session apply into the session document as a collaborator client; acks force a checkpoint first.
- [x] Git, FUSE, and CLI all read and write whole Markdown files in the first cut. Reconciliation is diff3 against a stored shadow base, not similarity matching.
- [x] diff3 conflicts converge as data: non-conflicting hunks apply; conflicting hunks become conflict review items. File writes never fail with reconciliation errors.
- [x] Review anchors are `{block_id, start_offset, end_offset}` in rows, converted exactly to/from Yjs relative positions at session seed/checkpoint.
- [x] `RawDocument`s (binary/non-Markdown) keep the existing byte path untouched.
- [x] Markdown export is deterministic and idempotent after one-time normalization; exact byte preservation is a non-goal.
- [x] Legacy autosave/draft/injection paths are deleted only after the Phase 6 vertical slice passes.

## Phase 0: Hard Architecture Gate

Three spikes prove the load-bearing mechanics before any storage schema, public API shape, or generated types change. If any gate fails, stop and revise the design spec before continuing. Record findings in `docs/superpowers/specs/2026-06-09-session-scoped-collab-phase-zero-findings.md`.

### Gate A: Rows ↔ Session Round-Trip Exactness

The seed and checkpoint projections must be lossless inverses, including anchors.

- [x] Build a fixture block tree covering: paragraphs, headings, nested list items, code blocks, links, inline marks, `raw_markdown` blocks, and review anchors at block start/middle/end and spanning to block end.
- [x] Seed a Yjs document from the fixture using `crates/quarry-collab-codec/src/yjs_builder.rs`, converting anchor offsets to Yjs relative positions.
- [x] Project the Yjs document back to rows with no intervening edits; assert byte-equal block content, identical tree shape, identical `block_id`s, and identical anchor offsets.
- [x] Repeat with concurrent simulated edits (text inserts before/inside/after anchors, block insert, block move) applied to the Yjs document before checkpoint; assert anchors land at the CRDT-resolved offsets and `block_id`s are preserved.
- [x] Record the exact anchor conversion rules (UTF-16 offsets, boundary affinity at block start/end) in the findings doc.

### Gate B: Server-as-Collaborator

A semantic operation applied into a live session must behave exactly like another human's edit.

- [x] In a Playwright spike (extend `ui/tests/live-collab-agent-smoke.spec.ts` or add `ui/tests/session-collaborator-spike.spec.ts`): two Chromium browsers typing in the same paragraph while the server applies a `replace_block_content` to a different block as a distinct Yjs client ID.
- [x] Assert: both browsers converge; in-flight keystrokes are not lost or reordered; the receiving tabs are not marked dirty; no session reseed or state replacement occurs; undo in each browser undoes only that browser's edits.
- [x] Apply a semantic op to the same block a human is actively typing in; assert convergence without rejection (awkward merged text is acceptable and expected).
- [x] Assert awareness (cursors) stays anchored across the server-applied edit.
- [x] Record in the findings doc how the server constructs Yjs transactions (client ID allocation, origin tagging so checkpoint attribution can distinguish browser vs gateway edits).

### Gate C: diff3 Identity Mapping

Base-mapped block identity must replace similarity guessing.

- [x] Codec-level spike: given (base export, incoming file, current canonical export), compute a three-way merge at block granularity.
- [x] Prove identity preservation for: unchanged blocks, edited blocks, inserted blocks, deleted blocks, reordered blocks — IDs flow through positional mapping against the base, with zero similarity scoring.
- [x] Prove a true conflict (same block edited in incoming and canonical since base) produces: the canonical side retained, plus a structured conflict artifact carrying the incoming hunk, block ref, and base context.
- [x] Prove anchors outside changed hunks are untouched; anchors inside genuinely changed hunks orphan (comments) or invalidate (suggestions) per existing review rules.
- [x] Prove the degenerate base cases: base == canonical (two-way import, IDs preserved for unchanged regions) and missing base (first import, fresh IDs).
- [x] Record hunk-to-operation mapping rules in the findings doc.

## Canonical Data Model

New tables in `crates/quarry-storage/src/lib.rs` (schema block near the existing `CREATE TABLE` statements at ~line 3540). Do not finalize column layout until Phase 0 findings are recorded.

- `blocks`: `block_id` (stable, ULID), `document_id`, `parent_block_id` (nullable), `position` (orderable sibling key), `block_type` (paragraph, heading, list_item, code_block, quote, image_embed, raw_markdown, table), `attrs` (JSON), `text` (UTF-8; offsets measured UTF-16 to match Yjs), `marks` (JSON ranges for inline marks/links).
- `block_review_items`: `id`, `document_id`, `block_id`, `kind` (comment|suggestion), `start_offset`, `end_offset`, `body`/`replacement`, `author`, `state` (open|resolved|orphaned|invalidated), `quote`, `context_before`, `context_after`, reply threading.
- `block_shadow_bases`: `surface` (git|fuse|cli), `scope_key` (peer/handle/path identifier), `document_id`, `base_markdown`, `base_version_id`, `updated_at`.
- `block_transactions`: `client_tx_id` (unique per document for idempotency), `document_id`, `actor_kind`, `actor_id`, `ops` (JSON), `resulting_version_id`, `created_at` — semantic mutation history; checkpoint commits record one coalesced `browser_session` row.
- Document attributes/frontmatter continue to live where the codec currently puts them; document clock remains the existing `document_versions` head.
- Drop at cleanup (Phase 7): `collab_recovery_states` (sessions are discardable; recovery is reseed-from-rows).

Document kinds: `.md`/`.markdown`/`text/markdown` → `BlockDocument`; everything else → `RawDocument` on the untouched byte path.

## Implementation Phases

### Phase 1: Canonical Block Rows in Storage

**Files:** `crates/quarry-storage/src/lib.rs` (schema + new `blocks` module or inline section), `crates/quarry-collab-codec/src/markdown.rs`, `crates/quarry-collab-codec/src/normalize.rs`, tests in `crates/quarry-storage/tests/storage_lifecycle.rs` and `crates/quarry-collab-codec/tests/`.

- [x] Add the four tables above behind the existing migration mechanism.
- [x] Implement `load_block_tree(document_id)` and `replace_block_tree(document_id, tree)` with ordering by `position`.
- [x] Implement Markdown → block rows import via the existing codec (`markdown.rs`), including frontmatter → document attrs and `raw_markdown` fallback for safe unsupported constructs; unsafe constructs return the codec's typed `Unsupported` error.
- [x] Implement block rows → Markdown export; property test `export == export(import(export))` after one-time normalization.
- [x] Implement review-anchor storage and the offset model (UTF-16); unit tests for anchors at block boundaries.
- [x] Storage lifecycle tests: import → restart → load tree → export is stable; review anchors survive restart.
- [x] Wire document-kind classification (BlockDocument vs RawDocument) at the storage boundary; RawDocument bytes prove untouched in `storage_lifecycle.rs`.

### Phase 2: Semantic Mutation Gateway (Rows-Authoritative Mode)

**Files:** new `crates/quarry-server/src/gateway.rs`, `crates/quarry-server/src/lib.rs` (routes), `crates/quarry-storage/src/lib.rs` (transactional apply), tests in `crates/quarry-server/tests/rest_api.rs`; OpenAPI regeneration into `ui/src/api/generated/`, helpers in `ui/src/api/client.ts`.

Envelope: `{client_tx_id, base_clock?, actor{kind,id,label}, ops[]}`. Response: `{status: committed|committed_rebased, document_clock, transaction_id, changed_block_ids[]}`.

- [x] Implement ops applied to rows inside one SQL transaction: `insert_block`, `delete_block`, `move_block` (placement-only; preserves `block_id`, content, children, anchors), `replace_block_content` (minimal prefix/suffix text diff; anchors outside the changed span untouched), `set_block_attrs`.
- [x] Implement inline formatting ops against the `marks` ranges on `blocks`: `add_mark`, `remove_mark`, `set_link`.
- [x] Implement review ops: `comment.add`, `comment.reply`, `comment.resolve`, `comment.delete`, `suggestion.add`, `suggestion.accept`, `suggestion.reject` against `block_review_items`.
- [x] Idempotency: duplicate `client_tx_id` returns the original ack without re-applying.
- [x] Clock handling: matching `base_clock` applies; stale-but-valid applies as `committed_rebased`; invalid returns typed retryable errors. No generic 409s.
- [x] Typed error payloads `{code, retryable, message}`: `STALE_BASE`, `BLOCK_DELETED`, `ANCHOR_NOT_FOUND`, `BLOCK_MOVE_CONFLICT`, `SUGGESTION_INVALIDATED`, `SUGGESTION_ALREADY_RESOLVED`, `UNSUPPORTED_MARKDOWN`, `INVALID_TRANSACTION`, `UNSUPPORTED_BLOCK_DOCUMENT` (RawDocument target).
- [x] Routes: `GET .../documents/{path}/blocks`, `POST .../documents/{path}/transactions`, `GET .../documents/{path}/review` projecting from rows; document events emitted for commits.
- [x] Multi-op transactions commit atomically as one version and one history row.
- [x] Regenerate OpenAPI JSON and TypeScript types; add `ui/src/api/client.ts` helpers with tests in `ui/src/api/client.test.ts`.
- [x] REST coverage in `rest_api.rs` for every op, every typed error, idempotency, and rebase acks.

### Phase 3: Ephemeral Sessions and the Mode Switch

**Files:** new `crates/quarry-server/src/session.rs`, `crates/quarry-server/src/collab.rs` (rewire websocket to sessions; delete injection gate), `crates/quarry-server/src/gateway.rs` (session-mode dispatch), tests in `crates/quarry-server/tests/rest_api.rs` and `crates/quarry-server/src/session.rs` unit tests.

- [ ] Implement per-document session lifecycle: first websocket subscriber seeds a fresh Yjs doc from rows (Gate A projection); updates broadcast to peers; awareness relayed, never persisted.
- [ ] Debounced checkpoint (target 2–5s after last update; tunable constant): project session doc → rows + one coalesced `browser_session` history row + document event. Checkpoint is the only durable effect of typing.
- [ ] Last subscriber leaves → final checkpoint → discard the session doc.
- [ ] Per-document async mutex serializes seed, checkpoint, discard, and transaction application. Transactions arriving mid-transition wait; they are never rejected because a session exists.
- [ ] Gateway session-mode: translate ops to Yjs edits, apply as a dedicated collaborator client ID (Gate B mechanics), force a checkpoint, then ack. `changed_block_ids` and history recorded the same as rows-mode.
- [ ] Delete the injection gate and its rejection paths from `collab.rs`; delete `LIVE_ROOM_ACTIVE`-class error emission. Grep proves no remaining producer.
- [ ] Server restart test: sessions vanish, reconnecting browser reseeds from rows, content equals last checkpoint.
- [ ] Concurrency tests: transaction racing seed; transaction racing final checkpoint/discard; two transactions during one session; checkpoint-before-ack ordering proven by reading rows immediately after ack.

### Phase 4: diff3 Markdown Reconciliation and Adapters

**Files:** new `crates/quarry-collab-codec/src/reconcile.rs`, `crates/quarry-storage/src/lib.rs` (shadow bases, local adapter helper), `crates/quarry-git/src/lib.rs`, `crates/quarry-fuse/src/lib.rs`, `crates/quarry-cli/src/lib.rs`, `crates/quarry-server/src/lib.rs` (Markdown `PUT` route), tests in `crates/quarry-collab-codec/tests/`, `crates/quarry-git/tests/git_roundtrip.rs`, `crates/quarry-fuse/tests/projection.rs`, `crates/quarry/tests/cli_smoke.rs`, `crates/quarry-server/tests/rest_api.rs`.

- [ ] Implement `reconcile(base, incoming, canonical) -> {ops, conflicts}` per Gate C rules: positional identity mapping, minimal ops for changed hunks, structured conflict artifacts for true three-way conflicts.
- [ ] Conflict artifacts persist as conflict review items (`kind = conflict` on `block_review_items`, carrying the losing hunk and base context), visible via `GET /review` and resolvable like other review items. File writes succeed regardless.
- [ ] Shadow base bookkeeping: Git records base per peer+path at export/import; FUSE records base per open handle at `open()`; CLI uses current canonical export as base (two-way degenerate case); server Markdown `PUT` honors `If-Match`-style clock as base selector, falling back to two-way.
- [ ] Route whole-file writers through reconcile → gateway transaction (rows-mode or session-mode per the switch): Git import/sync, FUSE create/write/release/truncate, CLI `put`, server Markdown `PUT`.
- [ ] RawDocument bypass coverage in all three adapters: bytes round-trip with no block tables touched.
- [ ] Adapter tests: editing one block externally preserves sibling `block_id`s and live anchors; concurrent canonical edit + external write converges with conflict review items for overlapping hunks; FUSE flush during an active browser session converges through the session (no errno for reconciliation outcomes).

### Phase 5: Browser Simplification

**Files:** `ui/src/app/App.tsx`, `ui/src/features/editor/PlateMarkdownEditor.tsx`, `ui/src/features/collab/rust-ws-provider.ts`, `ui/src/features/collab/session-events.ts`, `ui/src/api/client.ts`, tests alongside each; delete `ui/src/features/collab/flusher-lease.ts` and its test.

- [ ] Save state reduces to two inputs: websocket connection state and checkpoint ack (server confirms last checkpoint covers the client's latest update). UI states: `Saved`, `Saving…`, `Reconnecting (read-only)`.
- [ ] Disconnect → editor read-only with indicator; reconnect → reseed from canonical state via a fresh session; no local persistence of pending updates.
- [ ] Delete: autosave timers and draft `PUT`s, local draft storage/recovery, dirty/draft tracking, "External version available" classification in `session-events.ts`, flusher-lease machinery.
- [ ] Remote session updates (human or gateway-collaborator) render without marking the document dirty.
- [ ] Review UI reads anchors/states from `GET /review` rows projection, including the new conflict review items; orphaned/invalidated badges retained.
- [ ] Update `ui/src/app/workspace.test.tsx` and editor tests for the new save-state model; delete draft-recovery tests.

### Phase 6: Vertical Slice Gate

Do not delete remaining legacy paths until all pass:

- [ ] Two browsers same-paragraph typing converges character-by-character; cursors stay anchored (`ui/tests/` live spec).
- [ ] Agent `POST /transactions` mid-typing converges without rejection; ack implies durable rows.
- [ ] Git, FUSE, and CLI whole-file writes preserve sibling block IDs and anchors; true conflicts surface as review items in the UI.
- [ ] Browser disconnect/reconnect reseeds; server restart loses only post-checkpoint keystrokes; reload shows `Saved` from canonical state.
- [ ] RawDocument byte fidelity across REST, Git, FUSE, CLI.
- [ ] No code path can emit `LIVE_ROOM_ACTIVE`, create a local draft, or write Markdown for a BlockDocument outside the gateway/checkpoint paths (grep + test assertions).
- [ ] `cargo test --workspace`, `cd ui && bun run typecheck && bun run test`, and the Playwright suites pass.

### Phase 7: Cleanup and Docs

- [ ] Delete quarantined legacy code: autosave/draft endpoints and storage, injection gate remnants, `collab_recovery_states` table and its code, dead UI states.
- [ ] Update `crates/quarry-server/resources/agent-docs.md` and `crates/quarry-server/resources/quarry.SKILL.md`: `GET /blocks`, `POST /transactions`, stable `block_id` addressing, typed retryable errors, no facades.
- [ ] Update `README.md` architecture description and `docs/manual-test-plan.md`.
- [ ] Record limitations: online-only browsers, checkpoint-window crash loss, single-server sessions, hunk-level (not character-level) merge for external file writes.

## Test Plan Summary

- Projection exactness: rows ↔ session round-trip property tests incl. anchors (Gate A promoted to permanent codec tests).
- Gateway: per-op REST tests, typed error matrix, idempotency, rebase acks (`crates/quarry-server/tests/rest_api.rs`).
- Mode switch: seed/checkpoint/discard races, checkpoint-before-ack (`crates/quarry-server/src/session.rs` + `rest_api.rs`).
- Reconciliation: hunk taxonomy, conflict-as-review-item, degenerate bases (`crates/quarry-collab-codec/tests/`), adapter round-trips (`git_roundtrip.rs`, `projection.rs`, `cli_smoke.rs`).
- Browser: save-state model unit tests; live multi-browser + agent E2E (`ui/tests/`).

## Assumptions and Non-goals

- Greenfield; no migration of existing draft/recovery data.
- Single server owns the database and all sessions.
- Online-only browsers; crash loss within the checkpoint window is accepted (per design spec).
- No durable CRDT state, no per-keystroke history, no offline drafts, no `/edit`/`/ops` facades, no exact Markdown byte preservation.
- Same-block human/agent intent merging is convergence-only; finer text-range ops are future work.
- Revisit triggers (offline, multi-server, zero-loss durability) are listed in the design spec.
