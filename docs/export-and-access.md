# Export and Access

V1 access is through the embedded web UI, REST/OpenAPI, MCP-shaped resources, the CLI, Git materialization, and raw export. Mounted filesystem views are deferred.

Implemented in the first spine:

- `quarry server` serves the web UI and REST API.
- `GET /stats` exposes workspace diagnostics.
- `GET /refs` and `GET /tree/{ref}` expose refs and tree state.
- `GET /tree/{ref}/{path}` reads a tree entry and returns text content for blob-backed files and structured documents.
- `PUT /tree/{ref}/{path}` writes text and auto-commits a transaction.
- `DELETE /tree/{ref}/{path}` deletes a path, with agent guardrails on published refs.
- `GET /events` and `GET /events/ws` expose the append-only event stream for memory systems and live UI updates.
- `GET /refs/{ref}/snapshots` and `POST /refs/{ref}/restore` expose ref version history and restore.
- `POST /documents`, `GET /documents/{id}/state`, `POST /documents/{id}/ops`, `POST /documents/{id}/presence`, and `GET /documents/{id}/ws` expose structured document snapshots, ops, presence, and local WebSocket collaboration.
- `POST /drafts` creates a draft ref from a base ref.
- `POST /drafts/publish` publishes one ref into another.
- `POST /annotations` stores first-class annotation objects.
- `POST /binary-objects` stores an opaque binary pointer and attaches it to a ref path.
- `GET /binary-objects/{id}/content` streams local original bytes when the pointer has a local path; it does not extract or inspect binary contents.
- `POST /git/materialize` materializes a ref into a normal Git repo and commit, or performs a raw snapshot export.
- `POST /git/ingest` ingests a Git/worktree directory. Conflicting files create a conflict draft with local and incoming versions preserved.
- `POST /mcp` exposes MCP JSON-RPC `initialize`, `tools/list`, `tools/call`, `resources/list`, and `resources/read`.
- `POST /mcp/tools/{tool}` keeps the direct HTTP compatibility path for `quarry_status`, `quarry_list`, `quarry_read`, `quarry_write`, `quarry_comment`, `quarry_start_draft`, `quarry_publish_draft`, `quarry_events`, `quarry_snapshots`, `quarry_restore`, `quarry_create_document`, `quarry_document_state`, `quarry_document_op`, and `quarry_git_sync`.

Deferred:

- FUSE
- sync-folder projection
- SMB/Samba
- OCR or text extraction from binary/opaque assets
- built-in search endpoints
