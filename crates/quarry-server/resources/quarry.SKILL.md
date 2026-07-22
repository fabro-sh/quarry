---
name: quarry
description: Use when a Quarry locator URL is shared, joining a Quarry collaborative Markdown document on any origin, or using Quarry REST APIs.
allowed-tools:
  - Bash
  - WebFetch
---

# Quarry

Quarry is a local-first collaborative Markdown editor for humans and agents. Use
HTTP(S) requests against the same origin as the Quarry locator URL. Browser
automation is not needed for normal agent work.

A Markdown document is a tree of blocks with stable `block_id`s. Read
`GET $DOC/blocks`, address blocks by `block_id`, and send every mutation —
edits, comments, suggestions — as one transaction to `POST $DOC/transactions`.

Every semantic transaction should use the plain agent name as `actor.label`
(`Codex`, `Claude`, `Gemini`); this is the visible byline. Whole-document PUTs
use `X-Quarry-Transaction-Actor` for the same attribution. Presence uses
`X-Agent-Id: ai:<agent-name>` or another stable session id.

## Default Behavior

If the user shares a Quarry locator URL:

- Join immediately.
- Send `X-Agent-Id` on every request; POST `/presence` to announce your status.
- Read `GET $DOC/blocks` before editing or reviewing.
- Reply with the required ready message.
- Work in the Quarry document unless the user asks otherwise.
- Do not edit until the user gives an edit/review instruction.

Use review ops (`comment.add`, `suggestion.add`, …) for feedback requests and
proposals. Use edit ops (`replace_block_content`, `insert_markdown`, …) when
the user asks you to change document content. A concrete imperative Quarry
comment such as “Add this section,” “Change this wording,” or “Remove this
block” is a direct-edit instruction for that scoped change; do the work, reply
to the comment, and resolve the addressed thread. Do not respond only with a
promise or proposal. Unsolicited changes, and changes the user explicitly asks
to review before applying, stay as suggestions. Both operation families share
the same transaction envelope. To author or restructure a whole document,
prefer the Markdown `PUT` (see Whole-Document Markdown Writes) over
hand-assembling block ops.

## Locator URLs And Auth

Library locator URL format:

```text
http://127.0.0.1:5173/lib/<library>/documents/<path>?token=<token>
```

Tmp locator URL format:

```text
http://127.0.0.1:5173/tmp/72cb58585aa73e35758bc1141f79e32e
```

Extract:

- origin: `http://127.0.0.1:5173`
- library: the URL-decoded segment after `/lib/`
- path: the encoded path after `/documents/`
- token: locator for browser/collab joins on library documents
- tmp secret: the single URL segment after `/tmp/`

Library REST endpoints in the current full/local Quarry build are
trusted-localhost. Library locator tokens are not REST bearer auth unless
discovery metadata later says otherwise. For tmp documents on local or hosted
origins, the segment after `/tmp/` is both the document identifier and bearer
capability. Do not send a separate bearer token for tmp docs. Use `X-Agent-Id`
to identify your agent.

Build the document API URL with each library/path segment URL-encoded:

```bash
ORIGIN="http://127.0.0.1:5173"
LIBRARY_ENCODED="team%20notes"
PATH_ENCODED="folder/live%20doc.md"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/libraries/$LIBRARY_ENCODED/documents/$PATH_ENCODED"
```

For tmp documents:

```bash
ORIGIN="http://127.0.0.1:5173"
SECRET="72cb58585aa73e35758bc1141f79e32e"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/tmp/documents/$SECRET"
```

Tmp documents are Markdown-only scratch documents. Write them with
`Content-Type: text/markdown`; non-Markdown media types return 415, and
canonical UTF-8 Markdown over 1 MiB returns 413.

## Core Workflow

1. Show presence.
2. Read `GET $DOC/blocks`.
3. Reply exactly:

```text
Connected in Quarry and ready.
<one-sentence summary of the document>
I can edit directly, or leave comments and suggestions for you to review. What would you like me to do?
```

4. Wait for the user's instruction.
5. Before each write, use `block_id`s and the `document_clock` from the latest
   `/blocks` read.
6. After any event, re-read both `/blocks` and `/review` because review-only
   changes do not put comment or suggestion bodies in the block tree. For an
   error, follow the code-specific recovery under Error Handling.

When a user comment contains a concrete content request, completing the thread
means the requested content is present in the document—not merely that a reply
describes a future change. If the request is ambiguous or explicitly asks for
a proposal, add a suggestion and leave the thread open for the user's decision.

## Presence

```bash
curl -sS -X POST "$DOC/presence" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"status":"reading","by":"Codex"}'
```

Statuses: `reading`, `thinking`, `acting`, `waiting`, `completed`, `error`.

