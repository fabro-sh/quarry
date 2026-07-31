{
  "component": "git-integration",
  "paths": ["crates/quarry-git"],
  "entryPoints": [
    "crates/quarry-git/src/lib.rs:205 push_peer(store, library, peer_id) — public entry; library/peer_id come from API callers",
    "crates/quarry-git/src/lib.rs:270 pull_peer — fetches attacker-controlled remote content then imports it into the store",
    "crates/quarry-git/src/lib.rs:325 sync_peer — bidirectional reconcile driven by remote worktree state",
    "crates/quarry-git/src/lib.rs:825 import_worktree — imports every file of a (possibly remote-fetched) worktree into a library",
    "crates/quarry-git/src/lib.rs:1476 peer_config — peer repo path, branch, remote URL, max_delete_percent read from peer.config JSON (attacker-influencable if peer registration is exposed)",
    "crates/quarry-git/src/lib.rs:1574 worktree_snapshot_blocking — WalkDir over worktree; file paths, bytes, frontmatter/sidecar YAML are untrusted remote data",
    "crates/quarry-git/src/lib.rs:1326 fetch_remote_worktree_blocking — clone/fetch from a remote URL; network input of arbitrary git content"
  ],
  "sinks": [
    "crates/quarry-git/src/lib.rs:1362 RepoBuilder::clone(remote_url, repo_dir) — network fetch of arbitrary remote; no credential callbacks configured on FetchOptions/PushOptions (relies on libgit2 defaults)",
    "crates/quarry-git/src/lib.rs:1333 remote.fetch and remote.push (line 1408) with refspec built from peer-supplied branch via format!",
    "crates/quarry-git/src/lib.rs:1595 fs::read of every worktree file; untrusted bytes flow into put_document/write_block_markdown (lib.rs:1668, lib.rs:802)",
    "crates/quarry-git/src/lib.rs:1036 plan.repo_dir.join(&file.path) — document path from store joined onto repo_dir; traversal risk if a stored path contains .. or is absolute (validation assumed elsewhere)",
    "crates/quarry-git/src/lib.rs:1154 write_atomic — fs::write + fs::rename to worktree paths (also sidecar writer lib.rs:1179)",
    "crates/quarry-git/src/lib.rs:1254 clean_worktree — fs::remove_dir_all/remove_file of everything in repo_dir except .git; destructive if repo path is misconfigured",
    "crates/quarry-git/src/lib.rs:1354 fs::remove_dir(repo_dir) when cloning into an existing empty dir",
    "crates/quarry-git/src/lib.rs:1078 serde_yaml::from_str on untrusted frontmatter (also sidecar YAML lib.rs:1106) — deserialization of remote data",
    "crates/quarry-git/src/lib.rs:1198 serde_json::from_slice on .quarry/marker.json (also lib.rs:1231) — remote-controlled JSON parsed",
    "crates/quarry-git/src/lib.rs:594 store.delete_document driven by git-side file absence — remote can delete Quarry documents (bounded by enforce_delete_safety lib.rs:1769)",
    "crates/quarry-git/src/lib.rs:529 store.move_document from pair_renames (lib.rs:1717) — remote byte content steers document identity moves",
    "crates/quarry-git/src/lib.rs:1596 git2::Oid::hash_object — hashing of untrusted blob content",
    "crates/quarry-git/src/lib.rs:1376 revparse_single on refs/remotes/origin/{branch} built by format! from peer-supplied branch name"
  ],
  "assumptions": [
    "crates/quarry-git/src/lib.rs:1036 Assumes stored document paths are normalized/safe (no '..' or absolute) before repo_dir.join; normalize_path (quarry-core) is only applied on import (lib.rs:1594, lib.rs:902), not export",
    "crates/quarry-git/src/lib.rs:1483 Assumes peer.config 'repo' path and 'remote' URL were validated/authorized at peer registration time; this crate uses them verbatim for clone/push and for destructive clean_worktree",
    "crates/quarry-git/src/lib.rs:1492 Assumes branch name from config is safe for refspec/ref formatting and checkout (no refspec injection validation here)",
    "crates/quarry-git/src/lib.rs:1331 Assumes transport authentication is ambient (SSH agent, credential helpers); FetchOptions::new() sets no credentials or certificate-check callbacks",
    "crates/quarry-git/src/lib.rs:348 Assumes store.run_global_operation actually serializes peer operations as the GIT_BLOCKING_LANE comment claims",
    "crates/quarry-git/src/lib.rs:1579 WalkDir filter excludes only '.git'/'.quarry' by name; assumes symlinked files inside worktree are safe to follow and read (is_file() follows symlinks)",
    "crates/quarry-git/src/lib.rs:299 verify_marker is assumed to bind a worktree to a library, but marker.json itself is remote-controlled content"
  ],
  "trustBoundaries": [
    "crates/quarry-git/src/lib.rs:1362 Internet to local disk: clone/fetch of a remote git repo into repo_dir, then forced checkout (lib.rs:1382-1384) overwrites the local worktree",
    "crates/quarry-git/src/lib.rs:1595 Disk (remote-controlled worktree) to Quarry store: raw bytes + YAML metadata become documents via write_git_file_to_document (lib.rs:785) and write_markdown_file (lib.rs:1644)",
    "crates/quarry-git/src/lib.rs:1099 Remote sidecar '<path>.quarrymeta.yaml' metadata merged over frontmatter metadata (merge_metadata lib.rs:1110) and stored as document metadata, including content_type",
    "crates/quarry-git/src/lib.rs:1029 Quarry store to disk/network: export writes documents to worktree and pushes to remote (lib.rs:1401); data crosses from internal store to internet-facing remote",
    "crates/quarry-git/src/lib.rs:1476 Peer configuration (DB-backed JSON) to filesystem/network operations: config values reach clone, push, remove_dir_all without re-validation",
    "crates/quarry-git/src/lib.rs:1442 redact_remote_url strips userinfo before logging — implicit boundary where credentials in remote URLs must not leak into logs"
  ],
  "hotFiles": [
    "crates/quarry-git/src/lib.rs",
    "crates/quarry-git/tests/git_roundtrip.rs"
  ]
}