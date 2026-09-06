# Linux FUSE Notes

Quarry phase one targets Linux FUSE only. macOS and Windows filesystem support are out of scope.

The `quarry-fuse` crate mounts a Library through `fuse3` on Linux. The projection is over committed Quarry documents; directories are inferred from document paths and persisted through Quarry `dir_metadata` when created empty. The Linux mount uses `fuse3`'s inode-based API, and projection attributes use the storage `inodes` table so document and directory inode identities are stable across reopen and rename.

Supported operations include lookup/getattr, directory listing, open/read, create/write/flush/release auto-commit, mkdir, rename, unlink, rmdir, and truncate via setattr or `O_TRUNC` open. Directory mode and mtime updates from Linux `setattr` are stored in Quarry `dir_metadata` and survive reopening the projection. An open file handle retains its document identity and original read version. Its writes merge with concurrent Markdown edits. A rename preserves that identity; deletion rejects further publication and keeps the failed buffer in the projection. Writes that arrive during a flush remain pending for the next flush or close.

Renaming a temporary file over an existing Markdown document does not carry the target version used to prepare that file. Quarry rejects a changed replacement and retains both files. An identical replacement is a no-op. Use an editor that saves through an open handle, or a versioned HTTP/CLI write. Raw-file replacements retain ordinary filesystem behavior.

The storage layer publishes in-process events for visible document and directory writes. FUSE projections subscribe to those events and advance an invalidation generation for the mounted Library, so REST, Git, and FUSE publications share a common cache-invalidation channel.

Mount with:

```sh
cargo run -p quarry -- mount <library> <mountpoint>
```

Add `--read-only` for a read-only projection.

To expose REST and FUSE from the same Quarry process, add `--serve-addr`:

```sh
cargo run -p quarry -- mount <library> <mountpoint> --serve-addr 127.0.0.1:7831
```

Use this mode when REST writes must be visible through the mount, or FUSE writes must be read back through REST, without starting a second database owner.

Install prerequisites:

- Linux with FUSE support enabled.
- `fuse3` / `fusermount3` installed for unprivileged mounts.
- Permission for the current user to mount FUSE filesystems.

Limitations:

- Multi-file atomic edits must use REST transactions, not FUSE.
- `.quarry/` is reserved and rejected.
- macOS and Windows mounts are intentionally unsupported.

The Linux conformance test mounts real FUSE and runs kernel file operations concurrently with Git peer sync and HTTP edits. Run it with `/dev/fuse` and mount permission:

```sh
cargo test --locked -p quarry-fuse --test mixed_interfaces kernel_mounted -- --ignored
```
