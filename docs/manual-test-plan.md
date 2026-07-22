# Quarry Manual Test Plan

Updated: 2026-06-10

This plan is for manual exploratory and release-candidate testing of Quarry's
local-first document substrate, browser workspace, live collaboration, review
markup, agent APIs, Git sync, FUSE projection, and cross-surface consistency.

Use a fresh temporary Quarry root for every full run. Do not run destructive
steps against a real personal library.

Sections are tiered: **P0** (smoke — pass these before anything else),
**P1** (gating feature areas — must pass, or be explicitly waived, to ship), and
**P2** (observational — soak, performance, accessibility; record results but do
not block a release on fuzzy thresholds). The Release Gate at the end derives
directly from these tiers.

## Test Environments

- [ ] macOS or Linux developer machine for REST, CLI, browser, Git, and agent API tests.
- [ ] Linux machine or Linux container with `fuse3` for FUSE tests.
- [ ] Chromium browser for baseline browser tests.
- [ ] At least one second browser context or profile for live collaboration tests.
- [ ] Local bare Git repository for remote `pull` / `push` / `sync` tests.
- [ ] Optional: Firefox or WebKit for browser compatibility spot checks.

## Global Setup

- [ ] Create a disposable root:
  ```sh
  export QUARRY_ROOT="$(mktemp -d)/quarry-root"
  cargo run -p quarry -- server init
  ```
- [ ] Start the daemon on loopback:
  ```sh
  cargo run -p quarry -- server start --addr 127.0.0.1:7831
  ```
- [ ] Start the browser UI:
  ```sh
  cd ui
  QUARRY_API_ORIGIN=http://127.0.0.1:7831 bun run dev -- --host 127.0.0.1
  ```
- [ ] Open the UI and confirm it loads without console errors.
- [ ] Create at least two libraries: `manual-main` and `manual-other`.
- [ ] Keep browser devtools Network open for ETag, SSE, and WebSocket observations.
- [ ] For each section, record any console error, server log warning, failed request, stale banner, or unexpected conflict dialog.

## P0 Smoke: System Starts And Basic Persistence Works

- [ ] `GET /v1/health` returns healthy JSON.
- [ ] `GET /v1/openapi.json` returns all expected route groups.
- [ ] UI library picker shows `manual-main` and `manual-other`.
- [ ] Create `notes/smoke.md` in the UI.
- [ ] Type text, wait for the save status to become `Saved` (checkpoint ack), reload, and confirm content persisted.
- [ ] Read the same document through CLI:
  ```sh
  cargo run -p quarry -- get manual-main notes/smoke.md
  ```
- [ ] Read the same document through REST and confirm the body and `ETag` match the latest version.
- [ ] Confirm no unexpected conflict dialog or stale banner appears; the only save states are `Saved`, `Saving…`, and `Reconnecting (read-only)`.

## REST, CLI, Storage, ETags, And Transactions [P1]

### Libraries And Paths

- [ ] Create a library through REST with a simple slug.
- [ ] Try creating the same library twice and confirm duplicate handling is explicit.
- [ ] Create nested documents such as `notes/a.md`, `notes/sub/b.md`, and `space path/file name.md`.
- [ ] Confirm URL-encoded paths round-trip correctly through REST and UI routes.
- [ ] List documents with and without a `prefix` and confirm only matching paths appear.
- [ ] Confirm documents in `manual-main` are not visible in `manual-other`.
- [ ] Attempt path traversal-like paths such as `../outside.md` and confirm rejection or normalization prevents escaping the library.
- [ ] Attempt reserved `.quarry/` paths and confirm they are rejected where applicable.

### Document Writes And Content Storage

- [ ] `PUT` a small Markdown document and confirm it reads back inline through REST and CLI.
- [ ] `PUT` a binary or image document and confirm content type and byte size are correct.
- [ ] `PUT` a document larger than 64 KiB and confirm it reads back intact after daemon restart.
- [ ] Rewrite a large document to a small document and confirm reads still return the new content.
- [ ] Rewrite a small document to a large document and confirm reads still return the new content.
- [ ] Delete a document and confirm normal reads return not found while version history remains available if exposed.
- [ ] Move `notes/a.md` to `notes/renamed.md` and confirm old path is gone and new path retains content.
- [ ] Move a document over an existing path and confirm the behavior is explicit and data is not silently lost.
- [ ] Patch metadata on a document and confirm content is unchanged.

