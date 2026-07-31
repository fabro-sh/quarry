I've completed the review of `crates/quarry-fuse` under the authentication/authorization lens. I read `crates/quarry-fuse/src/lib.rs` in full, the canonical path validator (`crates/quarry-core/src/lib.rs:606`), the store-side inode mapping (`crates/quarry-storage/src/directories.rs:305-343`), the inode allocator, and the CLI mount wiring. Every candidate was traced to either an effective defense or a design property with no cross-principal attack path:

- **No in-FS permission checks** (`default_permissions(false)` at lib.rs:970, `access()` always Ok at :1298, uid/gid spoofed to requester at :1458): a repo-wide grep confirms `allow_other`/`allow_root` are never set, so fusermount3 (and the privileged fallback) restricts the mount to the mounting user — the kernel defense the design assumes actually holds; no other user can reach the mountpoint.
- **Read-only bypass**: every mutating op routes through `ensure_writable` (verified individually: create_file:221, open_file_for_write:267/:283, write_handle:309, set_handle_len:329, mkdir:390, set_directory_metadata:425, rename:448, unlink:568, rmdir:580, set_len:599); `commit_handle`/`flush`/`release` can only commit a dirty handle created by a guarded op; kernel MS_RDONLY is a second layer.
- **Traversal / `.quarry` escape**: all kernel names funnel through `join_child_path` (:1416, rejects `.`/`..`/`/`, non-UTF-8) then `normalize_path` (quarry-core:606, rejects `..`, backslash, root `.quarry`); rename/move destinations pass the same funnel.
- **Inode IDOR**: `path_for_inode` SQL is scoped by `library_id` (directories.rs:332), so an inode from another library cannot resolve; inodes only enter via the kernel from prior lookups.
- **TOCTOU in create/rename**: store-side `WritePrecondition::IfNoneMatch`/`IfMatch` (:246, :632, :714) turn lost races into errors, not unauthorized mutations.
- **Privilege escalation via mode bits**: file-mode setattr is silently ignored (:1066-1075); no mknod/symlink/setxattr ops exist.
- CSRF/SSRF/open-redirect: no HTTP surface exists in this component.
- `access()` returning Ok for `W_OK` on read-only mounts is misleading but has no exploit path — every actual mutation is independently blocked by the kernel and `ensure_writable`; it is a best-practice nit, not a finding.

```json
{
  "job_id": "research:005-fuse-fs-120d3a54:auth-and-access",
  "component": "fuse-fs",
  "lens": "authentication and authorization: auth bypass, missing or wrong authorization checks, IDOR, privilege escalation, CSRF, SSRF, open redirect, and race conditions in access decisions",
  "findings": []
}
```