Verification complete. My assessment:

**What I checked under the auth/authz lens in `crates/quarry-core`:**

1. **`CollabInviteToken` (lib.rs:399)** — traced the full data flow across the boundary: minted in `quarry-storage/src/documents.rs:290` (UUIDv4 id, role normalized to viewer/editor), but never validated anywhere — no redeem endpoint, no `revoked_at`/`role` check in any access decision (`collab_handlers.rs:19-31` upgrades the socket with no token parameter at all). However, the missing check lives in `quarry-server`, not this component, and the repository documents an explicit phase-one posture of "no auth" (`quarry-server/src/lib.rs:692-706` warns on non-loopback binds; `discovery.rs:256` declares `auth.mode: "trusted_localhost"`; `session.rs:68` documents the unauthenticated collab socket). Within that declared trust boundary there is no privilege boundary to cross, and there is no root control in `quarry-core` to anchor this to — the DTO definition itself is not a vulnerable control.

2. **`WritePrecondition` (lib.rs:311)** — traced from `If-Match`/`If-None-Match` header parsing (`headers.rs:123-168`) to enforcement: `check_precondition_conn` runs *inside* the `BEGIN IMMEDIATE` write transaction (`documents.rs:99-105`, `store.rs:252-278`), and the block-document path re-verifies the head in-transaction at commit (`blocks.rs:1165-1175`). The advisory pre-checks outside the transaction (`markdown_write.rs:235-283`) are not the enforced decision. No TOCTOU.

3. **`normalize_path` (lib.rs:606)** — all consumers across storage/git/fuse route untrusted paths through it; tmp routes use the secret-capability substitute. The gaps I probed (case-variant `.QUARRY`, NUL bytes, nested `.quarry`) have no complete attack path: NUL is rejected downstream by Rust fs APIs, and the case-collision scenario depends on a non-default case-insensitive filesystem with impact bounded to local metadata corruption — speculative, not panel-grade.

4. **Identifier newtypes (lib.rs:16-97), `#[serde(default)]` Option fields** — no authz decision branches on these fields anywhere; SQL access uses bound parameters; no concrete injection or confused-deputy path verified.

No complete source-to-sink path with an ineffective defense exists within this component under this lens.

```json
{
  "job_id": "research:009-core-shared-bcbb015d:auth-and-access",
  "component": "core-shared",
  "lens": "authentication and authorization: auth bypass, missing or wrong authorization checks, IDOR, privilege escalation, CSRF, SSRF, open redirect, and race conditions in access decisions",
  "findings": [],
  "summary": "Adversarial review of crates/quarry-core (full 724-line lib.rs read) under the authentication/authorization lens, with cross-boundary tracing into quarry-server, quarry-storage, quarry-git, and quarry-fuse. No panel-verified findings. (1) CollabInviteToken (lib.rs:399) is minted but never validated by any consumer — however the absent check resides in quarry-server, not this component, and the repository explicitly documents a phase-one no-auth, trusted-loopback posture (quarry-server/src/lib.rs:692-706, discovery.rs:256, session.rs:68), so no privilege boundary exists to cross and no root control in quarry-core anchors the issue. (2) WritePrecondition (lib.rs:311) is enforced atomically inside the BEGIN IMMEDIATE SQLite write transaction (documents.rs:99-105, store.rs:252-278) and re-verified in-transaction at block-commit time (blocks.rs:1165-1175); out-of-transaction pre-checks are advisory only — no TOCTOU. (3) normalize_path (lib.rs:606) is applied on every untrusted-path flow into storage, git, and FUSE; probed bypasses (case-variant .QUARRY, NUL bytes, nested .quarry) yield no complete attack path — NUL is rejected by downstream fs APIs and the case-collision scenario requires a non-default case-insensitive filesystem with bounded local impact. (4) Identifier newtypes and #[serde(default)] Option fields feed no authorization decision; no injection or confused-deputy path verified. Empty findings array reflects verified absence, not incomplete coverage."
}
```