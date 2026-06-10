# Session-Scoped Collaboration — Phase Zero Findings

**Verdict: all three gates PASS. The architecture proceeds unchanged.** The
design deltas at the end are additive clarifications, not revisions.

Plan: `docs/superpowers/plans/2026-06-09-session-scoped-collab-rewrite.md`
Design: `docs/superpowers/specs/2026-06-09-session-scoped-collab-design.md`

Evidence:

- Gate A: `crates/quarry-collab-codec/tests/phase_zero_gate_a.rs` (9 tests;
  commits `8ae78ae`, `e121bbd`)
- Gate B: `ui/tests/live-session-collaborator-spike.spec.ts` +
  `ui/tests/helpers/agent-collaborator.ts` (3 tests; commit `b250c0e`)
- Gate C: `crates/quarry-collab-codec/tests/phase_zero_gate_c.rs` (24 tests;
  commits `7cddf64`, `0deb68d`)

Each spike's module docs carry the full rule text; this doc is the
cross-gate summary and the list of obligations the later phases inherit.

## Gate A: Rows ↔ Session Round-Trip Exactness — PASS

The seed (rows → Yjs doc) and checkpoint (Yjs doc → rows) projections are
exact inverses through the production codec (`build_nodes`/`apply_built`,
`xmltext_to_slate`), including review anchors, under concurrent edits merged
from a second client.

**Anchor conversion rules (binding for Phases 1 and 3):**

- All offsets are UTF-16 code units; session docs use `OffsetKind::Utf16`.
  Yjs clock lengths are UTF-16, so sticky indices land on code-unit
  boundaries; surrogate pairs (emoji) round-trip exactly.
- `start_offset` → `StickyIndex` with `Assoc::After` at the first anchored
  character. An insert exactly at the start boundary is excluded (anchor
  never grows leftward).
- `end_offset` (exclusive) → `StickyIndex` with `Assoc::Before` at
  last-anchored-char + 1. An insert exactly at the end boundary is excluded
  (never grows rightward). `Assoc::After` is unrepresentable at end-of-text
  (`StickyIndex::at` returns `None`).
- Inline embeds (links) occupy one index unit in their parent text; row
  flat-offsets ↔ Yjs (branch, local index) requires a piecewise mapping.
  Anchors inside and spanning link text resolve exactly under concurrent
  edits.
- A fully deleted anchor range checkpoints as collapsed `start == end` at
  the deletion site; the row layer marks these orphaned. Zero-length anchors
  are never re-seeded into sessions.
- Plain text inserted inside a formatted span inherits that formatting
  (Yjs format markers): a bold run grows around an interior insert. The
  checkpoint mark-range projection must accept this.

**Block identity:** rides as an `id` attribute on block elements — the
production codec already round-trips it with zero changes. Checkpoint
requires ids on top-level blocks; the server assigns ids at checkpoint for
browser-created blocks that lack one.

**Move semantics (the one real caveat):** the slate-yjs doc shape has no
native element move; a move is delete + reinsert-clone. The clone preserves
the `id` attribute (identity survives), but sticky anchors into the moved
subtree are stranded on the dead branch (proven by a negative test). Rules:

- A server-performed move (gateway `move_block`) must transplant anchors:
  resolve the moved subtree's anchors to plain offsets → delete + reinsert
  → re-create sticky indices in the new element.
- A browser-initiated drag-drop move happens client-side where no transplant
  is possible. Mitigation (design delta 1): after every checkpoint the
  server re-places all anchors from the just-written rows into live
  branches, bounding anchor loss from client-side moves to one checkpoint
  window. A pure move leaves block text identical, so last-known offsets
  stay valid absent same-window text edits in that block.

**Phase 1 flags:** wikilinks (inline void, zero flat text) need a
zero-length element-range representation in rows — not yet modeled. Nested
list items in this Plate shape are flat `p` blocks with `indent`/
`listStyleType` attrs (no element nesting); true nesting exists in
`code_block` → `code_line`.

## Gate B: Server-as-Collaborator — PASS

A third Yjs client (Node-side y-websocket provider on its own `Y.Doc`)
joined live rooms and replaced block content while two browsers typed.
Everything held against the real current stack: convergence with zero
keystroke loss or reordering, no dirty/conflict UI, no remount/reseed, undo
isolation in both browsers, remote cursors anchored across reflow, and
same-block concurrent rewrite converging without rejection. This is exactly
the mechanic the Phase 3 gateway uses.

**Protocol facts (binding for Phase 3):**

- Room = `/v1/collab/{document_id}`; document UUID comes from the
  `x-quarry-document-id` response header. Wire protocol is standard y-sync
  v1 (yrs `DefaultProtocol` server-side).
- Client IDs are client-local random 32-bit per `Y.Doc`; nothing is
  allocated server-side.
- **Yjs transaction origins do not cross the wire.** The server applies
  inbound client updates with no origin (`transact_mut()`); browser and
  gateway updates are indistinguishable inside the doc. Checkpoint
  attribution must come from connection-level tracking (the server already
  logs a per-socket `collab_session_id`). Origins remain useful only for a
  doc-local actor's own bookkeeping (the server's seed/injection writes;
  browser undo scoping).

