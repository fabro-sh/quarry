# Session-Scoped Collaboration Architecture

**Status:** Design approved in discussion 2026-06-09. Alternative to the CRDT
block-tree rewrite plan (`quarry` repo,
`docs/superpowers/plans/2026-06-08-crdt-block-tree-rewrite.md`), which made
durable CRDT state canonical. This design rejects that plan's two highest-risk
components — the heuristic Markdown reconciler and the hybrid two-pipeline
concurrency seam — while keeping its proven parts.

## Problem

quarry-2's current architecture has three competing sources of truth for an
editable document: Markdown autosave drafts, the live Yjs room, and agent
injection. Their interactions produce the known failure modes: injection-gate
rejections (`LIVE_ROOM_ACTIVE`-class errors), `Failed`/draft-limbo save states,
"External version available" misclassifications, keystrokes mis-applied when
agent writes race live rooms, and browser latency from autosave/draft
bookkeeping running alongside typing.

The reviewed alternative (CRDT block-tree rewrite) fixes these by making a
durable Yrs document canonical, with two write pipelines coordinated by a
flush/reseed bridge and a similarity-matching Markdown reconciler. We are not
adopting it because both of those components are bespoke coordination machinery
whose correctness must be maintained under every race, and the reconciler can
only fail closed (typed errors surfacing as errno to POSIX callers) or guess.

## Requirements and accepted constraints

Hard requirements:

- Real-time character-level human-human co-editing in the browser
  (same-paragraph simultaneous typing must merge).
- Git, FUSE, and CLI are all first-cut read **and write** surfaces, writing
  whole Markdown files.
- Agents are first-class writers with stable block addressing, idempotency,
  and typed retryable errors.
- Review anchors (comments, suggestions) survive unrelated edits and never
  silently move to the wrong place.

Accepted constraints (these are load-bearing; revisit the design if they
change):

- **Online-only browsers.** No offline editing, no local drafts, no pending
  update persistence. A disconnected browser blocks editing until it
  reconnects and reseeds.
- **Crash loss is acceptable.** Keystrokes since the last session checkpoint
  may be lost on server death. No session WAL.
- **Single server** owns the database and all live sessions (existing
  phase-one constraint).
- Greenfield: no migration of deployed data; legacy paths are deleted, not
  preserved.
- Markdown export is deterministic and semantically stable; exact byte
  preservation is a non-goal. One-time normalization on first import is
  accepted; export is idempotent afterward.

## Architecture

One sentence: **canonical state is a plain block-tree in SQL; a CRDT exists
only as the transport inside a live browser session; every writer — human,
agent, or adapter — converges on whichever of the two is currently
authoritative, and a per-document mode switch says which one that is.**

### Canonical state: block rows

The source of truth for a `BlockDocument` is relational:

- `blocks`: `block_id` (stable), `document_id`, `parent_block_id`, ordered
  sibling position, block type (paragraph, heading, list item, code block,
  quote, image/embed, raw_markdown), attrs, text content, inline
  marks/links as structured ranges (UTF-16 offsets).
- `review_items`: comments and suggestions anchored as
  `{ block_id, start_offset, end_offset }` plus quote/context fallback and
  state (`open`, `resolved`, `orphaned`, `invalidated`).
- Document attributes (frontmatter keys) with stable export order.
- `RawDocument`s (binary, non-Markdown) keep the existing byte path and are
  untouched by all of this.

No durable CRDT state exists anywhere. Backup is a database copy. Debugging is
SQL. There is no CRDT compaction, migration, or growth problem.

### Live sessions: ephemeral Yjs documents

- When the first browser opens a document for editing, the server seeds a
  fresh Yjs document from block rows, converting anchor offsets to Yjs
  relative positions (an exact conversion — the server controls the document
  state at seed time).
- Browsers exchange Yjs updates and awareness over the existing websocket.
  Awareness (presence, cursors) stays ephemeral and is never persisted.