### ETags And Preconditions

- [ ] Read a document and save its `ETag`.
- [ ] Write with the current `If-Match` and confirm success plus a new `ETag`.
- [ ] Write again with the old `If-Match` and confirm `412 Precondition Failed`.
- [ ] Make disjoint edits from an older version, send that version as
      `X-Quarry-Merge-Base` with the current `If-Match`, and confirm both sides
      survive.
- [ ] Repeat an overlapping merge and confirm the response reuses one conflict
      id; verify both **Keep current** and **Use incoming** resolve atomically.
- [ ] Create with `If-None-Match: *` and confirm success for a new path.
- [ ] Create again with `If-None-Match: *` at the same path and confirm `412`.
- [ ] Confirm UI stale-save handling shows the conflict workflow instead of overwriting.
- [ ] Confirm `HEAD` returns the same `ETag` and `X-Quarry-Document-Id` as `GET`.

### Explicit Transactions

- [ ] Begin a transaction for `manual-main`.
- [ ] Stage two document writes in the transaction.
- [ ] Before commit, confirm staged documents are not visible through normal reads.
- [ ] Commit and confirm both documents appear atomically.
- [ ] Begin another transaction, stage a write, roll it back, and confirm no staged content appears.
- [ ] Begin a transaction, stage a write to an existing document, mutate that document outside the transaction, then commit and confirm stale-head rejection.
- [ ] Confirm a failed transaction commit leaves the externally committed document visible and uncorrupted.

### Backup, Restore, GC, And Daemon Ownership

- [ ] Stop active writes, run `quarry server backup`, restore into a new root, and confirm documents, versions, and CAS objects read back.
- [ ] Run `POST /v1/admin/gc` or `quarry server gc` after deleting/replacing large documents and confirm reachable documents still read back.
- [ ] Try starting two daemon processes on the same root and confirm the second owner is rejected or cannot corrupt the database.
- [ ] Restart the daemon after in-progress client activity and confirm committed data remains intact.

## Browser Workspace And Editor [P1]

### Navigation And Document Management

- [ ] Deep-link directly to `/lib/manual-main/documents/notes/smoke.md` and confirm the document opens.
- [ ] Switch libraries and confirm selection, tree, right pane, and recent library state update.
- [ ] Create a document from the toolbar and confirm it appears in the tree.
- [ ] Rename or move a selected document and confirm the URL, tree, editor, and right pane retarget.
- [ ] Delete a selected document and confirm the editor clears or navigates predictably.
- [ ] Create a document path from an unresolved wiki-link and confirm the new target opens.
- [ ] Collapse and expand left and right panes, reload, and confirm layout persistence.
- [ ] Toggle light/dark theme and confirm persistence after reload.

### Save State And Session Checkpoints

The editor has no autosave drafts: typing flows through the live session and
durability comes from debounced checkpoints. The three save states are
`Saved`, `Saving…`, and `Reconnecting (read-only)`.

- [ ] Type a short edit and confirm the status shows `Saving…` then settles to `Saved` within the checkpoint debounce.
- [ ] Type multiple bursts quickly and confirm checkpoints are debounced/coalesced, not per keystroke (watch version history).
- [ ] Reload immediately after `Saved` and confirm the canonical content matches what was on screen.
- [ ] Write the same document through REST while it is open and confirm the change merges into the live editor as a collaborator edit (no dialog, no reload, not marked dirty).
- [ ] Stop the daemon while the document is open: confirm the editor becomes `Reconnecting (read-only)` and the last-known content stays visible.
- [ ] Restart the daemon and confirm the editor reconnects, reseeds from canonical state, and becomes editable with `Saved`.

### Markdown Editing Features

- [ ] Headings, paragraphs, bold, italic, strikethrough, inline code, code blocks, blockquotes, ordered lists, unordered lists, and task lists round-trip through save and reload.
- [ ] Markdown links render as links, can be edited, can be removed, and persist as Markdown.
- [ ] `[[wikilink]]` typed in the editor becomes a chip and persists as wiki-link Markdown.
- [ ] Wiki-links with aliases, spaces, and unresolved targets render correctly.
- [ ] Dropping or pasting an image stores an `assets/<hash>.<ext>` document and inserts Markdown image syntax.
- [ ] Existing image Markdown renders from the Quarry document endpoint.
- [ ] Mermaid code blocks render preview SVG and can toggle back to source.
- [ ] Tables load, edit, and save without losing alignment or cell text.
- [ ] Frontmatter at the top of a Markdown document remains intact through edit/save/reload.
- [ ] CriticMarkup-like text inside inline code or fenced code stays literal.
- [ ] Very large Markdown files open in the expected mode or show a clear limitation message.
- [ ] Binary documents show metadata/download or preview UI instead of mounting the rich editor.

