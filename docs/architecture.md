# Architecture

Quarry is a local-first document substrate for agents and developer tools: a single `quarry` binary with Turso-backed document storage, content-addressed large blobs, versioned document heads, explicit multi-document transactions, an Axum REST API with OpenAPI JSON, Git import/export, CLI operations, and a browser workspace with session-scoped live collaboration. The full phase-one design lives in [`spec.md`](../spec.md) and [`spec-browser.md`](../spec-browser.md).

## Collaboration architecture

Markdown documents (`BlockDocument`s) are canonical as relational block rows in SQL — a plain block tree with stable `block_id`s, inline mark/link ranges, and row-anchored review items (comments, suggestions, merge conflicts). There is no durable CRDT state:

- **Ephemeral sessions.** A Yjs document exists only while browsers are connected: seeded from rows on the first websocket subscriber, checkpointed back to rows on a debounce, discarded when the last browser leaves. The checkpoint is the only durable effect of typing; the browser save state reduces to `Saved` / `Saving…` / `Reconnecting (read-only)` driven by checkpoint-ack frames.
- **Semantic mutation gateway.** `POST /v1/libraries/{library}/documents/{path}/transactions` is the single public mutation contract (envelope `{client_tx_id, base_clock?, actor, ops[]}` with typed retryable errors and per-document idempotency). A per-document mode switch routes each transaction either directly to rows (no session) or into the live session as another collaborator (session active); acks force a checkpoint first, so an ack always means durable rows.
- **diff3 reconciliation.** Whole-file Markdown writes (Git sync, FUSE flushes, CLI puts, REST `PUT`, version restores) merge via diff3 against a stored shadow base and dispatch through the gateway. Identity maps positionally — sibling `block_id`s and review anchors survive external edits — and true conflicts become `kind = conflict` review items surfaced by `GET .../review`, never write failures.
- **RawDocuments** (everything that is not Markdown) keep the untouched byte path.

Agent-facing API documentation lives in `crates/quarry-server/resources/agent-docs.md` (served at `/agent-docs`) and `crates/quarry-server/resources/quarry.SKILL.md`.

## Known limitations

Intentional boundaries of the current architecture (see `docs/superpowers/specs/2026-06-09-session-scoped-collab-design.md` for revisit triggers):

- **Online-only browsers.** A disconnected browser is read-only until it reconnects and reseeds; there is no offline editing and no local draft persistence.
- **Checkpoint-window crash loss.** A server crash loses un-checkpointed session edits (the debounce window). There is no session WAL; sessions reseed from the last checkpointed rows.
- **Single-server sessions.** One server owns the database and all live sessions; sessions cannot move between servers.
- **Hunk-level external merges.** Whole-file writes (Git/FUSE/CLI/PUT) merge at diff3 hunk granularity, not character level: concurrent edits to the same region become conflict review items instead of fine-grained merges.
- **Unauthenticated collab websocket.** `/v1/collab/{document_id}` trusts loopback like the rest of the phase-one REST surface; invite tokens are locators, not auth.
- **Persistent checkpoint-failure loss.** A checkpoint that cannot project or export (a doc shape the Markdown writer rejects) is skipped with a warning and retried on the next edit; if the failing shape persists until the last subscriber leaves, the final checkpoint fails too and every edit since the last successful checkpoint is lost with the discarded session — unbounded loss while the shape persists. Containment rules (unknown marks dropped, unrepresentable blocks degraded to `raw_markdown`) exist to keep every reachable shape projectable; see the "Known hazards" notes in `quarry-server/src/session.rs`.
- **Session commit-failure window.** A session-mode transaction mutates the live doc before committing; if the commit fails and the caller ignores the typed retryable error, the merged content still lands via the next checkpoint without the transaction's review-item side effects (see the HONEST WINDOW note in `quarry-server/src/gateway.rs`).
- **Staged-transaction commits bypass the block model.** The explicit multi-document transaction API (`begin`/`stage`/`commit`) publishes staged versions atomically across paths on the byte path: committing a staged Markdown write clears that document's block projection fail-closed (it re-materializes on next read with fresh `block_id`s, dropping row-anchored review items) and does not coordinate with live sessions. Routing staged commits through the per-document gateway would break their cross-document atomicity, so this stands as a recorded limitation.
- **Per-document Git import commits for Markdown.** `git import` commits Markdown files one document at a time through the reconciler; the staged all-or-nothing rollback covers raw files only.

Phase-one boundaries also still apply: single user, one owning Quarry process per database/CAS root, Linux-only FUSE, no auth, no hosted service, no full-text/vector search, no Git LFS protocol, and no POSIX-perfect filesystem guarantee. Longer production soak tests would still be useful before treating Quarry as a hardened service.
