# Shared editing intents

## Objective

Make the Rust document engine responsible for the meaning of an edit in Editing
or Suggesting mode. Keep Plate responsible for browser input and presentation.
Keep Automerge responsible for character identity and history.

The recent deletion fix repairs a real defect. It still lets the browser infer
proposal continuation from nearby review records. That inference is too weak:
two adjacent edits by the same person can be separate intentions.

## Contract

- An edit states its native target, action, and mode.
- Creating a suggestion and continuing a named suggestion are explicit choices.
  The engine checks ownership, state, unchanged targets, and adjacency.
- A caret move, a new selection, a mode change, history navigation, or reload
  ends automatic continuation. A remote update alone does not end it.
- Selecting existing proposed content explicitly addresses that proposal. Its
  characters stay in their existing native sources when edited.
- Suggestion identity, local undo groups, and request IDs are independent.
  Request retries must never create another suggestion.
- An unsupported action fails before any canonical content is published.
- The browser executes edits synchronously through WASM. A keypress does not
  wait for HTTP or rebuild the entire document.

## Implementation

1. Add a shared edit contract and native interpretation for text replacement,
   insertion, deletion, formatting, and supported block actions. Keep review
   decisions and comment commands separate from content edits.
2. Put proposal continuation validation and composition in that layer. Preserve
   existing native characters, proposal IDs, replies, and creation metadata.
   Remove the browser's same-author/adjacency search for a proposal to reuse.
3. Route Plate input through the contract. Retain Plate's word and character
   selection, composition handling, paste normalization, and DOM updates.
   Keep explicit capability errors for structural suggestions the native model
   cannot yet represent; do not manufacture copied replacement trees.
4. Exercise the same contract through the native builder, WASM, and HTTP replay.
   Generated API types must come from Rust. Keep stale-write checks scoped to
   the affected targets rather than unrelated review activity.
5. Run native, browser, transport, and performance checks. Update the local QA
   server after checks pass, preserving its document and review state.

## Evidence required

- One range deletion and a sequence of adjacent deletions in the same declared
  intent produce one equivalent review item. Separate intentions stay separate.
- Insertions, replacements, and proposed-text edits preserve native ownership.
- Unicode, word deletion, formatting boundaries, selections, composition, undo,
  reload, and remote edits have explicit regression coverage.
- Concurrent unrelated edits and comments do not split an active suggestion.
  Competing changes to its target or decision fail atomically.
- Replaying browser requests on the authority produces the same native state.
  Tests vary operation grouping and request boundaries.
- Existing Plate feature and cross-browser tests pass. Chrome latency stays
  within the existing 20 ms p95 keypress and 50 ms maximum-frame budgets.
  Add a Suggesting-mode latency case and report any unexplained failures.

## Scope and limits

This changes the edit boundary, not the storage architecture or editor product.
Git and FUSE continue to express file writes through the native authority. They
must not infer browser gesture identity from text diffs.

Full structural replacement suggestions and mixed canonical/proposed selection
replacement need native representations that preserve every retained character.
Any remaining unsupported combinations will be named in the final evidence;
passing text tests alone will not be presented as full editor conformance.

## Unresolved questions

None block implementation. The default is explicit continuation, with a fresh
intent after navigation or reload. Existing review items remain readable.
