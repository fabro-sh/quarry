# Inline Rendering of Agent Markdown-Insert Suggestions — Architectural Sketch

Status: design sketch for engineering review

## Goal and motivation

When a human switches to Suggesting mode and types, the proposed content appears inline for every collaborator immediately, Google-Docs style. When an agent proposes multi-block content through `suggestion.add_markdown`, the proposal is visible only as a card in the Comments panel: nothing appears in the document body, and reviewers routinely miss it.

The goal is to make an open markdown-insert suggestion materialize inline in the live session — rendered as suggested (insert-marked) blocks at its anchor position, visible to all connected collaborators, and acceptable or rejectable from both the review rail and the Comments panel. Accepting it converts the blocks to canonical content in place; rejecting it removes them from view without ever having touched canonical rows.

This should be achieved by reusing the existing suggestion representation, not by adding a parallel one. The session already renders suggested content purely from `suggestion_<id>` text marks, regardless of which collaborator produced them, so the feature reduces to teaching the server to synthesize those marks from a rows-backed review item — and teaching the projection to translate them back.

## Current architecture (verified against the code)

- A markdown-insert suggestion is a rows-backed review item whose `context_after` discriminator is `quarry:markdown_insert:v1`; its Markdown payload lives in `replacement` (`quarry-storage/src/blocks.rs`).
- Session seeding is gated by one predicate: `doc_represented` (`quarry-server/src/session.rs`) excludes markdown-insert items, so they never become session marks. Everything downstream — the rail, inline rendering, remote sync — is mark-driven and would work unchanged if marks existed.
- The session model has no "suggested new block" shape. `SessionAnchor` is `(block_id, start, end)`; `seed_session_nodes` can only splice inline insert text within an existing block. Element-level `suggestion` attributes are used only for block deletion.
- Human suggesting-mode typing of a whole new block produces an ordinary element whose text leaves are all marked `suggestion` + `suggestion_<id>` with `type: 'insert'`. At checkpoint, the projection emits that block as a canonical row with empty text plus a collapsed inline insert anchor — a different shape from the markdown-insert item, and one that leaks an empty block into canonical rows while the suggestion is pending.
- The injection machinery already exists: any REST transaction against a live session flows through `apply_session_transaction` → `apply_desired_state`, which minimally edits the shared Yjs doc under the write lock and updates the `ReviewMeta` map, tagged with an `agent-injected:*` origin that browsers treat as a benign refresh.
- ID stability across checkpoints is achieved by embedding the item ID in the mark name and reconciling by ID-keyed upsert (`reconcile_review_items`). Accept for markdown-insert runs through the same engine in rows mode and session mode (`accept_suggestion` → `insert_markdown`), then pushes the resulting canonical image into the session.

## Proposed design

### 1. A session representation for suggested blocks

Materialize an open markdown-insert item as one or more new element nodes placed immediately after its anchor block, with every text leaf carrying `suggestion: true` and `suggestion_<item-id>` (`type: 'insert'`), plus the item's entry in the `ReviewMeta` map. This is exactly the shape human suggesting mode already produces for new blocks, so the editor, rail, and remote-sync rendering require no changes.

Codec work in `quarry-collab-codec`:

- Extend the seed input beyond `SessionAnchor` with a block-insert shape, e.g. `SessionBlockInsert { id, after_block_id, rows }`, produced by parsing the item's `replacement` Markdown with the existing fragment parser.
- Teach `seed_session_nodes` to emit the synthesized elements at the anchor position with all-leaves insert marks.
- Teach the projection to recognize a maximal run of blocks whose every leaf carries the same single insert-suggestion ID, emit **no canonical rows** for that run, and return a block-insert result (ID, preceding canonical block, serialized Markdown) alongside the existing anchors.

### 2. Recognition keyed to existing items (safe phase 1)

In phase 1, the projection maps a fully-insert-marked run back to a markdown-insert item only when the mark ID matches a known open markdown-insert item. Human-typed new-block suggestions (whose IDs are not markdown-insert items) keep today's behavior exactly, so phase 1 changes nothing for human suggesting mode.

Phase 2 can then converge the human path onto the same shape: a human-typed fully-suggested block becomes a markdown-insert item at first checkpoint instead of an empty canonical row plus a collapsed anchor. That would eliminate the empty-block leak into canonical rows (visible today in exports and whole-file reads while a human suggestion is pending) — but it is a behavior change deserving its own review and tests, and the phase 1 keying makes it safely separable.

