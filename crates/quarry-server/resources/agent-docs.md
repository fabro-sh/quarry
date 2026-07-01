# Quarry Agent Docs

Quarry is a local-first collaborative Markdown editor for humans and agents. Use
plain HTTP requests against the local `/v1` API to read, comment, suggest, and
edit documents. Browser automation is not needed for normal agent work.

The main Quarry-specific rule: a Markdown document is a tree of blocks with
stable `block_id`s. Read `GET /blocks`, address blocks by `block_id`, and send
every mutation — edits, comments, suggestions — as one semantic transaction to
`POST /transactions`. Never synthesize block ids.

## Two-Minute Version

1. If you received a Quarry link, extract the origin and document locator.
2. Register presence with a stable `X-Agent-Id`.
3. Read `GET /blocks` for the block tree, ids, and the current `document_clock`.
4. Reply with the required ready message.
5. Wait for the user's instruction before editing.
6. Send one `POST /transactions` envelope with your ops (edits and review ops
   share the same vocabulary and commit atomically). To author or restructure
   a whole document, `PUT` it as plain Markdown instead (see Whole-Document
   Markdown Writes).
7. On a retryable error (`retryable: true`), re-read `/blocks` and resubmit
   once with a fresh `base_clock` and a NEW `client_tx_id`.

## I Just Received A Quarry Link

Library document invite links look like this:

```text
http://127.0.0.1:5173/lib/team%20notes/documents/folder/live%20doc.md?token=invite-token
```

Tmp document links look like this:

```text
http://127.0.0.1:5173/tmp/72cb58585aa73e35758bc1141f79e32e
```

Use the link as a locator. The origin is the API origin. For library documents,
the library is the segment after `/lib/`, and the document path is the portion
after `/documents/`.

```sh
ORIGIN="http://127.0.0.1:5173"
LIBRARY_ENCODED="team%20notes"
LIBRARY="team notes"
PATH_ENCODED="folder/live%20doc.md"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/libraries/$LIBRARY_ENCODED/documents/$PATH_ENCODED"
```

For a tmp document, the segment after `/tmp/` is the share secret. It is both
the document locator and the bearer capability. Anyone with this URL can access
the tmp document. Omit the library and build the document API from
`/v1/tmp/documents/$SECRET`:

```sh
ORIGIN="http://127.0.0.1:5173"
SECRET="72cb58585aa73e35758bc1141f79e32e"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/tmp/documents/$SECRET"
```

For a raw document path like `notes/Project Plan.md`, encode each path segment
and keep slash separators: `notes/Project%20Plan.md`.

Tmp Markdown documents support the same block reads, semantic transactions,
comments, suggestions, presence, and document event streams as library
Markdown documents. They remain temporary documents: they do not have library search, graph, Git,
backlinks, promote from library routes, or library pending-event polling.

## Auth And Locator Tokens

Quarry REST agent APIs are trusted-localhost for now. For library document
invite URLs, the `?token=` value identifies the shared document for
browser/collab joins, but REST agent endpoints on this host do not currently
enforce bearer-token auth.

For tmp documents, the `/tmp/{secret}` URL segment is the bearer capability and
the document identifier. Do not treat it as an agent identity; use `X-Agent-Id`
for that.

## Headers And Identity

Use a stable agent id for the session, such as `ai:codex:<short-id>` or
`ai:claude:<short-id>`.

- `Content-Type: application/json`
- `X-Agent-Id: <agent-id>` for presence and event ack identity

Idempotency rides in the transaction body: `client_tx_id` is unique per
document, and replaying the same `client_tx_id` returns the original ack
without re-applying. Use the plain agent name as `actor.label` (`Codex`,
`Claude`, `Gemini`); this is the visible byline.

## Read The Document

Read the canonical block tree:

```sh
curl -sS "$DOC/blocks"
```

The response carries the rows and the current document clock:

