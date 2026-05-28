# CLI API Daemon And Server-Managed Mounts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox syntax for progress tracking.

**Goal:** Move Quarry's normal CLI commands to the local REST API with daemon autostart, while keeping FUSE mounts owned by the same server process.

**Architecture:** The CLI becomes a thin HTTP client for all normal store operations. The server is the only long-lived process that opens `QuarryStore` during regular use. Bootstrap and lifecycle commands remain exceptions. FUSE mounts are not implemented through HTTP file operations; instead, `quarry mount` asks the server to start a mount in the server process, then optionally waits as a foreground controller.

**Tech Stack:** Rust workspace, `clap`, `tokio`, `axum`, `reqwest`, existing `quarry-server`, `quarry-fuse`, `quarry-git`, `quarry-storage`.

---

## Current State

- `crates/quarry-cli/src/lib.rs` directly opens `QuarryStore` through `open_at(root, db, cas)` for most commands.
- `Command::Serve` opens the store directly and runs `quarry_server::serve(store, addr)`.
- `Command::Mount` opens the store directly. With `--serve-addr`, the same CLI process runs both FUSE and REST server through `tokio::select!`.
- The REST server already covers the main CLI surface: libraries, documents, transactions, git sync, conflicts, admin GC, health, and OpenAPI.
- `quarry-fuse::mount_library` currently owns a blocking lifecycle: mount, wait for Ctrl-C, unmount.
- Existing CLI smoke tests exercise direct CLI operations and often seed data by opening `QuarryStore` directly.

## Target Behavior

- `quarry init` remains a direct filesystem bootstrap command.
- `quarry server start`, `quarry server stop`, `quarry server status`, and equivalent compatibility commands manage the daemon.
- Normal commands such as `put`, `get`, `list`, `tx`, `conflicts`, `git-*`, and `gc` resolve a server and call HTTP.
- If no healthy server is recorded for `--root`, normal commands start one automatically unless `--no-autostart` is set.
- `--server-url` bypasses discovery and autostart and sends requests to the provided server.
- Autostart binds to loopback with an OS-assigned port by default.
- The server writes metadata only after it has successfully bound and can answer health checks.
- `quarry mount` resolves or autostarts the same server, requests a server-owned mount, and then either waits in the foreground or exits if `--detach` is supplied.
- `quarry unmount <mountpoint>` asks the server to unmount.
- Server shutdown unmounts all active mounts before exiting.

## Explicit Non-Goals

- Do not implement FUSE by making a REST request for every filesystem read.
- Do not introduce systemd, launchd, or another service manager in this pass.
- Do not require a user-visible auth model for loopback autostart in this pass. Keep the design compatible with adding a token later.
- Do not switch to Unix sockets in this pass. Use TCP loopback now and keep the lifecycle API isolated enough to swap transports later.
- Do not rewrite the server API shape except where mount lifecycle endpoints are needed.

## Required Design Decisions

- Add a dedicated typed HTTP client crate, `quarry-client`, instead of embedding raw `reqwest` calls throughout the CLI.
- Keep server lifecycle and autostart policy in `quarry-cli`, not in the generated or typed HTTP client.
- Refactor `quarry-server` so it can serve on an already-bound listener and report the actual address for `127.0.0.1:0`.
- Add a server metadata file under the Quarry root, for example `<root>/.quarry/server.json`, plus a start lock such as `<root>/.quarry/server.lock`.
- Make FUSE mount management part of server state, with `quarry-server` depending on `quarry-fuse`.
- Split `quarry-fuse` into a reusable mount handle API plus the current blocking convenience wrapper.

## Proposed Files

- `Cargo.toml`
  - Add `crates/quarry-client` as a workspace member.
  - Add workspace dependencies for `reqwest` and a URL/path encoding crate if needed.

- `crates/quarry-client/Cargo.toml`
  - New crate for typed API calls.

- `crates/quarry-client/src/lib.rs`
  - Define `QuarryClient`, request/response helpers, path encoding helpers, and typed methods matching the REST API.

- `crates/quarry-cli/Cargo.toml`
  - Add dependency on `quarry-client`.
  - Remove direct dependencies that are only needed because normal commands open the store directly, after all call sites are converted.

