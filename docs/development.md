# Development Guide

## Workspace layout

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

## Feature flags

The default build enables `tmp-documents` (scratch documents with capability-URL sharing plus the embedded browser workspace). The library document surface — path-addressed libraries, `put`/`get`/`list`/`move`/`delete`, explicit transactions, Git sync, FUSE mounts — is gated behind `lib-documents`:

```sh
cargo build --release -p quarry --features lib-documents
```

Library-surface quickstart (requires `lib-documents`):

```sh
quarry init .quarry
printf 'hello\n' > /tmp/hello.md
quarry put notes notes/hello.md /tmp/hello.md
quarry get notes notes/hello.md
quarry serve --addr 127.0.0.1:7831
```

The server binds to `127.0.0.1` by default. Non-loopback binds print a warning because phase one intentionally has no auth.

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