**Hazards the rewrite must remove or respect during transition:**

- **The collab websocket has no auth** — invite tokens are locator-only
  today. Acceptable under phase-one single-user loopback posture, but the
  Phase 3 session layer must not widen exposure, and the limitation belongs
  in Phase 7's recorded limitations (design delta 2).
- `reseed_clean_room_if_head_changed` runs on every socket join — a join
  while the head has moved triggers a legacy reseed. Phase 3's session
  lifecycle replaces this; until then, gateway-style clients must connect
  before heads move (join-order hazard).
- Legacy flusher election picks the minimum client ID among awareness states
  carrying `quarryCollab.sessionId`. A non-browser collaborator must NOT set
  that awareness field, or it can win the flusher lease. (Moot after Phase 5
  deletes the flusher.)
- y-websocket clients must explicitly send an awareness query (message
  type 3) to see pre-existing peers.
- Today, a collaborator's edit is persisted via the browser flusher as an
  "Autosaved edits" browser transaction — it works, but attribution folds
  into the human autosave. This is precisely what the Phase 3
  checkpoint/attribution path replaces.

## Gate C: diff3 Identity Mapping — PASS

Positional base-mapping replaces similarity guessing with zero unresolvable
ambiguities. The reconciler (test-local prototype for Phase 4's
`reconcile.rs`) survived adversarial probing (empty docs, position-0
inserts, swaps, rotations, twins, all-identical blocks, multi-conflict,
interleaved op kinds) with deterministic output.

**Hunk-to-operation mapping rules (binding for Phase 4; full text in the
spike's module docs):**

1. Block-ify base/incoming/canonical with the production Markdown codec.
2. Exact-equality LCS `base↔incoming` and `base↔canonical` with pinned
   tie-breaks (equal blocks match front-to-back; deletions before
   insertions).
3. Base indices stable in BOTH alignments anchor the regions between them.
   Region classification, in order: `incoming == base` → keep canonical, no
   ops; `incoming == canonical` → converged, no ops; `canonical == base` →
   incoming-only, emit ops; otherwise → conflict.
4. Conflicts never fail the write: keep the canonical slice, emit artifact
   `{block_ids, base, incoming, canonical, after_block_id}`; other regions
   still apply. `after_block_id` = the stable block immediately preceding
   the region in the merged document (`None` = document start) — the
   guaranteed-surviving attachment point for the Phase 4 conflict review
   item.
5. Incoming-only regions: move pairing first (global, exact content
   equality, multiplicity exactly 1 on both sides), then positional replace
   pairing per gap (k-th delete ↔ k-th insert; content change →
   `replace_block_content`, attrs change → `set_block_attrs`, both →
   replace then attrs in that order), leftovers → `delete_block` /
   `insert_block` with fresh IDs in document order.
6. Op ordering contract: replaces/attr-sets → deletes → placements;
   placement positions are final-merged indices applied ascending.
7. Edit-vs-delete is a conflict: canonical block kept, artifact's empty
   incoming slice records the delete intent. Canonical-delete vs
   incoming-edit mirrors it: artifact has empty `block_ids`/`canonical` and
   anchors via `after_block_id`.
8. Anchor fates: `replace_block_content`/`delete_block` targets → comments
   orphan, suggestions invalidate; untouched, attrs-only, moved, and
   conflicted-but-retained blocks → anchors untouched.

**Characterized limitations (each pinned by a test):**

- Duplicate ambiguity: identical twins resolve positionally (harmless —
  byte-identical blocks); unmatched multiplicity ≥ 2 refuses pairing
  outright → delete + fresh IDs. No guessing, ever.
- An incoming edit adjacent to a conflict (no stable separator between
  them) is absorbed into the conflict artifact rather than auto-applied.
- A moved-and-edited block is not move-paired; it degrades to delete +
  fresh insert (its anchors orphan).
- A move whose source sits inside a conflict region duplicates content
  (fresh insert at destination + canonical copy retained in the conflict
  region); resolving the conflict item removes the stale copy.
- Type changes (even h2 → h3) lose identity via delete + insert — there is
  no `set_block_type` op (design delta 3).

**Perf warning:** the spike's O(n·m)-space LCS and O(n²) stable-anchor
intersection must not be transcribed verbatim; production needs a
linear-space diff (Myers/Hirschberg) and sorted-merge intersection.

## Design deltas (additive; apply during the relevant phases)

1. **Checkpoint anchor re-placement (Phase 3):** after every checkpoint,
   the server re-places all anchors from the just-written rows into live
   branches, bounding anchor loss from client-side block moves to one
   checkpoint window.
2. **Websocket auth posture (Phases 3/7):** the collab websocket is
   unauthenticated; keep loopback-only posture and record the limitation.
3. **Consider `set_block_type` (Phase 2 vocabulary):** without it, type
   changes lose block identity and review anchors through the Markdown
   adapter. Decide before the gateway op set freezes.
4. **Connection-level attribution (Phase 3):** checkpoint/mutation history
   attribution comes from per-connection tracking, never Yjs origins.
5. **Gateway awareness hygiene (transition only):** the gateway client must
   not set `quarryCollab.sessionId` awareness state while the legacy
   flusher exists.
