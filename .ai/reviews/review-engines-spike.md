# Quarry document engines: comparative spike

**Recommendation:** Do not select Automerge merely to fix orphaned comments. For Quarry's current online browser-and-agent workflow, central ProseMirror has the strongest result in this spike. Automerge has a real advantage over the tested Yjs bindings when a paragraph splits. It is a reasonable choice if native Rust document commands or independent offline editing are requirements. The evidence does not establish it as the best complete architecture.

This narrows the earlier Automerge preference. Both Automerge and Yjs/Yrs passed native Rust interoperability. All tested editor integrations still need explicit move handling. Automerge's ProseMirror integration also produced editor attribute differences that need attention.

The executable spike is in `.ai/spikes/review-engines/`. Production code and app dependencies are unchanged. Source baseline: `d967c14a0b8d733b556b1702345c871cf50d1e08`.

**What was compared**

| Alternative | Implementation used | What Quarry would own |
| --- | --- | --- |
| Central ProseMirror | Real `prosemirror-collab` clients, an in-memory authority, persisted steps, and explicit comment range mapping | Durable commits, request deduplication, range policy, structural commands, and a JavaScript document engine behind the Rust API |
| Automerge + ProseMirror | Real upstream binding, native Automerge text, cursors, and independent thread records | Schema mapping, range boundary policy, structural commands, and review decisions |
| Durable Yjs + ProseMirror | Real upstream binding, relative positions, independent thread map, binary persistence | Schema and structural operations, review decisions, Rust command semantics |
| Durable Yjs + current Slate core | Quarry's installed Slate/Yjs binding, relative positions, independent thread map, binary persistence | Removal of row reconstruction, structural operations, review decisions, Rust command semantics |

These are stronger alternatives than Quarry's current reconstruction loop. None rebuilds a CRDT from JSON on normal restart. The Slate tests use the core binding beneath Plate. They do not run the full Plate UI.

**The differences that affect correctness**

Two clients start from the same document. One changes its structure. The other comments on `TARGET` and inserts `!` inside it before receiving the structural change. Correct behavior preserves `TAR!GET` in the intended paragraph, preserves the comment on that text, and preserves the structural change.

| Operation racing with typing and a comment | Automerge + PM | Yjs + PM | Yjs + Slate core | Central PM |
| --- | --- | --- | --- | --- |
| Add a link elsewhere in the paragraph | Text and target survive; link title differs between views | Pass | Pass | Pass |
| Make the paragraph a list item | Pass | Target disappears; edit is lost | Pass with Quarry-style list attributes | Pass |
| Change paragraph to heading | Text and target survive; internal block flag differs between views | Target disappears; edit is lost | Pass | Pass |
| Split immediately before target | Pass | Edit moves to the wrong side; comment covers `!` | Same failure | Pass |
| Move paragraph | Edit and target can attach to the wrong paragraph | Edit and target can attach to the wrong paragraph | Edit is lost; target disappears | Edit is lost; target collapses |

The ProseMirror list test wraps a paragraph in a list. The Slate list test changes `listStyleType` and `indent`, as Quarry does. These are different representations of the same user action. The move test uses delete-and-insert in ProseMirror and `move_node` in Slate.

The strongest counterexample uses repeated text:

```text
Before:
  See docs and TARGET here.
  See docs and TARGET elsewhere.

Client A: move the first paragraph below the second.
Client B: comment on TARGET in the first paragraph; type ! inside it.

Required:
  See docs and TARGET elsewhere.
  See docs and TAR!GET here.

Automerge + PM and Yjs + PM actually produced:
  See docs and TAR!GET elsewhere.
  See docs and TARGET here.
```

In this case the comment still quotes `TAR!GET`. A test that checks only the quoted text would pass while the attachment is wrong. The spike checks the full resulting text and editor tree as well.

The three ProseMirror candidates ran 10 scenarios across ASCII, emoji/combining characters, and repeated text, in two delivery/commit orders: 180 runs. Automerge met the text-and-target requirement in 54 of its 60 runs, but exact editor-tree agreement reduced that to 42. Central PM met the full checks in 54 of 60. Yjs + PM met them in 36 of 60.

The Slate core ran nine related scenarios across the same three fixtures and two delivery orders: 42 of 54 met the full checks. Its scenario set and list representation differ, so these totals are not a common score for ranking all four architectures.

All candidates preserved surviving text after partial target deletion and retained the thread after full target deletion. All passed the current-target save/reload case. These properties do not distinguish Automerge.

Evidence: `binding-cases.mjs`, `editor-engines.mjs`, `binding-results.json`, `slate-cases.mjs`, and `slate-results.json`.

**Why Automerge wins the split case, but not moves**