```json
{
  "document_id": "doc_123",
  "document_clock": "version_123",
  "blocks": [
    {
      "block_id": "01J9ZX...",
      "parent_block_id": null,
      "position": 0,
      "block_type": "h1",
      "attrs": {},
      "text": "Title",
      "marks": [],
      "links": []
    }
  ]
}
```

- `block_id`s are stable: they survive edits, moves, and whole-file writes from
  Git/FUSE/CLI. Copy them verbatim; never invent or guess them.
- Blocks form a tree (`parent_block_id` + sibling `position`). `text` is the
  block's flat text; all offsets into it are UTF-16 code units.
- `marks` are inline formatting runs. Each run is `{start, end, marks}` where
  `marks` is an OBJECT keyed by mark name — for example
  `{"start": 0, "end": 5, "marks": {"bold": true}}`, never
  `{"type": "bold"}` or a list. Mark names: `bold`, `italic`,
  `strikethrough`, `underline`, `superscript`, `subscript`, `code`.
- `links` are `{start, end, url}` ranges.
- `raw_markdown` blocks carry their source in `attrs.markdown` and have no flat
  text — edit them with `set_block_attrs`, not text ops.

Fallback whole-document read (rendered Markdown, with the current clock in the
`ETag` header):

```sh
curl -sS "$DOC"
```

## Required Ready Reply

After reading the document, reply to the user with exactly this shape:

```text
Connected in Quarry and ready.
<one-sentence summary of the document>
I can edit directly, or leave comments and suggestions for you to review. What would you like me to do?
```

Do not edit before this reply unless the user already gave a clear edit
instruction in the same request.

## Transactions: The Single Mutation Contract

Every mutation is one envelope to `POST $DOC/transactions`:

```json
{
  "client_tx_id": "codex-7f3a-1",
  "base_clock": "version_123",
  "actor": { "kind": "agent", "id": "ai:codex:abc123", "label": "Codex" },
  "ops": [
    { "op": "replace_block_content", "block_id": "01J9ZX...", "text": "Revised title" }
  ]
}
```

- `client_tx_id`: any unique string per document. Duplicates replay the
  original ack without re-applying — safe to retry a timed-out request with
  the SAME id, but use a NEW id for a rebuilt request.
- `base_clock` (optional): the `document_clock` you read. Matching or omitted
  acks `committed`; an older-but-known clock applies against the current rows
  and acks `committed_rebased`; an unknown clock fails with retryable
  `STALE_BASE`.
- `ops` apply sequentially and commit atomically as ONE new version — one bad
  op fails the whole transaction with no partial write.

The ack:

```json
{
  "status": "committed",
  "document_clock": "version_124",
  "transaction_id": "btx_456",
  "changed_block_ids": ["01J9ZX..."]
}
```

The ack means the change is durable in canonical storage. If browsers have the
document open in a live session, the transaction is applied into that session
as another collaborator and checkpointed before the ack — you never need to
wait, retry, or coordinate with live editors.

### Edit Operations

- `insert_block` — `{position, block_type, text?, attrs?, marks?, links?, parent_block_id?}`.
  `position` is the sibling index under the parent (top level when omitted).
  `marks` uses the run shape above:
  `[{"start": 0, "end": 5, "marks": {"bold": true}}]`.
- `delete_block` — `{block_id}`. Deletes the block and its descendants.
- `move_block` — `{block_id, position, parent_block_id?}`. Placement only:
  content, children, ids, and review anchors ride along.
- `replace_block_content` — `{block_id, text, marks?, links?}`. Review anchors
  outside the changed span survive; anchors overlapping it orphan (comments)
  or invalidate (suggestions).
- `set_block_type` — `{block_id, block_type, attrs?}`. Changes the type while
  preserving id, text, marks, links, children, and anchors. Not valid to or
  from `raw_markdown`.
- `set_block_attrs` — `{block_id, attrs}`. Replaces attrs wholesale (for
  `raw_markdown` blocks, `attrs.markdown` must stay a non-empty string).
- `add_mark` — `{block_id, start, end, marks}` over UTF-16 offsets. `marks`
  is an object of mark names to merge into the range, e.g. `{"bold": true}`.
