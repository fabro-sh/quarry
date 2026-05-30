# Review markup layer (comments, suggestions, highlights) — design

**Date:** 2026-05-30
**Status:** Approved design, pre-implementation
**Repo area:** `ui/` (quarry-browser, PlateJS editor)

## Goal

Add a review/markup layer to the existing Plate editor: **comments**, **suggested insertions / deletions / replacements** (track changes), and **highlights**, each with author attribution. The review layer must persist **inside the Markdown file** so it survives Git export and is readable in any editor — i.e. the Roughdraft model, adapted to quarry's Plate-based UI.

## Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| **Persistence** | In the Markdown itself | Durable, Git/plain-text portable; quarry's `.md` stays the single source of truth. No backend schema changes. |
| **On-disk format** | Adopt Roughdraft Flavored Markdown (RFM) | CriticMarkup markers + `{#id}` inline refs + YAML endmatter. Battle-tested edge cases + interop with any agent/tool that speaks RFM. |
| **Attribution** | Free-form `by:` label | `user` / `AI` / an agent name. Maps 1:1 to RFM's string `by` field; no identity registry. |
| **v1 scope** | All five markers + live "Suggesting" mode | Closest to Roughdraft; natively supported by Plate's suggestion plugin. |
| **Architecture** | A — Plate marks + RFM codec + client store | Reuse Plate plugins + Potion UI; one codec layer owns Markdown ⇄ RFM. |
| **Concurrency** | Single-writer now, collab-agnostic | nanoid ids + merge-friendly endmatter so a future Yjs layer isn't foreclosed. Real-time collab out of scope for v1. |
| **Block-level suggestions** | Degrade to inline (RFM-pure) | New/removed blocks → inline `{++/--}`; block-type changes applied directly (untracked). Full block fidelity deferred. |

## Reference implementations

- **PlateJS source** (`~/p/udecode/plate`, MIT): `@platejs/comment`, `@platejs/suggestion` (incl. `isSuggesting` mode, `acceptSuggestion`/`rejectSuggestion`), `BaseHighlightPlugin`, and `@platejs/markdown`'s `rules: MdRules` extension surface (`{ mark, serialize, deserialize }` per node/mark key, plus a `remarkPlugins` hook).
- **Potion** (`~/p/fabro-sh/potion`, MIT): production wiring of the above. Reuse `floating-discussion-app.tsx` (rail + collision layout), `block-suggestion-app.tsx` (`useResolveSuggestion` — derives insert/delete/**replace**/update from marks), `comment-app.tsx`, the comment/suggestion toolbar buttons, and the "Suggesting" pill — restyled to quarry tokens, with the data source swapped from tRPC/Postgres → the client store.
- **Roughdraft** (`~/p/Lex-Inc/roughdraft`): the RFM spec, conformance fixtures, and JSON schema we adopt and test against.

**Key divergence from Potion:** Potion persists Plate JSON + a Yjs binary and treats Markdown as export-only (it `disallowedNodes: [KEYS.suggestion]` — drops suggestions — and offloads comment threads to Postgres). Quarry does the inverse: everything round-trips through RFM Markdown. We reuse Potion's *editor UX*, not its *persistence*.

---

## Section 1 — Architecture & module boundaries

**Core idea:** the editor keeps Plate's native review representation in memory (marks + a discussion store); a **codec layer** is the only thing that touches Markdown, translating that representation ⇄ RFM (CriticMarkup body + YAML endmatter). The backend never learns about the review layer — it's all document bytes.

**Data flow (one codec, both directions):**

```
 LOAD                                                     SAVE (debounced)
 ────                                                     ────
 .md bytes                                                Plate value ──┐
   │  splitEndmatter()                                                  │ MarkdownPlugin.serialize
   ├──────────────► body ──remark-parse + remarkCriticMarkup──►        │  (+ review MdRules → CriticMarkup)
   │                         Plate value (comment_/suggestion_ marks)  ▼
   └──────────────► endmatter YAML ──► hydrate ► discussionStore ──► serializeEndmatter()
                                                       ▲                 │
                                              rail UI mutates           ▼
                                                                  body + "\n---\n" + endmatter ──► .md bytes
```

