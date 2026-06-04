---
name: quarry
description: Use when connecting an agent to a local Quarry collaborative Markdown document through a locator URL or localhost REST API.
allowed-tools:
  - Bash
  - WebFetch
---

# Quarry

Quarry is a local-first collaborative Markdown editor with presence, comments, suggestions, document activity events, and block edit APIs.

## Start Here

When given a Quarry locator URL, use it to identify the shared document for the browser/collab join. REST agent APIs on the same localhost origin are trusted-localhost for now; the locator token is not REST bearer auth.

1. Register presence before reading or editing.
   - Choose an id like `ai:codex:<short-id>` or `ai:claude:<short-id>`.
   - `POST /v1/libraries/{library}/documents/{path}/presence`
   - Headers: `Content-Type: application/json`, `X-Agent-Id: <agent-id>`
   - Body: `{"status":"reading","by":"<agent name>"}`
2. Read the latest block snapshot.
   - Prefer `GET /v1/libraries/{library}/documents/{path}/snapshot`.
   - Fallback to `GET /v1/libraries/{library}/documents/{path}` only if snapshot is unavailable.
3. After reading, reply exactly in this shape:

```text
Connected in Quarry and ready.
<one-sentence summary of the document>
I can edit directly, or leave comments and suggestions for you to review. What would you like me to do?
```

4. Do not edit until the user gives further instructions.
5. While working, monitor activity with `GET /v1/libraries/{library}/documents/{path}/events/stream`, or poll `GET /v1/libraries/{library}/events/pending?after=<last-seen-id>`.
6. Refresh the snapshot after any activity or stale edit before replying, commenting, suggesting, or editing.

## Agent APIs

- Presence: `POST` and `GET /v1/libraries/{library}/documents/{path}/presence`
- Snapshot: `GET /v1/libraries/{library}/documents/{path}/snapshot`
- Events: `GET /v1/libraries/{library}/documents/{path}/events/stream`, `GET /v1/libraries/{library}/events/pending`, `POST /v1/libraries/{library}/events/ack`
- Direct block edits: `POST /v1/libraries/{library}/documents/{path}/edit`
- Comments and suggestions: `POST /v1/libraries/{library}/documents/{path}/ops`
- OpenAPI: `GET /v1/openapi.json`

Encode document paths one segment at a time. For example, `notes/Project Plan.md` becomes `notes/Project%20Plan.md`.

## Safe Workflow

- Use the latest `baseToken` and block `ref` values from `/snapshot`.
- Use `?dryRun=1` before risky direct edits or review operations.
- Use comments and suggestions for review feedback; use direct block edits only when the user asks you to change the document.
- If an edit returns `STALE_BASE`, refresh the snapshot and retry against the new `baseToken`.
- Include a human-readable `by` value for presence, comments, and suggestions.
- Do not assume Proof-only operations exist in Quarry. Quarry does not currently support `rewrite.apply`, `comment.reply`, or REST bearer-token enforcement.

For deeper setup details, fetch `/agent-docs`, `/.well-known/agent.json`, and `/v1/openapi.json` from the Quarry origin.
