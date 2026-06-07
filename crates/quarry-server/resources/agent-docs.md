# Quarry Agent Docs

Quarry is a local-first collaborative Markdown editor for humans and agents. Use
plain HTTP requests against the local `/v1` API to read, comment, suggest, and
edit documents. Browser automation is not needed for normal agent work.

The main Quarry-specific rule: edits are block-scoped. Read `/snapshot`, copy
the current `baseToken` and exact block `ref` values, then send those values
back to `/edit` or `/ops`. Never synthesize refs.

## Two-Minute Version

1. If you received a Quarry link, extract the origin, library, and document path.
2. Register presence with a stable `X-Agent-Id`.
3. Read `GET /snapshot`.
4. Reply with the required ready message.
5. Wait for the user's instruction before editing.
6. Use `/ops` for comments and suggestions; use `/edit` for direct block edits.
7. If the document changed, re-read `/snapshot` and retry once with fresh refs.

## I Just Received A Quarry Link

Quarry invite links look like this:

```text
http://127.0.0.1:5173/lib/team%20notes/documents/folder/live%20doc.md?token=invite-token
```

Use the link as a locator. The origin is the API origin, the library is the
segment after `/lib/`, and the document path is the portion after `/documents/`.
Keep document path segments URL-encoded in REST URLs and preserve `/` separators.

```sh
ORIGIN="http://127.0.0.1:5173"
LIBRARY_ENCODED="team%20notes"
LIBRARY="team notes"
PATH_ENCODED="folder/live%20doc.md"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/libraries/$LIBRARY_ENCODED/documents/$PATH_ENCODED"
```

For a raw document path like `notes/Project Plan.md`, encode each path segment
and keep slash separators: `notes/Project%20Plan.md`.

## Auth And Locator Tokens

Quarry REST agent APIs are trusted-localhost for now. The `?token=` value in a
browser invite URL identifies the shared document for browser/collab joins, but
REST agent endpoints on this host do not currently enforce bearer-token auth.

Do not send the locator token as a REST bearer token unless future discovery
metadata explicitly says to do so. Check `/.well-known/agent.json` when in doubt.

## Headers And Identity

Use a stable agent id for the session, such as `ai:codex:<short-id>` or
`ai:claude:<short-id>`.

- `Content-Type: application/json`
- `X-Agent-Id: <agent-id>` for presence and event ack identity
- `Idempotency-Key: <unique-key>` is supported on `/edit` and `/ops`

Use a readable `by` value in presence, comments, suggestions, and replies so the
human can see who acted.

## Read The Document

Prefer the block snapshot:

```sh
curl -sS "$DOC/snapshot"
```

Snapshot responses contain `documentId`, `baseToken`, and `blocks`. Each block
has Markdown content and a `ref` that identifies that exact block in that exact
document version:

```json
{
  "documentId": "doc_123",
  "baseToken": "version_123",
  "blocks": [
    {
      "ref": {
        "ordinal": 0,
        "contentHash": "abc123"
      },
      "markdown": "# Title\n\n"
    }
  ]
}
```

Use the latest top-level `baseToken` and the target block's `ref.ordinal` for
`/edit` and `/ops`. Snapshot responses always include `ref.contentHash`, but it
is optional in write requests: omit it when writing immediately after reading
the same `/snapshot`, and include it when you refresh only the token via `HEAD`
but reuse older refs and want shifted or edited blocks to be caught. `/snapshot`
returns the easiest JSON form: the raw version id. Write endpoints also accept
ETag-shaped values copied from HTTP `ETag` headers, such as `"version_123"` or
`W/"version_123"`.

If you only need to refresh the token after a write and do not need fresh
ordinals or hashes, you can read the `ETag` header with `HEAD $DOC`. Re-read
`/snapshot` when you need current block refs.

Fallback full document read:

```sh
curl -sS "$DOC"
```

## Required Ready Reply

After reading the snapshot, reply to the user with exactly this shape:

```text
Connected in Quarry and ready.
<one-sentence summary of the document>
I can edit directly, or leave comments and suggestions for you to review. What would you like me to do?
```

Do not edit before this reply unless the user already gave a clear edit
instruction in the same request.

## Choose The Right Operation

Use `/ops` when the user asks for review, feedback, comments, suggestions, or
track-change style proposals.

