# HTTP, Git, FUSE and browser concurrency

This report records the concurrency work in `cf2dfc9`. Later deletion-suggestion changes and their validation are covered in [Adjacent deletion suggestions](suggesting-adjacent-deletions.md).

This records the completed concurrency checks for the current candidate. It does not certify the complete Plate restoration or every filesystem editing pattern.

## Defects reproduced and corrected

| Failure | Reproduction | Change |
| --- | --- | --- |
| A late agent comment selected the wrong repeated text | Browser inserts an identical prefix after the agent reads offsets | HTTP transactions require the original `base_clock`; native character identity resolves that read |
| A stale block deletion removed unseen browser typing | Agent deletes a block after browser typing or formatting | Reject the whole request, including earlier operations in that transaction |
| Typing into a deleted block could be acknowledged without visible text | Offline browser reconnects after agent deletion | Reject the native write and retain the browser recovery draft across reload |
| Simultaneous Git, FUSE and HTTP writes deadlocked | Six sequential orders pass; the first simultaneous run times out | All writers take the global operation gate before the document mutex |
| FUSE acknowledged a newer buffer that had never been published | Pause a flush, write a second buffer, then complete the first flush | Serialize publication per handle; acknowledgements cover only captured bytes |
| An open FUSE handle could write into a different document at its old pathname | Rename or delete through HTTP, recreate the old pathname, then flush | Retain document identity; resolve its current path inside publication; preserve a failed buffer |
| Unversioned HTTP Markdown saves discarded intervening browser edits | Send an old whole-file body after browser typing | Existing-document saves require the original read version; missing headers return 428 |
| Plain Git import and FUSE temporary-file replacement overwrote intervening HTTP edits | Prepare a file, edit through HTTP, then publish the old file without a base | Reject changed unversioned replacements and retain the input files |

The lock-order regression also exposed excessive debug future sizes. The large write futures now use heap allocation. No test stack-size limit or latency threshold was relaxed.

## Write contracts

- Agent transactions carry the version of their original read. `/blocks` returns `document_clock`; `/review` returns `baseToken`. Read both again if they differ. Exact retries keep the original body and transaction ID.
- HTTP `If-Match` is strict. A changed head returns 412 without mutation. `X-Quarry-Merge-Base` merges from the original saved version. Conflicting incoming text becomes review material. Existing Markdown without either header returns 428. An unversioned PUT creates only; `If-None-Match: *` makes this explicit.
- CLI Markdown updates use `--base-version`. Git peer sync carries its recorded baseline. Plain Git import can create documents or repeat identical bytes; it cannot replace changed existing Markdown without a baseline.
- FUSE open handles retain identity and the version captured when they open. Flush and close publish in order. New buffered writes remain pending until published. Renames preserve the handle's target. Deletion rejects publication instead of recreating a file or touching a replacement document.
- A temporary-file rename does not identify the version used to prepare that file. Changed replacement of existing Markdown is rejected, and both files remain. This is a filesystem UX limitation. Supporting that pattern safely needs explicit read-version provenance; inferring a version from a filename, current content, or recent reads is insufficient.

The mounted test prepares content from the same open writable handle. Read-close-reopen editor workflows are not covered by that guarantee.

The per-handle failed FUSE buffer is retained in memory. This is not a durable filesystem recovery archive. Browser recovery drafts are persisted and can be exported and imported into a separate document.

## Coverage

`crates/quarry-fuse/tests/mixed_interfaces.rs` uses a real HTTP listener, a real Git worktree and the same FUSE publication code used by the mount. It runs all six write orders and twenty simultaneous schedules. The assertions compare native bytes, HTTP projections, durable state, stable block IDs, exact comment attachments, conflicts and retry receipts. Additional controlled schedules cover overlapping flush/write/close and HTTP rename/delete with pathname reuse.

The Linux kernel test mounts real FUSE. It opens a file before Git and HTTP change the document, then performs kernel reads, truncation, writes, `fsync`, close and rename. It verifies all edits, the comment target, read visibility and document identity. The test is explicitly selected in Linux CI. It is not a mocked mount.

Twenty consecutive mounted runs passed before the final unversioned-write guard was added. The final Linux workspace run also passed the mounted test.

| Final candidate check | Result |
| --- | --- |
| macOS workspace, all features | 606 passed; zero failed or ignored |
| Linux workspace, all features, including mounted FUSE | 606 passed; zero failed or ignored |
| Clippy, all features and targets, warnings as errors | Passed on macOS and Linux |
| UI suite | 193 passed in 19 files |
| TypeScript, production UI build, release server build | Passed |
| Generated API schema, format, architecture inventory, diff checks | Passed |
| Production browser suite | 155 passed; four explicit skips; zero failed |
| Installed Chrome 152, normal windows, production build | 15 passed; zero failed |

The [checkpoint record](http-browser-concurrency-checkpoint.json) contains artifact and source hashes. The Linux run used the cached Rust 1.95 image with networking disabled; it needed no package installation.

The four browser skips are two WebKit native-drag cases that the harness cannot drive and two non-WebKit executions of a WebKit-specific diagnostic negative control. After a Linux-only test allocation fix, Linux lint and all seven mixed-interface tests passed again. The Clippy component was pinned to Rust 1.95, whose manifest is dated April 16, 2026; its installation was confined to disposable containers.

Baseline failure evidence is retained in `target/restore-plate-http-put-version-baseline.log`, `target/restore-plate-mixed-interface-diagnostic.log`, `target/restore-plate-fuse-flush-race-baseline.log`, `target/restore-plate-fuse-lifecycle-baseline.log`, and `target/restore-plate-unversioned-files-baseline.log`.

## Chrome responsiveness and local QA

All fifteen normal-window runs in installed Chrome 152.0.7977.77 passed the unchanged limits: p95 keydown-to-next-frame latency at or below 20 ms, and no measured frame gap above 50 ms. The test document has 1,401 blocks and 114,814 bytes.

Nine rapid-typing runs measured p95 of 9.9–14.2 ms, with a maximum frame gap of 16.5 ms. Six runs with 400 keys plus concurrent agent edits and comments measured p95 of 9.0–10.6 ms, with a maximum frame gap of 17.5 ms. All runs verified exact saved content and comment targets after reload. Fifteen distinct frame-observer artifacts recorded no long animation frames. No other test or build jobs ran during the measurement.

The local QA server was restarted after a graceful shutdown and SQLite backup. Its blocks and reviews were identical before and after restart. A read-only check in installed Chrome found one editor, Saved status and no page errors. It preserved all 41 blocks, one comment and one suggestion. The manual Chrome window was opened, and the document and plan event streams were reconnected.

## Remaining migration work

Plate/Slate is the editor and Automerge is the durable authority. This phase does not complete clipboard cut/paste identity or all canonical structural operations in Suggesting mode. Earlier installed-Chrome frame stalls remain unexplained. Passing subsequent runs does not erase that evidence.
