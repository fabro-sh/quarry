# Multi-tab saving regression

The QA page could remain on **Saving…** because each tab opened two SSE streams.
Three tabs consumed Chrome's six HTTP/1.1 connections to the origin. Reads and
save requests then waited for a connection. This was a transport failure before
the request reached the document engine.

The earlier concurrent-browser tests used separate browser contexts. Those
contexts did not share the connection pool that triggers this failure.

## Direct evidence

`target/save-queue/reproduce.mjs` opened three read-only tabs in one Chrome
context against the QA server. Before the fix:

- Tabs one and two showed Saved; tab three showed Saving….
- An ordinary HTTP read remained queued for three seconds.
- Closing tab one immediately released the read and cleared tab three's status.

The same script after the update showed Saved in all three tabs. The HTTP read
completed in 1.3 ms with all tabs still open. Logs are `reproduce.log` and
`qa-after-reproduction.log` in `target/save-queue/`.

The connection limit is also documented by
[MDN](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events).

## Change

Browser notifications use WebSockets on the existing event URLs. Each JSON
message carries the existing event type and payload. Ordinary HTTP clients keep
the existing SSE response, including agent presence and document filtering.

The browser retries disconnected or stalled handshakes. Reconnection triggers
an authoritative document read. Failed notification connections trigger reads
without opening an HTTP stream. Page suspension and close release connections.
Vite forwards WebSocket upgrades during development.

The server checks browser Origin and document access before upgrading. The
notification socket accepts no content commands. Native subscriptions follow
document identity through a rename. Temporary document events omit secret paths.
Heartbeat writes are bounded, and shutdown closes subscriptions.

The HTTP command path, durable outbox, request receipts, Automerge document
authority, and Plate editing behavior remain in use. No draft storage was cleared.

## Verification

- 213 UI tests passed against production WASM.
- Server tests passed in all-features (230), temporary-only (116), library-only
  (200), and minimal feature-surface (1) builds. These counts overlap.
- Clippy passed with warnings denied for all four feature configurations.
- Production UI build, TypeScript, generated API schema, architecture inventory,
  and whitespace checks passed.
- Nine new production browser cases passed across Chrome, Firefox, and WebKit.
  Each document scope runs eight tabs in one browser context, holds browser
  command delivery, adds another browser edit and an agent comment, then checks
  convergence, save status, exact comment targets, reload, and continued editing.
  The fallback case disables WebSockets and verifies saves and agent updates.
- Real socket tests cover SSE compatibility, library filtering, stable document
  identity after rename, rejected origins and capabilities, viewer access,
  rejection of incoming content commands, temporary path privacy, and shutdown.

- The complete production browser suite passed 182 cases, with four existing
  platform-specific skips. It includes offline drafts, lost save responses,
  comments on repeated text, document moves, viewer invitations, composition,
  undo, and adjacent deletion suggestions. Log: `browser.log`.
- Three Chrome cases also passed through the development proxy. Log:
  `dev-proxy.log`. The proxy logged connection resets during tab teardown;
  all save, refresh, and target assertions passed.
- Six isolated, headed Chrome latency tests passed on Chrome 152.0.7977.77.
  The 114,814-byte document measured 10.0–11.0 ms at the 95th percentile for
  key-to-frame latency across its beginning, middle, and end. With agent review
  activity during 400 keystrokes, the values were 8.9 and 10.3 ms. Suggesting
  typing measured 13.8 ms; repeated deletion measured 15.7 ms. All seven
  scenarios met the existing 20 ms percentile and 50 ms maximum frame-gap
  budgets. The largest measured frame gap was 28.1 ms (rounded up).
  Logs and detailed samples: `latency.log` and `latency-results/`.

These timing measurements apply to this machine and workload, not every device.

## QA instance

The server on port 7836 was updated after the new browser cases passed. The
closed database was backed up first. Blocks and review snapshots were identical
before and after the restart. Existing agent event subscriptions were restored.
The QA document was not edited by the read-only reproduction.

Already-open tabs need a reload to replace the old SSE client. The server keeps
SSE available for HTTP agents and older clients. This fix therefore requires
loading the updated browser bundle; restarting the server alone cannot replace
JavaScript in an existing tab.

## Scope of the evidence

These tests cover local HTTP/1.1, direct production assets, and the documented
notification protocol. They do not certify every deployment proxy or network
failure. A deployment that blocks WebSocket upgrades uses the tested HTTP-read
fallback. This change does not alter the previously documented limits on native
suggestion operations.

All logs and dependency publication-age checks are under `target/save-queue/`.
No new dependency was installed within 24 hours of its publication.