Use `/edit` when the user asks you to directly change document content.

Write endpoints also accept `?dryRun=1` when you want to validate refs and
planned changes without committing them.

## How Block Refs Work

A block `ref` targets a top-level Markdown block. Its `ordinal` is the block
position for the current top-level `baseToken`; its optional `contentHash` is an
extra guard you can send when reusing older refs after a token-only refresh. If
the document changes, old refs may become stale.

When choosing a target:

- Pick the block whose `markdown` contains the text you want to edit or review.
- Use the block's `ordinal`; include `contentHash` only when you want the
  optional shifted-block guard.
- If the target spans multiple blocks, use multiple operations.
- If a write reports `STALE_BASE`, discard old refs, fetch a fresh snapshot, and
  rebuild the operation from the new refs.
- On `/ops`, include `quote` when anchoring to specific text within the chosen
  block; `quote` complements, but does not replace, the optional hash guard.

## Direct Edits

Direct edit operations are `replace_block`, `insert_before`, `insert_after`,
`delete_block`, and `replace_document`.

Replace a block with an idempotency key:

```sh
curl -sS -X POST "$DOC/edit" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H "Idempotency-Key: edit-abc123-1" \
  -d '{
    "baseToken": "version_123",
    "operations": [
      {
        "op": "replace_block",
        "ref": {
          "ordinal": 0
        },
        "block": { "markdown": "# Revised title\n\n" }
      }
    ]
  }'
```

Insert several blocks at one anchor with `blocks`:

```sh
curl -sS -X POST "$DOC/edit" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H "Idempotency-Key: edit-abc123-2" \
  -d '{
    "baseToken": "version_123",
    "operations": [
      {
        "op": "insert_after",
        "ref": {
          "ordinal": 0
        },
        "blocks": [
          { "markdown": "First inserted paragraph\n" },
          { "markdown": "Second inserted paragraph\n" }
        ]
      }
    ]
  }'
```

Each inserted or replacement block must be one Markdown block. Do not send a
whole multi-section document as one replacement block unless the target block is
itself one Markdown block. For multiple insertions at the same ref, use
`blocks` on one `insert_before` or `insert_after` operation instead of repeated `insert_after`
calls on the same ref.

To replace the whole document, send exactly one `replace_document` operation.
It uses the top-level `baseToken` and does not accept `ref`, `block`, or
`blocks`. Empty Markdown is valid. The replacement is stored as one Markdown
document, then split into normal snapshot blocks on the next read.

```json
{
  "baseToken": "version_123",
  "operations": [
    {
      "op": "replace_document",
      "markdown": "# Title\n\nBody\n"
    }
  ]
}
```

## Comments And Suggestions

Use `/ops` for review feedback. Supported operations are `comment.add`,
`comment.reply`, `comment.delete`, `comment.resolve`, `suggestion.add`,
`suggestion.accept`, and `suggestion.reject`.

`/ops` accepts a batch of one or more operations. All operations share one
top-level `baseToken` and `by` author, resolve refs against the original
snapshot, and commit atomically. A single batch may freely mix `comment.add`
and `suggestion.add`: annotations never shift block ordinals, so a whole review
anchors to one snapshot — send it as one batch instead of refreshing the token
between annotations. Since the batch is all-or-nothing, dry-run it first
(`?dryRun=1`) to catch a bad `quote`, then POST the same body to commit.

Operations in the same batch generally must target disjoint original spans
within each block. One narrow exception is supported: if `comment.add` omits
`quote` and overlaps another same-block annotation, Quarry stores that comment as
a comment-only block marker at the end of the block text so the batch can commit.
Quoted comments overlapping suggestions, partial overlaps, and nested review
markup still conflict.

A full review as one batch:

```json
{
  "baseToken": "version_123",
  "by": "Codex",
  "operations": [
    { "op": "comment.add", "ref": { "ordinal": 4 }, "quote": "NVIDIA cards",
      "body": "Mention the open kernel module here." },
    { "op": "suggestion.add", "kind": "replace", "ref": { "ordinal": 11 },
      "quote": "A2 and B2", "content": "the slots your board's manual lists" },
    { "op": "comment.add", "ref": { "ordinal": 18 }, "quote": "memory test",
      "body": "Name a tool, e.g. memtest86+." }
  ]
}
```

