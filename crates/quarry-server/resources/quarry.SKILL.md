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
6. After events or stale writes, re-read `/snapshot` and rebuild the request.

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
`baseToken`, and copy the exact `ref` object for the target block. Never invent
or reuse old refs.

```bash
curl -sS "$DOC/snapshot"
```

Important response fields:

```json
{
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

Choose the block whose `markdown` contains the text you want to edit or review.
If the target spans multiple blocks, use multiple operations.

## Direct Edits

Supported `/edit` operations: `replace_block`, `insert_before`,
`insert_after`, `delete_block`.

Dry run non-trivial direct edits:

```bash
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

```bash
curl -sS -X POST "$DOC/edit" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -H "Idempotency-Key: edit-abc123-1" \
  -d @edit.json
```

Insert several blocks at one anchor with `blocks`:

```bash
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

Each inserted or replacement block must be one Markdown block. When inserting
multiple blocks at one ref, use `blocks` on one `insert_before` or
`insert_after` operation instead of repeated `insert_after` calls on the same ref.

## Comments And Suggestions

Supported `/ops` operations: `comment.add`, `comment.reply`,
`comment.delete`, `comment.resolve`, `suggestion.add`,
`suggestion.accept`, `suggestion.reject`.

Add a comment:

```bash
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

```bash
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

Suggestion kinds: `insert`, `delete`, `remove`, `replace`, `substitution`.
`insert`, `replace`, and `substitution` require `content`.

Reply, resolve, or accept:

```bash
curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"W/\"version_123\"","op":"comment.reply","parentId":"c_123","body":"Thanks, I will adjust this.","by":"Codex"}'

curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"W/\"version_123\"","op":"comment.resolve","id":"c_123"}'

curl -sS -X POST "$DOC/ops" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"baseToken":"W/\"version_123\"","op":"suggestion.accept","id":"s_123"}'
```

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
| `STALE_BASE` | Re-read `/snapshot`, rebuild with fresh `baseToken` and refs, retry once |
| Missing or invalid `ref` | Use the exact ref from the current snapshot |
| Unsupported operation | Check `/.well-known/agent.json` for supported operations |
| Failed dry run | Fix the request before committing |
| Stream lag or pending events | Re-read `/snapshot` before acting |

If a retryable write still fails after one fresh snapshot, stop and report the
raw error instead of guessing.

## When Quarry Looks Wrong

Collect raw evidence before summarizing:

- request URL, method, status, and response body
- library, document path, and agent id
- `baseToken` and block refs used
- event id, `nextAfter`, and `collab_session_id` if relevant
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
