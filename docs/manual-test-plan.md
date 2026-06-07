# Quarry Manual Test Plan

Updated: 2026-06-07

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
  cargo run -p quarry -- init "$QUARRY_ROOT"
  ```
- [ ] Start the daemon on loopback:
  ```sh
  cargo run -p quarry -- --root "$QUARRY_ROOT" serve --addr 127.0.0.1:7831
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
- [ ] Type text, wait for save status to become `Saved`, reload, and confirm content persisted.
- [ ] Read the same document through CLI:
  ```sh
  cargo run -p quarry -- --root "$QUARRY_ROOT" get manual-main notes/smoke.md
  ```
- [ ] Read the same document through REST and confirm the body and `ETag` match the latest version.
- [ ] Confirm no unexpected `External version available`, `Local draft`, or `Latest remote` UI appears.

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

- [ ] Stop active writes, run `quarry backup`, restore into a new root, and confirm documents, versions, and CAS objects read back.
- [ ] Run `POST /v1/admin/gc` or `quarry gc` after deleting/replacing large documents and confirm reachable documents still read back.
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

### Autosave, Drafts, And Conflict UI

- [ ] Type a short edit and confirm save status progresses to `Saved`.
- [ ] Type multiple bursts quickly and confirm saves are debounced, not per keystroke.
- [ ] Close or reload with an unsaved draft and confirm draft recovery uses the correct document and ETag.
- [ ] Create a remote REST write while the browser has an unsaved local draft and confirm the conflict dialog shows local and remote versions.
- [ ] Choose the remote version and confirm local draft is cleared.
- [ ] Resolve by editing the local version and confirm a new clean save succeeds.
- [ ] Confirm save failure states are visible if the daemon is stopped during autosave.

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
- [ ] Review endmatter at the bottom remains distinct from frontmatter.
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

## Review Markup: Comments, Suggestions, And RFM Persistence [P1]

- [ ] Load a document containing `{==text==}{>>body<<}{#id}` and matching `comments:` endmatter.
- [ ] Confirm the marked range is decorated in the editor and appears in the review rail.
- [ ] Add a new comment from selected text and confirm no Markdown marker is saved until the draft is submitted.
- [ ] Submit a comment draft and confirm CriticMarkup plus endmatter are persisted.
- [ ] Cancel a comment draft and confirm no marker or endmatter is persisted.
- [ ] Reply to a comment and confirm the reply has `re: <parent>` in endmatter.
- [ ] Resolve a comment and confirm `status: resolved` persists.
- [ ] Delete a comment and confirm the in-text mark and endmatter entry are removed.
- [ ] Hover a comment card and confirm the matching text mark highlights.
- [ ] Switch to Suggesting mode and type an insertion; confirm `{++text++}{#id}` persists with `suggestions:` endmatter.
- [ ] Delete text in Suggesting mode and confirm deletion suggestion behavior is visible and persistent.
- [ ] Create or load a substitution `{~~old~>new~~}{#id}` and confirm the rail shows old and new text.
- [ ] Accept an insertion and confirm text remains while markers/endmatter disappear.
- [ ] Reject an insertion and confirm suggested text disappears.
- [ ] Accept a deletion and confirm deleted text disappears.
- [ ] Reject a deletion and confirm deleted text remains.
- [ ] Accept a substitution and confirm only new text remains.
- [ ] Reject a substitution and confirm only old text remains.
- [ ] Save and reload after every accept/reject case and confirm no stale suggestion card returns.
- [ ] Test two suggestions and one comment in the same paragraph and confirm all survive round-trip.
- [ ] Test malformed CriticMarkup and confirm the editor leaves it as text instead of losing content.
- [ ] Test duplicate or missing review ids in imported Markdown and confirm behavior is visible and non-destructive.

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

### Concurrent Editing And Flusher Lease

- [ ] User A types in one paragraph and User B sees it without reload.
- [ ] User B types in another paragraph and User A sees it without reload.
- [ ] Both users type near the same paragraph at the same time and confirm the merged result has both edits and no duplicated seed content.
- [ ] Confirm only one browser performs the debounced REST flush for a clean live session.
- [ ] Close the current flusher tab and confirm the remaining tab eventually saves future edits.
- [ ] Confirm save status returns to `Saved` in both browsers.
- [ ] Reload both browsers and confirm the last flushed Markdown matches the visible editor content.
- [ ] Confirm normal flush `doc.changed` events do not show `External version available`.

### Review Markup In Live Collaboration

- [ ] User B switches to Suggesting mode and types a suggestion; User A sees the suggestion mark and rail card without reload.
- [ ] User A accepts the suggestion; User B sees the mark disappear and final text remain.
- [ ] User A adds a comment; User B sees the comment mark and rail card.
- [ ] User B replies or resolves the comment; User A sees the updated rail state.
- [ ] Reload both browsers and confirm comments/suggestions persist as RFM Markdown.
- [ ] Confirm no stale suggestion card remains after accept/reject on either browser.

### Recovery, Reconnect, Move, Delete, And External Writes

- [ ] Disconnect one browser, continue editing in the other, reconnect, and confirm convergence.
- [ ] Stop the daemon during active edits, restart, reopen the document, and confirm recovery state loses at most the expected debounce window.
- [ ] While both browsers have the document open, write a different version through REST or CLI and confirm live UI surfaces an external-change/stale workflow instead of silent overwrite.
- [ ] Resolve the external-change workflow and confirm live editing can resume.
- [ ] Move the active document from another surface and confirm the live session retargets to the new path.
- [ ] Delete the active document from another surface and confirm the UI offers a clear discard or resurrect path.
- [ ] Confirm edits made after a move flush to the new path, not the old path.
- [ ] Confirm no `Local draft` / `Latest remote` dialog appears during normal CRDT-only edits.