### Search, Links, Graph, Versions, And Right Pane

- [ ] Search by path finds matching documents.
- [ ] Search by title metadata finds matching documents.
- [ ] Search by body text finds matching documents.
- [ ] Search suggestions update as the query changes.
- [ ] Outgoing links show resolved and unresolved wiki-links.
- [ ] Backlinks update after saving a document that links to another document.
- [ ] Reindex a library and confirm search/backlinks stay consistent.
- [ ] Graph endpoint or graph UI renders a small library accurately.
- [ ] Version list shows new entries after saves.
- [ ] Version diff shows expected line changes.
- [ ] Restore an old version and confirm it creates a new head rather than mutating history.
- [ ] Conflict tab shows open Git conflicts and can open the affected path.

## Review: Comments, Suggestions, And Conflict Items [P1]

Review items are row-anchored (`{block_id, start_offset, end_offset}`) and
projected by `GET /review`; the document text never carries CriticMarkup.

- [ ] Add a comment from selected text, submit it, and confirm it appears in the review rail and survives reload.
- [ ] Cancel a comment draft and confirm nothing persists.
- [ ] Reply to a comment and confirm the threaded reply persists.
- [ ] Resolve a comment and confirm its state persists and it is filtered from default `GET /review`.
- [ ] Delete a comment and confirm the in-text highlight and rail card are removed.
- [ ] Hover a comment card and confirm the matching text mark highlights.
- [ ] Switch to Suggesting mode and type an insertion; confirm the suggestion mark and rail card persist across reload.
- [ ] Delete text in Suggesting mode and confirm deletion suggestion behavior is visible and persistent.
- [ ] Accept and reject insertion, deletion, and substitution suggestions; after each decision, reload and confirm no stale suggestion card returns.
- [ ] Test two suggestions and one comment in the same paragraph and confirm all survive round-trip.
- [ ] Edit the text under a comment anchor (overlapping its range) and confirm the comment shows an orphaned badge rather than disappearing.
- [ ] Edit the text under an open suggestion and confirm it shows invalidated; confirm accept fails typed and reject dismisses it.
- [ ] Attempt to `PUT` Markdown containing CriticMarkup (`{++x++}`) and confirm a typed `UNSUPPORTED_MARKDOWN` rejection, not silent acceptance.
- [ ] Produce a diff3 conflict (see Git section) and confirm the conflict review item appears in the rail with kept and incoming text, and can be resolved/dismissed without changing the document.

## Live Browser Collaboration And CRDT/Yjs [P1]

These are among the highest-risk tests. Use two isolated browser contexts with
different `quarry:author` values.

### Session Join, Awareness, And Cursors

- [ ] Open the same Markdown document in User A and User B contexts.
- [ ] Confirm both users see the same initial content.
- [ ] Confirm both users connect to `/v1/collab/{document_id}` over WebSocket.
- [ ] Confirm remote cursor or presence labels appear when both users focus/type.
- [ ] Confirm author labels/colors are stable across reload.
- [ ] Confirm non-Markdown or binary documents do not try to start rich CRDT editing.

### Concurrent Editing And Checkpoints

- [ ] User A types in one paragraph and User B sees it without reload.
- [ ] User B types in another paragraph and User A sees it without reload.
- [ ] Both users type in the same paragraph at the same time and confirm the merged result has both edits and no duplicated seed content.
- [ ] Confirm save status returns to `Saved` in both browsers (the server checkpoints the shared session; there is no browser-side flusher).
- [ ] Close one tab and confirm the remaining tab keeps editing and reaching `Saved`.
- [ ] Reload both browsers and confirm the persisted Markdown matches the visible editor content.
- [ ] Confirm checkpoint `doc.changed` events do not disturb the open editors (no dialogs, no dirty state).

### Review Markup In Live Collaboration