- `remove_mark` — `{block_id, start, end, marks}`. Unlike `add_mark`, here
  `marks` is a LIST of mark names to clear, e.g. `["bold"]`.
- `set_link` — `{block_id, start, end, url}`; `url: null` removes links in the
  range.

Block types are `p`, `h1`–`h6` (the heading level IS the type), `blockquote`,
`code_block` (with `code_line` children), `mermaid`, `table` (with `tr` and
`th`/`td` children), `img`, `hr`, and `raw_markdown`. There is no list-item
type: a list item is a `p` row whose attrs carry the list shape —
`{"indent": 1, "listStyleType": "disc" | "decimal" | "todo"}` plus `checked`
for todos and `listStart` for ordered lists (`indent` defaults to 1 when
omitted). Copy unfamiliar shapes from a `GET /blocks` read of a document that
already contains them.

Insert a paragraph after the current second block:

```sh
curl -sS -X POST "$DOC/transactions" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{
    "client_tx_id": "codex-7f3a-2",
    "base_clock": "version_124",
    "actor": { "kind": "agent", "id": "ai:codex:abc123", "label": "Codex" },
    "ops": [
      { "op": "insert_block", "position": 2, "block_type": "p",
        "text": "A new paragraph." }
    ]
  }'
```

### Review Operations

Comments and suggestions anchor to `{block_id, start, end}` ranges (UTF-16,
`end` exclusive) and share the transaction envelope, so a whole review lands
as one atomic batch:

- `comment.add` — `{block_id, start, end, body, quote?}`. `quote` is an
  optional copy of the anchored text for display.
- `comment.reply` — `{item_id, body}`. `item_id` may be an open comment
  thread, an existing reply in that thread, or an open suggestion.
- `comment.edit` — `{item_id, body}`. Edits open comment roots or replies
  only; resolved, orphaned, suggestion, and conflict ids are rejected.
- `comment.resolve` / `comment.delete` — `{item_id}`. Resolving never rewrites
  document text.
- `suggestion.add` — `{block_id, start, end, replacement, body?, quote?}`.
  `replacement` replaces the anchored range when accepted; an empty
  `replacement` proposes a deletion; a collapsed range (`start == end`)
  proposes an insertion.
- `suggestion.accept` — `{item_id}`. Applies the replacement, resolves the
  suggestion, and deletes its replies.
- `suggestion.reject` — `{item_id}`. Resolves without changing text and deletes
  its replies (also the way to dismiss an orphaned/invalidated suggestion).
- `conflict.add` — reconciler plumbing that records a merge-conflict review
  item without mutating the document; agents normally only read these via
  `GET /review` and resolve them with `comment.resolve` / `comment.delete`.

A full review as one transaction:

```json
{
  "client_tx_id": "codex-review-1",
  "base_clock": "version_124",
  "actor": { "kind": "agent", "id": "ai:codex:abc123", "label": "Codex" },
  "ops": [
    { "op": "comment.add", "block_id": "01J9ZX...", "start": 0, "end": 12,
      "quote": "NVIDIA cards", "body": "Mention the open kernel module here." },
    { "op": "suggestion.add", "block_id": "01J9ZY...", "start": 23, "end": 32,
      "quote": "A2 and B2", "replacement": "the slots your board's manual lists" }
  ]
}
```

For a tight word-level redline, anchor only the words that change rather than
the whole sentence.

`GET /review` returns `editedAt` on comments and replies when the latest row
timestamp differs from the creation timestamp; otherwise it is `null`.

## Whole-Document Markdown Writes

Block transactions are for surgical edits, comments, and suggestions. To
author a document from scratch or restructure one wholesale, skip the block
vocabulary entirely and `PUT` the document as plain Markdown — the server
parses headings, lists, marks, links, tables, and code fences from ordinary
syntax:

```sh
curl -sS -X PUT "$DOC" \
  -H "Content-Type: text/markdown" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H 'If-Match: "<document_clock>"' \
  --data-binary @article.md
```