Presence expires 60 seconds after your last request. Any document API call
carrying `X-Agent-Id` refreshes it automatically (auto-registering you as
`waiting` on first contact), as does holding the document event stream open
with your `X-Agent-Id`. POST `/presence` when you want to declare a status
change or display name, not as a keepalive.

Library presence entries include `path`. Tmp presence entries omit `path`; use
the requested `$DOC` URL plus `documentId` to correlate them.

## Blocks And Stable Ids

```bash
curl -sS -H "X-Agent-Id: $AGENT_ID" "$DOC/blocks"
```

The response is `{document_id, document_clock, blocks: [...]}`. Each block has
a stable `block_id`, `parent_block_id` + `position` (tree shape),
`block_type`, `attrs`, flat `text`, and `marks`/`links` ranges. All offsets
are UTF-16 code units; range ends are exclusive.

- `block_id`s survive edits, moves, and Git/FUSE/CLI file writes. Copy them
  verbatim from `/blocks`; never invent them.
- `document_clock` is the version your read corresponds to — pass it as
  `base_clock` so the server can detect staleness.
- `raw_markdown` blocks carry their source in `attrs.markdown`; edit them with
  `set_block_attrs`, never with text ops.

`GET $DOC` returns the rendered Markdown (the current clock rides in `ETag`).

## Transactions

One envelope per mutation batch; ops apply in order and commit atomically:

```bash
curl -sS -X POST "$DOC/transactions" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{
    "client_tx_id": "codex-7f3a-1",
    "base_clock": "version_123",
    "actor": { "kind": "agent", "id": "ai:codex:abc123", "label": "Codex" },
    "ops": [
      { "op": "replace_block_content", "block_id": "01J9ZX...", "text": "Revised title" }
    ]
  }'
```

The ack is `{status, document_clock, transaction_id, changed_block_ids}`:

- `status` is `committed`, or `committed_rebased` when your `base_clock` was
  an older known version and the ops still applied cleanly against the
  current rows.
- The ack means the change is durable. Live browser sessions receive the
  transaction as another collaborator before the ack returns — no
  coordination needed.
- `client_tx_id` is the idempotency key (unique per document): replaying the
  same id returns the original ack without re-applying. Reuse the SAME id only
  to retry a timed-out request; use a NEW id for rebuilt requests.

Edit ops:

| op | shape |
|---|---|
| `insert_block` | `{position, block_type, text?, attrs?, marks?, links?, parent_block_id?, block_id?}` |
| `insert_markdown` | `{after_block_id?, markdown}` — parses and inserts a multi-block fragment; omit the anchor for document start |
| `delete_block` | `{block_id}` (descendants too) |
| `move_block` | `{block_id, position, parent_block_id?}` — placement only |
| `replace_block_content` | `{block_id, text, marks?, links?}` |
| `set_block_type` | `{block_id, block_type, attrs?}` — compatible content/anchors preserved |
| `set_block_attrs` | `{block_id, attrs}` — replaces attrs wholesale |
| `add_mark` | `{block_id, start, end, marks}` — `marks` is an object, e.g. `{"bold": true}` |
| `remove_mark` | `{block_id, start, end, marks}` — `marks` is a LIST of names, e.g. `["bold"]` |
| `set_link` | `{block_id, start, end, url}` (`url: null` removes) |

Block types: `p`, `h1`–`h6`, `blockquote`, `code_block` (+ `code_line`
children), `mermaid`, `table` (+ `tr`/`th`/`td` children), `img`, `hr`,
`raw_markdown`. There is NO list type (`ul`/`ol`/`li` are rejected): a list
item is a `p` block with attrs
`{"indent": 1, "listStyleType": "disc" | "decimal" | "todo"}` (`indent`
defaults to 1; `checked` for todos, `listStart` for ordered lists).

Only text-backed blocks (`p`, headings, `blockquote`, and `code_line`) accept
flat `text`, `marks`, or `links`. Container, void, and `raw_markdown` blocks
reject those fields. `set_block_type` also rejects conversions that would
discard flat text or container children.

A mark run (in `/blocks` reads and in `insert_block`/`replace_block_content`
`marks`) is `{start, end, marks}` where `marks` is an OBJECT keyed by mark
name: `[{"start": 0, "end": 5, "marks": {"bold": true}}]` — never
`{"type": "bold"}` or a list. Mark names: `bold`, `italic`, `strikethrough`,
`underline`, `superscript`, `subscript`, `code`.

Review ops (same envelope, freely mixable with edit ops):

