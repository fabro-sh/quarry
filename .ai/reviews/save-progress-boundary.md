# Save progress boundary assessment

The WebSocket change fixes the reproduced multi-tab connection exhaustion.
The broader save-progress boundary remains incomplete.

## Remaining failure reproduced

A separate Chrome test used the current release binary and an isolated store.
It held document-state reads while command requests remained available.

| Condition | Durable pending edits | Acknowledged commands | Display |
| --- | ---: | ---: | --- |
| Refresh held; user types | 1 | 0; no command request sent | Saving… |
| First refresh released; next refresh held | 0 | 1; server contains the text | Saving… |
| All reads released | 0 | 1 | Saved |

The browser reported no application errors. The QA document was not changed.
The diagnostic script and output are `target/save-queue/save-boundary-assessment.ts`
and `target/save-queue/save-boundary-assessment.log`.

`DocumentSession.sync()` runs outbox delivery, a document-state read, document
merge, cache storage, and save-status publication in one sequential loop.
`kick()` cannot start another delivery while that loop awaits a read. Save status
also includes `again`, which can represent a notification requiring a refresh.
The request helper has no application deadline for reads or command attempts.

These are distinct concerns: preserving local edits, confirming server commits,
receiving remote changes, and maintaining notification connectivity.

## Recommended architectural boundary

Keep the durable outbox, stable request IDs, server receipts, Automerge authority,
Plate editor, and browser WebSocket transport. Give document-session management
explicit responsibility for independent save and refresh progress.

- Persist each local edit before delivery. Retain unsent and uncertain requests.
- Drive command delivery from pending edits, independently of document refreshes.
  Preserve command ordering and existing cross-tab delivery safeguards.
- Derive save status from local persistence and acknowledged commands, including
  edits still waiting to enter the outbox. Track remote freshness separately.
- Treat notifications as hints to refresh. Reconnection and periodic checks must
  recover missed hints. Coalesce refresh work and reject stale response effects.
- Bound network attempts and retries. Timing out an attempt must retain its
  persisted request identity because the server may already have committed it.
- Own notification subscriptions at the application level. Components should
  register interests instead of independently managing persistent connections.

Independent network progress still requires controlled application of responses
to the model. Running the existing loop steps in parallel would not establish
these guarantees.

The next conformance checks should hold refreshes while commands succeed, fail a
refresh after a successful commit, drop notifications without closing the socket,
lose an acknowledgement, and close the delivering tab. Assert server state,
durable pending requests, exact replay, eventual refresh after connectivity
recovers, and truthful status independently. Retain the multi-tab and keypress
latency checks.

This assessment adds a diagnostic and records findings. It does not implement
the recommended session refactor. The previous transport test results remain
valid for the cases they cover.
