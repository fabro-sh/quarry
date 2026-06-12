# First-Run Onboarding Modal & Change Attribution

**Date:** 2026-06-12
**Status:** Approved (revised after rebase onto origin/master)

## Purpose

When a user opens the Quarry workspace UI for the first time, show a modal that
briefly introduces Quarry and collects their name. The name is recorded as the
transaction `actor` on every document mutation the UI makes, so version history
shows who made each change.

## What already exists on this base

Much of the original design landed independently; this spec builds on it:

- **Author identity:** `ui/src/features/review/identity.ts` stores a free-form
  author label in localStorage under `quarry:author` (`loadAuthor` /
  `saveAuthor` / `normalizeAuthor`), defaulting to `'user'` when unset. It is
  used as the `by:` label on review items and as `byHint` on collab invites.
- **Settings field:** the Settings dialog already has an **Author** text field
  editing that key.
- **Attribution transport:** mutation endpoints read an optional
  `X-Quarry-Transaction-Actor` header into `TransactionMetadata.actor`
  (`transaction_metadata_from_headers` in `crates/quarry-server/src/lib.rs`).
  The UI client supports it via `DocumentMutationOptions.transactionActor`
  (`mutationHeaders` in `ui/src/api/client.ts`).
- **Display:** the version history pane already renders the transaction actor.

**The gaps:** nothing in the UI ever sets `transactionActor`, so UI writes are
unattributed; and the author is never collected — first-run users silently
write as `'user'`.

## Decisions

- **Storage:** reuse the existing `quarry:author` localStorage key and
  `identity.ts` helpers. No new key, no server-side persistence.
- **First-run trigger:** the raw localStorage key being absent (not
  `loadAuthor()`'s `'user'` fallback) means the user never chose a name —
  show the modal.
- **Dismissal:** the modal cannot be dismissed without entering a name. No
  close button, no click-outside close, no Escape close.
- **Wiring:** plumb the author into `transactionActor` for every UI document
  mutation via the existing `browserMutationOptions()` helper.

## The modal

Follows the existing `SettingsDialog` pattern in `ui/src/app/App.tsx` (fixed
overlay, centered card, `useDialogFocusTrap`), minus all dismissal affordances.

Content:

1. Short welcome copy (2–3 sentences, matching the existing Fabro-themed
   tone): Quarry is a local-first workspace for versioned documents; every
   change is saved with full history.
2. One autofocused text input labeled **Your name**, with helper text:
   "Quarry records your name on every change you make, so history shows who
   did what."
3. A **Get started** primary button, disabled until the trimmed input is
   non-empty. Enter submits when valid.

Input handling: trim before save (via `saveAuthor`); whitespace-only counts as
empty; input `maxLength` of 120 characters. Saving writes `quarry:author` and
closes the modal permanently for that browser. Users from before this feature
will see the modal once, since a name was never explicitly collected.

## Attribution wiring

### UI

`browserMutationOptions()` (`ui/src/app/App.tsx`) currently returns
`{ originId }` and is already passed to every document mutation call site
(save, create, upload, move, delete, restore). Add the author:

```ts
return { originId, transactionActor: author };
```

The two conflict-dialog call sites that build options inline are routed
through the same helper so no mutation path is missed.

When the author is the `'user'` default (key absent — only possible
transiently before the modal saves), omit `transactionActor` so those writes
stay unattributed rather than stamped `'user'`.

### Server: header decoding

`X-Quarry-Transaction-Actor` values are percent-encoded UTF-8. Browser `fetch`
rejects non-Latin-1 header values and axum's `HeaderValue::to_str()` rejects
non-ASCII bytes, so a name like "José" would otherwise fail the request. The
client sends `encodeURIComponent(name)`; the server percent-decodes the header
value in `transaction_metadata_from_headers`. Plain-ASCII values without `%`
decode to themselves, so existing senders (e.g. agents passing `Codex`) are
unaffected.

### Server: delete / move / restore

Only `put_document` honors the transaction metadata headers today. The
`delete_document`, move, and restore handlers pass no actor; the corresponding
storage methods (`delete_document_with_origin`, `move_document_with_origin`,
`restore_document_version_with_origin` in `crates/quarry-storage/src/lib.rs`)
hardcode `None` into their transaction records. Each gains an
`actor: Option<String>` parameter threaded into its `insert_transaction_conn`
/ `TransactionMetadata`, and the REST handlers pass the decoded header value.

### Server: live session checkpoints (the main edit path)

Typing in the editor does not go through REST at all: the server owns the live
Yjs session and periodically checkpoints it via `commit_block_mutation`,
hardcoding `transaction_actor: Some("browser")`
(`crates/quarry-server/src/session.rs`). This is where most human edits are
attributed, so it must use the real name.

The browser already publishes the author into Yjs awareness: the Plate editor
configures cursor data as `{ color, name: author }`, which slate-yjs stores in
each client's awareness state under the `data` field. At checkpoint time the
server derives the actor from awareness instead:

- Collect `data.name` from every connected client's awareness state; trim,
  drop empties, dedupe, sort, join with ", " (multiple participants produce
  "Avery, Blake").
- Cache the last non-empty result on the session so the final checkpoint
  (which can run after the socket closes and awareness empties) still
  attributes correctly.
- Fall back to `"browser"` when no name was ever seen — preserving today's
  behavior for nameless sessions.

SSE (`/v1/events`) is receive-only; nothing to do there.

## Editing the name later

Already exists: the Settings dialog **Author** field. Unchanged, except it now
also affects attribution (same key). Clearing it removes the key
(`saveAuthor` semantics), which re-triggers the onboarding modal on next load.

## Testing

**Rust (`crates/quarry-server/tests/rest_api.rs`):**

- PUT with `X-Quarry-Transaction-Actor: Avery` records actor `Avery` on the
  resulting version; percent-encoded `Jos%C3%A9` decodes to `José`; no header
  keeps the pre-existing default (`null` on raw-document paths, the gateway's
  `"rest"` label on markdown writes).
- DELETE / move / restore with the header record the actor on their
  transaction records.
- A live session whose client publishes awareness cursor data
  `{ data: { name: "Avery" } }` checkpoints with `transaction_actor: "Avery"`;
  sessions without awareness names keep checkpointing as `"browser"` (existing
  test stays green).

**UI (vitest, `ui/src/app/workspace.test.tsx` / `ui/src/api/client.test.ts`):**

- Modal renders when `quarry:author` is absent; not when present.
- Get started is disabled for empty/whitespace input; enabled otherwise.
- Saving persists the trimmed name and closes the modal.
- Document writes include `X-Quarry-Transaction-Actor` with the
  percent-encoded name when set, and omit it when the key is absent.

## Out of scope

- Server-side persistence of the name; multi-user identity; avatars.
- Attributing CLI/agent/Git writes (agents already self-declare via the block
  transaction `actor` and review `by:` labels).
- Backfilling attribution on existing versions.
- Per-participant attribution within a shared collab session.
