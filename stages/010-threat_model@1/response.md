{
  "component": "git-layer",
  "job_id": "threat:004-git-layer-a17792f3",
  "paths": ["crates/quarry-git"],
  "entryPoints": [
    "crates/quarry-server/src/git_handlers.rs:98 — HTTP POST /v1/libraries/{library}/git/import: caller-supplied `repo` filesystem path flows unvalidated into import_worktree (line 104).",
    "crates/quarry-server/src/git_handlers.rs:123 — HTTP POST /v1/libraries/{library}/git/export: caller-supplied `repo` path, `branch`, `force_large` flow into export_worktree (lines 129-133).",
    "crates/quarry-server/src/git_handlers.rs:55 — HTTP POST create_git_peer: attacker-controlled GitPeerRequest (library slug, repo path, remote URL, branch, max_delete_percent) stored verbatim as peer config JSON (line 69).",
    "crates/quarry-server/src/git_handlers.rs:145 — HTTP POST git pull/push/sync endpoints trigger pull_peer/push_peer/sync_peer with stored (attacker-writable) peer config.",
    "crates/quarry-cli/src/lib.rs:796 — CLI git import/export/pull/push/sync commands feed user arguments into the same quarry-git functions.",
    "crates/quarry-git/src/lib.rs:1476 — peer_config reads repo path, branch, remote URL, max_delete_percent from stored peer config JSON; boundary where untrusted request data enters the crate.",
    "crates/quarry-git/src/lib.rs:1362 — remote git clone/fetch: remote-supplied repository contents (paths, file bytes, sidecar YAML, marker.json) enter via fetch_remote_worktree_blocking / RepoBuilder::clone.",
    "crates/quarry-git/src/lib.rs:1579 — local worktree traversal (WalkDir) reads every file under a caller-chosen directory (also scan_worktree_import_files at line 887)."
  ],
  "sinks": [
    "crates/quarry-git/src/lib.rs:1254 — clean_worktree: recursive fs::remove_dir_all / fs::remove_file on every entry of a caller-supplied repo_dir (everything except .git) before export; destructive deletion keyed entirely on the request's repo path.",
    "crates/quarry-git/src/lib.rs:1354 — fetch_remote_worktree_blocking: fs::remove_dir(repo_dir) on caller-supplied path prior to clone.",
    "crates/quarry-git/src/lib.rs:1036 — execute_worktree_export joins DB-held document paths onto repo_dir and writes via write_atomic (line 1154, fs::write + fs::rename); no in-crate re-normalization of the joined path at the sink.",
    "crates/quarry-git/src/lib.rs:1175 — write_sidecar writes <path>.quarrymeta.yaml under repo_dir; the same files are later read back as trusted metadata at sidecar_metadata (line 1100).",
    "crates/quarry-git/src/lib.rs:1333 — network I/O to attacker-controlled remote URL via libgit2: remote.fetch (line 1333), remote.push (line 1408), RepoBuilder::clone (line 1362); ensure_remote overwrites the origin URL (line 1462); credentials come from ambient libgit2 helpers/SSH agent, none configured in-crate.",
    "crates/quarry-git/src/lib.rs:1405 — push refspec built by string interpolation of attacker-supplied branch name (refs/heads/{branch}:refs/heads/{branch}); refspec/refname injection surface; also checkout_remote_branch ref construction (line 1375) and commit_all set_head (line 1275).",
    "crates/quarry-git/src/lib.rs:1078 — split_frontmatter and sidecar_metadata (line 1106): serde_yaml deserialization of untrusted file content and sidecar YAML; marker.json parsed with serde_json (lines 1198, 1231).",
    "crates/quarry-git/src/lib.rs:1595 — fs::read of every worktree file into memory with no size cap on import/snapshot (also line 903); memory-exhaustion sink — export enforces a 5 MiB threshold (line 997) but import does not.",
    "crates/quarry-git/src/lib.rs:683 — untrusted worktree content/paths written into the canonical document store: put_document (lines 683, 802), write_markdown_file (line 1668), move_document (line 529), delete_document (line 594).",
    "crates/quarry-git/src/lib.rs:1286 — commit_all creates commits with fixed signature 'Quarry <quarry@local>', no signing; attribution/repudiation-relevant identity use.",
    "crates/quarry-git/src/lib.rs:1596 — git2::Oid::hash_object SHA-1 hashing used for change detection (git_changed comparisons, lines 570-572); collision-relevant trust in hash equality."
  ],
  "assumptions": [
    "crates/quarry-git/src/lib.rs:1036 — export assumes document paths stored in the DB were normalized by normalize_path at write time (no '..', no backslashes); the crate re-joins them onto repo_dir without re-validating. See crates/quarry-core/src/lib.rs:606.",
    "crates/quarry-git/src/lib.rs:1492 — assumes `branch` and `remote` strings from peer config are safe to interpolate into refspecs/refnames and pass to libgit2; no refname validation in-crate.",
    "crates/quarry-git/src/lib.rs:1500 — assumes the `repo` path is an intended worktree; the marker check (verify_or_write_marker, line 1195) is the only guard, and export writes the marker if absent before clean_worktree wipes the directory.",
    "crates/quarry-git/src/lib.rs:887 — WalkDir filter assumes skipping names .git/.quarry protects git internals; assumes entry.file_type().is_file() plus fs::read (line 1595) cannot escape repo_dir via symlinks (walkdir does not follow links, but symlinked file targets under repo_dir are still read).",
    "crates/quarry-git/src/lib.rs:1442 — redact_remote_url assumes credentials only appear as userinfo before '@'; scp-style URLs (git@host:path) and URLs with tokens in path/query are logged verbatim.",
    "crates/quarry-git/src/lib.rs:1769 — enforce_delete_safety assumes max_delete_percent from peer config is a meaningful guard; the default of 100 (line 1508) disables it.",
    "crates/quarry-server/src/git_handlers.rs:55 — handlers show no authorization check before accepting arbitrary repo paths/remote URLs; assumes an authn/authz layer upstream of these routes."
  ],
  "trustBoundaries": [
    "crates/quarry-git/src/lib.rs:1476 — stored peer config (attacker-writable JSON via HTTP API) becomes filesystem paths, remote URLs, and refspecs used for destructive and network operations.",
    "crates/quarry-git/src/lib.rs:1362 — untrusted remote git server content reaches the local worktree (clone/fetch + force checkout, line 1383) and is then imported into the canonical document store via write_git_file_to_document (line 785).",
    "crates/quarry-git/src/lib.rs:1078 — untrusted file bytes become YAML frontmatter/sidecar metadata and then stored document metadata; merge_metadata (line 1110) lets sidecar values overwrite frontmatter, including an attacker-controlled content_type that steers is_block_file routing (line 1632).",
    "crates/quarry-git/src/lib.rs:983 — document store to filesystem: DB-held document paths/content are written to an arbitrary caller-chosen repo_dir on export, crossing from internal trust to the host filesystem.",
    "crates/quarry-git/src/lib.rs:1211 — marker verification trusts .quarry/marker.json content (attacker-controllable if they control the worktree) as proof the repo belongs to a library."
  ],
  "hotFiles": [
    "crates/quarry-git/src/lib.rs:1 — the entire crate is one ~1900-line file containing all entry points, path joins, deletion, remote I/O, deserialization, and sync reconciliation; must be read in full.",
    "crates/quarry-server/src/git_handlers.rs:1 — HTTP layer feeding untrusted repo paths/URLs/branch into quarry-git; needed to judge what validation happens upstream.",
    "crates/quarry-core/src/lib.rs:606 — normalize_path, the only path-validation primitive the git layer relies on for document paths.",
    "crates/quarry-storage/src/sync.rs:27 — create_git_peer / list_git_peers: how attacker-supplied peer config is persisted and replayed.",
    "crates/quarry-cli/src/lib.rs:848 — CLI peer-add/import/export surface that assembles the same config JSON."
  ]
}