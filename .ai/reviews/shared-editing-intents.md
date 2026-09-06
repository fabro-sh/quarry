# Shared editing intents

## Result

The meaning of a content edit now belongs to the Rust document engine. Plate
describes the action and its native target. The same interpreter executes in
browser WASM and on the HTTP authority. Automerge remains the source of character
identity and history. Plate/Slate remains the editor.

The implementation follows [the shared plan](../plans/shared-editing-intents.md).
The local QA server now runs the verified build. Its document and review
snapshots are identical before and after the restart. The plan was synced back
from Quarry after verification.

## The boundary

`Command::Edit` carries an `EditAction` and an `EditMode`:

- `Direct` changes the addressed content.
- `Suggest { id, author }` starts a named suggestion for canonical content.
- `Continue { id, author }` continues the named text suggestion. It never searches
  for a nearby suggestion to reuse.

The engine checks the proposal state, author, native ownership, unchanged
deletion targets, and adjacency. An edit that explicitly addresses proposed text
changes its existing native characters. It does not copy the proposal into a new
source. Comments, replies, proposal identity, and creation metadata survive.

The contract covers text insertion, deletion and replacement, formatting, block
updates, movement, deletion, insertion, and supported edits within proposed block
trees. The browser no longer constructs `propose_*` commands. An architecture
check guards that boundary.

Three independent identities now have distinct responsibilities:

| Identity | Responsibility |
| --- | --- |
| Suggestion ID | The review item that a person accepts or rejects |
| Undo group | Which local changes one undo operation reverses |
| Request ID | Exact native replay and durable delivery receipts |

A pointer action, navigation, changed selection, mode change, undo/redo, or reload
ends automatic continuation. An unchanged Slate selection synchronization or an
unrelated remote edit does not. A pause can split undo groups without splitting
the suggestion.

The outbox still combines never-sent, causal text requests into one delivery.
It recognizes the new edit actions without changing their modes, proposal IDs,
request IDs, or bases. A lost response retries the persisted envelope exactly.

## Concurrent execution

The authority expands each edit against the version its caller actually saw.
Expansion runs sequentially on a private candidate, so a later command can
address proposed text created earlier in the same request. Delayed-write checks
inspect the resulting native commands before any merge is published.

Unrelated text edits, comments, and suggestions can merge. Competing changes to a
continuation's proposal or deletion target fail atomically. A private merge also
checks that a proposal valid on its original branch remains valid with concurrent
work. Existing durable SQL publication and receipt rules remain in force.

Git and FUSE still reconcile versioned file writes through the native authority.
They do not infer browser intentions from Markdown differences.

## Browser defects found during verification

The initial browser candidate treated Slate's repeated `select()` calls as new
user intentions. Slate synchronizes its selection on a throttle. That internal
synchronization could split the first deleted character into another proposal.
Explicit input boundaries now distinguish user actions from synchronization.

The broader Chrome run then exposed stale selection at `beforeinput`: the first
character reached the newly selected paragraph, but Slate restored the old
paragraph for later characters. The browser selection is now captured before
Slate's temporary target-range handling. Composition start uses the same capture
before it deletes a selected range.

The React regression reproduces that exact interval without dispatching
`selectionchange`. Removing the new handler makes it fail at the restored caret;
restoring the handler makes it pass. The original browser shortcut scenario also
passes with the fix.

## Verification

- Full native workspace, all features: **617 passed**, no failures or ignored
  tests. This includes HTTP sessions, durable receipt/restart checks, Git, FUSE,
  and simultaneous interface tests.
- Final UI suite using production WASM: **209 passed** across 20 files. The Plate
  conformance fixture replays browser requests through the native Rust host and
  compares the resulting document views.
- Seven new native intent tests vary deletion segmentation, request boundaries,
  remote changes, dependent proposal-source edits, invalid continuations, and
  cross-block deletion followed by typing.
- HTTP regression covers temporary and library documents, combined and separate
  requests, an interleaved agent suggestion/comment, native projections, restart,
  and exact receipt replay.
- Outbox tests cover new edit commands, legacy primitive requests, immutable
  retries, concurrent tab claims, and distinct suggestions in one delivery.
- Workspace Clippy with warnings denied, Rust formatting, generated API types,
  production UI build, and the native architecture inventory pass.
- Final production browser matrix: **173 passed, 4 expected skips**, across
  installed Chrome, Firefox, and WebKit. The skips are two WebKit drag cases and
  the WebKit-only navigation diagnostic case in the other two engines.
- Isolated headed Chrome latency suite: **6 passed**, covering seven measured
  scenarios. Chrome version: `152.0.7977.77`. No build or other automated test job
  ran during the measurements.

The native suite ran before a Clippy-only removal of a redundant `else`; the
final WASM and browser builds include that cleanup. Logs and traces are retained
under `target/shared-editing/`.

## Chrome responsiveness

The documents contain at least 1,400 paragraphs and exceed 100 KiB. The existing
budget is 20 ms p95 from keypress to the next animation frame, and a 50 ms maximum
frame gap. Every measured scenario passed both checks on its first run here.

| Scenario | Keys | p95 key-to-frame | Maximum frame gap |
| --- | ---: | ---: | ---: |
| Editing, first block | 100 | 13.60 ms | 11.87 ms |
| Editing, middle block | 100 | 10.00 ms | 17.10 ms |
| Editing, last block | 100 | 11.20 ms | 14.50 ms |
| Editing, first block, concurrent agent review | 400 | 9.50 ms | 16.70 ms |
| Editing, last block, concurrent agent review | 400 | 10.50 ms | 16.70 ms |
| Suggesting, typing | 100 | 13.20 ms | 26.53 ms |
| Suggesting, repeated deletion | 60 | 15.00 ms | 17.70 ms |

Each 100-key Editing case used three HTTP deliveries. Both concurrent cases
received the agent change during typing. The Suggesting case also verifies one
insertion proposal, one adjacent-range deletion proposal, and unchanged canonical
text after saving. Timing values are measurements on this machine, not a bound
for every device or workload.

## Local QA

The updated release server runs on port **7836**. Its process ID at verification
was **55057**. The sandbox opened in the user's Chrome. A separate read-only
browser check confirmed the Plate editor, existing comments, suggestions, and
formatting render correctly. That inspection browser was then closed.

The restart helper saved a database backup, compared both block and review
snapshots, and reconnected the review streams. The unrelated server on port 7832
was not changed. The temporary schema and browser-test servers were stopped.

Evidence includes `qa-update.log`, `qa-before-*`, `qa-after-*`, `qa.png`,
`browser-final.log`, `chrome-latency.log`, `latency-summary.json`, and the tested
source/binary hashes under `target/shared-editing/`.

## Limits

Canonical paragraph split/join suggestions and full structural replacement
suggestions remain unsupported. Some rich inline paste/removal combinations also
require a selection-based operation. A replacement spanning canonical and
proposed content requires deciding the intervening suggestion first. These cases
fail explicitly; they do not publish copied replacement trees or direct canonical
changes in Suggesting mode.

Extending a deletion range requires adjacent characters in one canonical block.
A selection can delete across blocks, and subsequent typing can continue that
selection as one replacement. That operation retains the original block structure.

Automated composition uses native Chrome composition events. Firefox and WebKit
use synthetic composition events; this does not prove every OS input method.
Mounted OS FUSE behavior is not exercised by the native interface tests.

The earlier adjacent-deletion report retains its initial unexplained Chrome
benchmark failures. This work's selection-race fix is not proof of their cause.
The measurements above describe only the completed runs for this change.
