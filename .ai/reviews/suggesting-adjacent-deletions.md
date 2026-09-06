# Adjacent deletion suggestions

Date: 2026-09-06. Base commit: `cf2dfc9`.

Adjacent deletions by the same author within one text block now extend one native proposal. Canonical text remains unchanged until acceptance. The proposal ID, discussion and original character identities survive extension, undo, reload and concurrent edits. Existing separate suggestions are not consolidated.

## Causes and changes

Each deletion previously called `propose_replacement` with a fresh ID. The projection also rendered an empty editable inline for a deletion and placed the caret inside it. The next deletion could cross from that inline into canonical text and fail with `A suggesting edit must create a native proposal`.

The projection now omits empty inserted text. Backspace and Delete skip text already proposed for removal. Adjacent ranges extend an existing same-author deletion through `extend_deletion_proposal`, with a shared undo group. Forward deletion handles the two caret positions at an inserted proposal boundary.

The native command validates the proposal author, open state, empty insertion, unchanged targets and adjacency. It retains the existing proposal record. Changed targets or competing decisions fail atomically. A delayed extension checks its own proposal and targets, so an unrelated agent suggestion does not block it. Different authors and separated ranges stay separate.

## Verification

Three regression cases reproduced the original error before the fix. A separate negative test reproduced the overly broad stale-review check before the final refinement.

| Check | Result |
| --- | --- |
| All-feature macOS Rust workspace | 609 passed before the final delayed-command refinement |
| Native document suite after that refinement | 59 passed |
| Clippy | Workspace passed; final native command refinement also passed |
| Final UI suite | 199 passed across 19 files |
| Production build, generated schema, format and architecture checks | Passed |
| Broad production browser matrix | 164 passed, 4 expected skips, before the final delayed-command refinement |
| Final deletion browser matrix | 12 passed across installed Chrome, Firefox and WebKit |
| Local QA restart | Document blocks and reviews preserved; final OpenAPI matches generated schema |

The browser tests cover Backspace, Delete, marked text, Unicode, selection deletion, concurrent agent content and comments, reload, acceptance and rejection. The final delayed-request test holds a deletion POST, lets an agent publish another suggestion, then releases the original request. Both suggestions survive. Native and adapter tests also cover undo/redo, author separation, gaps, reload, competing decisions and changed targets. Adapter requests replay identically in Rust.

The broad browser skips are the two unsupported WebKit native-drag cases and the two non-WebKit executions of a WebKit-only diagnostic control. The final native refinement only changes validation of delayed deletion extensions; the final native, UI and browser regression runs exercise that path.

## Responsiveness

Chrome 152.0.7977.77 ran the existing 1,401-block, 114,814-byte typing benchmark in normal windows. Nine measured runs passed the unchanged limits of p95 key-to-frame latency at most 20 ms and maximum frame gap at most 50 ms. The highest measured p95 was 13.5 ms; the largest frame gap was 22.232 ms. These are typing measurements, not a separate large-document Suggesting-mode benchmark.

Two initial cases failed before producing latency measurements. In one, the editor did not appear. In the other, only 14 of 100 intended characters appeared in the selected paragraph. Each case then passed three consecutive reruns. Their causes remain unexplained; the successful reruns do not erase those failures or the earlier Chrome outliers recorded in the concurrency report.

## Evidence and scope

[The checkpoint](suggesting-adjacent-deletions-checkpoint.json) records source hashes, the release hash, test log hashes and individual latency measurements. Raw logs and frame artifacts are under `target/suggest-delete-*` locally.

The QA server on port 7836 runs the final fix. Refresh existing Chrome tabs to load its browser code. This change does not complete the remaining canonical structural Suggesting operations or clipboard identity work.