**Modules** (new `ui/src/features/review/`, plus small edits to `features/editor/`). Each unit has one job and a narrow interface:

| Unit | Responsibility | Interface (rough) | Depends on |
|---|---|---|---|
| `review/rfm-types.ts` | TS model for the review layer + endmatter schema | types only | — |
| `review/identity.ts` | "who am I" → free-form `by:` label | `currentAuthor(): string` | — |
| `editor/remark-criticmarkup.ts` | **Parse**: tokenize `{++ ++}`,`{-- --}`,`{~~ ~> ~~}`,`{>> <<}`,`{== ==}`,`{#id}` into mdast nodes; **literal inside code** | unified plugin | mdast, unist-util-visit |
| `editor/review-md-rules.ts` | **Serialize**: Plate `MdRules` for `comment`/`suggestion`/`highlight` → CriticMarkup + `{#id}` | `MdRules` | rfm-types |
| `review/endmatter.ts` | split/join trailing `---` YAML; parse ⇄ serialize the `comments:`/`suggestions:` map | `splitEndmatter`, `parse`, `serialize` | `yaml` (new dep) |
| `review/rfm-codec.ts` | Orchestrator wiring the above into the existing codec | `markdownToReview(md)→{value,meta}`, `reviewToMarkdown(value,meta)→md` | all of the above |
| `review/discussion-store.ts` | Zustand store: threads/bodies/authors/resolved keyed by id; hydrated by codec, mutated by rail | store + actions | rfm-types |
| `editor/review-kit.ts` | Plate plugin wiring: comment + suggestion (+`isSuggesting`) + highlight, nanoid ids, accept/reject | plugin array | @platejs/comment, /suggestion |
| `review/ui/*` | Rail, suggestion cards, comment composer, toolbar buttons, "Suggesting" pill — adapted from Potion | React components | store, editor |

**Changes vs stays:**
- **Extend** `markdown-codec.ts` / `PlateMarkdownEditor.tsx` to route through `rfm-codec` and add `review-kit` to the plugin list. The existing `lastSerializedRef` / reset-echo logic stays and now guards the richer round-trip.
- **Reuse** Potion's rail/cards/composer/toolbar, restyled, with the data source swapped to the Zustand store.
- **No Rust/Turso changes.** Verify (not change): `quarry-git`'s frontmatter / marker-safety treats a trailing `---\n<yaml>` endmatter as opaque content, distinct from leading frontmatter and reserved sidecars.

**Isolation win:** only `rfm-codec` + its three helpers know about Markdown/RFM; the editor and UI work purely in Plate's mark/store model, so the review UX and the on-disk format evolve independently.

---

## Section 2 — RFM format mapping

| Capability | Plate (in-memory) | RFM in the `.md` |
|---|---|---|
| **Highlight** | `highlight` mark | `{==text==}` — no trailing comment |
| **Comment** (on range) | `comment_<id>` mark + store thread | `{==anchor==}{>>body<<}{#id}` + endmatter `id: {by, at}` |
| **Reply** | store entry, *no* doc mark | endmatter `id: {body, by, at, re: <parentId>}` |
| **Suggest insert** | `suggestion_<id> {type:insert}` | `{++text++}{#id}` + endmatter `id: {by, at}` |
| **Suggest delete** | `suggestion_<id> {type:remove}` | `{--text--}{#id}` |
| **Suggest replace** | insert-leaf + remove-leaf sharing one id | `{~~old~>new~~}{#id}` |
| **Resolve comment** | store `isResolved` | endmatter `status: resolved` (+ optional `resolved: summary`) |
| **Accept/Reject suggestion** | removes the mark | marker **disappears** (change applied or reverted) — only *pending* suggestions exist as CriticMarkup |

**Highlight-as-anchor convention:** RFM anchors comments to a `{==...==}` span, so a commented range serializes by wrapping its text as the anchor: `{==this sentence==}{>>body<<}{#id}`. On parse, `{==text==}` *followed by* `{>>...<<}{#id}` → a `comment_<id>` mark; a bare `{==text==}` → a plain `highlight` mark.