- `crates/quarry-cli/src/lib.rs`
  - Keep command parsing.
  - Add global flags: `--server-url` and `--no-autostart`.
  - Route normal commands through `QuarryClient`.
  - Keep direct store access only for bootstrap/lifecycle exceptions.

- `crates/quarry-cli/src/server_lifecycle.rs`
  - New module for server metadata, discovery, autostart, wait-for-health, stop, and status.

- `crates/quarry-server/src/lib.rs`
  - Add listener-based serving.
  - Add shutdown support.
  - Add mount manager state and mount lifecycle routes.

- `crates/quarry-fuse/src/lib.rs`
  - Add a nonblocking mount handle API.
  - Preserve the existing blocking wrapper for compatibility and tests.

- `crates/quarry/tests/cli_smoke.rs`
  - Update CLI tests to expect autostarted server behavior.

- `crates/quarry-server/tests/rest_api.rs`
  - Add mount route and daemon lifecycle-adjacent coverage where possible.

- Docs:
  - `README.md`
  - `docs/operations/rest-api.md`
  - `docs/operations/fuse.md`
  - `docs/operations/backup-restore.md`
  - `docs/operations/install-linux.md`

---

## Phase 1: Add The Typed HTTP Client

- [ ] Create `crates/quarry-client`.

  Expose a small typed client:

  ```rust
  pub struct QuarryClient {
      base_url: reqwest::Url,
      http: reqwest::Client,
  }
  ```

  Include constructors:

  ```rust
  impl QuarryClient {
      pub fn new(base_url: impl AsRef<str>) -> anyhow::Result<Self>;
      pub fn from_url(base_url: reqwest::Url) -> Self;
  }
  ```

- [ ] Implement path-safe URL construction.

  Requirements:

  - Library names and document paths must be encoded as path segments.
  - A document path such as `notes/hello.md` must address the server route as a wildcard path while still escaping unsafe characters.
  - Empty paths must be rejected client-side with a clear error.
  - Leading slashes in document paths must be normalized or rejected consistently with current CLI behavior.

- [ ] Implement typed methods for existing REST coverage.

  Minimum methods:

  ```rust
  health()
  openapi_json()
  create_library(name)
  list_libraries()
  head_doc(library, path)
  get_doc(library, path)
  list_docs(library)
  put_doc(library, path, bytes)
  patch_doc(library, path, request)
  move_doc(library, from, to)
  delete_doc(library, path)
  begin_tx(library)
  stage_put(tx, path, bytes)
  stage_metadata(tx, path, request)
  stage_move(tx, from, to)
  stage_delete(tx, path)
  commit_tx(tx)
  rollback_tx(tx)
  list_peers()
  import_peer(...)
  export_peer(...)
  pull(...)
  push(...)
  sync(...)
  list_conflicts(...)
  get_conflict(...)
  resolve_conflict(...)
  gc(...)
  ```

- [ ] Preserve server error details.

  Add an error type or helper that includes:

  - HTTP status.
  - Response body when the body is text or JSON.
  - Request method and URL path.

- [ ] Add client tests.

  Test scenarios:

  - Health request succeeds against an in-process server.
  - Put/get round trip works for a path containing nested segments.
  - Missing documents return a useful error with HTTP status.
  - Path encoding works for spaces and special characters.

  Suggested command:

  ```bash
  cargo test -p quarry-client
  ```

## Phase 2: Add Server Lifecycle Metadata And Autostart

