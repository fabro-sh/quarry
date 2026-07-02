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

Every write should use the plain agent name as `actor.label` (`Codex`,
`Claude`, `Gemini`); this is the visible byline. Presence uses `X-Agent-Id:
ai:<agent-name>` or another stable session id.

## Default Behavior

If the user shares a Quarry locator URL:

- Join immediately.
- Send `X-Agent-Id` on every request; POST `/presence` to announce your status.
- Read `GET $DOC/blocks` before editing or reviewing.
- Reply with the required ready message.
- Work in the Quarry document unless the user asks otherwise.
- Do not edit until the user gives an edit/review instruction.

Use review ops (`comment.add`, `suggestion.add`, …) for feedback requests. Use
edit ops (`replace_block_content`, `insert_block`, …) only when the user asks
you to directly change document content. Both share the same transaction
envelope. To author or restructure a whole document, prefer the Markdown `PUT`
(see Whole-Document Markdown Writes) over hand-assembling block ops.

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

Quarry REST agent APIs are trusted-localhost for now. Library locator tokens are
not REST bearer auth unless discovery metadata later says otherwise. For tmp
documents, the segment after `/tmp/` is the document identifier and capability.
Do not send a separate bearer token for tmp docs. Use `X-Agent-Id` to identify
your agent.

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
| `insert_block` | `{position, block_type, text?, attrs?, marks?, links?, parent_block_id?}` |
| `delete_block` | `{block_id}` (descendants too) |
| `move_block` | `{block_id, position, parent_block_id?}` — placement only |
| `replace_block_content` | `{block_id, text, marks?, links?}` |
| `set_block_type` | `{block_id, block_type, attrs?}` — id/text/anchors preserved |
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
| `suggestion.accept` | `{item_id}` — applies the replacement and deletes suggestion replies |
| `suggestion.reject` | `{item_id}` — resolves without changing text and deletes suggestion replies |

Anchors are `{block_id, start, end}` offsets into the block's `text`; `quote`
is an optional copy of the anchored text for display. An empty `replacement`
proposes a deletion; a collapsed range (`start == end`) proposes an insertion.
For a tight word-level redline, anchor only the words that change.

## Whole-Document Markdown Writes

To author or restructure substantial content, skip block ops and `PUT` the
whole document as Markdown — the server parses lists, marks, and links from
ordinary syntax, so there is no block/attrs vocabulary to get wrong:

```bash
curl -sS -X PUT "$DOC" \
  -H "Content-Type: text/markdown" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H 'If-Match: "<document_clock>"' \
  --data-binary @article.md
```

- `Content-Type: text/markdown` is required for whole-document Markdown
  writes. Do not rely on client defaults: form submission media types such as
  `application/x-www-form-urlencoded` are rejected for extensionless tmp
  document URLs, and missing `Content-Type` is rejected there too.
- Send `If-Match` with the clock you last read. It selects the merge base:
  the write is diff3-merged against the current document, so concurrent
  edits survive instead of being overwritten. A known-but-stale clock still
  merges; an unknown one fails 412. No `If-Match` degenerates to a two-way
  merge against the current document.
- `block_id`s and review anchors survive the rewrite. Merge leftovers become
  `conflicts` in `GET $DOC/review` — never write failures.
- Use `PUT` to create or rewrite documents wholesale; use block transactions
  for surgical edits, comments, and suggestions on existing content.
- Quarry refuses to change an existing Markdown block document into a raw
  document unless you explicitly opt in with
  `X-Quarry-Allow-Document-Kind-Change: true`. Agents should not send that
  header for normal Markdown authoring or editing.

After a `PUT`, re-read `GET $DOC/blocks`: ambiguous Markdown can land as
`raw_markdown` blocks, preserved verbatim but not block-addressable.

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
| `UNKNOWN_BLOCK_TYPE` (400) | `block_type` outside the vocabulary; the message lists valid types |

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