| op | shape |
|---|---|
| `comment.add` | `{block_id, start, end, body, quote?}` |
| `comment.reply` | `{item_id, body}` — targets an open comment thread or open suggestion |
| `comment.edit` | `{item_id, body}` — open comment roots/replies only |
| `comment.resolve` / `comment.delete` | `{item_id}` |
| `suggestion.add` | `{block_id, start, end, replacement, body?, quote?}` |
| `suggestion.add_block_delete` | `{block_id, body?, quote?}` — proposes deleting the block and descendants |
| `suggestion.add_markdown` | `{after_block_id?, markdown, body?}` — proposes a structural Markdown insertion |
| `suggestion.accept` | `{item_id}` — applies the replacement or structural insertion/deletion and deletes suggestion replies |
| `suggestion.reject` | `{item_id}` — resolves without changing text and deletes suggestion replies |
| `conflict.keep_canonical` | `{item_id}` — resolves while retaining the current hunk |
| `conflict.accept_incoming` | `{item_id}` — verifies the current hunk, atomically replaces it with incoming, and resolves |

Anchors are `{block_id, start, end}` offsets into the block's `text`; `quote`
is an optional copy of the anchored text for display. An empty `replacement`
proposes a deletion; a collapsed range (`start == end`) proposes an insertion.
For a tight word-level redline, anchor only the words that change.
Use `suggestion.add_block_delete` when the block itself should disappear;
deleting all of a block's text leaves the empty block in place. A block-delete
suggestion follows the block across text edits and moves. Its optional `body`
is the rationale shown to the reviewer.
Use `insert_markdown` or `suggestion.add_markdown` for headings, fenced code,
tables, lists, and multi-block sections. Quarry parses the fragment and creates
the full block tree atomically, avoiding hand-built container/child ids.

## Whole-Document Markdown Writes

To replace or restructure most of a document, `PUT` the whole document as
Markdown. For a localized multi-block addition, prefer `insert_markdown` so
unrelated blocks are not part of the write:

```bash
curl -sS -X PUT "$DOC" \
  -H "Content-Type: text/markdown" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H "X-Quarry-Transaction-Actor: $AGENT_NAME" \
  -H 'If-Match: "<document_clock>"' \
  -H 'X-Quarry-Merge-Base: "<document_clock>"' \
  --data-binary @article.md
```

- `Content-Type: text/markdown` is required for whole-document Markdown
  writes. Do not rely on client defaults. Tmp document URLs require a Markdown
  media type, reject missing or non-Markdown `Content-Type` with 415, and
  reject canonical UTF-8 Markdown larger than 1 MiB with 413.
- Send `If-Match` with the current clock as a strict compare-and-swap
  precondition. A stale value fails 412 without changing the document.
- Send `X-Quarry-Merge-Base` with the clock whose content your Markdown was
  based on. It may be an older known version and drives diff3 independently
  of `If-Match`; an unknown value fails 412. Omitting it degenerates to a
  two-way merge. After a stale `If-Match`, re-read the head, update only
  `If-Match`, and retain the original merge base.
- `block_id`s and review anchors survive the rewrite. Merge leftovers become
  `conflicts` in `GET $DOC/review` — never write failures. A 200 alone does
  NOT mean your Markdown is now the document: inspect both `changed` and
  `conflicts` in the reply. `conflicts` counts the open conflict review items
  this write created or reused; `conflict_items` gives their stable ids and
  hunk payloads. If it is non-zero, the document kept the current text in
  those regions and your incoming text is parked in review. Re-read
  `GET $DOC/blocks` and `GET $DOC/review`, incorporate any canonical edits
  that should survive into your Markdown, and only then re-`PUT` the
  reconciled file with the fresh clock. Do not blindly resend the old file.
  Resolve each item with `conflict.keep_canonical` or
  `conflict.accept_incoming`; accepting first verifies that the canonical hunk
  has not changed. The reply also carries
  `changed: false` when a byte-identical write was a no-op.
- Use `PUT` to create or rewrite documents wholesale; use block transactions
  for surgical edits, comments, and suggestions on existing content.
- For library documents, Quarry refuses to change an existing Markdown block document into a raw
  document unless you explicitly opt in with
  `X-Quarry-Allow-Document-Kind-Change: true`. Agents should not send that
  header for normal Markdown authoring or editing.
- Tmp documents cannot be changed into raw documents.

After a `PUT`, re-read `GET $DOC/blocks`: ambiguous Markdown can land as
`raw_markdown` blocks, preserved verbatim but not block-addressable.

## Reading Review State

```bash
curl -sS -H "X-Agent-Id: $AGENT_ID" "$DOC/review"
curl -sS -H "X-Agent-Id: $AGENT_ID" "$DOC/review?includeResolved=1"
```

Returns `documentId`, `baseToken` (current clock), `comments` with nested
`replies`, unapplied `suggestions`, and `conflicts`. Comments and suggestions
carry `anchor: {blockId, startOffset, endOffset}` while their block exists —
feed those straight into follow-up ops. Suggestions expose their optional
rationale as `body`. Resolved items are omitted unless `includeResolved=1`;
`orphaned`/`invalidated` items always show.