**Endmatter schema** (RFM-compatible; `by` is the free-form label):

```yaml
---
comments:
  <id>: { by: <label>, at: <iso>, body?: <md>, re?: <parentId>, status?: resolved, resolved?: <summary> }
suggestions:
  <id>: { by: <label>, at: <iso> }   # suggestion text lives inline; type comes from the marker
```

**Round-trip rules the codec enforces:**
- **IDs = nanoid**, generated at creation. One id space: the Plate mark id *is* the `{#id}` *is* the endmatter key. (Collab-agnostic.)
- **Root comment body**: inline in `{>>...<<}` when simple; falls back to endmatter `body:` when multi-block or containing a `<<}` delimiter (RFM forbids the close delimiter inline). Replies are always endmatter `body:`.
- **Substitution pairing**: on save, an adjacent remove+insert sharing an id collapses to `{~~old~>new~~}`; on load it re-splits into the two leaves (mirrors Potion's `useResolveSuggestion`).
- **Code is literal**: `remark-criticmarkup` skips `code`/`inlineCode` mdast nodes.
- **Overlap**: fully-coincident comments on one anchor stack (`{==a==}{>>..<<}{#c1}{>>..<<}{#c2}`); *partial* overlaps split at boundaries into adjacent anchors (lossy on exact coverage — see Limitations).

**Block-level suggestions (v1 policy = degrade to inline):** a suggested new/removed block → inline `{++...++}`/`{--...--}` spanning its text + break (RFM-pure, round-trips as standard CriticMarkup). Block-type changes (e.g. heading→paragraph) are **not** tracked as suggestions in v1 — they apply directly. Full block fidelity is deferred (see Deferred).

---

## Section 3 — Data flow & editor integration

**Invariant:** two in-memory sources (the Plate value's marks + the discussion store) serialize into one Markdown string, and one Markdown string hydrates both. `rfm-codec` is the single chokepoint.

**Load / external-change** (extends the existing `resetPlateEditor` + `lastContentRef` path):

```
content prop ──► markdownToReview(md) ──► { value, meta }
                                              │        └─► discussionStore.hydrate(meta)
                                              └─────────► editor value (comment_/suggestion_ marks)
```

The `useEffect` that resets the editor when `content` changes also re-hydrates the store, so an agent rewriting the file on disk refreshes prose *and* threads.

**Edit — three mutation sources, one save:**
- *Editor marks* — add comment (`setDraft` → promote), toggle highlight, live **Suggesting mode** (`isSuggesting` reroutes typing/deletion into suggestion marks), **accept/reject** (removes marks). Changes the Plate value.
- *Store-only* — type a reply, edit/resolve a comment in the rail. Changes the store, not the value.
- Save trigger is therefore **"value changed *or* store changed"**. A small store subscription bridges store changes into the same debounced save the editor uses.

**Save** (extends the existing `onValueChange` → `lastSerializedRef` echo-guard):

```
(value, storeSnapshot) ──► reviewToMarkdown() ──► md ──► onChange(md)
   │                          ├─ MarkdownPlugin.serialize + review MdRules → CriticMarkup body
   │                          └─ serializeEndmatter(store ∩ live ids) → "\n---\n<yaml>"
   └─ walk value for live comment_/suggestion_ ids ──► drop orphaned store threads
```

- **Orphan pruning:** only ids still present as marks get endmatter entries. Delete a commented sentence → its mark vanishes → its thread is dropped on the next serialize. Replies survive while their parent id is live.
- **Echo guard reused:** `reviewToMarkdown(value, store)` is compared to `lastSerializedRef`; identical output → skip `onChange`. Because the store is part of the serialized input, a reply-only change still produces a different string and saves; a save round-tripping back as `content` won't re-fire.

**Identity:** `currentAuthor()` (default `"user"`, configurable) feeds Plate's `currentUserId` option and stamps new endmatter `by:`. Agents don't go through the editor — they write RFM into the `.md` directly with their own `by:` label, picked up on the next load (the human↔agent loop).

**Conflict on save** is unchanged: the backend's ETag/precondition handling + `PlateMarkdownEditor`'s external-change reset apply to the whole document blob, so the review layer inherits quarry's existing single-writer conflict behavior.

---

## Section 4 — Error handling, edge cases & testing

**Never lose content — degrade, don't crash:**
- Unclosed `{++` / `{>>` / `{~~` → left as plain text, non-fatal diagnostic.
- **Endmatter guard** (from Roughdraft): a trailing `---` + YAML block is review endmatter *only* if it parses to an object with a `comments:`/`suggestions:` key **and** the body has a `{#id}` ref (or a document-level comment). Otherwise it stays prose — never hijack a doc that legitimately ends in `---`.
- `{#id}` ref with no endmatter entry → render with synthesized defaults (`by: unknown`) + diagnostic, don't drop text. Dangling `re:` → treat reply as top-level.
- Comment body containing `<<}` → auto-route to endmatter `body:`.
- Duplicate ids (possible in imported docs; ~impossible with nanoid) → last-wins + diagnostic.

**Idempotence is a hard requirement.** `serialize(parse(md))` must be **stable for the review regions** — re-saving an untouched doc must not churn markers or reorder endmatter (deterministic key order). This prevents spurious diffs/conflicts and gets its own first-class property test. (Prose may still be lightly reformatted by remark — accepted today.)

**Backend check (not a change):** confirm `quarry-git`'s frontmatter/marker-safety treats a trailing `---\n<yaml>` as opaque content.

**Testing** (aligned with "no loops in tests" / "no shared mutable state in tests"; follows the existing `markdown-codec.test.ts` + `MarkdownEditor.test.tsx` patterns):

| Layer | What | How |
|---|---|---|
| **Round-trip (backbone)** | Each capability + combos parse→serialize→parse stable, model preserved | Fixtures via `it.each` (one reported case each, no in-body loops). Reuse Roughdraft's conformance fixtures + JSON schema as an oracle. |
| **Unit** | endmatter split/parse/serialize; substitution pairing; `remark-criticmarkup` tokenization incl. code-literal; highlight-vs-comment-anchor disambiguation; orphan pruning | Pure-function vitest |
| **Editor integration** | add comment, toggle highlight, suggesting-mode typing→mark, accept/reject, rail reply→store→serialize | Testing Library + jsdom |
| **E2E** | open doc w/ markers → rail renders → add suggestion → save → reload persists | Playwright (already configured; stable selectors) |
| **Locked limitations** | block-suggestion→inline degrade; partial-overlap split; resolved round-trips as `status:` | Explicit assertions so trade-offs are intentional, not regressions |

---

## Known limitations (v1, intentional)

- **Block-type changes** (heading→paragraph, etc.) are not tracked as suggestions — applied directly.
- **Block insert/delete** is represented as inline `{++/--}` spanning the block, losing the "this is a block operation" semantics (e.g. a suggested line break).
- **Partial (non-coincident) overlapping comments** split at boundaries — exact per-comment coverage is approximated.
- **Rich comment bodies** are constrained to Markdown text (no arbitrary Plate block content in a comment body).

## Deferred / open

- **Standalone RFM module (Approach C)**: extract a spec-faithful, scanner-based RFM parser as an independent module once the format stabilizes, to give quarry's CLI/agents server-side RFM validation and reuse.
- **Real-time collaboration (Yjs/CRDT)**: not in v1. The codec + nanoid id scheme are designed not to foreclose it; adding it would make the CRDT the live source of truth and Markdown the at-rest serialization (the Potion model).
- **Quarry RFM block extension**: a future spec extension for full block-level suggestion fidelity (pairs naturally with the C-later module).

## Dependencies & integration checks

- **New UI dependency:** `yaml` (endmatter parse/serialize) and `nanoid` (ids) — confirm versions ≥ 24h old per repo policy before install.
- **Plate packages to add:** `@platejs/comment`, `@platejs/suggestion` (highlight is in `@platejs/basic-nodes`, already present).
- **Verify** `quarry-git` endmatter opacity (above).