Read existing review work without parsing CriticMarkup:

```sh
curl -sS "$DOC/review"
curl -sS "$DOC/review?includeResolved=1"
```

`GET $DOC/review` returns `documentId`, `baseToken`, root `comments` with nested
`replies`, and current unapplied `suggestions`. By default, resolved comments
are omitted; add `includeResolved=1` to include them. Suggestions include
`quote`, `content`, and `preview: { "before": "...", "after": "..." }` so you
can decide whether to accept or reject without parsing CriticMarkup.

### Processing Review Feedback

To clear a document's review queue, read `GET $DOC/review`, then prefer
`POST $DOC/review` for a convenience workflow that can process suggestion
decisions, direct block edits, and comment resolutions in one agent request. It
is a wrapper over the existing `/ops` and `/edit` behavior, not a new
transaction model; internally, edit operations use the `edit.` prefix and block
refs are refreshed by ordinal after earlier wrapper phases.

```json
{
  "baseToken": "version_123",
  "by": "Codex",
  "operations": [
    { "op": "suggestion.accept", "id": "s1" },
    {
      "op": "edit.replace_block",
      "ref": { "ordinal": 4 },
      "block": { "markdown": "Updated block markdown\n\n" }
    },
    { "op": "comment.resolve", "id": "c1" }
  ]
}
```

When the wrapper is not a fit, use the lower-level route sequence: accept or
reject open suggestions in a single `/ops` batch when possible, apply
comment-requested content changes with `/edit`, resolve the handled comments
with `/ops`, then verify that `GET $DOC/review` returns `comments: []` and
`suggestions: []`.

Add a comment:

```sh
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H "Idempotency-Key: ops-abc123-1" \
  -d '{
    "baseToken": "version_123",
    "by": "Codex",
    "operations": [
      {
        "op": "comment.add",
        "ref": {
          "ordinal": 0
        },
        "quote": "Title",
        "body": "Consider making this title more specific."
      }
    ]
  }'
```

Suggest a replacement:

```sh
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H "Idempotency-Key: ops-abc123-2" \
  -d '{
    "baseToken": "version_123",
    "by": "Codex",
    "operations": [
      {
        "op": "suggestion.add",
        "kind": "replace",
        "ref": {
          "ordinal": 0
        },
        "quote": "Title",
        "content": "Project Plan"
      }
    ]
  }'
```

Suggestion kinds (each renders as a CriticMarkup redline for the reviewer):

- `insert` — add `content` immediately after the `quote` span, or at the end of
  the block when `quote` is omitted. Removes nothing. Requires `content`.
- `delete` / `remove` — propose removing the `quote` span. Aliases. No `content`.
- `replace` / `substitution` — propose replacing the `quote` span with `content`.
  Aliases; `substitution` is the default when `kind` is omitted. Requires
  `content`.

Diff granularity comes from the size of the `quote`, not the kind — `replace`
and `substitution` are the same operation. For a tight word-level redline,
quote only the words that change rather than the whole sentence. To turn
"drivers from the distribution" into "drivers from the distribution's
repositories", use `replace` with `quote` `distribution` and `content`
`distribution's repositories`, not the entire sentence.

The `quote` field anchors the annotation to a span inside the block and must
match the block's `markdown` exactly. Omit it to anchor the whole block. When
present, it is an exact, case- and whitespace-sensitive substring that must
occur exactly once: zero matches fail with `ANCHOR_NOT_FOUND`, and two or more
fail with `AMBIGUOUS_ANCHOR` (extend the quote with adjacent text to make it
unique). An empty string is rejected; omit the field instead. `/ops` commits
atomically, so one bad `quote` fails the whole batch — copy quotes verbatim
from the current `/snapshot`.

Reply to or delete comments:

```sh
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"version_123","by":"Codex","operations":[{"op":"comment.reply","parentId":"c_123","body":"Thanks, I will adjust this."}]}'

curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"version_123","operations":[{"op":"comment.delete","id":"c_123"}]}'
```

`comment.reply` requires `parentId` for the root comment and `body`; `id` is
optional. `comment.delete` accepts a root comment id or reply id. Deleting a
root removes its inline comment mark and direct replies. Deleting a reply
removes only that reply metadata.

