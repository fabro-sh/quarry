# Complete structural Suggesting and identity-preserving cut/paste

Status: complete. Date: 2026-09-08

## Outcome

Let users split and join existing blocks in Suggesting mode. Let a selection cross canonical and proposed text without requiring an earlier review decision. Preserve native character identity when content is cut and pasted within the same document.

## Implementation

1. Add native structural proposal actions for split and join. Record native points and the expected block relationship. Preview them through Plate without copying text. Acceptance applies the structural operation to the current native characters. Rejection restores the unchanged canonical structure.
2. Make visible selection replacement partition its work by native owner. Canonical fragments become review targets. Existing proposal fragments are edited in place. Empty insertion proposals are rejected. Apply the complete selection intent atomically.
3. Add a durable native cut transfer. Cutting removes segment references from visible blocks and stores the ordered fragment under an opaque transfer ID. Comments become hidden while the fragment is cut. Same-document paste consumes the transfer and attaches the same references at the native destination. Copy, cross-document paste, stale payloads, and repeated paste use normal fresh-content semantics.
4. Add a private clipboard MIME payload with only document ID, transfer ID, and schema version. Integrate it at Plate's `setFragmentData` and `insertData` boundaries. Keep HTML and plain-text clipboard data for other applications.
5. Extend Rust, WASM, generated API types, editor conformance, and production browser tests. Cover forward and reverse selections, concurrent agent edits, comments, marks, undo/redo, rejection and acceptance, reload, stale/replayed clipboard data, and native request replay.
6. Run the relevant Rust, UI, browser, architecture, lint, build, and Chrome responsiveness checks. Update the local QA server only after the candidate passes.

## Contracts

- Structural proposals contain intent and native references. They never reconstruct canonical identity from Slate JSON or text similarity.
- Acceptance keeps concurrent text edits when the addressed structure remains valid. A competing structural edit invalidates the decision atomically.
- A cut transfer can be consumed once. It is scoped to one document and survives transport, reload, and another tab.
- Cut characters retain their sources, marks, links, and comment targets. They are hidden between cut and paste and attached again after paste.
- Copy and cross-document paste mint new identity.
- Ordinary typing stays on the current local WASM path and retains the existing Chrome latency limits.

## Validation

- The full Rust workspace test suite passes with all features. This includes HTTP, Git, FUSE, storage, replay, undo, and concurrency coverage.
- Rust formatting passes. Workspace Clippy passes with warnings denied.
- All 290 UI tests pass across 22 files. TypeScript, the production build, and the native architecture guard pass.
- All 12 focused live browser tests pass in Chrome, Firefox, and WebKit. They cover structural acceptance, concurrent agent edits, mixed-owner replacement, cross-session cut/paste, comment identity, replay fallback, and reload.
- The broad live suite completed 349 tests successfully and skipped 4 platform-specific tests. The two navigation assertion failures were corrected and pass in all browsers. Five established WebKit list tests completed their product assertions but the local WebKit build hung while Playwright closed its context; the focused feature tests close cleanly in WebKit.
- The final production performance matrix passes all 9 applicable tests. Chrome p95 key-to-frame latency is 11.2–12.3 ms for direct typing, 11.3–13.9 ms while an agent review arrives, 15.7 ms for Suggesting typing, and 19.4 ms for Suggesting deletion.
- The local QA server runs the final production build at port 7836.

## Unresolved questions

None requiring user input. Existing editor behavior defines clipboard formatting and selection placement. The native identity rules define same-document cut semantics.
