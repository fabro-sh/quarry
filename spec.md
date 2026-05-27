# Quarry Phase One Spec
## Purpose
Quarry is a local-first document substrate for agents and developer tools. It gives agents a durable memory/workspace that can be accessed as structured documents over HTTP, as ordinary files through a Linux FUSE mount, and as a Git-synchronized repository for human review and portability.

Phase one delivers the smallest complete version of that substrate: a single-user, single-server daemon that stores versioned documents in TursoDB, supports multi-document transactions, exposes a REST API, syncs with Git without destructive conflicts, mounts libraries as a Linux filesystem, and ships as a usable CLI/binary.

Quarry is not the memory product itself in this phase. It is the lower storage layer that future search, MCP, UI, and agent-memory products can build on.
## Phase One Outcome
At the end of phase one, a user can:

- Create multiple Quarry Libraries on one Quarry server.
- Store all Libraries in one server-owned TursoDB database.
- Write, read, list, rename, delete, and version documents by path.
- Attach arbitrary JSON metadata to documents.
- Store small text documents inline and large/binary documents in content-addressed storage.
- Use auto-commit writes for simple operations.
- Use explicit transactions for atomic multi-document edits.
- Access Libraries through a local REST API with generated OpenAPI documentation.
- Override the default REST bind address explicitly when needed.
- Import from and export to a normal Git working tree.
- Run explicit bidirectional Git sync against one remote/branch while preserving both sides of conflicts.
- Mount a Library through Linux FUSE and use standard tools like `ls`, `cat`, `find`, `rg`, `vim`, and `cp`.
- Package and run Quarry as a local daemon/CLI without needing a hosted service.

## Design Center
- Single user.
- Single server process.
- One TursoDB database for all Libraries on that server.
- One running Quarry process owns the TursoDB database and CAS directory at a time.
- REST binds to `127.0.0.1` by default.
- REST bind address can be explicitly overridden by the user.
- Git sync is explicit, not ambient background sync.
- REST is the canonical control and transaction surface.
- FUSE is a Linux-only projection over committed state, not the source of truth.
- Git is an interoperability and version-control surface, not the storage engine.

## Non-Goals
These are intentionally out of scope for phase one:

- Hosted multi-user service.
- Auth, ACLs, sharing, teams, or remote tenancy.
- Web UI beyond reserving future routes.
- CRDT or real-time collaborative editing.
- Full-text search, vector search, embeddings, ranking, or memory extraction.
- Full MCP server product. A minimal skeleton/shim may ship during packaging, but production MCP tools are later work.
- Git LFS or git-annex protocol support.
- Multi-device automatic sync.
- Multiple Quarry daemons concurrently writing the same database.
- POSIX-perfect filesystem behavior.
- macOS FUSE support.
- Windows filesystem support.
- Symlinks and hard links in FUSE.
- Executing build artifacts directly from the FUSE mount as a supported workflow.

## Core Product Model
### Library
A Library is the logical isolation boundary for documents, transactions, sync state, conflicts, and mount state.

Phase one uses one physical TursoDB database for all Libraries on the server. Libraries are rows and foreign-key boundaries inside that database, not separate database files. This keeps the server simple, makes cross-library administration possible, and matches the desired server model.

The server also owns one CAS root. CAS content can be deduplicated across Libraries because content identity is global to the server.
### Document
A Document is addressed by a normalized, case-sensitive path-like key inside a Library.

Examples:

- `notes/project-plan.md`
- `agents/research/openai.json`
- `artifacts/screenshots/home.png`

The canonical storage model is:

- Stable document identity.
- Mutable document head.
- Immutable document versions.
- Immutable content blobs.
- JSON metadata snapshots per version.

This avoids in-place mutation as the durable history model. A write creates a new version and advances the document head atomically at commit time.
### Directories
Directories are derived from path prefixes, S3-style. Quarry does not make directories the canonical storage model in phase one.

To satisfy FUSE requirements, add a small `dir_metadata` sidecar table keyed by library and directory path for POSIX attributes and empty directory support. FUSE gets stable inodes from an allocated `inodes` table scoped by Library.
### Metadata
Document metadata is canonical JSON. Phase one indexes only known hot fields:

- `content_type`
- `created_at`
- `updated_at`