Automerge stores rich text as a text sequence with block markers. In the tested binding, inserting a paragraph boundary preserved the identity of the following characters. Its upstream rich-text API explicitly separates text, marks, and block markers. [Automerge rich text](https://automerge.org/docs/reference/documents/rich-text/)

The tested Yjs editor bindings moved text into a newly created text container during a split. Positions in the old container could not follow that text. This is a result of these representations and operations, not a proof that Yjs cannot support correct splits.

For moves, all four editor paths deleted or recreated content in a way that failed to preserve the pending edit's intended attachment. Automerge's text diff also matched repeated text in the wrong paragraph. Choosing a different CRDT does not define what a move means.

A separate native test stores each block's body under a stable block ID and changes only its order field when moving it. Both Automerge and Yjs preserve the concurrent edit and comment in that test. This proves that preserving the text object solves this particular move. It does not prove a complete tree model. The test has one mover, no editor binding, no concurrent parent changes, and no paragraph split across separate bodies. A per-block model can introduce new work for splits and ranges that cross blocks.

The existing Slate binding has a useful additional feature: it migrates stored positions during moves. A test with its actual `storePosition` API preserved an already-known comment. The same move lost a comment that existed only on the other client. Repairing known anchors cannot repair anchors that have not arrived yet.

Evidence: `suite.mjs` stable-block cases; the final two cases in `slate-results.json`; installed `@slate-yjs/core` move/split implementations.

**Automerge's integration costs are concrete**

The link test produced `title: null` in one editor and `title: ""` in the other. The heading test produced `isAmgBlock: false` versus `true`. Text, heading level, and comment targets survived. These are editor projection differences; this spike does not show an Automerge core merge failure. They still fail an exact editor-state invariant and need normalization or binding fixes before migration.

The upstream ProseMirror integration describes itself as beta software and requires a schema adapter. That matches the need to validate Quarry's complete schema before adopting it. [Automerge ProseMirror binding](https://github.com/automerge/automerge-prosemirror)

A plain pair of Automerge cursors also did not implement the desired exclusive end boundary. Inserting `+` exactly after `TARGET` made the target quote `TARGET+`. The cursor `move` option controls behavior after deletion; it is not insertion affinity. Native Automerge marks with `expand: "none"` passed both boundary tests. Yjs relative positions with explicit association also passed both tests. Integrating native review ranges with the editor remains application work.

**Rust and persistence: real tests, different responsibilities**

Both native paths passed this sequence:

1. JavaScript creates text containing an emoji and a combining character, a bold range, and a comment target.
2. JavaScript saves the actual CRDT bytes and cursor references.
3. Native Rust loads those bytes, resolves the JavaScript target, inserts `!` inside it, and saves.
4. JavaScript reloads the Rust output and verifies the text, target, and bold range.

Automerge 3.4.1 interoperated with Rust `automerge` 0.11.0. Yjs 13.6.31 interoperated with `yrs` 0.27.2. Both native implementations needed explicit UTF-16 indexing to match JavaScript. This covers text, formatting, and target identity. It does not cover Quarry's entire editor schema.

Automerge still has an architectural distinction: JavaScript uses its Rust core through WebAssembly. Yjs and Yrs are separate implementations. The successful test establishes interoperability for the fixture, not that these maintenance models are identical. [Automerge implementation](https://github.com/automerge/automerge)

Both CRDTs also accepted a delayed comment created before a binary save/restart. Rebuilding either from plain JSON lost the cursor identity. Central PM passed the delayed-client restart test when its step history was retained. Resetting its version and discarding history broke it. A central design must retain enough history to rebase old clients, or explicitly reject clients older than a retained-history boundary.

Evidence: `rust/src/main.rs`, `verify-interop.mjs`, `interop-results.json`, `suite.mjs`, and `central-restart-results.json`.

**Review decisions still need an authority**

Two clients independently checked that a proposal was open, inserted its text, and marked it accepted. Both Automerge and Yjs converged to an accepted proposal with `APPROVEDAPPROVED` in the document. Convergence did not enforce one acceptance.

A small authority test serialized decisions and deduplicated request IDs. Two distinct acceptance requests plus a retry produced one insertion. This policy can sit above any candidate. The test does not implement database transactions or crash recovery; those remain necessary for a production authority.

Evidence: the acceptance cases in `suite.mjs` and `core-results.json`.

**Cost difference measured locally**

This is a core-operation microbenchmark on an Apple M5 Max, Node v26.0.0. Each engine starts with 100,000 characters and receives 500 scattered single-character insertions. Results use five measured runs after one warmup. There is no DOM, network, disk I/O, or comment mapping in the timing.

| Engine | Median total time for 500 edits | Median reload time | Saved bytes |
| --- | --- | --- | --- |
| Automerge | 14.58 ms | 29.22 ms | 6,691 |
| Yjs | 1.12 ms | 0.34 ms | 114,499 |
| Central ProseMirror | 2.64 ms | 0.22 ms | 248,579 |

Automerge was slower in this workload. Its saved representation was much smaller. The input is deliberately repetitive, which helps compression. Retention guarantees also differ: Automerge keeps change history; Yjs state is not equivalent historical storage; the PM measurement includes initial/current snapshots and the step log. These byte counts are not an equal-retention comparison. These times do not establish browser typing latency, memory use, cold WebAssembly startup, or large-document scalability.

Evidence: `benchmark.mjs` and `benchmark-results.json`, which also contain the 10,000-character run and per-operation percentiles.

**What this says about Quarry's current bug**

The production codec reproduced two failures independently of the alternative engines:

1. Adding only a comment to a paragraph with an inline link replaced existing character identity. The same operation on a plain paragraph preserved it. The document's visible text and formatting stayed equal in both cases.
2. Projecting a comment attached to proposed insertion text retained the suggestion but dropped the comment anchor.

Existing source then treats a surviving review record with no extracted anchor as orphaned. The current gateway also kills an overlapping anchor even if some of its target text survives. These are application and reconstruction rules. A library swap that retains those rules will retain these bugs.

Evidence: `baseline.py` and `baseline-results.log`; `crates/quarry-collab-codec/src/session_doc.rs` (`append_leaf`, `clobber_inline_children`); `crates/quarry-server/src/session.rs` (`reconcile_prior_root`); `crates/quarry-server/src/gateway.rs` (`adjust_anchor`). The baseline runner temporarily creates an integration test from existing test helpers and removes it in `finally`.

**Architecture decision supported by the evidence**

For the stated online review workflow, I would choose one central ProseMirror document engine. Browser edits and agent commands would use that engine. It would commit document changes, review records, and request results together. Markdown, block rows, and highlights would be views of the committed document. Keep discussion state independent of target state. Keep proposal content addressable while it is under review.

That choice has a real cost: Quarry needs a JavaScript document engine behind its Rust API, rather than reimplementing ProseMirror semantics in Rust. It also needs explicit move semantics and a rule for edits that cannot be safely rebased. The ordinary ProseMirror move in this spike failed; the recommendation does not imply those requirements are already solved.

If independent offline editing or a native Rust command engine is a firm requirement, Automerge becomes the stronger candidate to develop. Its flat rich-text sequence solved the tested split problem, and its shared Rust core is useful. Adoption would still require fixing the demonstrated projection differences and proving moves, schema coverage, and review behavior.

Keeping Slate and persisting Yjs is a valid smaller migration. It fixes neither the tested concurrent split nor move by itself. Switching from Slate to the tested Yjs–ProseMirror binding introduced additional failures for headings and lists, so this spike gives no reason to prefer that combination.

**Reproduce and interpret the results**

```sh
cd .ai/spikes/review-engines
npm ci --ignore-scripts --no-audit --no-fund
cargo build --locked --manifest-path rust/Cargo.toml
npm test
npm run benchmark
```

The Slate comparison uses Quarry's existing `ui/node_modules`: Slate 0.120.0, `@slate-yjs/core` 1.0.2, Yjs 13.6.31. The isolated spike pins Automerge 3.4.1, `@automerge/prosemirror` 0.2.0, `y-prosemirror` 1.3.7, and `prosemirror-collab` 1.3.1. Package and crate lockfiles contain the remaining exact versions.

Before installing, all 86 locked npm package entries and 76 locked registry crates were checked against a release-age cutoff of 2026-09-04 02:04:36 UTC. Crates newer than the cutoff were downgraded before download/build. The audits are saved beside the lockfiles. No app dependency was upgraded.

`npm test` checks 180 ProseMirror binding runs, 56 Slate binding runs, 15 core assertions, delayed-client restart, native interoperability, and two production-code counterexamples. Some assertions deliberately prove a failure. A successful run means the recorded observations were reproduced. It does not mean every candidate meets every product requirement. Unexpected errors or changes to the recorded outcomes fail the runner.

**Limits of the proof:** ProseMirror tests run actual `EditorView` plugins under jsdom. Slate tests run the actual core binding without the Plate UI. The in-app browser was unavailable, so there is no real-browser validation. Delivery is controlled with two clients and duplicate delivery, not randomized network fuzzing. This spike does not validate IME input, undo/redo, tables, wiki links, Markdown round trips, complete suggestion acceptance with target transfer, arbitrary concurrent tree moves, database crash recovery, or production scale. It proves the listed counterexamples and successful fixtures, not correctness under all possible edits.
