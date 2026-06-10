---
name: quarry
description: Use when a Quarry locator URL is shared, joining a local Quarry collaborative Markdown document, or using localhost Quarry REST APIs.
allowed-tools:
  - Bash
  - WebFetch
---

# Quarry

Quarry is a local-first collaborative Markdown editor for humans and agents. Use
plain HTTP against the local Quarry origin. Browser automation is not needed for
normal agent work.

A Markdown document is a tree of blocks with stable `block_id`s. Read
`GET $DOC/blocks`, address blocks by `block_id`, and send every mutation —
edits, comments, suggestions — as one transaction to `POST $DOC/transactions`.

Every write should carry a readable `actor.label`. Presence uses
`X-Agent-Id: ai:<agent-name>` or another stable session id.

## Default Behavior

If the user shares a Quarry locator URL:

- Join immediately.
- Register presence before reading.
- Read `GET $DOC/blocks` before editing or reviewing.
- Reply with the required ready message.
- Work in the Quarry document unless the user asks otherwise.
- Do not edit until the user gives an edit/review instruction.

Use review ops (`comment.add`, `suggestion.add`, …) for feedback requests. Use
edit ops (`replace_block_content`, `insert_block`, …) only when the user asks
you to directly change document content. Both share the same transaction
envelope.

## Locator URLs And Auth

Locator URL format:

```text
http://127.0.0.1:5173/lib/<library>/documents/<path>?token=<token>
```

Extract:

- origin: `http://127.0.0.1:5173`
- library: the URL-decoded segment after `/lib/`
- path: the encoded path after `/documents/`
- token: locator for browser/collab joins

Quarry REST agent APIs are trusted-localhost for now. The locator token is not
REST bearer auth unless discovery metadata later says otherwise.

Build the document API URL with each library/path segment URL-encoded:

```bash
ORIGIN="http://127.0.0.1:5173"
LIBRARY_ENCODED="team%20notes"
PATH_ENCODED="folder/live%20doc.md"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/libraries/$LIBRARY_ENCODED/documents/$PATH_ENCODED"
```

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
6. After events or a retryable error, re-read `/blocks` and rebuild the
   request with a fresh `base_clock` and a NEW `client_tx_id`.

## Presence

```bash
curl -sS -X POST "$DOC/presence" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"status":"reading","by":"Codex"}'
```

Statuses: `reading`, `thinking`, `acting`, `waiting`, `completed`, `error`.

## Blocks And Stable Ids

```bash
curl -sS "$DOC/blocks"
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
| `insert_block` | `{position, block_type, text?, attrs?, parent_block_id?}` |
| `delete_block` | `{block_id}` (descendants too) |
| `move_block` | `{block_id, position, parent_block_id?}` — placement only |
| `replace_block_content` | `{block_id, text, marks?, links?}` |
| `set_block_type` | `{block_id, block_type, attrs?}` — id/text/anchors preserved |
| `set_block_attrs` | `{block_id, attrs}` — replaces attrs wholesale |
| `add_mark` / `remove_mark` | `{block_id, start, end, marks}` |
| `set_link` | `{block_id, start, end, url}` (`url: null` removes) |

Review ops (same envelope, freely mixable with edit ops):

| op | shape |
|---|---|
| `comment.add` | `{block_id, start, end, body, quote?}` |
| `comment.reply` | `{item_id, body}` |
| `comment.resolve` / `comment.delete` | `{item_id}` |
| `suggestion.add` | `{block_id, start, end, replacement, body?, quote?}` |
| `suggestion.accept` | `{item_id}` — applies the replacement |
| `suggestion.reject` | `{item_id}` — resolves without changing text |

Anchors are `{block_id, start, end}` offsets into the block's `text`; `quote`
is an optional copy of the anchored text for display. An empty `replacement`
proposes a deletion; a collapsed range (`start == end`) proposes an insertion.
For a tight word-level redline, anchor only the words that change.

## Reading Review State

```bash
curl -sS "$DOC/review"
curl -sS "$DOC/review?includeResolved=1"
```

Returns `documentId`, `baseToken` (current clock), `comments` with nested
`replies`, unapplied `suggestions`, and `conflicts`. Comments and suggestions
carry `anchor: {blockId, startOffset, endOffset}` — feed those straight into
follow-up ops. Resolved items are omitted unless `includeResolved=1`;
`orphaned`/`invalidated` items always show.

`conflicts` are diff3 merge leftovers from whole-file writers (Git, FUSE, CLI,
Markdown PUT): the document kept the canonical side; the losing hunk is data —
`afterBlockId`, `baseMarkdown`, `incomingMarkdown`, `canonicalMarkdown`.
Resolve or dismiss with `comment.resolve` / `comment.delete`; resolution never
mutates the document.

To clear a review queue: decide suggestions (`suggestion.accept` /
`suggestion.reject`), apply comment-requested prose changes with edit ops,
resolve handled comments, then verify `GET $DOC/review` returns empty
`comments` and `suggestions`.

## Events

Events are activity signals, not document content. Re-read `/blocks` after an
event before replying, commenting, suggesting, or editing.

```bash
curl -N "$DOC/events/stream"
curl -sS "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/pending?after=0"
```

The pending response includes `events` and `nextAfter`. Store `nextAfter` for
the next poll. Ack processed events when useful:

```bash
curl -sS -X POST "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/ack" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"eventId": 42}'
```

## Error Handling

Failures are typed `{code, retryable, message}`. `retryable: true` = re-read
`/blocks`, rebuild with a fresh `base_clock` and NEW `client_tx_id`, retry
once. `retryable: false` = the ops as stated can never succeed; rebuild.

| Error | Action |
|---|---|
| `STALE_BASE` (412) | Re-read `/blocks`, resubmit with the fresh `document_clock` |
| `BLOCK_MOVE_CONFLICT` (412) | A concurrent structural change won; re-read and retry once |
| `BLOCK_DELETED` (404) | The `block_id` is gone; re-read `/blocks` and rebuild |
| `ANCHOR_NOT_FOUND` (404) | The review `item_id`/anchor does not exist; re-read `/review` |
| `SUGGESTION_INVALIDATED` (422) | Accept impossible; `suggestion.reject` dismisses it |
| `SUGGESTION_ALREADY_RESOLVED` (422) | Someone decided first; re-read `/review` |
| `UNSUPPORTED_MARKDOWN` (422) | The content is refused (e.g. CriticMarkup); fix the content |
| `UNSUPPORTED_BLOCK_DOCUMENT` (422) | Not a Markdown document; block APIs do not apply |
| `INVALID_TRANSACTION` (400) | Malformed envelope/op; fix the request |

If a retryable write still fails after one fresh read, stop and report the raw
error instead of guessing.

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
support `rewrite.apply` or REST bearer-token enforcement.
