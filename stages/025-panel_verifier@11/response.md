```json
{
  "job_id": "panel:F2:reachability",
  "candidate_id": "F2",
  "verdict": "TRUE_POSITIVE",
  "lens": "REACHABILITY",
  "reasoning": "Every element of the reachability chain was verified in the source. (1) Attacker-controlled source: POST /v1/libraries/{library}/git/export is registered with no auth middleware at crates/quarry-server/src/lib.rs:464-467; the entire router (crates/quarry-server/src/lib.rs:196-219) carries only api_error_envelope, request_tracing, and security_headers middleware — no authentication, authorization, or Host/Origin validation anywhere in the crate (the only auth-related code is the explicit 'phase one has no auth' warning at lib.rs:695). (2) GitExportRequest.repo is a free-form caller string (crates/quarry-server/src/git_handlers.rs:108-114) and handler git_export passes it unvalidated as std::path::Path::new(&request.repo) at git_handlers.rs:132 (exact match to the quoted snippet and claimed line) into quarry_git::export_worktree. (3) The sink is as destructive as claimed: execute_worktree_export (crates/quarry-git/src/lib.rs:1029-1058) runs fs::create_dir_all (1030), verify_or_write_marker (1031; per 1195-1209 it only refuses when a marker already exists naming a different library, otherwise silently writes one), then clean_worktree (1032; defined 1254-1269) which fs::remove_dir_all/fs::remove_file's every directory entry except .git, then writes document files via repo_dir.join(&file.path) and write_atomic (1036-1047). (4) Attacker influence over file paths/contents is reachable: POST /v1/libraries create_library (lib.rs:374-376) and PUT /v1/libraries/{library}/documents/{*path} (lib.rs:407-414) are equally unauthenticated, and the only path guard, quarry_core::normalize_path (crates/quarry-core/src/lib.rs:606-628, applied at crates/quarry-storage/src/documents.rs:84), rejects '..', backslash, and the .quarry/ prefix but accepts dotfile paths like .ssh/authorized_keys, so the export can plant that file into an attacker-chosen home directory after wiping it. (5) Deployment reachability: the default bind is loopback 127.0.0.1:7831 (crates/quarry-cli/src/lib.rs:261) with no Host/Origin check, so a same-origin DNS-rebinding page the operator visits can reach the listener; binding non-loopback produces only a stderr warning (warn_if_non_loopback, lib.rs:692-704). A second unauthenticated path to the same sink exists via stored peer configs: push_peer_inner (crates/quarry-git/src/lib.rs:215-245) calls export_worktree on the unvalidated peer repo. Attempted refutation under the REACHABILITY lens failed: the source is genuinely attacker-controlled, no route on the path has a guard, and no canonicalization, containment check, allowlist, or auth gate intervenes between the HTTP request and the recursive delete/write. The marker check is a same-library guard, not a containment defense, and the 'phase one, no auth by design' posture does not neutralize the finding as written, which describes exactly this unauthenticated arbitrary-directory wipe/overwrite primitive.",
  "decisive_evidence": [
    "crates/quarry-server/src/git_handlers.rs:132",
    "crates/quarry-server/src/lib.rs:196-219",
    "crates/quarry-server/src/lib.rs:464-467",
    "crates/quarry-git/src/lib.rs:1029-1058",
    "crates/quarry-git/src/lib.rs:1254-1269",
    "crates/quarry-git/src/lib.rs:1195-1209",
    "crates/quarry-core/src/lib.rs:606-628",
    "crates/quarry-cli/src/lib.rs:261",
    "crates/quarry-server/src/lib.rs:692-704"
  ]
}
```