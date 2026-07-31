{
  "job_id": "threat:005-fuse-fs-120d3a54",
  "component": "fuse-fs",
  "mode": "scan",
  "entryPoints": [
    "crates/quarry-fuse/src/lib.rs:954 mount_library_with_shutdown: mounts the store as a FUSE filesystem; invoked by CLI mount at crates/quarry-cli/src/lib.rs:518 and :535. Local processes become the untrusted input source via the kernel.",
    "crates/quarry-fuse/src/lib.rs:1021 Filesystem::lookup(parent inode, name): untrusted filename enters via child_path -> join_child_path.",
    "crates/quarry-fuse/src/lib.rs:1051 setattr: kernel-supplied size/mode/mtime drive set_handle_len/set_len and set_directory_metadata.",
    "crates/quarry-fuse/src/lib.rs:1083 mkdir: untrusted name + mode from local process.",
    "crates/quarry-fuse/src/lib.rs:1103 unlink / :1108 rmdir: untrusted name resolved to a store path.",
    "crates/quarry-fuse/src/lib.rs:1113 rename: two untrusted names -> from_path/to_path driving move/replace/delete and directory recursion.",
    "crates/quarry-fuse/src/lib.rs:1126 open(inode, flags): read vs write/truncate branch; allocates an in-memory handle holding full document content.",
    "crates/quarry-fuse/src/lib.rs:1143 read(inode, fh, offset, size): kernel-controlled offset/size reach read_slice.",
    "crates/quarry-fuse/src/lib.rs:1164 write(fh, offset, data): untrusted bytes and offset reach write_handle buffer growth.",
    "crates/quarry-fuse/src/lib.rs:1278 create(parent, name): creates a store document from an untrusted filename.",
    "crates/quarry-fuse/src/lib.rs:1319 path_for_inode: kernel-supplied u64 inode is the second untrusted handle namespace, resolved via store.path_for_inode.",
    "crates/quarry-fuse/src/lib.rs:1327 child_path / :1416 join_child_path: sole filename filter (rejects '.', '..', '/', non-UTF-8) then normalize_mount_path; all kernel path input funnels here."
  ],
  "sinks": [
    "crates/quarry-fuse/src/lib.rs:308 write_handle: handle.content.resize(offset, 0) then copy_from_slice — kernel-controlled u64 offset grows an in-memory Vec with no per-file or per-handle size cap (memory exhaustion).",
    "crates/quarry-fuse/src/lib.rs:328 set_handle_len: Vec::resize to attacker-chosen u64 size — same memory-exhaustion sink.",
    "crates/quarry-fuse/src/lib.rs:598 set_len: loads the full document into memory and resizes to a u64 size before put_document.",
    "crates/quarry-fuse/src/lib.rs:679 commit_handle: UTF-8 validation then write_block_markdown (diff3 reconcile) or put_document with WritePrecondition; every FUSE write persists to QuarryStore here as DocumentSource::Fuse.",
    "crates/quarry-fuse/src/lib.rs:447 rename: replace_document, file-over-markdown delete+write (:485-499), and directory rename loops move_document per descendant (:522-540) — multi-document mutation from one untrusted rename pair.",
    "crates/quarry-fuse/src/lib.rs:567 unlink -> store.delete_document; :579 rmdir -> store.remove_directory; :389 mkdir -> store.ensure_directory + update_directory_metadata.",
    "crates/quarry-fuse/src/lib.rs:915 read_slice: bounds-clamped copy out of document content; offset converted with unwrap_or(usize::MAX).",
    "crates/quarry-fuse/src/lib.rs:979 Session::mount_with_unprivileged / :985 mount: invokes fusermount3 helper / privileged mount on a caller-supplied mountpoint.",
    "crates/quarry-fuse/src/lib.rs:964 tokio::fs::create_dir_all(mountpoint): filesystem write I/O on a caller-supplied path.",
    "crates/quarry-fuse/src/lib.rs:928 content_type_for_path: mime_guess on an untrusted filename selects markdown-reconcile vs raw put path (is_block_document, :924)."
  ],
  "assumptions": [
    "crates/quarry-fuse/src/lib.rs:970 default_permissions(false) plus access() at :1298 returning Ok for every mask: the projection performs no permission checks itself and assumes kernel/mountpoint DAC (fusermount3 default: mounting user only, no allow_other) restricts who can reach it. With allow_other or a root mount, every local user gets full read/write with uid/gid spoofed to the requester (:1458).",
    "crates/quarry-fuse/src/lib.rs:884 normalize_mount_path delegates all traversal/backslash/reserved-name rejection to quarry_core::normalize_path (crates/quarry-core/src/lib.rs:606) and assumes the storage layer re-validates paths on its side.",
    "crates/quarry-fuse/src/lib.rs:224 create_file path_exists check is a TOCTOU against concurrent writers via other surfaces (REST/CLI/git); assumes store-side WritePrecondition::IfNoneMatch enforces the conflict.",
    "crates/quarry-fuse/src/lib.rs:447 rename assumes store move_document/replace_document reject escapes and .quarry collisions; this crate validates destinations only via normalize_mount_path.",
    "crates/quarry-fuse/src/lib.rs:1319 assumes store.path_for_inode/inode_for_path mapping is stable and scoped to the mounted library; no check that a resolved inode belongs to this library's tree before serving I/O.",
    "crates/quarry-fuse/src/lib.rs:154 list_documents capped at 10_000: assumes libraries stay below that; truncation silently drops entries from readdir and from the directory-rename recursion (:525), leaving unmoved documents.",
    "crates/quarry-fuse/src/lib.rs:748 ensure_writable is the only read-only enforcement; assumes every mutating op routes through it (setattr mode/mtime on files is silently ignored rather than rejected, :1066-1075)."
  ],
  "trustBoundaries": [
    "crates/quarry-fuse/src/lib.rs:1012 impl Filesystem for FuseProjection: the kernel/userspace boundary — any local process that can reach the mountpoint crosses into the document store with the mounting user's full store privileges; no caller authentication or per-UID policy inside the crate.",
    "crates/quarry-fuse/src/lib.rs:1429 to_fuse_attr reports uid/gid as req.uid/req.gid with synthesized perms (0o644/0o555), masking real backing permissions and presenting stored content as owned by the reader.",
    "crates/quarry-fuse/src/lib.rs:679 commit_handle: bytes from an arbitrary local process cross into the versioned store as committed transactions attributed to source FUSE, reconciled into markdown blocks and broadcast via store events (watch_store_events, :842).",
    "crates/quarry-cli/src/lib.rs:505 Command::Mount shares one store/SessionHub with an optional embedded HTTP server (serve_addr, :529), so FUSE writes immediately cross into the network-facing REST/collab surface.",
    "crates/quarry-core/src/lib.rs:611 normalize_path rejects .quarry/: boundary between the mount-visible namespace and Quarry's internal metadata namespace."
  ],
  "hotFiles": [
    "crates/quarry-fuse/src/lib.rs — entire component: projection logic (:38-809), path helpers (:884-933), Linux fuse3 Filesystem impl (:936-1517); read in full.",
    "crates/quarry-core/src/lib.rs:606 — normalize_path, the canonical path validator all FUSE path safety depends on; also parent_dirs (:630).",
    "crates/quarry-cli/src/lib.rs:499 — Command::Mount wiring: how mountpoint/library/read_only and the embedded server reach the mount.",
    "crates/quarry-fuse/tests/projection.rs — existing projection tests revealing intended invariants (read-only mode, rename semantics, truncate) against which gaps can be judged."
  ]
}