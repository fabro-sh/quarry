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
- `Idempotency-Key: <unique-key>` is supported on `/edit` for direct block edits

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
  "baseToken": "W/\"version_123\"",
  "blocks": [
    {
      "ref": {
        "baseToken": "W/\"version_123\"",
        "ordinal": 0,
        "contentHash": "abc123"
      },
      "markdown": "# Title\n\n"
    }
  ]
}
```

Use the latest top-level `baseToken` and the exact `ref` from the current
snapshot for `/edit` and `/ops`.

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

Use `?dryRun=1` before non-trivial direct edits or review operations. Dry runs
validate refs and planned changes without committing them.

## How Block Refs Work

A block `ref` is a concurrency guard. It includes the block's `baseToken`,
position, and content hash. If the document changes, old refs may become stale.

When choosing a target:

- Pick the block whose `markdown` contains the text you want to edit or review.
- Copy the whole `ref` object from the current snapshot.
- If the target spans multiple blocks, use multiple operations.
- If a write reports `STALE_BASE`, discard old refs, fetch a fresh snapshot, and
  rebuild the operation from the new refs.

## Direct Block Edits

Direct block edit operations are `replace_block`, `insert_before`,
`insert_after`, and `delete_block`.

Dry run a replacement:

```sh
curl -sS -X POST "$DOC/edit?dryRun=1" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{
    "baseToken": "W/\"version_123\"",
    "operations": [
      {
        "op": "replace_block",
        "ref": {
          "baseToken": "W/\"version_123\"",
          "ordinal": 0,
          "contentHash": "abc123"
        },
        "block": { "markdown": "# Revised title\n\n" }
      }
    ]
  }'
```

Commit with an idempotency key:

```sh
curl -sS -X POST "$DOC/edit" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H "Idempotency-Key: edit-abc123-1" \
  -d @edit.json
```

Insert several blocks at one anchor with `blocks`:

```sh
curl -sS -X POST "$DOC/edit?dryRun=1" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{
    "baseToken": "W/\"version_123\"",
    "operations": [
      {
        "op": "insert_after",
        "ref": {
          "baseToken": "W/\"version_123\"",
          "ordinal": 0,
          "contentHash": "abc123"
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

## Comments And Suggestions

Use `/ops` for review feedback. Supported operations are `comment.add`,
`comment.reply`, `comment.delete`, `comment.resolve`, `suggestion.add`,
`suggestion.accept`, and `suggestion.reject`.

Add a comment:

```sh
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{
    "baseToken": "W/\"version_123\"",
    "op": "comment.add",
    "ref": {
      "baseToken": "W/\"version_123\"",
      "ordinal": 0,
      "contentHash": "abc123"
    },
    "quote": "Title",
    "body": "Consider making this title more specific.",
    "by": "Codex"
  }'
```

Suggest a replacement:

```sh
curl -sS -X POST "$DOC/ops?dryRun=1" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{
    "baseToken": "W/\"version_123\"",
    "op": "suggestion.add",
    "kind": "replace",
    "ref": {
      "baseToken": "W/\"version_123\"",
      "ordinal": 0,
      "contentHash": "abc123"
    },
    "quote": "Title",
    "content": "Project Plan",
    "by": "Codex"
  }'
```

Suggestion kinds are `insert`, `delete`, `remove`, `replace`, and
`substitution`. `insert`, `replace`, and `substitution` require `content`.
`delete` and `remove` use the quoted text or block anchor.

Reply to or delete comments:

```sh
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"W/\"version_123\"","op":"comment.reply","parentId":"c_123","body":"Thanks, I will adjust this.","by":"Codex"}'

curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"W/\"version_123\"","op":"comment.delete","id":"c_123"}'
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
  -d '{"baseToken":"W/\"version_123\"","op":"comment.resolve","id":"c_123"}'

curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"W/\"version_123\"","op":"suggestion.accept","id":"s_123"}'
```

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
such as `version_id`/`etag` and may include `collab_session_id`. When live
`/ops` mutations are injected into an open browser editor, `collab_session_id`
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
  again, rebuild the request with the new `baseToken` and block refs, and retry
  once.
- Missing or invalid `ref`: use the exact block ref from the current snapshot.
  Do not invent refs or reuse refs from a previous version.
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
- Event id, `nextAfter`, and `collab_session_id` if relevant
- Whether a fresh `/snapshot` and one safe retry changed the outcome
- Any visible mismatch between REST responses and the open browser document

Then report the evidence to the user. Do not keep retrying destructive writes.

## Safety Rules

- Register presence before reading, commenting, suggesting, or editing.
- Do not edit until the user gives explicit instructions.
- Prefer comments and suggestions for review requests.
- Use direct edits for implementation requests.
- Refresh the snapshot after any event and after any stale write.
- Use `?dryRun=1` before risky edits or suggestions.
- Include a readable `by` value so the user can see who acted.
- Fetch `/.well-known/agent.json` and `/v1/openapi.json` when you need current
  route metadata or schemas.