Resolve a comment or decide a suggestion:

```sh
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"version_123","operations":[{"op":"comment.resolve","id":"c_123"}]}'

curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"version_123","operations":[{"op":"suggestion.accept","id":"s_123"}]}'
```

`suggestion.accept` applies the proposed edit to the document automatically.
`comment.resolve` only changes the comment's review state; it does not rewrite
the document text. If a comment asks for a prose change, apply that change with
`/edit` and then resolve the comment.

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

List presence for the same document:

```sh
curl -sS "$DOC/presence"
```

## Events

Events are activity signals for long-lived agents. They are not the source of
truth for document text. Re-read `/snapshot` after an event before replying,
commenting, suggesting, or editing.

Prefer the document event stream:

```sh
curl -N "$DOC/events/stream"
```

If a stream is not practical, poll pending events:

```sh
curl -sS "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/pending?after=0"
```

The poll response contains `events` and `nextAfter`. Store `nextAfter` and pass
it as `after` on the next poll.

`doc.changed` events are sparse wake signals. They include revision metadata
such as `version_id`/`etag` and may include `origin_id`. When live
`/ops` mutations are injected into an open browser editor, `origin_id`
starts with `agent-injected:`. The event may include a review metadata patch
such as `review.comments`, `review.suggestions`, `review.removeComments`, or
`review.removeSuggestions`.

Ack processed events when useful:

```sh
curl -sS -X POST "$ORIGIN/v1/libraries/$LIBRARY_ENCODED/events/ack" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"eventId": 42}'
```

## Errors And Retry Rules

- `STALE_BASE`: the document changed since your snapshot. Fetch `/snapshot`
  again, rebuild the request with the new `baseToken` and ordinals, and retry
  once. If you are intentionally reusing older refs after a token-only refresh,
  include `contentHash` as the optional hash guard.
- Missing or invalid `ref`: use an ordinal from the current snapshot; include
  `contentHash` only when you want the optional hash guard.
- `ANCHOR_NOT_FOUND`: the `quote` is not a substring of the block. Copy it
  verbatim from `/snapshot`.
- `AMBIGUOUS_ANCHOR`: the `quote` occurs more than once in the block. Extend it
  with adjacent text until it is unique.
- Unsupported operation: check `/.well-known/agent.json` for current
  `edit_operations` and `ops_operations`.
- Stream lag or pending events: refresh `/snapshot` before continuing.
- Failed dry run: fix the request before committing the same operation.

If a retryable write still fails after one fresh snapshot, stop and report the
raw error to the user instead of guessing.

## Discovery And Schemas

Use discovery when you need current route metadata or schemas:

```sh
curl -sS "$ORIGIN/.well-known/agent.json"
curl -sS "$ORIGIN/v1/openapi.json"
curl -sS "$ORIGIN/quarry.SKILL.md"
```

Discovery includes route hints, auth mode, supported presence statuses,
supported edit operations, supported review operations, and known limitations.

## Known Limitations

- REST agent endpoints currently trust localhost and do not enforce bearer-token
  auth.
- Invite URL tokens are document locators for browser/collab joins, not REST
  auth tokens.
- Direct block edits operate on whole Markdown blocks, not arbitrary character
  ranges.
- Inserted or replacement content for `/edit` must be one Markdown block.
- Quarry does not currently support Proof-only operations such as
  `rewrite.apply`.

## When Quarry Looks Wrong

If a read, write, event, or browser-visible state looks wrong, collect raw
evidence before summarizing:

- Exact request URL, method, status, and response body
- Library, document path, and agent id
- `baseToken` and block `ref` values used
- Event id, `nextAfter`, and `origin_id` if relevant
- Whether a fresh `/snapshot` and one safe retry changed the outcome
- Any visible mismatch between REST responses and the open browser document

Then report the evidence to the user. Do not keep retrying destructive writes.

## Safety Rules

- Register presence before reading, commenting, suggesting, or editing.
- Do not edit until the user gives explicit instructions.
- Prefer comments and suggestions for review requests.
- Use direct edits for implementation requests.
- Refresh the snapshot after any event and after any stale write.
- Include a readable `by` value so the user can see who acted.
- Fetch `/.well-known/agent.json` and `/v1/openapi.json` when you need current
  route metadata or schemas.