Everything else remains flexible JSON. EAV tables, typed schemas, and rich schema validation are deferred.
### Content Storage
Use hybrid inline/CAS storage:

- Store content <= 64 KiB inline in TursoDB.
- Store content > 64 KiB in CAS and keep only a content reference, byte size, and hash metadata in TursoDB.
- Decide inline vs CAS on every write based on the current content size.
- When content moves from inline to CAS or vice versa, old unreferenced blobs become eligible for GC.

Use BLAKE3 for content hashes. The phase-one CAS is a disk store with a Git-like fanout layout:

```text
cas/objects/ab/cdef0123...
```

Writes must be atomic: write to a temp file, flush, then rename into place.
## Storage Engine Decision
Phase one uses TursoDB directly via the `turso` Rust crate from `https://github.com/tursodatabase/turso`.

Do not use `libsql` or `rusqlite` as the phase-one backend. Do not make backend choice a user-facing configuration knob.

Keep the storage code behind an internal module boundary so Turso-specific details do not leak into REST, Git, FUSE, or CLI code. This boundary is for maintainability, not for shipping multiple database backends.

Required Turso behavior:

- All Libraries live in one TursoDB database.
- All write paths use explicit transaction handling.
- Write transactions use a consistent `BEGIN`/`COMMIT`/`ROLLBACK` wrapper.
- Busy/locked errors use bounded retry with backoff.
- The daemon is the only process allowed to write the database.
- Startup uses a lock file or equivalent guard to reject multiple Quarry daemons over the same database path.

The phase-zero Turso spike is still required, but it defines transaction wrappers, retry policy, and operational limits. It is not a backend selection gate.
## Data Model
Phase one includes these entities:

- `libraries`
- `documents`
- `document_versions`
- `transactions`
- `transaction_changes`
- `blobs`
- `sync_peers`
- `sync_state`
- `conflicts`
- `dir_metadata`
- `inodes`

Representative schema shape:

```sql
libraries(
  id TEXT PRIMARY KEY,
  slug TEXT UNIQUE NOT NULL,
  created_at TEXT NOT NULL,
  settings_json TEXT NOT NULL
);

documents(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  head_version_id TEXT,
  deleted_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(library_id, path)
);

document_versions(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  tx_id TEXT NOT NULL,
  content_hash TEXT,
  inline_content BLOB,
  metadata_json TEXT NOT NULL,
  content_type TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  created_at TEXT NOT NULL
);

transactions(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  state TEXT NOT NULL,
  actor TEXT,
  source TEXT NOT NULL,
  message TEXT,
  provenance_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  committed_at TEXT
);

transaction_changes(
  tx_id TEXT NOT NULL,
  path TEXT NOT NULL,
  change_type TEXT NOT NULL,
  old_version_id TEXT,
  new_version_id TEXT
);

blobs(
  hash TEXT PRIMARY KEY,
  hash_alg TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  storage_backend TEXT NOT NULL,
  created_at TEXT NOT NULL
);

sync_peers(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  config_json TEXT NOT NULL
);

sync_state(
  peer_id TEXT NOT NULL,
  path TEXT NOT NULL,
  last_synced_doc_version_id TEXT,
  last_synced_git_oid TEXT,
  PRIMARY KEY(peer_id, path)
);

conflicts(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  ours_version_id TEXT,
  theirs_version_id TEXT,
  status TEXT NOT NULL,
  discovered_at TEXT NOT NULL,
  resolved_at TEXT
);

dir_metadata(
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  mode INTEGER,
  mtime TEXT,
  PRIMARY KEY(library_id, path)
);

inodes(
  library_id TEXT NOT NULL,
  inode INTEGER NOT NULL,
  path TEXT NOT NULL,
  PRIMARY KEY(library_id, inode),
  UNIQUE(library_id, path)
);
```

Storage invariants:

- Exactly one of `inline_content` or `content_hash` is set for a materialized document version.
- Document head changes only through a committed transaction.
- Blob files are immutable once written.
- Conflict records preserve both sides until explicitly resolved.
- Deleted documents keep tombstone/history by default.
- True purge/excision is not a phase-one default operation.

## Transaction Semantics
All write paths flow through the same transaction system.

Supported modes:

