# Collaborative editor session lifecycle

Date: 2026-06-07
Status: Proposal

## Summary

The collaborative editor should keep a stable live editing session mounted for
the selected document. Once a text document is open, the editor session should
be keyed by document identity, not by the latest persisted version or REST
cache state.

In a greenfield implementation, the browser would create a `DocumentSession`
for the open document. That session owns the Yjs document/provider, current
editor content, persisted head metadata, save state, and adopted flush
versions. REST/SWR fetches bootstrap the session and refresh surrounding
metadata, but they do not determine whether the editor remains mounted.

Core rule:

> Document identity controls editor lifecycle. Document versions control
> persistence metadata.

The current flash/reload behavior comes from mixing those two concepts.

## Observed Failure

Two browser clients had the same document open:

`http://127.0.0.1:5173/lib/demo/documents/testing-sync.md`

While both clients were editing concurrently, the document sometimes appeared
to reload or flash instead of applying remote edits seamlessly.

Server logs showed that the collab server was broadcasting Yjs updates, but the
browser websocket lifecycle churned immediately after REST saves. Around
`2026-06-07T17:08:02Z` for `testing-sync.md`:

- `document.put.committed` persisted a new version for doc id
  `c252bf68-3562-491e-917c-c97a39d6b299`.
- SSE emitted `doc.changed` for that version.
- A collab socket for the same document closed immediately after.
- A new collab socket opened shortly after for the same document.

The same close/reopen pattern repeated around `17:08:04Z` and `17:08:07Z`.
That points to frontend remount/reinitialization, not a server-side failure to
relay Yjs operations.

## Current Root Cause

In `ui/src/app/App.tsx`, the selected editor body is gated by
`selectedDocumentBodyReady`. That gate requires the local `etag`,
`loadedDocumentRef.current.etag`, and the SWR-fetched document `etag` to all
match.

During a collab flush, `save()` updates local persisted-head state first:

- `loadedDocumentRef.current`
- React `etag`
- collab flush ack state

The SWR cache for `/v1/document` is updated later. During that short gap, local
state has the new `etag` while the SWR document still has the old `etag`.
Therefore `selectedDocumentBodyReady` becomes false.

When that happens, the UI renders `LoadingDocument` instead of `DocumentBody`.
That unmounts `PlateMarkdownEditor`. Its cleanup destroys the Yjs lifecycle,
which closes the websocket. When the cache catches up, the editor remounts and
reinitializes the Yjs provider. The user sees that as a flash or reload.

This is an architectural lifecycle issue. The app treats version mismatch as
document unavailability, even though the active live editing session is valid
and should continue running.

## Design Principle

The editor must not be coupled to cache freshness.

REST document fetches answer "what is the latest persisted markdown snapshot?"
Yjs answers "what is the live collaborative editor state?"

Once a live Yjs session is active, ordinary persisted-head changes should not
tear it down. The editor should only unmount when the user closes the document,
switches to another document identity, changes to an unsupported content type,
or the app intentionally resets the session after an explicit rebase/recovery
decision.

## Proposed Greenfield Architecture

Introduce an explicit `DocumentSession` object for the selected document.

```ts
interface DocumentSession {
  documentId: string;
  library: string;
  path: string;
  contentType: string;

  ydoc: Y.Doc;
  provider: CollabProvider;

  markdown: string;
  persistedHead: {
    etag: string;
    versionId: string;
  };

  saveState: 'clean' | 'drafted' | 'saving' | 'saved' | 'stale' | 'failed';
  sessionId: string;

  adoptedVersionIds: Set<string>;
  adoptedEtags: Set<string>;

  externalChange: null | {
    kind: 'changed' | 'deleted';
    etag?: string;
    versionId?: string;
  };
}
```

The exact type shape can vary, but ownership should not:

- The session owns the editor lifecycle.
- The session owns the current persisted head.
- The session records which durable versions were produced or adopted by the
  live session.
- UI data caches are consumers of session state, not parents of editor
  lifecycle.

## Lifecycle

### Opening a document

1. Fetch the document once through REST.
2. Create a `DocumentSession` keyed by `documentId`.
3. Seed Yjs from the fetched markdown.
4. Mount the editor.
5. Connect the collab provider.

Loading UI is only shown before step 2 completes.

### Saving a live collab session

1. Serialize the live editor state to markdown.
2. Save through REST with `If-Match` using `session.persistedHead.etag`.
3. On success, synchronously update `session.persistedHead`.
4. Add the returned `etag` and `versionId` to `adoptedEtags` and
   `adoptedVersionIds`.
