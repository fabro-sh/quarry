# Independent save and refresh progress

The browser now delivers durable edits independently of document refreshes.
An acknowledged edit remains **Saved** when an update read stalls or fails.
Remote update failures have a separate **Reconnecting for updates…** indicator.

The earlier reproduction is recorded in `save-progress-boundary.md`. It held a
document-state read while command delivery remained available. The old session
sent no command until that read completed. It then kept showing Saving after
the server acknowledgement because another read was pending.

## Responsibilities

- `DocumentDelivery` owns local persistence, the ordered outbox, cross-tab save
  ownership, retries, and save status. Buffered editor operations and unfinished
  local writes count as unsaved work. An acknowledgement removes a request
  before status is published.
- `DocumentSession` owns remote reads and their application to the model. A
  notification requests a refresh. Delivery does not wait for that refresh.
  Only receiving remote history advances the receive base; acknowledgements
  cannot move it past history that the model has not received.
- `documentRequest` bounds each attempt, including response-body reads, to ten
  seconds. A timeout or cancellation retains the durable request. A retry sends
  the same identity and envelope. Server receipts prevent a second commit.
- The application notification manager shares subscriptions by origin and
  access scope. Library editor and workspace consumers share one connection.
  Temporary capabilities and invitation tokens retain separate access scopes.
  A temporary page can also subscribe to its library sidebar's events.
- Successful reads schedule another read after fifteen seconds. Lost messages
  are recovered even when the socket remains open. Online, visibility, resume,
  and reconnect events also request progress. Failed attempts retry after three
  seconds. Browser timer throttling can extend these intervals.

The save worker persists before sending, coalesces unattempted edits, and
releases cross-tab ownership between finite passes. A closed or suspended tab
cancels its attempt. Another tab can replay the retained request. Local writes
finish before the outbox is closed. Explicit draft recovery invalidates old
reads so they cannot restore a previous model or metadata.

Plate/Slate still handles input. Automerge still owns document and review
meaning. This change does not replace either layer or add a dependency.

## Verification

All evidence for this change is under `target/save-boundary/`.

- 228 UI tests passed against release WASM (`unit-final-receipts.log`). New coverage
  includes held and failed refreshes, buffered input, slow local persistence,
  silent notification loss, exact retry after a lost acknowledgement, tab
  handoff, stale reads after recovery, wrong acknowledgement identities,
  request-body deadlines, and subscription scope and lifetime.
- TypeScript, the production UI build, the embedded release server build,
  architecture inventory, and whitespace checks passed.
- The full production browser suite passed 197 tests, with four existing
  platform-specific skips, using bundled Chromium, Firefox, and WebKit
  (`browser-bundled.log`). All fifteen new failure cases passed. These hold
  reads while two successive edits save, fail a read after acknowledgement,
  discard all messages on an open socket, lose a committed response, and close
  the tab that owns delivery. They check status, durable requests, exact replay,
  server version counts, reload, and the original comment target.
- The eight-tab cases also verify one notification connection per access scope.
  The missed-notification test waits for initial loading to finish and verifies
  that a later periodic read caused recovery. That test also passed through the
  Vite development proxy in installed Chrome (`dev-proxy-final.log`).
- Six isolated, headed Chrome latency tests passed on Chrome 152.0.7977.77.
  The rapid typing document contains 114,814 bytes. The concurrent tests deliver
  an agent comment and edit during 400 keystrokes, then verify exact reload and
  comment attachment. Suggesting tests also check one grouped deletion proposal.

| Chrome workload | p95 key-to-frame latency | Largest frame gap |
| --- | ---: | ---: |
| Rapid typing, beginning | 12.1 ms | 10.3 ms |
| Rapid typing, middle | 11.7 ms | 16.6 ms |
| Rapid typing, end | 10.6 ms | 9.3 ms |
| Concurrent review, beginning | 9.5 ms | 16.1 ms |
| Concurrent review, end | 10.9 ms | 17.4 ms |
| Suggesting typing | 14.3 ms | 25.1 ms |
| Suggesting deletion | 15.7 ms | 17.4 ms |

Every case met the existing 20 ms percentile and 50 ms frame-gap limits.
Rapid typing sent two or three requests for 100 keys and finished saving within
2.83 seconds. Timing samples are in `latency.json` and `latency-results/`.
These are measurements on this machine and workload, not every device.

The native engine and server source did not change during this boundary fix.
The previous Rust and socket-conformance evidence remains in
`multitab-saving.md`; it is not counted as a new run here.

The first focused browser run stalled in the test runner and was stopped.
The isolated handoff test then exposed a test assumption: the editing tab is
not necessarily the tab delivering the shared outbox. The test now records and
closes the actual sender. The socket assertion now checks one connection per
scope, including the separate library sidebar on temporary pages. The initial
logs remain available; they are not counted as passing suite runs.

The traced full run completed its Chrome assertions but stalled while retiring
workers. Read-only debugger inspection found `runFinished: true`, a stopped
worker, a disconnected browser, and an unresolved Playwright `Browser.close`
call. The run was stopped. Disabling traces did not resolve that shutdown stall
with the installed Chrome. The final full run uses Playwright's bundled browser
versions with failure traces enabled. Assertions and error collection are
unchanged. The installed Chrome is used for the separate latency and development
proxy checks. Inspector evidence is
in `worker-inspection.log`, `worker-fixtures.log`, and `worker-browser.log`.

## Local QA

The tested release was started on port 7836. The stopped database was backed up
before the restart. Both blocks and review snapshots are identical before and
after the restart (`qa-restart.log`). Existing agent event subscriptions were
restored. Already-open Chrome tabs need a reload to load the new session code.

The original QA URL was opened read-only in eight tabs in one installed Chrome
152 context. All tabs showed Saved and each used one notification socket. No
browser SSE streams or application errors occurred. An ordinary HTTP read
completed in 1 ms with every tab open. Blocks and review data remained unchanged
(`qa-check.json`).

## Limits

No client can confirm saving while every command request is unavailable. Such
requests remain durable and visibly pending or failed. These tests establish
independent progress when one network path is available and the other stalls.
They also establish exact retries when a commit succeeds but its response is
lost. They do not establish a fixed recovery time in suspended browser tabs.