- Auto-commit: single write creates and commits a transaction internally.
- Explicit transaction: caller creates a transaction, stages multiple document changes, then commits or rolls back.

Transaction records must store provenance:

- actor
- source: `rest`, `git`, `fuse`, `cli`
- message
- timestamp
- optional Git commit or sync peer
- optional import/export metadata

Required behavior:

- Commit atomically advances all affected document heads.
- Rollback leaves no visible partial document changes.
- Failed commits do not leave partially published CAS objects referenced by documents.
- Open transactions block GC for referenced temporary blobs.
- Git sync and GC acquire an exclusive global operation lock.
- Turso busy/locked errors are retried within a bounded policy and then surfaced clearly.

## REST API
Use `axum` for the API server and `utoipa`/`utoipa-axum` for generated OpenAPI.

Bind to `127.0.0.1` by default. The user can explicitly override the bind address with `quarry serve --addr`. Phase one has no auth, so non-loopback binds must emit a clear warning.

Canonical phase-one endpoints:

```http
POST   /v1/libraries
GET    /v1/libraries
GET    /v1/libraries/{library}

GET    /v1/libraries/{library}/documents?prefix=notes/&limit=100
GET    /v1/libraries/{library}/documents/{path}
HEAD   /v1/libraries/{library}/documents/{path}
PUT    /v1/libraries/{library}/documents/{path}
PATCH  /v1/libraries/{library}/documents/{path}/metadata
POST   /v1/libraries/{library}/documents/{path}/move
DELETE /v1/libraries/{library}/documents/{path}

POST   /v1/libraries/{library}/transactions
PUT    /v1/libraries/{library}/transactions/{tx}/documents/{path}
PATCH  /v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata
POST   /v1/libraries/{library}/transactions/{tx}/documents/{path}/move
DELETE /v1/libraries/{library}/transactions/{tx}/documents/{path}
POST   /v1/libraries/{library}/transactions/{tx}/commit
POST   /v1/libraries/{library}/transactions/{tx}/rollback

POST   /v1/libraries/{library}/git/peers
GET    /v1/libraries/{library}/git/peers
POST   /v1/libraries/{library}/git/import
POST   /v1/libraries/{library}/git/export
POST   /v1/libraries/{library}/git/peers/{peer}/pull
POST   /v1/libraries/{library}/git/peers/{peer}/push
POST   /v1/libraries/{library}/git/peers/{peer}/sync

GET    /v1/libraries/{library}/conflicts
GET    /v1/libraries/{library}/conflicts/{conflict}
POST   /v1/libraries/{library}/conflicts/{conflict}/resolve

POST   /v1/admin/gc
GET    /v1/health
GET    /v1/openapi.json
```

HTTP behavior:

- Reads return an `ETag` based on the visible document version.
- Writes support `If-Match` for compare-and-swap updates.
- Creates support `If-None-Match: *`.
- Storage busy/contention maps to `503 Service Unavailable` with `Retry-After`.
- Missing documents return `404`.
- Conflicting preconditions return `412 Precondition Failed`.

## Git Sync
Git sync is a peer adapter over Quarry documents.

Phase-one limits:

- One Git peer can be configured first; multiple peer rows are allowed but not required to be fully exercised.
- One remote and one branch per sync peer.
- Sync is explicit through REST/CLI, not always-on watching.
- Use `git2`/libgit2 for phase-one Git operations.
- Keep a `GitSync` module boundary so `gix` can replace read-heavy paths later.

### Import
`git/import` reads a working tree and ingests files into Quarry documents.

Rules:

- Ignore `.git/`.
- Reserve `.quarry/` for Quarry metadata and sentinels.
- Markdown metadata may be read from YAML frontmatter.
- Non-Markdown metadata may be read from sidecars such as `path.ext.quarrymeta.yaml`.
- Import creates a Quarry transaction with `source = git`.

### Export
`git/export` materializes Library documents into a Git working tree and commits them.

Rules:

- Document path maps to the same file path in the repo.
- `.quarry/marker.json` records the Library ID and refuses sync if mismatched.
- Markdown metadata can be exported as frontmatter when configured.
- Binary metadata exports as sidecar files.
- Very large binary content is not pushed through Git LFS in phase one.

Binary policy:

