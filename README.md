# Quarry

Quarry is a local-first collaborative storage substrate for humans and AI agents. This repo currently contains a working local v1 from `spec.md`: a single `quarry` CLI, a local SQLite/WAL store, content-addressed blobs, refs, draft/publish flows, transactions, annotations, events, ref snapshots/restore, structured document snapshots/ops/presence, opaque binary pointer records, Git materialize/ingest, MCP tool and JSON-RPC routes, WebSocket event/document channels, an embedded Potion-style web UI, and a REST API.

The v1 spine intentionally does not implement FUSE, sync-folder mounts, OCR, binary text extraction, auth, or built-in search.

## Quick Start

```sh
cargo run -p quarry-cli -- init
cargo run -p quarry-cli -- write notes/hello.md --content '# Hello from Quarry'
cargo run -p quarry-cli -- read notes/hello.md
cargo run -p quarry-cli -- server --addr 127.0.0.1:7831
```

Then open `http://127.0.0.1:7831`.

Useful endpoints:

- `GET /stats`
- `GET /refs`
- `GET /tree/{ref}`
- `GET /tree/{ref}/{path}`
- `PUT /tree/{ref}/{path}`
- `DELETE /tree/{ref}/{path}`
- `GET /events`
- `GET /events/ws`
- `GET /refs/{ref}/snapshots`
- `POST /refs/{ref}/restore`
- `POST /documents`
- `GET /documents/{id}/state`
- `GET /documents/{id}/snapshot`
- `GET /documents/{id}/snapshots`
- `POST /documents/{id}/ops`
- `POST /documents/{id}/presence`
- `GET /documents/{id}/events`
- `GET /documents/{id}/ws`
- `POST /drafts`
- `POST /drafts/publish`
- `GET /binary-objects`
- `POST /binary-objects`
- `GET /binary-objects/{id}/content`
- `GET /annotations`
- `POST /annotations`
- `POST /git/materialize`
- `POST /git/ingest`
- `POST /mcp`
- `POST /mcp/tools/{tool}`
- `GET /openapi.json`

Use URL encoding for refs with slashes, for example `published%2Fmain`.

The TypeScript client lives in `sdk/quarry-sdk-ts`.

## CLI

```sh
quarry init
quarry doctor
quarry status
quarry refs
quarry write docs/example.md --content 'hello' --actor-id codex --actor-kind agent
quarry read docs/example.md
quarry draft start --name draft/review
quarry write docs/example.md --ref draft/review --content 'hello from a draft'
quarry comment --target ref:published/main:path:docs/example.md --body 'revise this'
quarry annotations --target ref:published/main:path:docs/example.md
quarry events --limit 20
quarry snapshots --ref published/main
quarry restore <snapshot-id> --ref published/main
quarry document create docs/rich.md --text 'hello'
quarry document op <document-id> --op-json '{"kind":"replace_text","text":"updated"}'
quarry document presence <document-id> --cursor-json '{"path":[0,0],"offset":3}'
quarry document state <document-id>
quarry draft publish draft/review --target published/main
quarry binary add assets/spec.pdf --hash abc123 --size 42 --media-type application/pdf
quarry export /tmp/quarry-export --ref published/main
quarry git materialize /tmp/quarry-git --ref published/main --branch main
quarry git ingest /tmp/quarry-git --ref published/main --actor-id git --actor-kind git-import
quarry sync /tmp/quarry-git --ref published/main --branch main
quarry compact
quarry inspect transaction <id>
quarry server
```

By default the local data directory is `.quarry`. Override it with `--data-dir` or `QUARRY_DATA_DIR`.

## Validation

Run the automated suite:

```sh
cargo test --workspace
```

Run the local v1 demo:

```sh
scripts/quarry_v1_demo.sh
```