Semantics:

- `If-Match` selects the MERGE BASE, not a strict precondition: the write is
  diff3-merged (`base`, your file, current canonical) so edits that landed
  after your read survive instead of being overwritten. A known-but-stale
  clock still merges cleanly; an unknown clock fails 412. Omitting
  `If-Match` degenerates to a two-way merge against the current document.
  `If-None-Match: *` creates a new document.
- `block_id`s and review anchors survive the rewrite — unchanged blocks keep
  their ids, so existing comments and suggestions stay anchored.
- True merge conflicts never fail the write: each one commits atomically as
  a conflict artifact and surfaces in `GET $DOC/review` under `conflicts`.
- A byte-identical body is a no-op (no new version).
- Write failures are content errors only: CriticMarkup (typed
  `UNSUPPORTED_MARKDOWN`), invalid frontmatter YAML, or non-UTF-8 bytes.

The response carries the new version; live browser sessions receive the merge
as a collaborator edit, same as transactions.

After a `PUT`, re-read `GET $DOC/blocks` before block-level follow-ups:
constructs the codec cannot model as first-class blocks land as
`raw_markdown` blocks — preserved and rendered verbatim, but not addressable
by text/marks ops, comments, or suggestions.

## Reading Review State

```sh
curl -sS "$DOC/review"
curl -sS "$DOC/review?includeResolved=1"
```

`GET $DOC/review` projects from the canonical review rows: `documentId`,
`baseToken` (the current clock), root `comments` with nested `replies`,
unapplied `suggestions`, and `conflicts`. Each comment and suggestion carries
`anchor: {blockId, startOffset, endOffset}` — use those ids and offsets
directly in follow-up ops. By default resolved items are omitted
(`includeResolved=1` includes them); `orphaned` and `invalidated` items always
show.

`conflicts` are diff3 merge conflicts from whole-file writers (Git, FUSE, CLI,
Markdown PUT): the canonical side stayed in the document and the losing hunk
is preserved as data — `afterBlockId` (`null` = document start),
`baseMarkdown`, `incomingMarkdown`, and `canonicalMarkdown`. Resolve or
dismiss them with `comment.resolve` / `comment.delete`; resolution never
mutates the document.

To clear a review queue: accept or reject open suggestions, apply any
comment-requested prose changes with edit ops, resolve the handled comments —
all in as few transactions as you like — then verify `GET $DOC/review` returns
empty `comments` and `suggestions`.

## Presence

Register or update presence before reading, commenting, suggesting, or editing:

```sh
curl -sS -X POST "$DOC/presence" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"status":"reading","by":"Codex"}'
```

Statuses are `reading`, `thinking`, `acting`, `waiting`, `completed`, and
`error`.

Presence expires 60 seconds after the last update. Holding the document event
stream open with your `X-Agent-Id` (see Events) refreshes it automatically for
as long as the stream stays connected; without a stream, re-POST `/presence`
at least once per minute while active. When your stream disconnects, your
presence is removed.

List presence for the same document:

```sh
curl -sS "$DOC/presence"
```

Library presence entries include the document `path`. Tmp presence entries omit
`path`; use the requested `$DOC` URL plus `documentId` to correlate them.

## Events

Events are activity signals for long-lived agents. They are not the source of
truth for document text. Re-read `/blocks` after an event before replying,
commenting, suggesting, or editing.

Prefer the document event stream. Send your `X-Agent-Id` so the open stream
also keeps your presence alive:

```sh
curl -N -H "X-Agent-Id: $AGENT_ID" "$DOC/events/stream"
```

If a stream is not practical for a library document, poll pending events:

```sh
curl -sS "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/pending?after=0"
```

The poll response contains `events` and `nextAfter`. Store `nextAfter` and pass
it as `after` on the next poll.

`doc.changed` events are sparse wake signals. They include revision metadata
such as `version_id`/`etag` and may include `origin_id`. Every write path —
browser checkpoints, agent transactions, Git/FUSE/CLI file writes — emits the
same event shape.

