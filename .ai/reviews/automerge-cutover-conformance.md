# Historical Automerge cutover conformance

This report describes the earlier cutover checkpoint. The editor described here has since been replaced by the restored Plate/Slate editor. See [Plate restoration conformance](plate-restoration-conformance.md) for the current restoration status and evidence. The measurements below remain historical evidence.

The application now uses Automerge as the authority for Markdown content, structure, review and history. The browser runs the same Rust engine through WASM. ProseMirror handles input and display. The previous editor, collaboration engine, checkpoint reconstruction and row mutation engine are removed.

This report records the implementation, evidence and limits. All final checks listed below passed after the incremental synchronization and draft-cache fixes.

## What changed

- Every Markdown creation and publication path maintains native state: browser commands, agent transactions, REST, storage puts, staged commits, CLI, Git, FUSE, import, restore and fork. Block rows and Markdown are derived projections.
- Review targets retain native character identity. Split, join, move, proposal acceptance and undo preserve surviving characters. Removing a target retains its discussion with an explicit target state. Repeated text is never used to relocate a native comment.
- State, indexes, versions, transaction records and command receipts commit atomically. Lost responses retry the same durable request. A failed candidate cannot become visible.
- The browser persists requests before transport. Never-sent causal text requests can share one publication. Each command retains its identity; an attempted delivery cannot change its ID or payload. Competing tabs cannot overwrite pending rows.
- After the initial archive, synchronization transfers only missing native changes. Incremental receive checks its base, resulting heads, complete dependencies and unchanged root identity. Native patches invalidate affected caches before full candidate validation. Composition keeps a fixed receive base until its deferred changes can be applied. Queued cache writes capture the received heads that belong to their own bytes.
- Startup imports existing Markdown and review records atomically per document and resumes after interruption. Uncertain imported targets retain their data without guessing an attachment. An upgrade with unpublished binary drafts stops before schema changes so the older version can save them first.
- A `.quarry` archive preserves native history, retained characters, reviews and metadata. Import creates a separate document identity. Existing document history cannot be replaced by an archive or a fresh same-ID document.

## Verification

| Check | Result | Evidence |
| --- | --- | --- |
| macOS Rust workspace, all features | 573 passed, zero ignored | `target/cutover-workspace-incremental-complete.log` |
| macOS default workspace | 562 passed, zero ignored | `target/cutover-default-incremental-complete.log` |
| All-target, all-feature Clippy | Passed with warnings denied | `target/cutover-clippy-incremental-complete.log` |
| Library-only and no-document-feature Clippy | Passed for server, CLI and binary, including test targets | `target/cutover-lib-only-incremental.log`, `target/cutover-no-documents-incremental.log` |
| Native incremental cache/history contracts | Five tests passed; includes repeated differential scenarios | `target/cutover-incremental-native-test.log` |
| Native HTTP sessions | 16 passed, testing both document scopes | `target/cutover-http-incremental.log` |
| UI suite | 156 passed across 17 files | `target/cutover-ui-final.log` |
| Generated API | Passed; all three state address forms include `since` | `target/cutover-api-incremental-generation.log` |
| Linux workspace, all features | 572 passed, zero ignored; offline cached Docker image, serial build | `target/cutover-linux-incremental-serial.log` |
| Server feature variants | 109 temporary-only, 193 library-only, one no-document surface test passed | `target/cutover-tmp-only-tests.log`, `target/cutover-lib-only-tests.log`, `target/cutover-no-documents-tests.log` |
| UI production build and TypeScript | Passed | `target/cutover-ui-build-final.log` |
| Chromium, Firefox, WebKit conformance | 63 passed, zero retries or skips | `target/cutover-browser-final.log` |
| Chrome comparison and concurrent stress | All six repeated runs passed | `target/cutover-chrome-final-repeated.log` |
| Large-document performance across three browsers | Five passed; four explicit non-Chrome skips for the Chrome-only cases | `target/cutover-performance-final.log` |
| Formatting, architecture inventory and diff checks | Passed | `target/cutover-final-static-checks.log` |

The tests include interrupted WASM downloads and retry; delayed IndexedDB writes during remote receive, offline closure and reconnect; Unicode and repeated text; delayed comments; both merge orders; structural changes; late review after proposal acceptance and undo; private drafts; offline close/reopen; lost responses; archive import; authorization and revocation; rollback and restart; concurrent SQL reads; image upload; source drafts; tables; actual keyboard input; accessibility; and narrow viewports. The block capability matrix runs 68 scenarios across all 17 registered block kinds.

WASM editor tests replay their requests in a native Rust process and compare results. Warm-cache and incremental-history tests compare with fresh archive loads, including invalid and incomplete input. A copied test-only orphaning rule and its two obsolete tests were removed; they are not counted as passes.

## Chrome responsiveness

