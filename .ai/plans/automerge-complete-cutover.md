# Complete Automerge adoption

Automerge is the document authority for every Markdown document. The browser, agents, filesystem writes, and imports use the same document commands. Markdown and block rows are projections. There is no alternate editor or collaboration engine.

## Implementation

1. Make native state mandatory at creation and migrate stored Markdown documents. Preserve review records and historical references. Route every write through the document authority.
2. Remove the Yjs session protocol, checkpoint reconstruction, and the separate row mutation engine. Retain the public agent operations as translations into document commands.
3. Replace the collaboration codec with an editor-independent Markdown codec. Remove Slate and Yrs serialization, dependencies, and fixtures.
4. Make the ProseMirror editor the only editor. Integrate its review controls, document permissions, save state, and sharing with the application. Remove Plate, Slate, and Yjs code and dependencies.
5. Complete identity-preserving undo and structural editing. Cover clipboard input, selection, formatting, links, images, tables, proposal previews, and remote changes.
6. Replace obsolete tests with native conformance tests. Test the document model, storage, all writers, HTTP permissions, browser persistence, reconnects, and concurrent review. Include Unicode, repeated text, deleted targets, split/move/join, proposal acceptance, retries, and rollback.
7. Update API discovery, documentation, build scripts, and CI. Run full Rust and UI checks, real-browser tests, and an inventory that rejects old engine imports and routes.

## Completion criteria

- New and existing Markdown documents always have durable Automerge state.
- No application path can reconstruct or overwrite native identity from editor JSON or Markdown.
- Comments survive structural edits and undo. Missing targets remain explicit discussions.
- Commands publish state, projections, versions, and receipts atomically. Retried commands apply once.
- Browser and agent edits converge through the same rules, including after restart and lost responses.
- Old engine packages, runtime modules, transport routes, and fallback switches are removed.
- Tests exercise the actual default application, not an opt-in implementation.

## Unresolved questions

None. The user has authorized the complete cutover and comprehensive testing.
