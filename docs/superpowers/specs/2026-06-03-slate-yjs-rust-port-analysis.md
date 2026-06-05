# Slate <-> Yjs Rust port analysis
**Date:** 2026-06-03 **Status:** Analysis note **Related spec:** `docs/superpowers/specs/2026-06-03-collaborative-editing-design.md`
## Question
The collaborative editing design defers "silent merge of agent edits into a live human Yjs session" because the server would need Slate <-> Yjs conversion. This note records what TypeScript code currently owns that behavior and how much of it would need to be ported to Rust for the deferred work.
## Current source inventory
Quarry itself currently has **0 LOC** of TypeScript Slate <-> Yjs implementation. Quarry's editor has Markdown/RFM <-> Plate conversion in `ui/`, but no Yjs binding implementation.

The Slate <-> Yjs behavior comes from three places:

1. `@slate-yjs/core@1.0.2`, installed in Potion at: `../potion/node_modules/@slate-yjs/core`
  
2. Plate's wrapper package, local checkout: `~/p/udecode/plate/packages/yjs/src`
  
3. Potion's application/server wiring: `~/p/fabro-sh/potion/src/server/yjs` and editor provider files
  
## Actual Slate <-> Yjs codec
The real binding is `@slate-yjs/core@1.0.2`.

{==Potion's installed package is compiled JS, but `dist/index.js.map` embeds the original TypeScript source paths and contents. The source map reports **2,238 LOC** of runtime TypeScript source.==}{>>sdfa<<}{id="c1" by="user" at="2026-06-05T13:15:46.016Z"}

Important source-map entries:

| Source path from map | LOC | Role |
| --- | ---: | --- |
| `../src/utils/convert.ts` | 49 | Whole-tree conversion: `yTextToSlateElement`, `slateNodesToInsertDelta`, `slateElementToYText` |
| `../src/utils/delta.ts` | 98 | Y.Text insert-delta normalization, length, and slicing |
| `../src/utils/slate.ts` | 12 | Slate property extraction |
| `../src/utils/location.ts` | 158 | Slate path/offset <-> Yjs offset mapping |
| `../src/utils/position.ts` | 295 | Slate point/range <-> Yjs relative-position mapping |
| `../src/applyToYjs/*` | 426 | Slate operation -> Yjs mutation |
| `../src/applyToSlate/*` | 326 | Yjs event -> Slate operations |
| `../src/plugins/withYjs.ts` | 285 | Slate editor wrapper that connects local ops and remote Yjs events |
| `../src/plugins/withYHistory.ts` | 184 | Yjs history/undo integration |
| `../src/plugins/withCursors.ts` | 270 | Awareness/cursor integration |
| Shared helpers (`object.ts`, `clone.ts`, `yjs.ts`) | 135 | Utility support |

{~~Type-only declarations not included in the JS source map add about **46 LOC**:~>adf~~}{id="s1" by="user" at="2026-06-05T13:16:07.012Z"}

- `dist/model/types.d.ts`: 38 LOC
  
- `dist/applyToYjs/types.d.ts`: 8 LOC
  
### Minimal codec subset
For whole-tree or whole-block conversion, the smallest relevant subset is roughly:

- `utils/convert.ts`: 49 LOC
  
- `utils/delta.ts`: 98 LOC
  
- `utils/slate.ts`: 12 LOC
  
- type definitions: about 46 LOC
  
- parts of `utils/location.ts`: up to 158 LOC when locating block/range targets
  
- maybe `object.ts` / `clone.ts`: up to 124 LOC when manipulating attributes/marks
  

That is roughly **200 LOC** for basic Slate node <-> `Y.XmlText` conversion, or **350-500 LOC** if block/range location and mark-safe edits are included.
## Plate wrapper
Plate's local package is `@platejs/yjs` in `~/p/udecode/plate/packages/yjs/src`.

The local checkout is version 53.0.0, while Potion pins 52.0.13. Potion's installed 52 package does not include TypeScript sources or sourcemaps, so these paths are used as the best inspectable source reference.

Relevant files:

| File | LOC | Role |
| --- | ---: | --- |
| `packages/yjs/src/utils/slateToDeterministicYjsState.ts` | 66 | Deterministic initial Yjs update from Slate value |
| `packages/yjs/src/lib/BaseYjsPlugin.ts` | 351 | Yjs init, provider creation, shared root selection, initial seeding |
| `packages/yjs/src/lib/withPlateYjs.ts` | 64 | Chooses shared `Y.XmlText` and applies Yjs/cursor/history wrappers |
| `packages/yjs/src/lib/withTYjs.ts` | 38 | Thin typed wrapper around `@slate-yjs/core` `withYjs` |
| Provider registry/types/built-ins | 508 | Provider lifecycle abstraction, mostly not relevant to Rust codec |
| `packages/yjs/src/react/YjsPlugin.tsx` | 7 | React wrapper |

Total inspected Plate wrapper source: **1,034 LOC**.

The conversion-sensitive Plate parts are much smaller:

- deterministic seeding: about **66 LOC**
  
- shared root choice and init flow: a subset of `BaseYjsPlugin.ts`, especially the default `ydoc.get('content', Y.XmlText)` and initial `slateNodesToInsertDelta` path
  
- typed wrapper around `withYjs`: about **38 LOC**
  
## Potion usage
Potion is the clearest example of server-side Yjs persistence with Slate conversion:

| File | LOC | Role |
| --- | ---: | --- |
| `src/server/yjs/document.ts` | 161 | Loads/stores Yjs snapshots and calls the Slate/Yjs conversion functions |
| `src/server/yjs/server.ts` | 186 | Hocuspocus server callbacks and persistence lifecycle |
| `src/server/yjs/auth.ts` | 66 | Auth helper, not conversion |
| `src/server/yjs/types.ts` | 18 | Context/document types |
| `src/components/editor/plate-provider.tsx` | 317 | Client provider setup and `YjsPlugin` init/destroy |
| `src/registry/ui/remote-cursor-overlay.tsx` | 133 | Cursor rendering |

The actual server-side conversion in Potion is only a few lines in `document.ts`:

- Slate -> Yjs: get `content` as `Y.XmlText`, delete current content, apply `slateNodesToInsertDelta(value)`
  
- Yjs -> Slate: get `content` as `Y.XmlText`, call `yTextToSlateElement(sharedRoot)`
  

Potion demonstrates that the app glue is small when a JS server can call `@slate-yjs/core` directly. Quarry's Rust server cannot call that package without either porting it or embedding/calling JS.
## Quarry Markdown/RFM codec
The deferred feature is not only Slate <-> Yjs. Agents submit Markdown/block operations, and Quarry's durable document format is Markdown with RFM review markup.

Today, Markdown/RFM <-> Plate lives in the UI:

| File | LOC | Role |
| --- | ---: | --- |
| `ui/src/features/editor/markdown-codec.ts` | 112 | Markdown <-> Plate value orchestration |
| `ui/src/features/review/rfm-codec.ts` | 113 | RFM Markdown <-> Plate review marks and metadata |
| `ui/src/features/review/apply-critic-markup.ts` | 144 | CriticMarkup token expansion into Plate marks |
| `ui/src/features/review/review-md-rules.ts` | 66  | Review mark serialization rules |
| `ui/src/features/review/endmatter.ts` | 85  | Review YAML endmatter parsing/serialization |
| `ui/src/features/editor/remark-inline-marks.ts` | 84  | Remark inline-mark handling |

Known inspected subtotal: **604 LOC**, excluding imported Plate Markdown, remark, GFM, wiki-link, table, mermaid, and image behavior.

This matters because a Rust-only silent agent merge needs some way to turn an agent's block markdown into the same Slate-ish structure the browser expects inside the live `Y.XmlText`.
## What Rust would need for the deferred feature
The deferred feature is:

> An agent's block edit or suggestion merges silently and instantly into the live `Y.Doc`, with the same CRDT behavior as another human's keystrokes.

That does **not** require a full headless Slate editor in Rust for the first useful version. The browser remains responsible for translating received Yjs updates into Slate editor operations.

The Rust server needs to:

1. Locate the live room's `yrs::Doc`.
  
2. Find the shared root `Y.XmlText` equivalent at key `"content"`.
  
3. Locate a target block by stable block id.
  
4. Convert replacement/inserted block markdown into the Slate/Yjs nested `XmlText` representation.
  
5. Mutate the live `yrs` document transactionally.
  
6. Broadcast the resulting Yjs update to connected browser peers.
  
7. Persist or schedule flush without reintroducing a stale Markdown boundary conflict.
  

For suggestions as live Yjs marks, it additionally needs to:

1. Emit the same mark/attribute shape used by Plate's suggestion layer.
  
2. Preserve `suggestion_<id>` attributes and metadata such as `id`, `type`, `userId`, and `createdAt`.
  
3. Decide where review metadata lives during a live session, since RFM endmatter is at-rest Markdown and not automatically present as Yjs marks.
  

Current in-memory suggestion shape in Quarry:

```ts
interface SuggestionMark {
  id: string;
  type: 'insert' | 'remove' | 'update';
  userId: string;
  createdAt: number;
}
```

It is read from leaf properties named `suggestion_<id>` plus a generic `suggestion: true` marker.
## Porting estimates
### Narrow MVP: block-only live injection
Scope:

- Whole-block `replace_block`, `insert_before`, `insert_after`, `delete_block`
  
- No server-side inline range edit
  
- No live suggestions as Yjs marks
  
- Possibly no Markdown/RFM parsing beyond a constrained block subset
  

Approximate Rust implementation:

- **800-1,500 LOC** production code
  
- Mostly a narrow Slate/Yjs block codec, block-id lookup, `yrs` transaction logic, and tests
  

TS semantics being replicated:

- `utils/convert.ts`
  
- `utils/delta.ts`
  
- `utils/slate.ts`
  
- subset of `utils/location.ts`
  
- Plate deterministic/shared-root conventions
  

This is the smallest version that makes agent block edits appear in a live human session without waiting for a Markdown re-read.
### Practical deferred feature: block + inline range + review marks
Scope:

- Block ops
  
- `replace_range` / `find_replace_in_block`
  
- Live suggestion insertion/removal marks
  
- Enough Markdown/RFM conversion to transform agent-submitted markdown into the live Plate/Yjs representation
  

Approximate Rust implementation:

- **2,000-3,500 LOC** production code
  
- Significant fixture/property testing required
  

TS semantics being replicated:

- core conversion subset
  
- location/range mapping
  
- parts of apply-to-Yjs semantics
  
- Quarry's RFM mark shape
  
- enough Markdown/RFM parsing to avoid diverging from the UI codec
  

This is the likely cost of implementing the deferred section as product users would understand it.
### Full headless Slate/Yjs parity
Scope:

- Reproduce full `@slate-yjs/core` operation behavior
  
- Preserve all editor-level semantics, history, positions, cursor-sensitive ranges, and remote event translation behavior
  

Approximate Rust implementation:

- **4,000-6,000 LOC** production code
  
- Large compatibility fixture suite
  
- High maintenance burden whenever Slate, Plate, or `@slate-yjs/core` changes
  

This is probably not worth doing unless the Rust server is intended to become a true headless Slate peer.
## What not to port
For the deferred server-side merge feature, Rust should not need:

- `applyToSlate/*`: browsers already translate incoming Yjs events into Slate operations.
  
- `withYjs.ts`: Rust is not wrapping a Slate editor instance.
  
- `withYHistory.ts`: history is a browser/editor concern.
  
- `withCursors.ts`: cursor awareness is separate from content mutation.
  
- most of `position.ts`: unless the server starts preserving selections/bookmarks or doing complex range surgery.
  
- Plate provider lifecycle wrappers: Quarry will use Rust `yrs` and Axum WebSocket infrastructure.
  
## Testing strategy to keep TypeScript and Rust in sync
If Rust ports any Slate <-> Yjs behavior, treat the TypeScript implementation as the **canonical oracle** and make Rust continuously prove compatibility against it. Do not rely on a visual or line-for-line port review.
### Golden fixture corpus
Create a shared fixture directory, for example:

```text
fixtures/slate-yjs-compat/
  manifest.json
  documents/
    simple-paragraph.json
    nested-blocks.json
    review-suggestion.json
    table-alignment.json
  operations/
    replace-block.json
    insert-before.json
    range-replace.json
```

Each fixture should include the relevant inputs and expected observable outputs:

- input Markdown/RFM, when the Rust path includes Markdown parsing
  
- canonical Plate/Slate JSON
  
- operation payloads, such as `replace_block`, `insert_before`, or `replace_range`
  
- expected normalized Plate/Slate JSON after the operation
  
- optional deterministic Yjs update bytes only when byte stability is intentional
  

The corpus should cover:

- empty documents and empty text leaves
  
- nested blocks and mixed inline marks
  
- stable nanoid block ids
  
- comments and suggestions
  
- substitution suggestions (`remove` + `insert` with one shared id)
  
- links, tables, code blocks, mermaid blocks, wiki links, and images where supported
  
- emoji and other UTF-16-sensitive text
  
- deletes, inserts, splits, merges, and replacement near block boundaries
  
- malformed or partial RFM that should remain literal text
  
### TypeScript oracle generator
Add a Node/Bun script that runs the pinned TypeScript stack and writes canonical fixtures. It should call the same code the UI uses:

- `markdownToReview`
  
- `reviewToMarkdown`
  
- `slateNodesToInsertDelta`
  
- `yTextToSlateElement`
  
- Plate's deterministic seed path when testing initial Yjs state
  

The script should output normalized JSON with deterministic object key ordering where possible. It should also write a compatibility manifest containing:

- `@slate-yjs/core` version
  
- `@platejs/yjs` version
  
- Plate package versions relevant to Markdown and review behavior
  
- Quarry fixture generator version or git SHA
  

CI should fail if those package versions change and fixtures were not regenerated.
### Rust compatibility tests
Rust tests should load the same fixtures and verify observable behavior:

- Rust Markdown/RFM parsing matches the TypeScript-generated Plate/Slate JSON, if Rust implements that layer.
  
- Rust Slate-ish JSON -> `yrs::Doc` -> exported JSON matches the TypeScript oracle.
  
- Rust-applied block operations produce a Yjs state that JavaScript can read via `yTextToSlateElement` as the expected Plate/Slate JSON.
  
- TypeScript-produced Yjs state can be loaded by Rust and then safely mutated.
  
- IDs, marks, block attributes, and review metadata survive round-trips.
  

The highest-value test is cross-runtime:

1. TypeScript creates a `Y.Doc` from a fixture.
  
2. Rust loads the update into `yrs`.
  
3. Rust applies an agent operation.
  
4. TypeScript loads the resulting update.
  
5. TypeScript reads `content` with `yTextToSlateElement`.
  
6. The normalized result matches the expected Plate/Slate JSON.
  

That proves the browser-facing behavior, which matters more than Rust's internal representation.
### Compare behavior, not raw Yjs bytes
Raw Yjs update bytes are often not stable because client ids, transaction grouping, and update ordering can differ while representing the same CRDT state. Prefer these comparisons:

- normalized Plate/Slate JSON from TypeScript's `yTextToSlateElement`
  
- normalized Markdown/RFM from TypeScript's `reviewToMarkdown`
  
- preserved ids and attributes
  

Only compare raw bytes for deliberately deterministic paths, such as Plate's deterministic initial seed.
### Differential and property tests
After the golden corpus is stable, add generated tests:

- Generate small Slate trees with bounded depth and marks.
  
- Generate block-level operations against valid block ids.
  
- Run the operation through TypeScript and Rust.
  
- Compare normalized observable output.
  

Run a small deterministic corpus in every PR. Run broader fuzz/property tests nightly or behind an explicit CI job so ordinary PRs stay fast.
### Fixture regeneration workflow
When the TypeScript stack changes:

1. Run the TS oracle generator.
  
2. Review the fixture diff as an intentional semantic change.
  
3. Run the Rust compatibility suite.
  
4. If Rust fails, either update Rust to match the new TS behavior or explicitly decide the Rust feature does not support that case.
  

This keeps drift visible. The failure mode should be "compatibility fixture changed and Rust no longer matches," not silent divergence in production.
## Recommendation
Do not port the full TypeScript stack.

A safer path is:

1. **Keep v1 as designed:** agents write Markdown over HTTP with ETag backstop; humans use Yjs live sessions.
  
2. **Add a narrow live-injection bridge later:** only whole-block operations by nanoid block id.
  
3. **Defer inline range operations and live suggestions:** keep them on the HTTP Markdown path until block-level Yjs injection is proven stable.
  
4. **If live suggestions become a requirement:** first define where review metadata lives in Yjs, then implement only the mark schema needed by Quarry's review layer.
  
5. **Avoid server-side full Slate parity:** it creates a second implementation of a fast-moving editor codec and will be hard to keep in lockstep.
  

The honest estimate is:

- **~1-1.5k Rust LOC** for a narrow block-only MVP.
  
- **~3k Rust LOC** for the deferred feature as written.
  
- **Full parity is not worth porting** unless Quarry intentionally grows a headless Slate runtime on the server.
