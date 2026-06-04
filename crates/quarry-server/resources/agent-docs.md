# Quarry Agent Docs

Quarry exposes local REST APIs for agents on `/v1`. Use these docs when an agent has a Quarry locator URL, library slug, and document path and needs to read, review, or edit a collaborative Markdown document.

## Quickstart

Set these placeholders first:

```sh
ORIGIN="http://127.0.0.1:7831"
LIBRARY="main"
PATH_ENCODED="notes/Project%20Plan.md"
AGENT_ID="ai:codex:abc123"
AGENT_NAME="Codex"
DOC="$ORIGIN/v1/libraries/$LIBRARY/documents/$PATH_ENCODED"
```

For nested document paths, encode each path segment and keep `/` separators. For example, `notes/Project Plan.md` becomes `notes/Project%20Plan.md`.

1. Register presence.
2. Read `/snapshot`.
3. Reply to the user with the required ready message.
4. Watch events while working.
5. Edit only after the user asks.

## Auth And Locator Tokens

Quarry REST agent APIs are trusted-localhost for now. Browser invite URL tokens identify the shared document for browser/collab joins, but REST agent endpoints on this host do not currently enforce bearer-token auth.

Use the locator URL to find the Quarry origin, library, document path, and invite context. Do not send the locator token as a REST bearer token unless future discovery metadata explicitly says to do so.

## Headers And Identity

Use a stable agent id for the session, such as `ai:codex:<short-id>` or `ai:claude:<short-id>`.

- `Content-Type: application/json`
- `X-Agent-Id: <agent-id>` for presence and event ack identity
- `Idempotency-Key: <unique-key>` is supported on `/edit` for direct block edits

## Presence

Register or update presence before reading:

```sh
curl -sS -X POST "$DOC/presence" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"status":"reading","by":"Codex"}'
```

Statuses: `reading`, `thinking`, `acting`, `waiting`, `completed`, `error`.

List presence for the same document:

```sh
curl -sS "$DOC/presence"
```

## Snapshot Reads

Prefer a block snapshot before editing or reviewing:

```sh
curl -sS "$DOC/snapshot"
```

Snapshot responses contain `documentId`, `baseToken`, and `blocks`. Each block has a `ref` and Markdown content:

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

Use the latest `baseToken` and exact block `ref` values for `/edit` and `/ops`.

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

## Events

Prefer the document event stream:

```sh
curl -N "$DOC/events/stream"
```

If a stream is not practical, poll pending events:

```sh
curl -sS "$ORIGIN/v1/libraries/$LIBRARY/events/pending?after=0"
```

The poll response contains `events` and `nextAfter`. Store `nextAfter` and pass it as `after` on the next poll.

Ack processed events when useful:

```sh
curl -sS -X POST "$ORIGIN/v1/libraries/$LIBRARY/events/ack" \
  -H "Content-Type: application/json" \
  -H "X-Agent-Id: $AGENT_ID" \
  -d '{"eventId": 42}'
```

Refresh the snapshot after activity arrives and before replying, commenting, suggesting, or editing.

## Block Edits

Use direct block edits only after the user asks you to edit. Operations are `replace_block`, `insert_before`, `insert_after`, and `delete_block`.

Dry run first for non-trivial edits:

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

Each inserted or replacement block must be a single Markdown block. If a response reports `STALE_BASE`, fetch a new snapshot and retry with the new `baseToken` and block refs.

## Comments And Suggestions

Use `/ops` for review feedback. It supports `comment.add`, `suggestion.add`, `suggestion.accept`, `suggestion.reject`, and `comment.resolve`.

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

Suggestion kinds are `insert`, `delete`, `remove`, `replace`, and `substitution`. `replace` and `substitution` require `content`. `insert` requires `content`. `delete` and `remove` use the quoted text or block anchor.

Resolve or decide review items:

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

## Error Handling

- `STALE_BASE`: the document changed since your snapshot. Fetch `/snapshot` again and retry against the new `baseToken`.
- Missing or invalid `ref`: use the exact block ref from the current snapshot.
- Unsupported operation: check `/.well-known/agent.json` for current `edit_operations` and `ops_operations`.
- Stream lag or pending events: refresh the snapshot before continuing.

## Known Limitations

- REST agent endpoints currently trust localhost and do not enforce bearer-token auth.
- Invite URL tokens are document locators for browser/collab joins, not REST auth tokens.
- Quarry does not currently support Proof operations such as `rewrite.apply` or `comment.reply`.
- Direct block edits operate on whole Markdown blocks, not arbitrary character ranges.
- Inserted or replacement content for `/edit` must be one Markdown block.

## Safety Rules

- Register presence before reading, commenting, suggesting, or editing.
- Do not edit until the user gives explicit instructions.
- Prefer comments and suggestions for review requests.
- Use direct edits for implementation requests.
- Refresh the snapshot after any event and after any stale write.
- Use `?dryRun=1` before risky edits or suggestions.
- Include a readable `by` value so the user can see who acted.
- Fetch `/.well-known/agent.json` and `/v1/openapi.json` when you need current route metadata or schemas.