`conflicts` are diff3 merge leftovers from whole-file writers (Git, FUSE, CLI,
Markdown PUT): the document kept the canonical side; the losing hunk is data —
`afterBlockId`, `baseMarkdown`, `incomingMarkdown`, `canonicalMarkdown`.
Resolve with `conflict.keep_canonical` or `conflict.accept_incoming`; the
incoming action verifies and replaces the canonical hunk atomically.
Conflict-marker warnings already landed as content and support only the keep
action.

To clear a review queue: decide suggestions (`suggestion.accept` /
`suggestion.reject`), apply comment-requested prose changes with edit ops,
resolve handled comments, then verify `GET $DOC/review` returns empty
`comments` and `suggestions`.

## Events

Events are activity signals, not document or review content. After an event,
re-read both `/blocks` and `/review` before replying, commenting, suggesting,
or editing. Review-only changes do not put comment or suggestion bodies in the
block tree.

```bash
curl -N -H "X-Agent-Id: $AGENT_ID" "$DOC/events/stream"
curl -sS "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/pending?after=0"
```

Library streams include document paths. Tmp document-scoped streams omit
`path`, `from`, and `to`; use the requested `$DOC` URL plus `doc_id` to
correlate tmp events.

The pending response includes `events` and `nextAfter`. Store `nextAfter` for
the next poll. Ack processed events when useful:

```bash
curl -sS -X POST "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/ack" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"eventId": 42}'
```

Tmp documents do not expose pending-event polling. If you cannot keep their
event stream open, periodically re-read both `$DOC/blocks` and `$DOC/review`.

## Error Handling

Every `/v1` HTTP failure is `{code, retryable, message}`. `retryable: true`
means the code-specific recovery may succeed; it does not mean blindly replay
the same request. Retry at most once. `retryable: false` means no automatic
retry is safe; fix, rebuild, or report the request.

| Error | Action |
|---|---|
| `STALE_BASE` (412) | Re-read `/blocks`, resubmit with the fresh `document_clock` |
| `BLOCK_MOVE_CONFLICT` (412) | A concurrent structural change won; re-read and retry once |
| `PRECONDITION_FAILED` (412) | Re-read current state, rebuild with the current precondition, retry once |
| `SERVICE_BUSY` (503) | Honor `Retry-After`; replay the unchanged idempotent request, preserving `client_tx_id` |
| `BLOCK_DELETED` (404) | The `block_id` is gone; re-read `/blocks` and rebuild |
| `ANCHOR_NOT_FOUND` (404) | The review `item_id`/anchor does not exist; re-read `/review` |
| `SUGGESTION_INVALIDATED` (422) | Accept impossible; `suggestion.reject` dismisses it |
| `SUGGESTION_ALREADY_RESOLVED` (422) | Someone decided first; re-read `/review` |
| `UNSUPPORTED_MARKDOWN` (422) | The content is refused (e.g. CriticMarkup); fix the content |
| `UNSUPPORTED_BLOCK_DOCUMENT` (422) | Not a Markdown document; block APIs do not apply |
| `PAYLOAD_TOO_LARGE` (413) | Tmp Markdown content exceeds 1 MiB; shorten it |
| `INVALID_TRANSACTION` (400) | Malformed envelope/op; fix the request |
| `UNKNOWN_BLOCK_TYPE` (400) | `block_type` outside the vocabulary; the message lists valid types |
| `INVALID_REQUEST` / `METHOD_NOT_ALLOWED` (400/405) | Fix the HTTP request |
| `NOT_FOUND` / `GONE` (404/410) | Check the locator or document lifecycle |
| `CONFLICT` (409) | Re-read state and reconsider the requested operation |
| `UNSUPPORTED_MEDIA_TYPE` / `UNPROCESSABLE_ENTITY` (415/422) | Fix the content type or body |
| `INTERNAL_ERROR` (500) | Stop and report; do not blindly retry a write |

If a retryable write still fails after its one code-specific recovery, stop
and report the raw error instead of guessing.

## When Quarry Looks Wrong

Collect raw evidence before summarizing:

- request URL, method, status, and response body
- library, document path, and agent id
- `client_tx_id`, `base_clock`, and `block_id` values used
- event id, `nextAfter`, and `origin_id` if relevant
- whether a fresh `/blocks` read and one safe retry changed the outcome
- any mismatch between REST responses and the open browser document

Do not keep retrying destructive writes.

## Discovery

Fetch these for current route metadata and schemas:

```bash
curl -sS "$ORIGIN/.well-known/agent.json"
curl -sS "$ORIGIN/agent-docs"
curl -sS "$ORIGIN/v1/openapi.json"
```

Do not assume Proof-only operations exist in Quarry. Quarry does not currently
support `rewrite.apply` or REST bearer-token enforcement on library endpoints;
tmp document path secrets are bearer capabilities.
