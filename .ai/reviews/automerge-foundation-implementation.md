# Automerge document foundation

The Automerge foundation now runs through Quarry's storage, agent API, and browser. The browser uses the same Rust document rules through WebAssembly. ProseMirror translates editing operations into document commands.

Migration is explicit per document. Existing documents continue to use the legacy editor until migrated. This is an integrated foundation for evaluation. Broad migration still needs browser verification and the editor work listed below. No deployment or bulk migration was performed.

## What now owns the document

Automerge owns text identity, block structure, comments, proposals, and conflict records. Markdown, block rows, and editor nodes are projections. Document metadata and the version catalog remain in SQL. The commit writes the native bytes, projections, version, and request receipt in one SQL transaction.

Text stays in persistent sources. Blocks own segments of those sources. A split adds a boundary. A move changes placement. A join changes segment ownership. These operations retain existing character identities.

A discussion exists independently of its highlight. Its target can span several blocks. Deleted text, removed blocks, and targets that were already missing at import have distinct states. They do not erase the discussion.

Proposed text has native identity before acceptance. Acceptance places its existing segments into the document. A late comment on the proposal can therefore attach to the accepted text.

The server remains the authority for structure and review decisions. It executes typed commands against the caller's saved version. Browser and server requests generate the same native identities, including references to unsaved text. The server validates current targets before accepting a replacement or deletion. It rejects ambiguous stale structural or review decisions.

## Evidence

The earlier comparison remains in `review-engines-spike.md`. Its default-binding counterexamples motivated the command model. This implementation does not rely on a generic editor tree reconciliation to retain identity.

| Case | Verified result |
| --- | --- |
| Comment and typing made before a split, move, or join | Native targets and text survive both tested merge orders. |
| Repeated text in another paragraph | The comment stays with its original characters. |
| UTF-16 offsets, emoji, and boundary typing | Rust and browser requests agree. A late boundary comment excludes text outside its original selection. |
| Comment submitted after proposal acceptance | The target follows the accepted text into the body. |
| Replacement target changed since proposal | Acceptance fails without publishing candidate edits. |
| Server restart | Saved native history accepts valid delayed requests. |
| Lost acknowledgement | Retrying the exact request returns the original receipt. Acceptance occurs once. |
| Invalid command or injected SQL failure | State, projections, version, receipt, and events remain unchanged. |
| Two browser tabs | Separate draft rows preserve request order. Request IDs are scoped to the document. |
| Rejected browser request | The draft remains available for recovery or download. |
| Explicit draft recovery | A selection in unsaved text does not prevent returning to the saved document. |
| Fork of a migrated document | Review targets survive. The copy has a separate document identity and isolates subsequent writes. |
| Existing agent transaction API | Offset requests become native commands. Linked text retains comments through edits. |
| Existing Markdown PUT | Native identity survives compatible text edits. Same-line Markdown merge conflicts remain explicit review records. |
| Old SQL schema | Existing rows survive migration from global IDs to document-scoped IDs. |

Validation includes native domain tests, storage tests, HTTP integration tests, actual ProseMirror EditorView tests in jsdom, and a React page connected to the command session. Rust/WASM requests replay identically in the native Rust test host.

The full Rust run passed 359 tests across the document, WebAssembly wrapper, storage, and server crates with all features. Documentation tests passed. The full UI suite passed 418 tests. The browser production build, Clippy with all features and targets, Rust formatting, and whitespace checks passed.

The in-app browser was unavailable. These results do not establish a real-browser end-to-end pass. The Docker and CI build definitions were updated; they were not executed remotely.

## Where to inspect

- `crates/quarry-document`: document model, commands, validation, native targets, and regression tests.
- `crates/quarry-document-wasm`: thin browser wrapper around that same library.
- `crates/quarry-storage/src/document_state.rs`: durable bytes, version heads, receipts, and fork support.
- `crates/quarry-storage/src/document_import.rs`: one-time import and compatibility projections.
- `crates/quarry-server/src/document_engine.rs`: document authority and native HTTP requests.
- `crates/quarry-server/src/document_compat.rs`: existing agent commands translated to the native model.
- `ui/src/features/editor/document-editor.ts`: ProseMirror operation adapter.
- `ui/src/features/editor/document-session.ts` and `document-outbox.ts`: synchronization and durable browser requests.
- `ui/src/features/editor/NativeMarkdownEditor.tsx`: browser editor and review controls.

## Build and evaluate

Use Rust 1.90 or newer. The browser bridge requires `wasm32-unknown-unknown` and `wasm-bindgen-cli` 0.2.127. The package and crate versions added for this work were checked against the 24-hour age rule.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127 --locked
cd ui
bun install --frozen-lockfile --minimum-release-age 86400
bun run test
bun run build
```

The normal UI commands build the native bridge. CI has a shared setup action. Docker builds the bridge in a Rust stage before building the UI.

The following suffixes work under both the library document endpoint and the temporary document endpoint. They use the same document scope checks as the existing API. Their schemas are included in the server's OpenAPI response.

| Request | Purpose |
| --- | --- |
| `GET <document endpoint>/document-state` | Read the format. A migrated document also returns native bytes, its projection, and its document clock. |
| `POST <document endpoint>/document-migration` | Import an existing Markdown document once. Refuses an active legacy session. |
| `POST <document endpoint>/document-commands` | Commit an identified batch of commands with saved native heads. |

After migration, reopening the collaborative browser editor selects the native implementation. Legacy Yjs sessions and direct legacy storage writes cannot overwrite its state. Existing agent transactions and Markdown writes go through the native compatibility adapter.

Import preserves already-unattached comments and unavailable legacy proposals with their original records. It does not guess a new target for them. There is no downgrade endpoint.

## Work before broad migration

1. Verify the complete application in real browsers, including IME input, reconnects, multiple tabs, selection, and mobile layouts.
2. Add native undo semantics. Re-inserting deleted text with new character IDs would not restore the old comment target.
3. Finish editor parity: structural selections, rich clipboard input, drag/drop, images and links, table controls, and a full structural preview of proposed blocks. Mermaid currently displays its source. Raw Markdown remains a source block. The current proposal editor displays proposed text as one text surface.
4. Exercise older document formats and all restore/import flows on a migration corpus. Keep uncertain legacy targets explicit.
5. Measure large documents and long histories. Current transport and draft storage use complete native snapshots. Establish retention and compaction rules before removing history that delayed references may need.

The tests support the architectural choice: Automerge supplies durable text identity and history. Quarry's commands supply the structural and review semantics. That combination addresses the orphaning failures exposed by the spike.