Library document streams include document paths. Tmp document-scoped streams
omit `path`, `from`, and `to`; use the requested `$DOC` URL plus `doc_id` to
correlate tmp events without echoing the capability secret.

Ack processed events when useful:

```sh
curl -sS -X POST "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/ack" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"eventId": 42}'
```

Tmp documents do not currently expose a pending-events poll route. Keep the
tmp document stream open while active, or re-POST `$DOC/presence` at least once
per minute and re-read `$DOC/blocks` after the user asks you to continue.

## Errors And Retry Rules

Failures return a typed payload: `{code, retryable, message}`.
`retryable: true` means "re-read `/blocks` and resubmit with a fresh
`base_clock` and a new `client_tx_id`"; `retryable: false` means the ops as
stated can never succeed — rebuild the request instead of retrying it.

| code | status | retryable | meaning |
|---|---|---|---|
| `STALE_BASE` | 412 | yes | `base_clock` does not name a known version |
| `BLOCK_MOVE_CONFLICT` | 412 | yes | concurrent structural change beat your move |
| `BLOCK_DELETED` | 404 | no | a referenced `block_id` no longer exists |
| `ANCHOR_NOT_FOUND` | 404 | no | a referenced review `item_id`/anchor does not exist |
| `SUGGESTION_INVALIDATED` | 422 | no | the suggestion's text changed; reject to dismiss |
| `SUGGESTION_ALREADY_RESOLVED` | 422 | no | accept/reject raced a prior decision |
| `UNSUPPORTED_MARKDOWN` | 422 | no | content the codec refuses (e.g. CriticMarkup) |
| `UNSUPPORTED_BLOCK_DOCUMENT` | 422 | no | block APIs on a non-Markdown document |
| `INVALID_TRANSACTION` | 400 | no | malformed envelope or op |
| `UNKNOWN_BLOCK_TYPE` | 400 | no | a `block_type` outside the vocabulary; the message lists valid types |

If a retryable write still fails after one fresh `/blocks` read, stop and
report the raw error to the user instead of guessing.

## Discovery And Schemas

Use discovery when you need current route metadata or schemas:

```sh
curl -sS "$ORIGIN/.well-known/agent.json"
curl -sS "$ORIGIN/v1/openapi.json"
curl -sS "$ORIGIN/quarry.SKILL.md"
```

Discovery includes route hints, auth mode, supported presence statuses, the
supported `transaction_operations`, and known limitations.

## Known Limitations

- REST agent endpoints currently trust localhost and do not enforce bearer-token
  auth.
- Library invite URL tokens are document locators for browser/collab joins, not
  REST auth tokens. Tmp URL secrets are bearer capabilities.
- Block APIs apply to Markdown documents only; other content types are raw
  bytes (`UNSUPPORTED_BLOCK_DOCUMENT`).
- Same-block merges with live human typing are convergence-only: concurrent
  edits to the same text interleave rather than being rejected.
- Quarry does not currently support Proof-only operations such as
  `rewrite.apply`.

## When Quarry Looks Wrong

If a read, write, event, or browser-visible state looks wrong, collect raw
evidence before summarizing:

- Exact request URL, method, status, and response body
- Library, document path, and agent id
- `client_tx_id`, `base_clock`, and `block_id` values used
- Event id, `nextAfter`, and `origin_id` if relevant
- Whether a fresh `/blocks` read and one safe retry changed the outcome
- Any visible mismatch between REST responses and the open browser document

Then report the evidence to the user. Do not keep retrying destructive writes.

## Safety Rules

- Register presence before reading, commenting, suggesting, or editing.
- Do not edit until the user gives explicit instructions.
- Prefer comments and suggestions for review requests.
- Use direct edits for implementation requests.
- Re-read `/blocks` after any event and after any stale write.
- Use the plain agent name as `actor.label`; it is the visible byline.
- Fetch `/.well-known/agent.json` and `/v1/openapi.json` when you need current
  route metadata or schemas.