The previous-build comparison uses commit `d967c14a0b8d733b556b1702345c871cf50d1e08`, the same 114,814-byte document with 1,401 blocks, and 100 rapid keys. Runs are sequential on the same machine. The metric measures keydown to the next animation frame.

| Three-run result | Previous build | Final native build |
| --- | --- | --- |
| p50 key-to-frame | 6.3–6.5 ms | 10.8–12.2 ms |
| p95 key-to-frame | 12.0–12.9 ms | 12.9–14.6 ms |
| Maximum frame gap | 13.9–17.2 ms | 16.7–25.3 ms |
| Publication of 100 keys | Original mocked transport | Three real commits in 3.22–3.40 seconds |

The final implementation is slower than the previous build at the median and slightly slower at p95 in this fixture. All three p95 results remain below one 60 Hz frame. These measurements support the responsiveness requirement on this host; they do not prove zero overhead. The native gate requires p95 at or below 20 ms and no frame gap above 50 ms. It also checks publication and exact text after reload. The previous fixture used its original transport mocks. The native fixture uses the release Rust server and release WASM, with the Vite/React development shell. This is pinned Playwright Chromium, not every Chrome version or physical display measurement.

A second stress case types 400 characters while an agent adds a comment and edits the commented paragraph in the large document. It originally found a 117 ms full-archive merge and a 125 ms frame gap. The final three incremental runs measured p95 key-to-frame at **15.1–15.6 ms**, maximum frame gaps at **28.0–30.8 ms**, and native merge times at **8.7–8.8 ms**. The remote review appeared at key 41 or 42 of 400. All input, the agent edit and the exact comment target survived saving and reload.

The final all-browser performance run repeated the Chrome cases successfully: ordinary p95 was 13.4 ms with a 32.3 ms maximum frame gap; concurrent p95 was 15.1 ms with a 24.6 ms maximum gap. The separate formatted 117,488-byte fixture passed in Chromium, Firefox and WebKit. Its p95 automated key-plus-DOM checks were 52, 71 and 56 ms, and exact-text reloads took 1.12, 2.42 and 1.83 seconds. That metric includes automation and assertion overhead and must not be read as key-to-frame latency.

The comparison data also retains the earlier pre-incremental runs. They are not substituted for the final measurements.

See [comparison data](automerge-chrome-comparison.json), [formatted-document measurements](automerge-browser-performance.json), and [baseline instrumentation](chrome-baseline-benchmark.patch). The temporary previous-build worktree was removed after saving the evidence.

## Coverage map and behavior differences

[The coverage map](automerge-cutover-coverage-map.json) classifies all 387 extracted inventory entries, including 271 former UI entries and 35 former server session entries. Some entries are fixture-text false positives. This is a suite-level review map, not a claim that removed tests pass or that each old assertion has an identical replacement.

- Grouped menus replace the floating toolbar and previous review rail/card presentation. Native review actions, target focus, hover and Contents navigation have browser coverage.
- Wiki syntax is retained through Markdown insertion and explicit source editing with linked previews. Automatic conversion of typed brackets into inline wiki chips is absent.
- Table editing, rectangularity, alignment and navigation are tested. Column resizing is absent.
- The document tree remains virtualized. The previous mocked 10,000-document browser fixture was not rerun against native transport.
- Plain Markdown export contains the body and frontmatter. Use native archives for lossless review portability. Existing whole-file review-markup replacement is explicitly unsupported; use review operations. Initial review-markup import is supported.

## Failures found and corrected

The large-document stress test exposed the full-archive receive hitch. Native incremental receive removes that path from ordinary synchronization. A controlled storage test then exposed a cached draft whose recorded receive heads were newer than its bytes. Capturing those heads with the snapshot fixed the failure; offline closure and reconnect now converge in the same test.

One Firefox run failed before opening a document with “no WebAssembly compiler available.” The same case passed five fresh Firefox launches, and both later 63-case matrices passed. The underlying one-time compiler failure was not reproduced. Separately, failed engine initialization was cached forever; initialization now permits retry. Interrupted-download recovery is tested in all three browsers.

## Limits of the evidence

One Quarry process owns a database and CAS root. Structural publication and authorization remain server responsibilities. This is not a multi-server consensus implementation.

Linux tests compile the FUSE adapter and exercise its projection and publication behavior. They do not prove an actual kernel FUSE mount. Browser composition tests use composition events and Chinese text input; they are not physical operating-system IME tests. Browser undo grouping lasts for the current editor lifetime. Native document history and durable drafts survive reload separately.

[The verification record](automerge-verification.json) contains commands, result counts and evidence hashes. [The source manifest](automerge-tested-source-manifest.json) records the application source and generated WASM used for the final checks. CI is configured to enforce the native, UI, browser and performance contracts. These are local verification results, not a claim that a remote CI run has completed.

No commits, pushes or deployments were made. Unrelated user files and worktrees were preserved.
