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
quarry server --root .quarry init
printf 'hello\n' > /tmp/hello.md
quarry put notes notes/hello.md /tmp/hello.md
quarry get notes notes/hello.md
quarry server --root .quarry start --addr 127.0.0.1:7831
```

The server binds to `127.0.0.1` by default. Non-loopback binds print a warning because phase one intentionally has no auth.

### Trusted tmp-document creation addresses

Local servers do not trust forwarding headers and store no creation address by
default. A deployment behind CloudFront can opt into the CloudFront-generated
viewer address for anonymous tmp-document creation:

```sh
QUARRY_CLIENT_IP_SOURCE=cloudfront-viewer-address quarry server start
# equivalent: quarry server start --client-ip-source cloudfront-viewer-address
```

In this mode `POST /v1/tmp/documents` requires exactly one valid
`CloudFront-Viewer-Address` value in `IP:port` form. The server stores the
canonical IP without the source port and rejects creation if the trusted header
is missing or malformed. Do not enable this mode on a directly reachable server
or substitute `X-Forwarded-For`; the deployment must arrange for CloudFront to
generate the trusted header.

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

## Releases

Release versions, commits, and annotated tags are created by the private `quarry-dev` crate:

```sh
cargo dev release --dry-run --nightly
cargo dev release --bump patch
cargo dev release --bump minor
```

Stable releases must be cut locally from a clean, synchronized `main`. The nightly workflow runs the same command with a GitHub App token. Its `nightly` environment needs `FABRO_RELEASES_APP_CLIENT_ID` and `FABRO_RELEASES_APP_PRIVATE_KEY`, and the app installation needs repository Contents write access. Nightlies use the next stable line, for example `0.1.4-nightly.20260711` after `v0.1.3`; stable releases support bump selection only. Pushing the resulting `v*` tag starts `.github/workflows/release.yml`.

The release command runs the Rust and browser smoke before changing the repository. Use `--skip-tests` only after running those checks yourself. `--dry-run` does not fetch or mutate; fetch tags first when checking a local release plan. The final branch/tag push is atomic. If a failure leaves a local release commit and tag, inspect them and retry that atomic push rather than rerunning the release command. Never delete and reuse a release tag—cut the next version instead.