### 3. Server changes

- `doc_represented` includes open markdown-insert items; `doc_anchors` grows a companion that yields block-insert seeds (or the seed input becomes a single richer image structure).
- `reconcile_review_items` upserts extracted block-insert results into their prior items by ID: content edits inside the suggested blocks update `replacement`; disappearance of the marks (all suggested text deleted by a collaborator) resolves or invalidates the item under an explicit rule.
- `clamped` currently invalidates a markdown-insert item when its anchor block vanishes. With materialization, the anchor can instead be re-derived from the run's position (the preceding surviving canonical block), making the suggestion follow the document. Phase 1 may keep invalidation for simplicity; re-anchoring is a small follow-up.
- `suggestion.add_markdown` arriving while a session is open already flows through `apply_desired_state`; with the above, the blocks appear inline in every client with no additional plumbing. The op should validate that the fragment parses at add time (confirm current behavior) so a malformed payload cannot create an unmaterializable item.

### 4. Accept, reject, and identity

- **Reject** already resolves the item; the next desired-state image simply omits the synthesized blocks and they vanish from all clients. Canonical rows were never touched.
- **Accept** already inserts the parsed fragment as canonical rows and pushes the new image; the marked elements are replaced by canonical ones in place. One refinement is worth considering: mint the fragment's block IDs deterministically at suggestion-add time (persisted alongside the item) so that accept reuses the IDs the session elements already have. The elements then survive acceptance as the same nodes minus their marks — preserving peer cursors and avoiding a visible replace. Without it, accept still works but the elements are swapped.
- Rail cards for these suggestions should route accept/reject through the REST `suggestion.accept` / `suggestion.reject` path (as the Comments panel does), not the editor-local mark-resolution path used for human suggestions, so both surfaces converge on the one accept engine.

### 5. The critical invariant: fixed-point round-trip

Materialize → project → reconcile must be a fixed point: seeding an open item, projecting the session, and reconciling must yield the identical item (ID, anchor, normalized replacement) and an identical next seed. Any drift — a normalization difference between the stored Markdown and the re-serialized run, an off-by-one anchor — turns into blocks flapping in and out of the document on every checkpoint. This invariant should be a codec-level property test over arbitrary fragments and anchor positions, exercised before any server wiring.

Normalization deserves explicit care: `replacement` should be stored pre-normalized (the same canonical Markdown the projection re-serializes), so comparison is byte-equality rather than semantic equivalence.

## What does not change

- Storage schema: the item shape is untouched (the optional deterministic-IDs refinement extends the context payload backward-compatibly).
- Rows mode: documents without a live session keep exactly today's behavior; the Comments panel card remains the actionable surface there and stays as a secondary surface everywhere.
- The editor and rail UI: rendering is mark-driven and collaborator-agnostic; synthesized marks render like human ones. The only UI work is routing rail accept/reject for these items through REST and, optionally, a "shown inline — jump to it" affordance on the panel card.

## Testing

- Codec property tests for the round-trip fixed point (fragments spanning headings, lists, tables; anchors at document start, middle, end; adjacent runs with distinct IDs).
- Checkpoint-stability test: repeated checkpoints with an open materialized suggestion produce byte-identical rows and items (no flap).
- Two-client end-to-end: agent posts `suggestion.add_markdown` mid-session; both browsers show the inline suggested blocks; accept from the rail in one client and verify convergence in the other; reject and verify removal; accept from the Comments panel for parity.
- Edge cases: anchor block deleted while the suggestion is open; two suggestions anchored after the same block (deterministic ordering by creation time, then ID); a collaborator deleting all suggested text; session close and reopen re-seeding the still-open item.

## Open decisions for engineering review

1. Phase 2 convergence: should human-typed whole-block suggestions also become markdown-insert items, eliminating the empty-canonical-block leak? (Recommended eventually; separable.)
2. Deterministic fragment block IDs minted at add time, so accepted elements keep their identity — worth the small payload extension in phase 1?
3. Degradation rule when a materialized run stops being fully-insert (mixed suggestion IDs, partial mark removal): resolve, invalidate, or fall back to inline-anchor shape?
4. Anchor-loss policy in phase 1: keep today's invalidation, or re-anchor to the preceding surviving block immediately?
5. Ordering guarantee for multiple open suggestions at the same anchor: creation time, item ID, or explicit sequence?