- [ ] Add server metadata model in `crates/quarry-cli/src/server_lifecycle.rs`.

  Suggested record:

  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct ServerRecord {
      pub pid: u32,
      pub root: PathBuf,
      pub url: String,
      pub started_at: DateTime<Utc>,
      pub version: String,
  }
  ```

  Store at:

  ```text
  <root>/.quarry/server.json
  ```

  If the repo already has a better metadata directory convention, use that existing convention instead.

- [ ] Add a start lock to prevent concurrent autostart races.

  Store at:

  ```text
  <root>/.quarry/server.lock
  ```

  Requirements:

  - Only one process may spawn the daemon for a root at a time.
  - Other processes wait for metadata and health.
  - Stale metadata is ignored if health fails.

- [ ] Add server resolution function.

  Target API:

  ```rust
  pub async fn resolve_client(opts: ResolveServerOptions) -> anyhow::Result<QuarryClient>;
  ```

  Resolution order:

  1. If `--server-url` is present, return a client for that URL and do not autostart.
  2. Read `server.json`; if health succeeds, return that client.
  3. If `--no-autostart` is present, fail with a clear message.
  4. Acquire `server.lock`.
  5. Re-check metadata and health.
  6. Spawn detached server with `127.0.0.1:0`.
  7. Wait for metadata and health.
  8. Return client.

- [ ] Add detached server spawn.

  Pattern:

  - Use `std::env::current_exe()`.
  - Set current directory to the resolved root or caller current directory, consistently with current CLI semantics.
  - Spawn with stdio null.
  - Pass explicit root and bind address.
  - Add an internal flag or subcommand to identify daemon startup if needed.

  Example argv shape:

  ```text
  quarry server start --root <root> --addr 127.0.0.1:0 --daemon-child
  ```

- [ ] Add wait-for-health.

  Requirements:

  - Poll for up to 5 seconds.
  - Retry every 50 to 100 ms.
  - If the child exits early and that can be detected, report that.
  - Include the metadata path in timeout errors.

- [ ] Add lifecycle commands.

  Add:

  ```text
  quarry server start [--addr ADDR] [--foreground]
  quarry server status
  quarry server stop
  ```

  Compatibility:

  - Keep existing `quarry serve --addr ...` as a foreground alias for now.
  - Existing scripts using `serve` should continue to work.

- [ ] Refactor `quarry-server` listener startup.

  Add functions equivalent to:

  ```rust
  pub async fn serve_listener(
      store: QuarryStore,
      listener: tokio::net::TcpListener,
      shutdown: impl Future<Output = ()>,
  ) -> anyhow::Result<()>;
  ```

  Requirements:

  - `server start --addr 127.0.0.1:0` must write the actual bound URL to metadata.
  - Metadata must not be written before bind succeeds.
  - Metadata should be removed on graceful shutdown if it still points to this process.

- [ ] Add lifecycle tests.

  Test scenarios:

  - `resolve_client` returns explicit `--server-url` without reading metadata.
  - Stale metadata causes autostart.
  - `--no-autostart` fails when no server is healthy.
  - Two concurrent resolves do not spawn two healthy servers for the same root.
  - `server status` reports running server URL.
  - `server stop` stops the daemon and removes metadata.

## Phase 3: Convert Normal CLI Commands To HTTP

- [ ] Classify commands.

  Direct store access remains allowed only for:

  - `init`
  - `server start` / `serve`
  - low-level restore if implemented as an offline operation
  - tests or internal helpers that explicitly create fixtures

  Everything else must use `QuarryClient`.

- [ ] Add global CLI flags.

  Add to top-level options:

  ```text
  --server-url URL
  --no-autostart
  ```

  Semantics:

  - `--server-url` is explicit remote or loopback mode and never autostarts.
  - `--no-autostart` only applies when discovery is needed.
  - Both flags should be accepted for normal commands.
  - `--no-autostart` with `--server-url` should be harmless.

- [ ] Convert library/document commands.

  Commands:

  - `library create`
  - `library list`
  - `put`
  - `get`
  - `head`
  - `list`
  - `patch`
  - `mv`
  - `rm`

  Requirements:

  - CLI output remains compatible unless there is a clear reason to improve it.
  - File read/write still happens in the CLI for local source/destination paths.
  - Store mutation happens through HTTP only.

- [ ] Convert transaction commands.

  Commands:

  - `tx begin`
  - `tx put`
  - `tx metadata`
  - `tx mv`
  - `tx rm`
  - `tx commit`
  - `tx rollback`

  Requirements:

  - Transaction IDs are server-issued and passed back to API routes.
  - Existing CLI examples continue to work.

- [ ] Convert conflict commands.

  Commands:

  - `conflicts list`
  - `conflicts get`
  - `conflicts resolve`

  Requirements:

  - Preserve JSON output where currently present.
  - Preserve human-readable output where currently present.

- [ ] Convert git/sync commands.

  Commands:

  - peer list/import/export
  - pull
  - push
  - sync

  Requirements:

  - The server performs git import/export/pull/push on the same host.
  - CLI path arguments are sent as request fields.
  - Error messages must call out that paths are interpreted on the server host when `--server-url` is explicit.

- [ ] Convert admin commands.

  Commands:

  - `gc`
  - `backup`
  - `restore`

  Decision:

  - Implement `gc` as an API call.
  - Prefer implementing `backup` as a server API call so the server can coordinate with the open store.
  - Treat `restore` as an offline lifecycle operation unless a safe online restore API already exists. It should refuse if a healthy server is running for the same root, or stop the server before proceeding if an explicit force flag is added.

- [ ] Remove normal-command direct store openings.

  Acceptance check:

  - In `crates/quarry-cli/src/lib.rs`, normal command arms should call `resolve_client(...)` and client methods.
  - `open_at(...)` should no longer be used by normal command execution.
  - Direct `QuarryStore::open(...)` should be isolated to bootstrap/lifecycle/offline restore paths.

- [ ] Update CLI smoke tests.

  Test scenarios:

  - `quarry --root <tmp> init`
  - `quarry --root <tmp> put notes notes/hello.md /tmp/hello.md` autostarts a server.
  - A second normal command reuses the recorded server.
  - `quarry --root <tmp> --no-autostart list notes` fails before a server exists.
  - `quarry --root <tmp> server status` prints the URL.
  - `quarry --root <tmp> server stop` stops the server.

## Phase 4: Add Server-Managed FUSE Mounts

- [ ] Refactor `quarry-fuse` to expose a nonblocking mount handle.

  Target shape:

  ```rust
  pub struct MountHandle {
      mountpoint: PathBuf,
      join: tokio::task::JoinHandle<anyhow::Result<()>>,
      unmount: Box<dyn FnOnce() -> anyhow::Result<()> + Send>,
  }

  pub async fn start_mount(
      store: QuarryStore,
      library: LibraryId,
      mountpoint: PathBuf,
      options: MountOptions,
  ) -> anyhow::Result<MountHandle>;
  ```

  Preserve:

  ```rust
  pub async fn mount_library(...) -> anyhow::Result<()>;
  ```

  Reimplement it using `start_mount`, waiting for Ctrl-C, then unmounting.

- [ ] Add mount manager to `quarry-server`.

  Suggested state:

  ```rust
  pub struct MountManager {
      mounts: Mutex<HashMap<PathBuf, ActiveMount>>,
  }
  ```

  Requirements:

  - A mount is keyed by canonical mountpoint.
  - Duplicate mountpoint requests return conflict.
  - Active mounts are unmounted on server shutdown.
  - If a mount task exits unexpectedly, remove it from state and preserve an error for status if practical.

- [ ] Add server mount routes.

  Suggested routes:

  ```text
  GET    /v1/admin/mounts
  POST   /v1/admin/mounts
  DELETE /v1/admin/mounts/{mount_id_or_encoded_mountpoint}
  ```

  Request:

  ```json
  {
    "library": "notes",
    "mountpoint": "/tmp/quarry-notes",
    "cache_ttl_ms": 1000,
    "negative_ttl_ms": 250,
    "mount_options": []
  }
  ```

  Response:

  ```json
  {
    "id": "...",
    "library": "notes",
    "mountpoint": "/tmp/quarry-notes",
    "state": "mounted"
  }
  ```

- [ ] Convert `quarry mount`.

  New default flow:

  1. Resolve or autostart server.
  2. POST mount request.
  3. Print mountpoint and server URL.
  4. Unless `--detach` is set, wait for Ctrl-C.
  5. On Ctrl-C, DELETE the mount.

  Requirements:

  - Existing basic syntax should keep working:

    ```text
    quarry mount notes /mnt/notes
    ```

  - Remove or deprecate `--serve-addr` because the server is now the default owner.
  - If kept temporarily, document it as compatibility and route it through server lifecycle where possible.

- [ ] Add `quarry unmount`.

  Suggested syntax:

  ```text
  quarry unmount /mnt/notes
  ```

  Behavior:

  - Resolve or autostart server by default.
  - DELETE the server-managed mount.
  - With `--no-autostart`, fail if no healthy server exists.

- [ ] Add FUSE tests.

  Unit/integration tests that do not require actual kernel FUSE:

  - Mount manager rejects duplicate mountpoints.
  - Unmount removes active mount state.
  - Server shutdown calls unmount for active handles.

  Privileged/manual smoke test:

  ```bash
  mkdir -p /tmp/quarry-root /tmp/quarry-mount
  cargo run -p quarry -- --root /tmp/quarry-root init
  cargo run -p quarry -- --root /tmp/quarry-root library create notes
  printf 'hello\n' >/tmp/hello.md
  cargo run -p quarry -- --root /tmp/quarry-root put notes notes/hello.md /tmp/hello.md
  cargo run -p quarry -- --root /tmp/quarry-root mount notes /tmp/quarry-mount --detach
  cat /tmp/quarry-mount/notes/hello.md
  cargo run -p quarry -- --root /tmp/quarry-root unmount /tmp/quarry-mount
  cargo run -p quarry -- --root /tmp/quarry-root server stop
  ```

## Phase 5: Documentation Updates

- [ ] Update README command examples.

  Explain:

  - Normal commands autostart the local server.
  - `quarry server status` shows the daemon.
  - `quarry server stop` shuts it down.
  - `--server-url` is available for explicit remote/local API use.
  - `--no-autostart` is useful for scripts that require a pre-existing daemon.

- [ ] Update REST API docs.

  Add mount lifecycle endpoints and clarify that normal CLI commands use this API.

- [ ] Update FUSE docs.

  Clarify:

  - FUSE runs inside the same server process.
  - `quarry mount` is a controller command.
  - Foreground mode unmounts on Ctrl-C.
  - `--detach` leaves the mount active until `quarry unmount` or server stop.

- [ ] Update backup/restore docs.

  Clarify:

  - Backup behavior with a running server.
  - Restore behavior and any required server stop.
  - Recommended script sequence.

## Phase 6: Verification

- [ ] Format.

  ```bash
  cargo fmt
  ```

- [ ] Run focused tests.

  ```bash
  cargo test -p quarry-client
  cargo test -p quarry-cli
  cargo test -p quarry-server
  cargo test -p quarry --test cli_smoke
  ```

- [ ] Run workspace checks.

  ```bash
  cargo check --workspace
  cargo test --workspace
  ```

- [ ] Run Linux FUSE check where available.

  ```bash
  cargo check -p quarry-fuse --target x86_64-unknown-linux-gnu
  ```

- [ ] Run privileged FUSE smoke in Docker or a Linux host with FUSE enabled.

  Acceptance:

  - Mount command uses existing server metadata or autostarts a server.
  - Mounted files reflect store contents.
  - Mutations through HTTP invalidate FUSE cache through the existing event subscription path.
  - `quarry unmount` unmounts cleanly.
  - `quarry server stop` unmounts active mounts before exit.

---

## Risks And Mitigations

- Stale `server.json` can point to a dead process.
  - Mitigation: never trust metadata without health.

- Concurrent CLI commands can race to autostart.
  - Mitigation: use `server.lock`, then re-check metadata and health after acquiring the lock.

- Server-managed mounts make server shutdown more consequential.
  - Mitigation: track mount handles centrally and unmount on graceful shutdown.

- Explicit `--server-url` may point to another machine, making local path arguments ambiguous.
  - Mitigation: document and warn that git import/export and backup paths are interpreted by the server host.

- Online restore can corrupt or confuse an open store.
  - Mitigation: keep restore offline unless a coordinated restore API is deliberately designed.

- Adding `reqwest` increases CLI dependency weight.
  - Mitigation: isolate HTTP transport in `quarry-client`; the improved architecture is worth the dependency.

## Acceptance Criteria

- Normal CLI commands no longer open `QuarryStore` directly.
- A normal command against an initialized root autostarts a loopback server and succeeds.
- A second normal command reuses the existing server.
- `--no-autostart` fails clearly when no server is healthy.
- `--server-url` routes commands to the explicit API endpoint.
- Server lifecycle commands start, report, and stop the daemon.
- `quarry mount` creates a server-owned FUSE mount.
- `quarry unmount` removes a server-owned FUSE mount.
- Server stop unmounts active mounts.
- Existing REST API tests pass.
- Updated CLI smoke tests prove the new server-backed execution path.
