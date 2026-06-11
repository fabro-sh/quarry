# Quarry

Quarry is a local-first document substrate for agents and developer tools. This repository implements the Rust workspace from `spec.md`: a single `quarry` binary, Turso-backed document storage, content-addressed large blobs, versioned document heads, explicit multi-document transactions, an Axum REST API with OpenAPI JSON, Git import/export, CLI operations, and a browser workspace with session-scoped live collaboration.

## Quickstart

```sh
cargo run -p quarry -- init .quarry
printf 'hello\n' > /tmp/hello.md
cargo run -p quarry -- put notes notes/hello.md /tmp/hello.md
cargo run -p quarry -- get notes notes/hello.md
cargo run -p quarry -- serve --addr 127.0.0.1:7831
```

The server binds to `127.0.0.1` by default. Non-loopback binds print a warning because phase one intentionally has no auth.

Operational notes are in [docs/operations/install-linux.md](docs/operations/install-linux.md), [docs/operations/fuse.md](docs/operations/fuse.md), [docs/operations/git-sync.md](docs/operations/git-sync.md), [docs/operations/conflicts.md](docs/operations/conflicts.md), and [docs/operations/backup-restore.md](docs/operations/backup-restore.md).

## Collaboration Architecture

Markdown documents (`BlockDocument`s) are canonical as relational block rows in SQL — a plain block tree with stable `block_id`s, inline mark/link ranges, and row-anchored review items (comments, suggestions, merge conflicts). There is no durable CRDT state:

- **Ephemeral sessions.** A Yjs document exists only while browsers are connected: seeded from rows on the first websocket subscriber, checkpointed back to rows on a debounce, discarded when the last browser leaves. The checkpoint is the only durable effect of typing; the browser save state reduces to `Saved` / `Saving…` / `Reconnecting (read-only)` driven by checkpoint-ack frames.
- **Semantic mutation gateway.** `POST /v1/libraries/{library}/documents/{path}/transactions` is the single public mutation contract (envelope `{client_tx_id, base_clock?, actor, ops[]}` with typed retryable errors and per-document idempotency). A per-document mode switch routes each transaction either directly to rows (no session) or into the live session as another collaborator (session active); acks force a checkpoint first, so an ack always means durable rows.
- **diff3 reconciliation.** Whole-file Markdown writes (Git sync, FUSE flushes, CLI puts, REST `PUT`, version restores) merge via diff3 against a stored shadow base and dispatch through the gateway. Identity maps positionally — sibling `block_id`s and review anchors survive external edits — and true conflicts become `kind = conflict` review items surfaced by `GET .../review`, never write failures.
- **RawDocuments** (everything that is not Markdown) keep the untouched byte path.

Agent-facing API documentation lives in `crates/quarry-server/resources/agent-docs.md` (served at `/agent-docs`) and `quarry.SKILL.md`.

## Workspace

- `crates/quarry-core`: domain types, errors, metadata, path normalization.
- `crates/quarry-cas`: BLAKE3 disk CAS with atomic writes and reachability GC.
- `crates/quarry-storage`: Turso schema, migrations, transaction wrapper, libraries, documents, versions, conflicts, GC; canonical block rows, review-item rows, and diff3 shadow bases (`src/blocks.rs`).
- `crates/quarry-collab-codec`: Markdown ↔ block rows codec, rows ↔ Yjs session projections (seed/checkpoint), and the diff3 `reconcile` engine.
- `crates/quarry-git`: Git working tree import/export/sync with marker safety, frontmatter, sidecars, commits, optional remote transport, and per-peer shadow-base bookkeeping for reconciled Markdown sync.
- `crates/quarry-server`: Axum REST API and generated OpenAPI; the semantic mutation gateway (`gateway.rs`), ephemeral session lifecycle (`session.rs`), collab websocket transport (`collab.rs`), and the shared whole-file reconciled writer (`markdown_write.rs`).
- `crates/quarry-cli`: CLI command parsing and local UX (Markdown puts reconcile through the same writer).
- `crates/quarry-fuse`: Linux-only FUSE projection over committed Library state with read-only and auto-commit writable modes; Markdown writes reconcile per open handle.
- `crates/quarry`: binary crate.
- `ui/`: the browser workspace (React + Plate + slate-yjs) — live sessions over the collab websocket, the rows-backed review rail, and the Playwright suites.