- [ ] User B switches to Suggesting mode and types a suggestion; User A sees the suggestion mark and rail card without reload.
- [ ] User A accepts the suggestion; User B sees the mark disappear and final text remain.
- [ ] User A adds a comment; User B sees the comment mark and rail card.
- [ ] User B replies or resolves the comment; User A sees the updated rail state.
- [ ] Reload both browsers and confirm comments/suggestions persist as RFM Markdown.
- [ ] Confirm no stale suggestion card remains after accept/reject on either browser.

### Recovery, Reconnect, Move, Delete, And External Writes

- [ ] Disconnect one browser (e.g. devtools offline): confirm it becomes `Reconnecting (read-only)` with the last content visible; continue editing in the other browser; restore the connection and confirm the first browser reseeds and converges.
- [ ] Stop the daemon during active edits, restart, reopen the document, and confirm only the un-checkpointed debounce window is lost (sessions reseed from the last checkpoint).
- [ ] While both browsers have the document open, write a different version through REST or CLI and confirm the change merges into both live editors as a collaborator edit — no external-change dialog, no lost typing (overlapping same-region edits may surface as conflict review items).
- [ ] Move the active document from another surface and confirm the live session retargets to the new path.
- [ ] Delete the active document from another surface and confirm the UI behaves predictably (clears or navigates; no crash).
- [ ] Confirm edits made after a move checkpoint to the new path, not the old path.

### Performance And Scale For Live Sessions

- [ ] Open a document of at least 500 paragraphs and confirm join completes within a couple of seconds with no visible stall.
- [ ] Paste a large block of Markdown in one browser and confirm the other browser receives it.
- [ ] Edit rapidly for at least two minutes and confirm no runaway version churn or memory growth is obvious.
- [ ] Open three or more browser contexts and confirm convergence remains stable.

## Agent HTTP APIs And Human-Agent Collaboration [P1]

### Agent Discovery, Invite, And Presence

- [ ] Open the `Add agent` modal and confirm instructions include the document locator and `/presence`, `/blocks`, `/transactions`, and `/review`.
- [ ] Confirm `/.well-known/agent.json`, `/agent-docs`, and `/quarry.SKILL.md` are reachable.
- [ ] Mint a share token through UI, REST, or CLI and confirm it is scoped to the document.
- [ ] Revoke a share token and confirm it no longer appears in token listings if exposed.
- [ ] Register agent presence with `X-Agent-Id` and `status: reading`; confirm UI shows the agent.
- [ ] Update presence through `thinking`, `acting`, `waiting`, `completed`, and `error`; confirm UI status updates.
- [ ] Confirm invite URL tokens are not represented as REST auth.

### Blocks And Transactions

- [ ] `GET /blocks` returns `document_id`, `document_clock`, and the block tree with stable `block_id`s.
- [ ] Commit a `replace_block_content` transaction with a fresh `client_tx_id` and the current `base_clock`; confirm the ack is `committed` with the changed id in `changed_block_ids` and the content changes once.
- [ ] Replay the exact same `client_tx_id` and confirm the original ack is returned without a duplicate version.
- [ ] Test `insert_block` (top level and under a parent), `delete_block`, and `move_block`; confirm `move_block` preserves the moved block's id, content, and anchors.
- [ ] Test `set_block_type` (e.g. paragraph → heading) and confirm id, text, and anchors are preserved.
- [ ] Re-read `/blocks` after edits and confirm untouched sibling `block_id`s did not change.
- [ ] Send a garbage `base_clock` and confirm `412` with typed `{code: "STALE_BASE", retryable: true}`.
- [ ] Send an OLDER known `base_clock` with compatible ops and confirm a `committed_rebased` ack.
- [ ] Reference a deleted `block_id` and confirm typed `BLOCK_DELETED` (`retryable: false`).
- [ ] Send a malformed envelope and confirm typed `INVALID_TRANSACTION` (400).
- [ ] Send a multi-op transaction where the last op is invalid and confirm nothing committed (atomicity).
- [ ] With a live browser session open, commit a transaction and confirm it appears in the open editors as a collaborator edit without disturbing in-flight typing; confirm rows are durable at ack (read the document immediately).
- [ ] Confirm `POST /edit`, `POST /ops`, and `POST .../review` return 404 (deleted routes).

### Agent Review Operations

