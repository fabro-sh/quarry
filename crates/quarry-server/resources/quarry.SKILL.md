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

Every review or edit write should include a readable `by` value. Presence uses
`X-Agent-Id: ai:<agent-name>` or another stable session id.

## Default Behavior

If the user shares a Quarry locator URL:

- Join immediately.
- Register presence before reading.
- Read the current `/snapshot` before editing or reviewing.
- Reply with the required ready message.
- Work in the Quarry document unless the user asks otherwise.
- Do not edit until the user gives an edit/review instruction.

Use `/ops` for comments and suggestions. Use `/edit` only when the user asks you
to directly change document content.

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
2. Read `GET $DOC/snapshot`.
3. Reply exactly:

```text
Connected in Quarry and ready.
<one-sentence summary of the document>
I can edit directly, or leave comments and suggestions for you to review. What would you like me to do?
```

4. Wait for the user's instruction.
5. Before each write, use the latest `baseToken` and exact block `ref` values.
6. After events or stale writes, refresh `baseToken` (re-read `/snapshot`, or
   `HEAD $DOC` for just the token) and rebuild the request.

## Presence

```bash
curl -sS -X POST "$DOC/presence" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"status":"reading","by":"Codex"}'
```

Statuses: `reading`, `thinking`, `acting`, `waiting`, `completed`, `error`.

## Snapshot And Block Refs

Quarry writes are block-scoped. Read `/snapshot`, copy the top-level
`baseToken`, and target blocks by their `ref.ordinal`. `contentHash` is
optional in write requests: omit it when writing immediately after reading the
same `/snapshot`, and include it when you refresh only the token via `HEAD` but
reuse older refs and want shifted or edited blocks to be caught.

```bash
curl -sS "$DOC/snapshot"
```

Important response fields:

```json
{
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

Choose the block whose `markdown` contains the text you want to edit or review.
If the target spans multiple blocks, use multiple operations. On `/ops`, include
`quote` when anchoring to specific text within the chosen block; `quote`
complements, but does not replace, the optional shifted-block guard from
`contentHash`.

`baseToken` is opaque. Copy the raw value from `/snapshot` verbatim into
requests. Write endpoints also accept ETag-shaped values copied from HTTP
`ETag` headers, such as `"version_123"` or `W/"version_123"`.

To refresh only the token after a write — without re-downloading the document —
read the `ETag` header from a `HEAD` request:

```bash
curl -sS -I "$DOC" | tr -d '\r' | sed -n 's/^[Ee][Tt][Aa][Gg]: //p'
```

The `ETag` value is also accepted as a write `baseToken`, while `/snapshot`
returns the easier raw form. Re-read the full `/snapshot` when you also need
fresh ordinals or want fresh `contentHash` guards — for example after editing
the same block you are about to write to again.

## Direct Edits

Supported `/edit` operations: `replace_block`, `insert_before`,
`insert_after`, `delete_block`, `replace_document`.

Replace a block with an idempotency key:

```bash
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

```bash
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

Each inserted or replacement block must be one Markdown block. When inserting
multiple blocks at one ref, use `blocks` on one `insert_before` or
`insert_after` operation instead of repeated `insert_after` calls on the same ref.

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

Supported `/ops` operations: `comment.add`, `comment.reply`,
`comment.delete`, `comment.resolve`, `suggestion.add`,
`suggestion.accept`, `suggestion.reject`.

`/ops` takes a batch of one or more operations. All operations share one
top-level `baseToken` and `by` author, resolve refs against the original
snapshot, and commit atomically. Put several comments or suggestions in one
`operations` array instead of refreshing the token between annotations. Use an
`Idempotency-Key` header when committing a non-dry-run batch.

Read existing review work without parsing CriticMarkup:

```bash
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

```bash
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

```bash
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

The `quote` field anchors a comment or suggestion to a span inside the block.
It is optional and must match the block's `markdown` exactly:

- Omit `quote` to anchor the whole block.
- When present, `quote` is an exact substring — case-, whitespace-, and
  punctuation-sensitive — that must occur **exactly once** in the block.
- Zero matches fail with `ANCHOR_NOT_FOUND`; two or more fail with
  `AMBIGUOUS_ANCHOR`. To disambiguate, extend the quote with adjacent text
  until it is unique rather than shortening it.
- An empty string is rejected. Omit the field instead of sending `""`.

Because `/ops` commits atomically, one bad `quote` fails the entire batch, so
copy quotes verbatim from the current `/snapshot`.

Reply, resolve, or accept:

```bash
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"version_123","by":"Codex","operations":[{"op":"comment.reply","parentId":"c_123","body":"Thanks, I will adjust this."}]}'

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

### Leaving Several Annotations

If you refresh the token from `HEAD`, build the batch body with `jq -n` so the
header value is encoded as JSON:

```bash
BT=$(curl -sS -I "$DOC" | tr -d '\r' | sed -n 's/^[Ee][Tt][Aa][Gg]: //p')
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d "$(jq -n --arg bt "$BT" \
    '{baseToken:$bt, by:"Codex", operations:[
      {op:"comment.add",
       ref:{ordinal:0},
       quote:"Title", body:"Make this more specific."},
      {op:"suggestion.add", kind:"replace",
       ref:{ordinal:3},
       quote:"16 GB is workable",
       content:"16 GB is a reasonable starting point"}
    ]}')"
```

When you include `ref.contentHash`, it must match the current `/snapshot` for
that block. Omit it for the normal read-snapshot-then-write flow. Include a
cached hash only when you intentionally reuse older refs after refreshing just
the token and want the server to reject shifted or edited blocks.

## Events

Events are activity signals, not document content. Re-read `/snapshot` after an
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

| Error | Action |
|---|---|
| `STALE_BASE` | The document advanced. Refresh `baseToken` (`HEAD $DOC`, or `/snapshot` if you also need fresh ordinals or hash guards), rebuild, retry once |
| Missing or invalid `ref` | Use an ordinal from the current snapshot; include `contentHash` only when you want the optional hash guard |
| `ANCHOR_NOT_FOUND` | The `quote` is not a substring of the block. Copy it verbatim from `/snapshot` |
| `AMBIGUOUS_ANCHOR` | The `quote` occurs more than once in the block. Extend it with adjacent text until unique |
| Unsupported operation | Check `/.well-known/agent.json` for supported operations |
| Failed dry run | Fix the request before committing |
| Stream lag or pending events | Re-read `/snapshot` before acting |

If a retryable write still fails after one fresh snapshot, stop and report the
raw error instead of guessing.

## When Quarry Looks Wrong

Collect raw evidence before summarizing:

- request URL, method, status, and response body
- library, document path, and agent id
- `baseToken`, ordinals, and any `contentHash` guards used
- event id, `nextAfter`, and `origin_id` if relevant
- whether a fresh `/snapshot` and one safe retry changed the outcome
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