## Verification

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo check -p quarry-fuse --target x86_64-unknown-linux-gnu
cd ui && bun run fixtures:check
cd ui && bun run typecheck
cd ui && bun run test
cd ui && bun run test:e2e        # mock-API Playwright suite
cd ui && bun run test:e2e:live   # real-server live-collaboration suite
```

If the Slate/Yjs compatibility fixture check reports stale fixtures, regenerate them with
`cd ui && bun run fixtures:generate`, then rerun `cd ui && bun run fixtures:check`.

Current tests cover storage/CAS lifecycle, concurrent auto-commit writes, explicit transaction commit/rollback behavior, commit-time stale-head rejection, restart safety for open staged CAS writes, REST ETag/precondition/busy handling, OpenAPI exposure, and library scoping; block-model coverage includes rows ↔ session round-trip exactness with review anchors (the Gate A property tests), the gateway's per-op REST matrix with typed errors/idempotency/rebase acks, session lifecycle races (transaction vs seed/checkpoint/discard, checkpoint-before-ack), diff3 reconciliation hunk taxonomy and conflict-as-review-item persistence, and adapter round-trips proving sibling `block_id`s and live anchors survive Git/FUSE/CLI/PUT whole-file writes while RawDocument bytes bypass the block model exactly. Git sync coverage includes marker and reserved-sidecar safety, import rollback, one-sided changes/conflicts/deletes, large-delete safety, and local bare-remote transport. FUSE coverage includes invalidation events, stable inodes, handle-scoped truncate/write publication, and persisted directory metadata. The browser suites pin the save-state model, reconnect-reseed behavior, multi-browser convergence with a live agent collaborator, and the review rail against the rows projection.

## Known Limitations

Intentional boundaries of the current architecture (see `docs/superpowers/specs/2026-06-09-session-scoped-collab-design.md` for revisit triggers):

- **Online-only browsers.** A disconnected browser is read-only until it reconnects and reseeds; there is no offline editing and no local draft persistence.
- **Checkpoint-window crash loss.** A server crash loses un-checkpointed session edits (the debounce window). There is no session WAL; sessions reseed from the last checkpointed rows.
- **Single-server sessions.** One server owns the database and all live sessions; there is no multi-server session handoff.
- **Hunk-level external merges.** Whole-file writes (Git/FUSE/CLI/PUT) merge at diff3 hunk granularity, not character level: concurrent edits to the same region become conflict review items instead of fine-grained merges.
- **Unauthenticated collab websocket.** `/v1/collab/{document_id}` trusts loopback like the rest of the phase-one REST surface; invite tokens are locators, not auth.
- **Persistent checkpoint-failure loss.** A checkpoint that cannot project or export (a doc shape the Markdown writer rejects) is skipped with a warning and retried on the next edit; if the failing shape persists until the last subscriber leaves, the final checkpoint fails too and every edit since the last successful checkpoint is lost with the discarded session — unbounded loss while the shape persists. Containment rules (unknown marks dropped, unrepresentable blocks degraded to `raw_markdown`) exist to keep every reachable shape projectable; see the "Known hazards" notes in `quarry-server/src/session.rs`.
- **Session commit-failure window.** A session-mode transaction mutates the live doc before committing; if the commit fails and the caller ignores the typed retryable error, the merged content still lands via the next checkpoint without the transaction's review-item side effects (see the HONEST WINDOW note in `quarry-server/src/gateway.rs`).
- **Staged-transaction commits bypass the block model.** The explicit multi-document transaction API (`begin`/`stage`/`commit`) publishes staged versions atomically across paths on the byte path: committing a staged Markdown write clears that document's block projection fail-closed (it re-materializes on next read with fresh `block_id`s, dropping row-anchored review items) and does not coordinate with live sessions. Routing staged commits through the per-document gateway would break their cross-document atomicity, so this stands as a recorded limitation.
- **Per-document Git import commits for Markdown.** `git import` commits Markdown files one document at a time through the reconciler; the staged all-or-nothing rollback covers raw files only.

Phase-one boundaries also still apply: single user, one owning Quarry process per database/CAS root, Linux-only FUSE, no auth, no hosted service, no full-text/vector search, no Git LFS protocol, and no POSIX-perfect filesystem guarantee. Longer production soak tests would still be useful before treating Quarry as a hardened service.