- The server checkpoints the session document back to block rows on a
  debounce (target: a few seconds), converting relative positions back to
  offsets. Checkpointing is a pure projection of state the server owns; it
  cannot conflict.
- When the last browser disconnects, the server runs a final checkpoint and
  **discards** the session document. Every session starts from a fresh Yjs
  doc, so tombstone/history growth is bounded by one session's lifetime, and
  the payload a browser loads is proportional to document content, not edit
  history.
- Server restart: sessions vanish; browsers reconnect and reseed from rows.
  Work since the last checkpoint is lost (accepted).

### The mode switch

A document is in exactly one write mode at a time:

- **Rows-authoritative** (no live session): the gateway applies semantic
  operations directly to block rows inside an ordinary SQL transaction. No
  CRDT code runs.
- **Session-authoritative** (live session open): the gateway translates
  semantic operations into Yjs edits and applies them to the session document
  **as another collaborator** with its own client ID, riding the same
  update/broadcast path as browser keystrokes. The checkpoint propagates the
  result to rows. Agent transaction acks force a checkpoint first so an acked
  write is durable.

A per-document mutex serializes mode transitions (seed, final checkpoint,
discard) against incoming transactions. Writers arriving mid-transition wait
milliseconds; they are never rejected because a session exists. The
`LIVE_ROOM_ACTIVE` error class is structurally impossible: a live session is
the write path, not an obstacle to it.

### Semantic mutation gateway

