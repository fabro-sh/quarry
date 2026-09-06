# Native block conversion

Verified on 2026-09-06. This replaces the menu-level policy described in `list-block-conversion.md`.

## Boundary

`EditAction::ConvertBlock` expresses a conversion target. The shared Rust implementation in `crates/quarry-document/src/conversion.rs` owns attribute inheritance, list membership, and code wrapping and unwrapping. It runs in both the authority and browser WASM.

A target of `p` without a list means plain text, even when the current native kind is already `p`. A list target specifies its style and optional indentation, checked state, and start number. The converter removes all list properties together and preserves unrelated attributes.

Text-to-code conversion creates an empty container and retains the original text block as a code line. Converting a line out of code preserves the surrounding lines, splitting the container when necessary. Whole-container conversion preserves each line's text identity and order. Canonical and proposed trees use the same conversion implementation.

Flat property conversions reuse native block-update proposals. Structural conversions have a native conversion proposal with an expected structural snapshot. Acceptance keeps concurrent text edits. A competing structural change blocks acceptance without changing the document. Existing native text segments and comment targets are retained through conversion, acceptance, and undo.

The editor menus, list buttons, heading shortcuts, heading/list/code autoformat, and Plate block toggle submit native conversion intents. The adapter handles selection and materializing the empty input; it no longer owns conversion cleanup rules. Table insertion and source-block replacement remain distinct insertion and replacement operations.

The HTTP `set_block_type` operation without explicit attributes uses the same native action. Explicit attributes remain a strict property write. The separate gateway conversion cleanup function was removed. Raw `set_block` still rejects invalid blocks. Generated API types and agent instructions describe the new contract.

## Verification

- 498 Rust tests passed across the document engine, Markdown, storage, and server, with all features. A focused gateway test also passed after adding coverage for inherited list restart properties.
- 248 UI tests passed, including 74 native conformance tests. These replay WASM-generated command requests through the Rust engine and compare results. New cases include autoformat, Plate toggles, empty input, code proposals, and comment preservation.
- The full production browser suite passed 281 tests across Chromium, Firefox, and WebKit. Four existing platform-specific skips remain. New cases compare menus, shortcuts, autoformat, and HTTP results and exercise structural proposals after concurrent agent edits.
- Five focused conversion tests passed in installed Chrome 152.0.7977.77.
- All six isolated, headed Chrome typing benchmarks passed. Worst p95 key-to-frame time was **14.9 ms**; worst frame gap was **27.8 ms** rounded up. Existing budgets are 20 ms and 50 ms. Cases include 100 KiB documents, agent review during sustained typing, and Suggesting insertion/deletion.
- Workspace Clippy and all-feature server/CLI Clippy passed with warnings denied. TypeScript, formatting, architecture checks, production UI/server builds, and whitespace checks passed.

Detailed logs and browser artifacts are in `target/block-conversion/`.

## Local QA

Port 7836 serves the updated build. The closed database was backed up before restart. Document blocks and review data matched before and after restart. Eight installed Chrome tabs loaded with Saved status, one WebSocket each, and no application errors. The read-only verification preserved the document and review data. Existing tabs need a reload to load the updated editor.