- Allow ordinary binary export below 5 MiB by default.
- Warn or refuse export above 5 MiB unless explicitly forced.
- Always refuse content larger than the hosting platform limit when known.
- No Git LFS protocol support in phase one.

### Bidirectional Sync
Bidirectional sync uses a snapshot-based three-way diff:

1. Snapshot current Quarry document heads.
2. Snapshot current Git working tree or fetched remote tree.
3. Load last-known-synced state for that peer/path.
4. Classify each path into the sync matrix.
5. Apply one-sided changes automatically.
6. Preserve both copies for true conflicts.
7. Commit/export resulting Git state.
8. Update `sync_state` only after success.

Sync matrix must cover:

- both unchanged
- only Quarry changed
- only Git changed
- both changed same content
- both changed different content
- both deleted
- Quarry deleted / Git changed
- Quarry changed / Git deleted
- both created

Conflict rule:

- Never write inline Git conflict markers into canonical documents.
- Keep the canonical path as the local/Quarry winner by default.
- Store the Git side as a sibling conflict document.
- Record a `conflicts` row with both version IDs.
- Export both files to Git so human tools can inspect them.

Example:

```text
notes/plan.md
notes/plan.md.conflict-git-2026-05-27T14-30-00Z
```

Safety rules:

- Abort sync if `.quarry/marker.json` is missing or mismatched.
- Abort if the operation would delete or rewrite more than a configured percentage of tracked paths.
- Acquire an exclusive sync lock so normal writes do not interleave with sync publication.
- Preserve sync logs and transaction provenance for auditability.

## FUSE Filesystem
FUSE is a Linux-only projection of a Library's committed state.

Use a Linux FUSE implementation for phase one. `fuse3` is the preferred crate because native async handlers match the Tokio-based daemon. macOS FUSE-T/macFUSE support is not required and is not part of phase-one acceptance.

Delivery sequence:

1. Read-only Linux mount.
2. Writable Linux auto-commit mount.
3. Packaging/docs for Linux FUSE prerequisites.

Read operations:

- `lookup`
- `getattr`
- `readdir`
- `open`
- `read`

Write operations:

- `create`
- `write`
- `flush`/`release` publication
- `mkdir`
- `rename`
- `unlink`
- `rmdir`
- `setattr` for supported metadata

Write semantics:

- FUSE writes auto-commit.
- No explicit multi-file transactions through FUSE in phase one.
- Multi-file atomic edits must use REST transactions.
- Small writes to the same file should be coalesced before committing.
- REST, Git, and FUSE writes must invalidate each other's caches through an in-process event channel.

FUSE limitations:

- Linux only.
- No macOS FUSE support in phase one.
- No Windows filesystem support in phase one.
- No hard links.
- No symlinks in phase one.
- No guarantee that case-conflicting keys behave well on case-insensitive filesystems.
- `.quarry/` is reserved and protected.
- Executable/JIT-heavy workflows should use a normal filesystem for build artifacts, not the Quarry mount.

## CLI And Daemon
Ship a `quarry` binary that can run the daemon and perform basic local operations.

Required commands:

```text
quarry init <server-root>
quarry serve [--db <path>] [--cas <path>] [--addr 127.0.0.1:port]
quarry mount <library> <mountpoint> [--read-only]
quarry get <library> <path>
quarry put <library> <path> <file>
quarry list <library> [--prefix <prefix>]
quarry move <library> <from-path> <to-path>
quarry delete <library> <path>
quarry tx begin <library>
quarry tx commit <tx>
quarry tx rollback <tx>
quarry git import <library> <repo>
quarry git export <library> <repo>
quarry git sync <library> <peer>
quarry conflicts list <library>
quarry conflicts resolve <library> <conflict>
quarry gc
quarry backup <destination>
quarry restore <source>
```

The CLI may be a thin client over the local REST API where possible.
## Packaging And Documentation
Phase one ships:

- Cargo workspace.
- Single `quarry` binary.
- GitHub release artifact.
- Debian package or documented Linux install path.
- Linux FUSE setup notes.
- Quickstart.
- REST API docs from OpenAPI.
- Git sync behavior docs.
- Conflict-resolution docs.
- Backup/restore docs.
- Known filesystem limitations.

## Workspace Shape
Recommended Cargo workspace:

