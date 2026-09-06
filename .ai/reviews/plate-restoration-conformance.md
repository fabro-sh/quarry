# Plate restoration conformance

This report records the earlier navigation checkpoint. The [HTTP, Git, FUSE and browser concurrency report](http-browser-concurrency.md) contains the current work. Counts and pending checks below refer to this historical checkpoint.

This is a checkpoint, not a completion report. Quarry uses the actual Plate/Slate editor. Automerge remains the authority for document content, structure, comments, suggestions, history and durable saves. The browser uses the same Rust document engine through WASM.

The local QA server was updated without changing its document blocks or reviews. Chrome 152.0.7977.77 opened the restored editor with one editable document, a Saved status and no page errors. The SQLite files were copied after a graceful server shutdown. The document and plan event streams were reconnected.

## Restored behavior

The restored controls include the floating formatting toolbar, comment drafts and review cards, block menus and dragging, table controls, wiki-link chips, images, source editors and Contents navigation. Slate operations become native commands. Native undo preserves character identity. UI tests replay browser-generated requests in a separate Rust process and compare the resulting document.

Proposed table edits retain existing character sources through column insertion, row insertion and movement, removal, undo and acceptance. Removing every block in a proposal rejects that proposal. Rejected proposal text remains in history, and its comment targets become hidden until undo restores the proposal.

Private source drafts follow their native blocks through moves and proposal acceptance. Removed or incompatible blocks leave a recoverable draft. A new edit that finishes local storage during a state refresh keeps the status at Saving until its own publication completes.

## Navigation fixes

The mode menu restored focus in a later task. If the user selected document text before that task ran, the menu moved focus back to its button. Eight of thirty stress runs reproduced lost input. The menu now preserves focus already placed outside it. A deterministic test forces this ordering and also checks keyboard dismissal.

Page hide cancels an unfinished state read. Restoring a cached page resumes synchronization. Durable command delivery continues independently. Tests cover immediate and delayed restoration, pending local edits, remote changes, closing a session with a blocked read, and an unfinished command response.

WebKit can report a new fetch refused during navigation as an access-control page error even when its rejection is caught. The failure trace places this diagnostic after the main-frame navigation request and before the new page loads. The test harness excludes only that WebKit diagnostic for an exact same-origin native state endpoint during a main-frame navigation. Tests require active-page access errors and application teardown errors to remain visible. Other errors are not excluded by this rule.

## Verification at this checkpoint

| Check | Result |
| --- | --- |
| Rust workspace, all features, macOS | 587 passed; zero ignored |
| Rust default workspace, macOS | 576 passed; zero ignored |
| Rust workspace, all features, Linux | 586 passed; zero ignored |
| UI suite | 187 passed in 19 files |
| TypeScript and production build | Passed |
| Architecture inventory and diff checks | Passed |
| Production browser suite before the narrow WebKit diagnostic exception | 132 passed; one diagnostic failure; two explicit WebKit native-drag skips |
| Targeted WebKit navigation checks after that exception | 15 passed |
| Pinned Chromium input benchmarks | 15 passed |
| Formatted large-document benchmark | Passed in Chromium, Firefox and WebKit |

The final browser run for this checkpoint is still running. Native feature variants, Clippy, schema generation and the broader engine conformance results are retained in the earlier restoration logs. The engine has not changed during this navigation checkpoint.

## Chrome responsiveness

The production benchmark types 100 rapid keys in a 114,814-byte document with 1,401 blocks. It tests the first, middle and last blocks. Concurrent cases type 400 keys while an agent adds a comment and edits another paragraph. The gates require p95 keydown-to-next-frame latency at or below 20 ms, no frame gap above 50 ms, and exact saved text and targets after reload. Trace snapshots are disabled because their DOM walks affect the measurement.

All fifteen repeated pinned-Chromium runs passed. Rapid-typing p95 ranged from 10.2 to 14.0 ms. Concurrent p95 ranged from 10.4 to 14.0 ms. The largest frame gap was 22.3 ms. One hundred rapid keys were published in three requests per run.

Installed Chrome 152 produced different results by mode. Its headless run passed both concurrent cases and failed three rapid-typing cases. The first normal-window run passed. The fifteen-run normal-window matrix then passed thirteen cases and failed two because of isolated 90.6 and 125.1 ms frame gaps; one also reached 84.3 ms p95. Five subsequent CPU-profile runs and ten subsequent frame-observer runs passed without reproducing those stalls. Their cause remains unresolved. Successful repeats do not erase the failures.

These are local automation measurements. They do not prove performance on every device or measure physical display latency. No latency thresholds were relaxed.

## Remaining work

Full restoration still requires conformance work for proposed block split/join, structural edits in Suggesting mode, rich paste across proposed and canonical content, and clipboard identity. Partial formatting within wiki-link syntax also needs an explicit Markdown projection contract. These are not accepted as completed migration work.

The installed-Chrome outliers need further investigation. The test harness retains raw frame samples and can record CPU and long-animation-frame data without enabling DOM trace snapshots.

[The checkpoint record](plate-restoration-checkpoint.json) includes measured values, evidence hashes, browser versions, source hashes and the release-server hash. Earlier cutover reports describe historical checkpoints and do not certify the restored editor. No commits, pushes or deployments were made.