- [ ] Add a comment with `comment.add` anchored by `{block_id, start, end}` offsets.
- [ ] Add a whole-block comment (`start: 0`, `end` = block text length).
- [ ] Reference a non-existent review `item_id` and confirm `ANCHOR_NOT_FOUND`.
- [ ] Add a comment reply with `comment.reply`.
- [ ] Resolve a comment with `comment.resolve`.
- [ ] Delete a comment with `comment.delete`.
- [ ] Add replacement, deletion (empty `replacement`), and insertion (collapsed range) suggestions with `suggestion.add`.
- [ ] Accept and reject suggestions with `suggestion.accept` / `suggestion.reject`; confirm accept applies the replacement and reject leaves text unchanged.
- [ ] Confirm `GET /review` returns the items with `anchor: {blockId, startOffset, endOffset}` and that `includeResolved=1` reveals resolved items.
- [ ] Confirm a mixed edit+review transaction commits atomically as one version.
- [ ] Confirm UI rail and in-editor marks update in both browser contexts after agent review transactions land in the live session.

### Agent Events

- [ ] Open `/documents/{path}/events/stream` for an agent and confirm sparse events arrive after document changes.
- [ ] Call `/events/pending?after=<id>` and confirm polling returns missed events.
- [ ] Acknowledge events with `/events/ack` and confirm subsequent pending calls advance.
- [ ] Confirm events are wake signals only: agent re-reads `/blocks` before acting.

## Git Import, Export, Sync, And Conflict Preservation [P1]

### Import And Export

- [ ] Import a working tree with Markdown, nested paths, binary files, and frontmatter.
- [ ] Confirm imported frontmatter appears as metadata or preserves expected content semantics.
- [ ] Export the library to a new Git worktree and confirm files match documents.
- [ ] Confirm `.quarry/marker.json` is written and protects against exporting a different library into the same worktree.
- [ ] Confirm `.quarrymeta.yaml` sidecars are written for non-Markdown metadata.
- [ ] Attempt to create or export a document ending in `.quarrymeta.yaml` and confirm reserved sidecar safety.
- [ ] Export, commit, re-import, and confirm no unintended document churn.

### Peer Sync

- [ ] Create a local bare remote and a working tree peer.
- [ ] Add the peer with `quarry git peer add` and confirm `peer list`.
- [ ] Push Quarry-only changes to Git and confirm remote branch updates.
- [ ] Pull Git-only changes into Quarry and confirm documents appear.
- [ ] Sync when both sides are unchanged and confirm no extra Git commit is created.
- [ ] Sync when both sides changed to the same content and confirm convergence without conflict.
- [ ] Sync when both sides changed DIFFERENT regions of the same Markdown path and confirm both edits merge (diff3) with no conflict and sibling `block_id`s preserved.
- [ ] Sync when both sides changed the SAME region of a Markdown path: confirm the canonical side stays in the document and the Git side surfaces as a conflict review item in `GET /review` and the UI rail — no sibling file, no sync failure.
- [ ] Sync when both sides changed the same NON-Markdown path differently and confirm Quarry keeps its side while the Git side is preserved as a `*.conflict-git-*` sibling document.
- [ ] Delete a Markdown document in Quarry while Git changes it, sync, and confirm the Git side is preserved as a `*.conflict-git-*` sibling that opens as a normal block document in the UI.
- [ ] Confirm an open conflict record appears in CLI, REST, and UI for the sibling cases.
- [ ] Resolve the canonical document manually, mark conflict resolved, and confirm the record status changes.
- [ ] Confirm resolving a conflict does not delete conflict sibling documents automatically.
- [ ] Test change/delete conflicts in both directions.
- [ ] Test both-created conflicts.
- [ ] Test both-deleted cleanup.
- [ ] Test large-delete safety by configuring a sync that would delete too much and confirm it aborts.
- [ ] During a long sync, attempt a normal write and confirm operation locking prevents corrupt interleaving.
- [ ] Confirm `git.sync.completed` event updates the browser summary and conflicts tab.

## FUSE Projection [P1]

Run this section on Linux with `fuse3`. Waivable on non-Linux release candidates.

- [ ] Mount a library read-only:
  ```sh
  cargo run -p quarry -- mount manual-main /tmp/quarry-mount --read-only
  ```