5. Optimistically update document cache entries from the same action.
6. Refresh side-pane data such as versions, backlinks, outgoing links, and
   search.

At no point should the editor unmount. A newer persisted head is metadata for
the existing session.

### Receiving SSE events

Durable storage events should be classified relative to the active session:

- Same origin id: ignore as own mutation echo.
- Version or etag is in `adoptedVersionIds`/`adoptedEtags`: ignore as adopted
  session output.
- Same document and source is the live collab session: update metadata only.
- External REST/FUSE/Git write not represented in Yjs: keep editor mounted and
  show an external-change/rebase affordance.
- Delete/move: keep editor mounted long enough to let the user choose whether
  to close, save elsewhere, resurrect, or follow the move.

SSE should invalidate side data, not force editor remount.

### Background document refetch

When `/v1/document` refetches while a session exists:

- If the fetched head equals `session.persistedHead`, treat it as cache catchup.
- If it is an adopted version, update metadata and ignore content reload.
- If it is an unknown newer version, mark `externalChange` and preserve the live
  editor.
- If it is an older version than the session head, ignore it.

The editor should not render a loading state because a refetch is pending or
temporarily stale.

## UI Model

The main editor render should depend on active session existence:

```tsx
if (!activeSession) return <LoadingDocument />;

return (
  <DocumentBody
    session={activeSession}
    sideData={sideData}
  />
);
```

It should not depend on all REST cache metadata matching in the current render.

Side panes can independently show stale/loading states for versions, backlinks,
outgoing links, graph data, or conflicts. Those are not editor lifecycle
concerns.

## Why This Is Better Than Patching The Gate

A narrow patch would loosen `selectedDocumentBodyReady` so the editor stays
mounted when local `etag` is ahead of SWR. That would likely fix the immediate
flash.

However, the deeper problem is that the app has no durable concept of an open
document session. State is spread across React state, refs, SWR cache, Yjs
plugin options, and awareness. That makes harmless timing gaps visible as
editor lifecycle transitions.

An explicit session model gives one place to reason about:

- What document is open.
- Whether the editor is mounted.
- What version has been persisted.
- Which storage events are already adopted.
- Whether an external write needs user attention.

It also makes future agent edits, FUSE writes, Git sync, and collaboration
recovery easier to classify without adding more render-time gates.

## Suggested Migration Shape For The Current Code

This repo does not need to jump all the way to a new store in one change. A
pragmatic migration could be:

1. Replace `selectedDocumentBodyReady` as the editor mount gate with a looser
   active-session gate keyed by `documentId`.
2. Keep `DocumentBody` mounted when local `etag` is newer than the SWR document
   `etag`.
3. Move persisted-head updates into a single helper that updates:
   - `loadedDocumentRef`
   - React `etag`
   - collab ack state
   - SWR `/v1/document` cache
4. Preserve adopted version sets when the same `documentId` receives a new
   persisted head.
5. Treat document refetch mismatches as external-change classification, not as
   editor unavailability.
6. Later, collapse the related refs/state into an explicit `DocumentSession`
   reducer or store.

The first two steps should remove the visible flash. The remaining steps reduce
the chance of similar lifecycle bugs returning.

## Acceptance Criteria

- During concurrent editing, a successful collab flush does not unmount
  `PlateMarkdownEditor`.
- The websocket for the active document does not close/reopen after ordinary
  REST flush acknowledgements.
- Remote Yjs updates appear without a loading state.
- Version history, backlinks, outgoing links, and document list still refresh
  after a save.
- External non-Yjs writes are surfaced as review/rebase state while preserving
  the local live editor.
- Switching to a different document still destroys the old session and creates
  a new one.

## Tests To Add

- A React test where local save success updates `etag` before SWR document cache
  catches up; assert the editor remains mounted.
- A test for `doc.changed` events with adopted version ids/etags; assert they
  do not trigger document reload.
- A test for unknown external `doc.changed`; assert it sets external-change
  state without replacing editor content.
- A Playwright test with two browser contexts editing the same document; assert
  no visible loading state and no websocket reconnect after autosave.

## Open Questions

- Should `DocumentSession` live in React state, a reducer, or a small external
  store?
- Should Yjs provider ownership move entirely out of `PlateMarkdownEditor` so
  the editor component becomes a view over an already-created session?
- Should markdown serialization for persistence be leader-only, or should the
  server eventually persist Yjs state directly and derive markdown separately?
- How should external Git/FUSE writes be rebased into an active Yjs document
  when automatic merge is safe?