### Performance And Scale For Live Sessions

- [ ] Open a document of at least 500 paragraphs and confirm join completes within a couple of seconds with no visible stall.
- [ ] Paste a large block of Markdown in one browser and confirm the other browser receives it.
- [ ] Edit rapidly for at least two minutes and confirm no runaway version churn or memory growth is obvious.
- [ ] Open three or more browser contexts and confirm convergence and flusher election remain stable.

## Agent HTTP APIs And Human-Agent Collaboration [P1]

### Agent Discovery, Invite, And Presence

- [ ] Open the `Add agent` modal and confirm instructions include the document locator and `/presence`, `/snapshot`, `/edit`, and `/ops`.
- [ ] Confirm `/.well-known/agent.json`, `/agent-docs`, and `/quarry.SKILL.md` are reachable.
- [ ] Mint a share token through UI, REST, or CLI and confirm it is scoped to the document.
- [ ] Revoke a share token and confirm it no longer appears in token listings if exposed.
- [ ] Register agent presence with `X-Agent-Id` and `status: reading`; confirm UI shows the agent.
- [ ] Update presence through `thinking`, `acting`, `waiting`, `completed`, and `error`; confirm UI status updates.
- [ ] Confirm invite URL tokens are not represented as REST auth.

### Snapshot And Direct Edit Operations

- [ ] `GET /snapshot` returns `documentId`, `baseToken`, and top-level block refs.
- [ ] Use the exact block `ref` from `/snapshot` for `replace_block` dry run and confirm no write occurs.
- [ ] Commit `replace_block` with an `Idempotency-Key` and confirm content changes once.
- [ ] Replay the same idempotency key and body and confirm the response is replayed without a duplicate version.
- [ ] Replay the same key with a different body and confirm conflict/rejection.
- [ ] Test `insert_before` and `insert_after` with one block.
- [ ] Test `insert_before` and `insert_after` with multiple `blocks` at one ref.
- [ ] Test `delete_block`.
- [ ] Test `replace_document`.
- [ ] Send a stale `baseToken` and confirm `412` / `STALE_BASE`.
- [ ] Re-read `/snapshot`, rebuild the operation with fresh refs, and confirm retry succeeds.
- [ ] Send invalid Markdown or a multi-block replacement where one block is required and confirm validation is explicit.
- [ ] If a live browser session is open, confirm successful edit responses report `injection: "injected"` when injection is expected.
- [ ] If no live browser session is open, confirm successful edit responses clearly report non-injected/external write behavior.

### Agent Review Operations

- [ ] Add a comment with `comment.add` anchored by exact `quote`.
- [ ] Add a comment where `quote` is omitted and confirm whole-block anchoring behavior.
- [ ] Try a missing `quote` and confirm `ANCHOR_NOT_FOUND`.
- [ ] Try a repeated ambiguous `quote` in one block and confirm `AMBIGUOUS_ANCHOR`.
- [ ] Add a comment reply with `comment.reply`.
- [ ] Resolve a comment with `comment.resolve`.
- [ ] Delete a reply and then delete a root comment with `comment.delete`.
- [ ] Add insertion, deletion, and substitution suggestions with `suggestion.add`.
- [ ] Accept and reject each suggestion kind through `/ops`.
- [ ] Confirm `/ops` batches are atomic: one invalid operation causes no partial write.
- [ ] Confirm `/ops` preserves or injects into a live browser session as expected.
- [ ] Confirm UI rail and in-editor marks update in both browser contexts after agent `/ops`.
- [ ] Confirm the Markdown persisted by agent `/ops` remains byte-compatible enough to load through the UI codec.

### Agent Events

- [ ] Open `/documents/{path}/events/stream` for an agent and confirm sparse events arrive after document changes.
- [ ] Call `/events/pending?after=<id>` and confirm polling returns missed events.
- [ ] Acknowledge events with `/events/ack` and confirm subsequent pending calls advance.
- [ ] Confirm events are wake signals only: agent re-reads `/snapshot` before acting.

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
- [ ] Sync when both sides changed the same path differently and confirm Quarry keeps its side at the canonical path.
- [ ] Confirm the Git side is preserved as a `*.conflict-git-*` sibling document.
- [ ] Confirm an open conflict record appears in CLI, REST, and UI.
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
  cargo run -p quarry -- --root "$QUARRY_ROOT" mount manual-main /tmp/quarry-mount --read-only
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

- [ ] With UI open, write a document through REST and confirm SSE causes tree/right-pane/editor state to update or show an external-change banner.
- [ ] With UI open, write through CLI and confirm expected refresh behavior.
- [ ] With UI open, write through FUSE and confirm expected refresh behavior.
- [ ] With UI open, run Git sync and confirm document list, conflicts, links, and versions refresh.
- [ ] Simulate EventSource unavailable and confirm polling fallback eventually refreshes state.
- [ ] Confirm stream lag handling triggers broader revalidation.
- [ ] Confirm browser does not reset an active live CRDT editor for its own flush echo.
- [ ] Confirm browser does surface true external changes for the active live document.

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
- [ ] Run a 30-minute live editing soak with two browsers and periodic agent `/ops`; confirm no drift or stale UI.
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

- [ ] Markdown bytes are the durable source of truth for normal document reads.
- [ ] FUSE, REST, CLI, Git, browser, and agent operations converge on the same committed state.
- [ ] Agent review marks and human review marks remain compatible in Markdown.
- [ ] Deleting, moving, or restoring documents never loses version history unexpectedly.
- [ ] No data loss, silent stale overwrite, lingering stale conflict, or cross-library metadata leak remains open.
