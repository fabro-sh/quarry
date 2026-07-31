{
  "job_id": "threat:005-fuse-mount-9a6e73fc",
  "component": "fuse-mount",
  "entryPoints": [
    "crates/quarry-fuse/src/lib.rs:954 — mount_library_with_shutdown: caller-supplied library name and mountpoint path; create_dir_all(mountpoint) creates host directories; falls back from unprivileged to privileged FUSE mount",
    "crates/quarry-fuse/src/lib.rs:1021 — FUSE lookup: kernel-supplied parent Inode + untrusted OsStr name from any local process that can open the mountpoint",
    "crates/quarry-fuse/src/lib.rs:1051 — FUSE setattr: attacker-controlled size, mode, mtime on any inode (truncate, chmod, utimens)",
    "crates/quarry-fuse/src/lib.rs:1083 — FUSE mkdir: untrusted name plus mode/umask",
    "crates/quarry-fuse/src/lib.rs:1103 — FUSE unlink/rmdir: path deletion by name",
    "crates/quarry-fuse/src/lib.rs:1113 — FUSE rename: two untrusted names; rename-over-markdown routes temp content into write_block_markdown (whole-file reconcile)",
    "crates/quarry-fuse/src/lib.rs:1126 — FUSE open: raw caller flags (O_RDONLY vs O_TRUNC vs read-write) decide handle creation and truncation",
    "crates/quarry-fuse/src/lib.rs:1164 — FUSE write: arbitrary bytes, offset, and fh from any local process into write_handle",
    "crates/quarry-fuse/src/lib.rs:1278 — FUSE create: untrusted name creates a stored document and an open handle",
    "crates/quarry-fuse/src/lib.rs:308 — write_handle: fh/offset/data accepted with no cap on offset or total size; content buffer resized to offset+len",
    "crates/quarry-fuse/src/lib.rs:328 — set_handle_len/set_len: untrusted u64 size converted to usize and used in Vec::resize (allocation of attacker-chosen size)",
    "crates/quarry-fuse/src/lib.rs:1143 — FUSE read with fh==0 reads straight from store by inode-derived path; offsets/sizes caller-chosen"
  ],
  "sinks": [
    "crates/quarry-fuse/src/lib.rs:718 — QuarryStore::put_document persists attacker-controlled bytes/path/content_type to storage (commit_handle, create_file, set_len)",
    "crates/quarry-fuse/src/lib.rs:701 — QuarryStore::write_block_markdown: markdown parser/block reconciler invoked on attacker bytes after String::from_utf8 at line 695 (also rename path line 492, set_len line 614)",
    "crates/quarry-fuse/src/lib.rs:573 — QuarryStore::delete_document via unlink; delete after rename-over at line 497",
    "crates/quarry-fuse/src/lib.rs:502 — replace_document / move_document / move_directory: rename mutates stored paths, including bulk move of every document under a prefix (lines 522-540)",
    "crates/quarry-fuse/src/lib.rs:317 — Vec::resize to attacker-controlled offset and offset+data.len(): memory-allocation sink (potential OOM/DoS via huge sparse writes)",
    "crates/quarry-fuse/src/lib.rs:334 — Vec::resize in set_handle_len / set_len (line 603) with u64->usize size: same allocation sink",
    "crates/quarry-fuse/src/lib.rs:964 — tokio::fs::create_dir_all(mountpoint): host filesystem write at caller-provided path",
    "crates/quarry-fuse/src/lib.rs:979 — fuse3 Session mount/mount_with_unprivileged: uses fusermount3 SUID helper and privileged mount fallback",
    "crates/quarry-fuse/src/lib.rs:435 — update_directory_metadata: attacker-chosen mode/mtime persisted",
    "crates/quarry-fuse/src/lib.rs:1465 — chrono timestamp conversion from kernel Timestamp and RFC3339 parse of stored mtime (timestamp_from_rfc3339 line 1471)",
    "crates/quarry-fuse/src/lib.rs:270 — String::from_utf8 on stored document content loaded wholesale into memory per open handle"
  ],
  "assumptions": [
    "crates/quarry-fuse/src/lib.rs:884 — normalize_mount_path delegates all traversal/backslash/reserved-name defense to quarry_core::normalize_path (crates/quarry-core/src/lib.rs:606); assumed to reject '..', '.', empty segments, '.quarry'",
    "crates/quarry-fuse/src/lib.rs:1416 — join_child_path assumes to_str() UTF-8 conversion plus '.'/'..'/'/' rejection is sufficient; non-UTF-8 names become EINVAL",
    "crates/quarry-fuse/src/lib.rs:970 — default_permissions(false): assumes mount-level access control is enforced elsewhere (mountpoint ownership/allow_other); uid/gid echoed from req (line 1458) with no ownership check, so the store is assumed single-tenant",
    "crates/quarry-fuse/src/lib.rs:1319 — path_for_inode trusts store.inode_for_path/path_for_inode mapping; assumes inodes cannot be guessed to reach paths outside the library namespace",
    "crates/quarry-fuse/src/lib.rs:300 — read_handle/write_handle trust the numeric fh issued by this process; no fh-vs-inode binding check in write at line 1164",
    "crates/quarry-fuse/src/lib.rs:695 — markdown writes assume String::from_utf8 gating plus the storage-layer Phase 4 writer fully validates CriticMarkup and frontmatter",
    "crates/quarry-fuse/src/lib.rs:70 — assumes list_documents caps (Some(10_000)) and store-side path validation bound enumeration and bulk-rename work",
    "crates/quarry-fuse/src/lib.rs:458 — rename's head_document(...).is_ok() probe assumes NotFound is the only 'absent' signal; other errors silently take the directory branch"
  ],
  "trustBoundaries": [
    "crates/quarry-fuse/src/lib.rs:1012 — kernel VFS / any local process -> fuse3 Filesystem impl: all FUSE callbacks cross from unprivileged local callers into the Quarry store identity",
    "crates/quarry-fuse/src/lib.rs:1417 — OsStr (arbitrary kernel bytes) -> Rust str -> normalized store path: byte-to-UTF-8 boundary with EINVAL fallback",
    "crates/quarry-fuse/src/lib.rs:679 — commit_handle: in-memory handle buffer (untrusted bytes) -> QuarryStore write API (trusted persistence layer) with DocumentSource::Fuse provenance",
    "crates/quarry-fuse/src/lib.rs:842 — watch_store_events: store broadcast events -> FUSE invalidation state (store-to-mount feedback loop)",
    "crates/quarry-fuse/src/lib.rs:979 — process -> kernel via fusermount3/privileged mount: mount execution boundary on the host"
  ],
  "hotFiles": [
    "crates/quarry-fuse/src/lib.rs — entire crate (1517 lines): projection logic lines 64-913 (handles, rename, commit, path helpers) and linux_mount module 936-1516 (kernel-facing callbacks, join_child_path, to_errno)",
    "crates/quarry-core/src/lib.rs:606 — normalize_path / parent_dirs: the sole path-validation guarantee the mount relies on",
    "crates/quarry-fuse/Cargo.toml — dependency surface (fuse3, libc, chrono, mime_guess) and feature flags",
    "crates/quarry-storage — QuarryStore::{put_document, write_block_markdown, delete_document, replace_document, move_document, move_directory, inode_for_path, path_for_inode} and the CLI/daemon callers of mount_library_with_shutdown (mountpoint, read_only, mount options) needed to judge what validation happens past the boundary"
  ]
}