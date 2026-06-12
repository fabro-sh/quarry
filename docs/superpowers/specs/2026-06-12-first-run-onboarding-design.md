# First-Run Onboarding Modal & Change Attribution

**Date:** 2026-06-12
**Status:** Approved

## Purpose

When a user opens the Quarry workspace UI for the first time, show a modal that
briefly introduces Quarry and collects their name. The name is recorded as the
`actor` on every transaction the UI creates, so version history shows who made
each change.

## Decisions

- **Storage:** localStorage key `quarry:user-name`, alongside existing
  preference keys (`quarry:theme`, `quarry:active-library`). Per-browser; no
  server-side persistence.
- **Wiring:** a new optional `X-Quarry-Actor` request header on the REST
  auto-commit write endpoints, passed through to the transaction record.
- **Dismissal:** the modal cannot be dismissed without entering a name. No
  close button, no click-outside close, no Escape close.

## First-run detection

On workspace load, if `quarry:user-name` is missing or blank, render the
onboarding modal over the workspace. Saving a name writes the key and closes
the modal permanently for that browser. Users who used Quarry before this
feature will see the modal once — correct, since a name was never collected.

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

Input handling: trim before save; whitespace-only counts as empty; input
`maxLength` of 120 characters.

## Attribution wiring

### Server (Rust)

The REST auto-commit write handlers in `crates/quarry-server/src/lib.rs` read
an optional `X-Quarry-Actor` header and pass it to the storage layer:

- `put_document` (PUT `/v1/libraries/{library}/documents/{path}`)
- `delete_document` (DELETE same path)
- `move_document` (POST `.../move`)
- version restore (POST `.../versions/{version}/restore`)

The corresponding `quarry-storage` write methods gain an
`actor: Option<String>` parameter, forwarded to `insert_transaction_conn`
(which today receives a hardcoded `None` on these paths).

**Header encoding:** the value is percent-encoded UTF-8. Browser `fetch`
rejects non-Latin-1 header values, so names like "José" must be encoded by the
client; the server percent-decodes the header value before storing it. A
missing or empty header means `actor = None`, exactly as today.

The OpenAPI annotations document the new header parameter on these endpoints.

### UI client

In `ui/src/api/client.ts`, a helper reads `quarry:user-name` from localStorage
and returns `{ 'X-Quarry-Actor': encodeURIComponent(name) }`, or `{}` when
unset. Its result is merged into the request headers of `writeDocument` (which
backs `putDocument` and `createDocument`), `deleteDocument`, `moveDocument`,
and `restoreVersion`. No call-site signature changes.

### Display

No display work needed: the version history pane already renders
`transaction_actor` when present (`ui/src/app/App.tsx`, actor metadata row).

## Editing the name later

The existing Settings dialog gains a **Your name** text field that reads and
writes the same `quarry:user-name` key, with the same trim/empty rules. An
emptied name re-triggers the onboarding modal on next load (same detection
rule); writes made while the name is unset are unattributed.

## Testing

**Rust (`crates/quarry-server/tests/rest_api.rs`):**

- PUT with `X-Quarry-Actor: Bryan` records `transaction_actor = "Bryan"` on
  the resulting version.
- Percent-encoded header (`Jos%C3%A9`) decodes to `José` in the stored actor.
- PUT without the header still records no actor.

**UI (vitest, `ui/src/app/workspace.test.tsx` / `ui/src/api/client.test.ts`):**

- Modal renders when `quarry:user-name` is absent; not when present.
- Get started is disabled for empty/whitespace input; enabled otherwise.
- Saving persists the trimmed name and closes the modal.
- Document writes include the `X-Quarry-Actor` header when a name is set, and
  omit it when not.

## Out of scope

- Server-side persistence of the name; multi-user identity; avatars.
- Attributing CLI/agent/Git writes.
- Backfilling attribution on existing versions.
- Actor on explicit-transaction endpoints (they already accept `actor` in the
  request body).
- Actor on metadata patch and conflict-resolve endpoints (not called by the
  UI's write paths today; can adopt the same header later).