The public mutation contract for agents, CLI, Git, FUSE, imports, and browser
structural commands is unchanged in spirit from the reviewed plan
(*superseded as built:* browsers never call the gateway — ALL browser edits,
structural included, flow through the Yjs session and persist via
checkpoints; the gateway's clients are agents and the whole-file adapters):

- `GET  /v1/libraries/{library}/documents/{path}/blocks`
- `POST /v1/libraries/{library}/documents/{path}/transactions`

Envelope: `client_tx_id` (idempotency), optional `base_clock`, actor metadata,
`ops[]`. Operations: `insert_block`, `delete_block`, `move_block`
(placement-only, preserves `block_id`/content/children/anchors),
`replace_block_content` (minimal prefix/suffix diff, not wholesale),
`set_block_attrs`, mark/link ops, `comment.*`, `suggestion.*`.

Typed errors (`STALE_BASE`, `BLOCK_DELETED`, `ANCHOR_NOT_FOUND`, etc.) are
retained. They describe problems with the *content* of a write, never the
existence of a live session. Mutation history records semantic transactions
plus coalesced per-checkpoint summaries for browser typing; history is an
audit log, not a recovery mechanism.

Browser live typing and cursor movement never use `POST /transactions`; they
are Yjs session traffic.

### Whole-file Markdown writes: three-way merge, not similarity matching

Git, FUSE, and CLI write whole Markdown files. Reconciliation against the
canonical block tree uses **diff3 with a stored base**, the same trust model
as Git:

- Each adapter surface retains, per path, the canonical Markdown text it last
  exported (or imported) and that export's document clock — the shadow base.
- A whole-file write computes `diff3(base, incoming file, current canonical
  export)`.
- Unchanged regions map block identity through the base positionally (the way
  `git blame` tracks lines) — block IDs and review anchors are preserved by
  construction, not by similarity scoring.
- Changed hunks become minimal semantic operations (`replace_block_content`,
  `insert_block`, `delete_block`, `move_block`) submitted through the normal
  gateway with the shadow clock as `base_clock`.
- **Conflict-as-data:** where base, incoming, and canonical genuinely
  three-way conflict, the write still succeeds. The merged result applies the
  non-conflicting hunks; each conflicting hunk becomes a conflict review item
  (visible in the review queue, resolvable in the UI). POSIX callers never
  see a reconciliation errno; `vim` saves always succeed.
- Anchors inside genuinely changed regions follow the existing rules:
  comments orphan, suggestions invalidate. Anchors never move on a guess.

There is no similarity matcher, no ambiguity error matrix, and no
`ANCHOR_AMBIGUOUS`-style failure surfaced to file writers.

### Markdown codec

Deterministic import/export between block trees and Markdown, reused from the
existing codec work: supported syntax round-trips semantically; safe
unsupported block-level constructs become opaque `raw_markdown` blocks;
unsafe/ambiguous syntax returns typed errors on API import paths; frontmatter
round-trips with stable key order; `export == export(import(export))`.

### Browser/UI

- Plate binds to the session Yjs doc for editing (existing binding work
  retained).
- Save state collapses to two inputs: websocket connection state and
  checkpoint/transaction ack state. `Saved` means "connected, last checkpoint
  covers my edits."
- Deleted entirely: Markdown autosave, local draft storage and recovery,
  draft/dirty tracking, "External version available" classification, the
  injection gate and its error handling, offline pending-edit persistence.
- Disconnected state: editor becomes read-only with a reconnecting indicator;
  on reconnect it reseeds from canonical state.

## Failure modes and their handling

| Event | Behavior |
| --- | --- |
| Agent write during live typing | Merges as a collaborator edit; converges; same-block races may produce awkward text but never reject or lose CRDT state |
| Two humans typing in one paragraph | Yjs character-level merge (unchanged) |
| Whole-file write with stale base | diff3 merge; conflicts become review items; write succeeds |
| Transaction during session seed/discard | Waits on per-document mutex (ms), then applies in the new mode |
| Server crash | Sessions lost; browsers reseed; edits since last checkpoint lost (accepted) |
| Browser network drop | Editor read-only until reconnect + reseed; no local draft |
| Plate/Yjs binding bug corrupts a session | Discard session, reseed from rows; canonical state untouched |

## What this deletes from quarry-2 today

- Markdown autosave and draft endpoints/UI for editable documents
- Local draft recovery and draft-vs-server conflict UI
- The Yjs injection gate and `LIVE_ROOM_ACTIVE`-class rejection paths
- Save-state machinery beyond connection + ack
- SSE-based external-version classification for live documents

And relative to the reviewed plan, it never builds: durable Yrs storage and
its checkpoint/text-delta policy, the flush/reseed bridge, live-room recovery
state, the similarity reconciler, or offline draft persistence.

## Testing strategy

- **Property tests on the projections:** rows → Yjs seed → rows is identity;
  anchor offset ↔ relative position round-trips exactly at seed/checkpoint;
  codec export/import idempotence.
- **Concurrency tests at the mode switch:** transactions racing seed,
  checkpoint, and discard; forced-checkpoint-before-ack ordering; mutex
  starvation.
- **diff3 reconciliation suite:** unchanged/edited/inserted/deleted/reordered
  blocks; concurrent canonical edits; true conflicts produce review items and
  never errno; anchors preserved outside changed hunks, orphaned/invalidated
  inside.
- **Live E2E (Playwright):** multi-browser same-paragraph convergence; agent
  transaction mid-typing converges without rejection; disconnect/reconnect
  reseeds cleanly; reload shows `Saved` from canonical state.
- **Adapter round-trips:** Git/FUSE/CLI whole-file writes preserve sibling
  block IDs and review anchors; RawDocument bytes bypass everything.

## Non-goals

- Offline browser editing and local drafts
- Multi-server / hosted deployment (single owning process stands)
- Durable per-keystroke history or CRDT-state audit trails
- Exact Markdown byte preservation
- Perfect same-block human/agent intent merging (convergence only; finer
  text-range semantic ops are future work)

## Revisit triggers

This design trades generality for simplicity against explicit constraints.
Reopen it if any of these become requirements: offline clients, multi-server
session ownership, zero-keystroke-loss durability, or per-keystroke history.