```text
quarry/
  Cargo.toml
  crates/
    quarry-core/
    quarry-storage/
    quarry-cas/
    quarry-git/
    quarry-fuse/
    quarry-server/
    quarry-cli/
    quarry/
  examples/
```

Responsibilities:

- `quarry-core`: domain types, IDs, errors, metadata, document model.
- `quarry-storage`: TursoDB integration, migrations, transaction wrappers.
- `quarry-cas`: BLAKE3 hash type, disk CAS, GC.
- `quarry-git`: Git peer config, import/export, sync, conflicts.
- `quarry-fuse`: Linux mount implementation, inode cache, FUSE projection.
- `quarry-server`: axum routes, OpenAPI, REST semantics.
- `quarry-cli`: command parsing and local client UX.
- `quarry`: binary crate wiring daemon/server/mount.

## Delivery Increments
### Increment 0: Risk Spikes
Goal: prove the highest-risk Turso, Git, FUSE, and CAS details before building the durable architecture.

Deliver:

- Turso transaction/concurrency spike.
- Linux FUSE trivial filesystem spike.
- `git2` bidirectional conflict spike.
- CAS write-throughput spike.

Exit criteria:

- Turso wrapper can run concurrent write tests with bounded busy retry and no lost commits.
- Trivial Linux FUSE mount can read, write, rename, and list files.
- Git conflict spike preserves both copies without inline markers.
- CAS write path is fast enough for local large-file use.

### Increment 1: Core Storage And CAS
Deliver:

- Cargo workspace skeleton.
- Core domain types.
- TursoDB schema and migrations.
- Turso transaction wrapper and retry policy.
- Server-level database with multiple logical Libraries.
- Library create/open.
- Document put/get/list/delete.
- Document move/rename.
- Versioned document heads.
- JSON metadata.
- Hybrid inline/CAS storage.
- CAS GC.

Exit criteria:

- Integration test creates multiple Libraries in one TursoDB database.
- Integration test writes, reads, lists, renames, deletes, and versions 1000 mixed-size documents.
- Large documents are stored in CAS and survive restart.
- GC removes unreachable blobs and preserves reachable ones.

### Increment 2: Transactions And REST API
Deliver:

- Auto-commit transaction path.
- Explicit begin/stage/commit/rollback transaction path.
- Transaction provenance.
- Axum REST API.
- Generated OpenAPI.
- ETag/conditional write behavior.
- Local health endpoint.
- Explicit `--addr` bind override with warning for non-loopback addresses.

Exit criteria:

- `curl` can create a Library, put/get/list/delete documents, run explicit multi-document transactions, and roll back cleanly.
- OpenAPI spec validates.
- Concurrent write tests return correct success, retry, or precondition behavior.
- Server binds to `127.0.0.1` by default and to an alternate address only when explicitly configured.

### Increment 3: Git Import And Export
Deliver:

- Git peer config.
- Import from working tree.
- Export to working tree.
- Git commit creation.
- Metadata frontmatter/sidecar handling.
- `.quarry/marker.json`.

Exit criteria:

- Import/export round trip is byte-lossless for text and binary files under threshold.
- Metadata survives import/export.
- Wrong Library marker causes safe refusal.

### Increment 4: Bidirectional Git Sync
Deliver:

- Three-way sync state.
- Snapshot diff.
- Sync matrix implementation.
- Non-destructive conflict preservation.
- Conflict API.
- Sync safety limits.
- Manual `pull`, `push`, and `sync` commands/endpoints.

Exit criteria:

- Full sync matrix test suite passes.
- Conflicting edits create both files and a conflict record.
- Deletes never cascade beyond safety threshold.
- Sync updates `sync_state` only after a successful commit.

### Increment 5: Linux FUSE Mount
Deliver:

- Read-only Linux FUSE mount.
- Inode allocation/cache.
- Directory listing from prefixes.
- Read/write autocommit Linux mount.
- Write coalescing.
- Cache invalidation between REST/Git/FUSE.
- Linux FUSE prerequisite docs.

Exit criteria:

- `ls`, `cat`, `find`, `rg`, `vim`, `cp -r`, and `git status` work against a mounted Library on Linux.
- Writes through FUSE are visible through REST.
- REST writes are visible through FUSE after invalidation.
- Mount/unmount leaves no corrupted Library state.