- [ ] Confirm `ls`, `find`, `cat`, and `rg` read committed documents.
- [ ] Confirm writes fail cleanly in read-only mode.
- [ ] Mount writable with `--serve-addr` so REST and FUSE share one process.
- [ ] Create a file through the mount and read it through REST and UI.
- [ ] Write through REST and confirm the mounted file updates or invalidates correctly.
- [ ] Edit with an editor that writes a temp file then renames over the original and confirm final content is correct.
- [ ] Rename a file in the mount and confirm REST/UI see the new path.
- [ ] Delete a file in the mount and confirm REST/UI see deletion.
- [ ] Create and remove empty directories and confirm directory metadata survives remount.
- [ ] Truncate a file via shell redirection or `: > file` and confirm new content/size.
- [ ] Attempt `.quarry/` operations and confirm they are rejected.
- [ ] Confirm inode identity remains stable enough across reopen and rename for normal shell tools.
- [ ] Unmount cleanly and confirm the daemon shuts down without leaving stale locks.

## Events, Invalidation, And Cross-Surface Refresh [P1]

- [ ] With UI open, write a document through REST and confirm SSE refreshes the tree/right pane; the open editor receives the content through its live session.
- [ ] With UI open, write through CLI and confirm expected refresh behavior.
- [ ] With UI open, write through FUSE and confirm expected refresh behavior.
- [ ] With UI open, run Git sync and confirm document list, conflicts, links, and versions refresh.
- [ ] Simulate EventSource unavailable and confirm polling fallback eventually refreshes state.
- [ ] Confirm stream lag handling triggers broader revalidation.
- [ ] Confirm `doc.changed` for the open session-backed document never resets or dirties the editor (session updates carry the content; SSE is a metadata refresh signal).

## Security And Local-Only Boundaries [P1]

- [ ] Start daemon with default address and confirm it binds to `127.0.0.1`.
- [ ] Start daemon with a non-loopback address and confirm a clear warning is printed.
- [ ] Confirm docs/UI messaging does not imply invite tokens are a security boundary.
- [ ] Confirm REST mutating endpoints remain trusted-localhost and unauthenticated as documented.
- [ ] Try invalid JSON bodies and confirm clear 4xx responses, not panics.
- [ ] Try unsupported content types and confirm clear behavior.
- [ ] Try very long paths, unusual Unicode path segments, and case-distinct names and confirm no corruption.
- [ ] Confirm library scoping prevents one library's conflict, document, or token IDs from mutating another library.

## Reliability, Performance, And Soak [P2]

- [ ] Create 10,000 document paths and confirm tree navigation remains usable.
- [ ] Search a large library and confirm responses are bounded and UI remains responsive.
- [ ] Import/export a large Git tree and confirm completion or clear error.
- [ ] Run a 30-minute live editing soak with two browsers and periodic agent transactions; confirm no drift or stale UI.
- [ ] Restart daemon repeatedly during idle periods and confirm clean startup.
- [ ] Restart daemon during active browser use and confirm recovery or explicit failure.
- [ ] Fill disk or simulate write failure for CAS/database if practical and confirm user-visible failure without data loss.
- [ ] Run backup/restore after a long session and confirm restored root serves the final expected state.

## Accessibility And Usability [P2]

- [ ] Keyboard-only user can select library, open document, edit, search, create, rename, move, delete, and use right-pane tabs.
- [ ] Focus remains visible through toolbar, menu, modal, and review rail interactions.
- [ ] Add Agent modal, conflict dialog, Git modal, and settings modal trap and restore focus correctly.
- [ ] Buttons and icon controls expose accessible names.
- [ ] Save status, conflict banners, and recovery errors are perceivable.
- [ ] Run an accessibility scan on the main workspace and resolve critical issues.
- [ ] Confirm dense UI remains readable in light and dark themes.

## Release Gate

A release candidate is shippable when:

- [ ] All **P0** checks pass.
- [ ] All **P1** sections pass, with FUSE waivable on non-Linux candidates and live collaboration verified in at least Chromium.
- [ ] All **P2** sections have been run and their results recorded.
- [ ] Backup/restore round-trips to the final expected state.

And these cross-surface invariants held throughout testing (not restated per
section above — confirm them globally):

- [ ] Block rows are the durable source of truth for Markdown documents (exports are deterministic projections); raw bytes are the source of truth for everything else.
- [ ] FUSE, REST, CLI, Git, browser, and agent operations converge on the same committed state.
- [ ] Agent review marks and human review marks remain compatible in Markdown.
- [ ] Deleting, moving, or restoring documents never loses version history unexpectedly.
- [ ] No data loss, silent stale overwrite, lingering stale conflict, or cross-library metadata leak remains open.
