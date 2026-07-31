{
  "job_id": "threat:008-cli-and-binary-55d4ef1b",
  "component": "cli-and-binary",
  "entryPoints": [
    "crates/quarry-cli/src/lib.rs:441 — Cli::parse() ingests all argv (paths, library names, doc paths, URLs, roles, version ids, tx/conflict ids)",
    "crates/quarry-cli/src/lib.rs:208 — --root / QUARRY_ROOT env controls the store root directory (default .quarry)",
    "crates/quarry-cli/src/lib.rs:255-271 — server start flags: --db, --cas paths, --addr bind address, --client-ip-source / QUARRY_CLIENT_IP_SOURCE",
    "crates/quarry-cli/src/lib.rs:727-730 — --server / QUARRY_SERVER URL the CLI POSTs file content to",
    "crates/quarry-cli/src/lib.rs:711-712 — `open <file>` reads an attacker-influenceable local file and uploads its contents to the configured server",
    "crates/quarry-cli/src/lib.rs:560-561 — `put` reads an arbitrary local file path from argv into the document store",
    "crates/quarry-cli/src/lib.rs:49-51 — RUST_LOG / QUARRY_LOG_FORMAT env vars steer logging configuration",
    "crates/quarry-cli/src/detect_agent.rs:30-89 — agent-detection env vars (AI_AGENT and friends) plus existence of /opt/.devin shape stdout output",
    "crates/quarry-cli/src/lib.rs:948-959 — JSON/text responses from the quarry server (document.path secret, agent prompt) consumed by the CLI",
    "crates/quarry-cli/src/lib.rs:379-400 — git import/export/sync/pull/push take repo paths, peer names, and branch names from argv"
  ],
  "sinks": [
    "crates/quarry-cli/src/lib.rs:485 — fs::remove_dir_all(&root) in `server restore` wipes the resolved root before copying; root comes from CLI/env and default-relative path",
    "crates/quarry-cli/src/lib.rs:1028-1044 — copy_dir recursively copies source tree to destination with no symlink handling (fs::copy follows symlinks) for backup/restore",
    "crates/quarry-cli/src/lib.rs:1017 — fs::create_dir_all(root) creates the resolved root directory",
    "crates/quarry-cli/src/lib.rs:941-953 — reqwest POST/GET to user-controlled --server URL, sending file contents and interpolating server-returned `secret` into a follow-up URL path",
    "crates/quarry-cli/src/lib.rs:990 — open::that(browser_url) launches the OS browser/handler on a URL derived from the server response and --server flag",
    "crates/quarry-cli/src/lib.rs:605-617 — store.write_block_markdown with CLI-supplied path/markdown/base_version into the store",
    "crates/quarry-cli/src/lib.rs:626-638 — store.put_document with CLI-supplied library/path/bytes",
    "crates/quarry-cli/src/lib.rs:655-662 — create_collab_invite_token mints share tokens with user-supplied role string (default editor)",
    "crates/quarry-cli/src/lib.rs:792-841 — import_worktree/export_worktree/sync_peer/pull_peer/push_peer reach into quarry_git with user-supplied repo paths, peer names, branches",
    "crates/quarry-cli/src/lib.rs:1018-1023 — QuarryStore::open on root-joined quarry.db / cas paths",
    "crates/quarry-cli/src/lib.rs:457-464 — serve_with_config binds a TCP listener on user-supplied --addr",
    "crates/quarry-cli/src/lib.rs:518-542 — mount_library_with_shutdown mounts a FUSE filesystem at user-supplied mountpoint"
  ],
  "assumptions": [
    "crates/quarry-cli/src/lib.rs:213-215,448-488 — `server restore` assumes the operator intends the resolved root to be deleted; no confirmation, and root can come from QUARRY_ROOT env",
    "crates/quarry-cli/src/lib.rs:577,786-787 — comments assert the CLI process owns the database exclusively (no live sessions); nothing enforces that (lock_path: None at line 1021)",
    "crates/quarry-cli/src/lib.rs:566-567,599-604 — markdown-ness of a put is inferred from mime_guess on the local filename; content is assumed UTF-8 for markdown documents",
    "crates/quarry-cli/src/lib.rs:948-950 — assumes the server response is well-formed JSON containing document.path; the returned secret is trusted for URL construction",
    "crates/quarry-cli/src/lib.rs:905-910 — ensure_library auto-creates any library name the user types; assumes store layer validates/sanitizes library slugs",
    "crates/quarry-cli/src/lib.rs:271,461 — --client-ip-source trusts that the operator only sets it when a trusted proxy sits in front; the CLI itself cannot verify that",
    "crates/quarry-cli/src/lib.rs:261-262 — bind address defaults to loopback but the component assumes the operator understands exposure when overriding --addr/--serve-addr",
    "crates/quarry-cli/src/detect_agent.rs:30-89 — assumes environment variables accurately identify the calling agent; any process can set them to steer which output (agent prompt with secret vs browser URL) is printed"
  ],
  "trustBoundaries": [
    "crates/quarry-cli/src/lib.rs:441 — argv/env (user/agent controlled) crosses into filesystem and store operations",
    "crates/quarry-cli/src/lib.rs:711-713 — local file content crosses to a remote HTTP server chosen by --server/QUARRY_SERVER (data exfiltration path if the env var is attacker-influenced)",
    "crates/quarry-cli/src/lib.rs:940-959 — remote server responses (untrusted network data) cross into URL construction, stdout instructions for AI agents, and browser launch",
    "crates/quarry-cli/src/lib.rs:560-566,605-617 — local file bytes cross into the persistent document store/CAS as authenticated CLI-sourced documents",
    "crates/quarry-cli/src/lib.rs:457-464,507-533 — local store crosses a network boundary when served via --addr or mount --serve-addr",
    "crates/quarry-cli/src/lib.rs:655-663 — CLI operator authority crosses into capability tokens (collab invite tokens) consumable by less-trusted remote clients"
  ],
  "hotFiles": [
    "crates/quarry-cli/src/lib.rs",
    "crates/quarry-cli/src/detect_agent.rs",
    "crates/quarry/src/main.rs"
  ]
}