### Increment 6: Packaging And Operational Hardening
Deliver:

- `quarry` CLI/daemon.
- Linux release packaging.
- Backup/restore command for server database and CAS.
- Admin GC command.
- Structured logs for API, sync, FUSE, GC.
- Crash-safety/invariant test suite.
- Documentation.
- Optional MCP skeleton that delegates to REST.

Exit criteria:

- Fresh Linux install can follow quickstart from zero to mounted Library.
- Backup/restore reproduces Libraries, documents, metadata, versions, and CAS references.
- Basic observability is enough to debug sync and mount failures.
- A human and an agent can edit the same Library for a representative workflow without data loss.

## Testing Requirements
Storage tests:

- Turso schema migrations.
- Single TursoDB database with multiple Libraries.
- Path normalization.
- Case-sensitive keys.
- Document lifecycle.
- Document move/rename.
- Version history.
- Inline/CAS threshold transitions.
- CAS atomic writes.
- CAS GC reachability.
- Transaction commit and rollback.
- Busy retry behavior.
- Crash/restart around staged writes where practical.

REST tests:

- Endpoint success and failure cases.
- `ETag` and `If-Match`.
- `If-None-Match: *`.
- OpenAPI generation.
- Busy/retry mapping.
- JSON metadata validation and patch behavior.
- Default bind address.
- Explicit bind address override.

Git tests:

- Import/export round trip.
- Metadata frontmatter.
- Metadata sidecars.
- Binary threshold behavior.
- Marker mismatch.
- Every sync matrix case.
- Conflict record lifecycle.
- Large delete safety abort.
- Line-ending and BOM handling.
- Case-collision behavior.
- Reserved filename behavior.

FUSE tests:

- Linux read-only operations.
- Linux write/create/delete/rename.
- Directory metadata and empty directories.
- Cache invalidation.
- Editor save patterns.
- `rg`, `find`, `vim`, `cp -r`, `git status`.
- Unmount/remount consistency.

Operational tests:

- Backup/restore of TursoDB database plus CAS.
- GC under no active transactions.
- Exclusive lock around sync and GC.
- Daemon restart.
- Second-daemon lock rejection.
- CLI smoke tests.

## Risks And Mitigations
### Turso maturity
Risk: TursoDB may still expose rough edges around local transactions, busy handling, or concurrent writes.

Mitigation: make the Turso wrapper explicit, keep one writer-owning daemon process, use bounded busy retry, add concurrency soak tests, and keep transaction semantics in Quarry's application layer.
### Git reconciliation complexity
Risk: Two-way Git sync can lose data if treated as naive file mirroring.

Mitigation: require stored sync state, three-way diff, explicit sync matrix tests, and store-both conflict behavior.
### Linux FUSE semantics
Risk: Filesystem behavior is surprising, especially around writeback, rename, append, and editor save patterns.

Mitigation: deliver read-only first, then write autocommit, avoid explicit FUSE transactions, add write coalescing, and document limitations.
### Large binary sync
Risk: Git repositories become unusable if Quarry pushes large blobs directly.

Mitigation: CAS remains canonical; Git export warns/refuses above 5 MiB by default; full Git LFS support is deferred.
### Multi-process writes
Risk: Multiple daemons writing one TursoDB database can corrupt state or produce undefined locking behavior.

Mitigation: single-process ownership is a phase-one requirement; use lock files and clear startup errors.
### Future search/MCP/UI scope creep
Risk: Phase one grows into a full memory app.

Mitigation: reserve schema/API hooks, but ship only the storage substrate and thin operational surfaces.
## Confirmed Decisions
These decisions came out of the phase-one research review:

1. Library storage: one TursoDB database stores all Libraries on the server.
2. Database backend: use `https://github.com/tursodatabase/turso`; do not use `libsql` or `rusqlite` for phase one.
3. Git sync: explicit pull/push/sync only; no ambient watcher in phase one.
4. FUSE writes: auto-commit only; explicit multi-document transactions stay REST/CLI-only.
5. Binary Git policy: warn/refuse above 5 MiB by default; no Git LFS protocol in phase one.
6. Deletion policy: keep tombstones/history by default; true purge is deferred.
7. Auth policy: no auth in phase one; bind to `127.0.0.1` by default and allow explicit override.
