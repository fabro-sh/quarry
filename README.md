# Quarry

Quarry is a local-first document substrate for agents and developer tools. This repository implements the phase-one Rust workspace from `spec.md`: a single `quarry` binary, Turso-backed document storage, content-addressed large blobs, versioned document heads, explicit multi-document transactions, an Axum REST API with OpenAPI JSON, Git import/export, and CLI operations.

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

## Workspace

- `crates/quarry-core`: domain types, errors, metadata, path normalization.
- `crates/quarry-cas`: BLAKE3 disk CAS with atomic writes and reachability GC.
- `crates/quarry-storage`: Turso schema, migrations, transaction wrapper, libraries, documents, versions, conflicts, GC.
- `crates/quarry-git`: Git working tree import/export/sync with marker safety, frontmatter, sidecars, commits, and optional remote transport.
- `crates/quarry-server`: Axum REST API and generated OpenAPI.
- `crates/quarry-cli`: CLI command parsing and local UX.
- `crates/quarry-fuse`: Linux-only FUSE projection over committed Library state with read-only and auto-commit writable modes.
- `crates/quarry`: binary crate.

## Verification

```sh
cargo test --workspace
cargo check --workspace
cargo check -p quarry-fuse --target x86_64-unknown-linux-gnu
```

Current tests cover storage/CAS lifecycle, concurrent auto-commit writes, explicit transaction commit/rollback behavior, commit-time stale-head rejection for long-running transactions, restart safety for open staged CAS writes, REST ETag/precondition/busy handling, bind-address parsing, OpenAPI exposure, REST library scoping for conflicts and transaction routes, Git import/export marker and reserved-sidecar safety, Git import rollback on failure, sync one-sided changes/conflicts/deletes, sync-state publication failure safety, conflict scoping in REST and CLI, case-distinct Git paths when the filesystem supports them, large-delete safety, local bare-remote fetch/push transport, and FUSE projection semantics including invalidation events, stable storage-backed inodes, handle-scoped truncate/write publication, duplicate close cleanup, and persisted directory metadata.

## Acceptance Notes

The phase-one shape is implemented and covered by the local suites above. A privileged Linux Docker smoke has exercised one process serving REST and FUSE together with standard tools (`ls`, `cat`, `find`, `rg`, Vim save, `cp -r`, `git init`, `git status`) and cross-surface REST/FUSE visibility.

Remaining limitations are intentional phase-one boundaries: single user, one owning Quarry process per database/CAS root, Linux-only FUSE, no auth, no hosted service, no CRDT collaboration, no full-text/vector search, no Git LFS protocol, and no POSIX-perfect filesystem guarantee. Longer production soak tests would still be useful before treating Quarry as a hardened service.
