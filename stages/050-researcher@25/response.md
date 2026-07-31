{
  "job_id": "research:005-fuse-fs-120d3a54:crypto-and-secrets",
  "category": "crypto-and-secrets",
  "title": "fuse-fs crypto-and-secrets review: two unbounded-allocation DoS paths on the FUSE write/truncate hot path",
  "rationale": "The quarry-fuse component contains no cryptography, secrets, credentials, or security-relevant randomness: inodes are per-library sequential counters scoped by library_id in quarry-storage (path_for_inode filters WHERE library_id = ?1, so no cross-library resolution), no keys/tokens/passwords are handled, and to_fuse_attr's synthesized uid/gid is backstopped by the fusermount3 default (no allow_other is set), so weak-crypto, hardcoded-secret, weak-randomness, timing-side-channel, and credential-exposure candidates all dead-end. The only complete attacker source-to-sink paths found under the exposure side of this lens are two unbounded in-memory allocation sinks driven by attacker-controlled u64 values from the kernel: write_handle resizes a Vec to an arbitrary pwrite offset (lib.rs:317) and set_handle_len/set_len resize to an arbitrary ftruncate size (lib.rs:334/:603), with usize::try_from as the only guard (ineffective on 64-bit) and no per-file or global size cap. Both let any local process able to reach the mountpoint OOM-kill or abort the quarry daemon, which also hosts the embedded HTTP/collab server in the same process.",
  "findings": [
    {
      "category": "crypto-and-secrets",
      "title": "Unbounded sparse-write offset grows in-memory Vec, crashing the quarry daemon",
      "rationale": "A local process on the mountpoint fully controls the u64 offset passed to write_handle via pwrite(2); the only guard (usize::try_from) accepts every 64-bit value, MAX_WRITE_BYTES caps only per-request data length, and no per-file or global size limit exists, so handle.content.resize(offset, 0) zero-fills attacker-chosen gigabytes-to-terabytes and the daemon is OOM-killed or aborts, taking the embedded HTTP/collab server down with it.",
      "file": "crates/quarry-fuse/src/lib.rs",
      "line": 317,
      "symbol": "write_handle",
      "ruleId": "dos.unbounded-allocation",
      "identity": {
        "anchor": "sparse-write-offset-buffer-growth"
      },
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "HIGH",
      "impact": "Any local process able to reach the mountpoint can crash the quarry daemon with a single pwrite(2) at a huge offset: write_handle zero-fills an in-memory Vec up to the attacker-chosen u64 offset, so the process is OOM-killed or aborts on allocation failure. Because Command::Mount shares one process with the optional embedded HTTP/collab server (crates/quarry-cli/src/lib.rs:516-533), the crash also terminates the network-facing REST surface and the FUSE mount for every client.",
      "evidence": [
        "crates/quarry-fuse/src/lib.rs:1164: the FUSE `write` handler receives `offset: u64` and `data` straight from the kernel, i.e. from any local process that can reach the mountpoint, and forwards them to `self.write_handle(fh, offset, data)` with no validation.",
        "crates/quarry-fuse/src/lib.rs:1126: `open` hands a write handle (fh) to any caller on the mount; `access` at :1298 returns Ok for every mask and the mount is set up with `default_permissions(false)` at :970, so the projection itself performs no permission or size policy checks.",
        "crates/quarry-fuse/src/lib.rs:314: the only guard on the attacker-controlled offset is `usize::try_from(offset)`, which on 64-bit Linux accepts every u64 up to 2^64-1 (the kernel's own s_maxbytes cap, ~8 EiB, still permits offsets of many terabytes), so the guard is ineffective.",
        "crates/quarry-fuse/src/lib.rs:316: `if handle.content.len() < offset` compares the current buffer length against the attacker-chosen offset with no per-file, per-handle, or global cap; `MAX_WRITE_BYTES` (:951) limits only the per-request data length, never the offset.",
        "crates/quarry-fuse/src/lib.rs:317: `handle.content.resize(offset, 0)` grows the in-memory Vec to the attacker-chosen offset and zero-fills every new page, so the memory is actually touched: the process is OOM-killed, or aborts in the Rust allocator when the reservation fails.",
        "crates/quarry-cli/src/lib.rs:505: the mount and the embedded server share one store and one process, so the allocator abort/OOM triggered via the mount takes down the REST/collab server too."
      ],
      "snippet": "            handle.content.resize(offset, 0);",
      "exploitScenarios": [
        "Mount a library read-write as an unprivileged user (or run as any user able to reach the mountpoint).",
        "Open any document in the mount for writing, e.g. `fd = open('/mnt/quarry/doc.md', O_WRONLY)`.",
        "Issue one sparse write at a huge offset, e.g. `pwrite(fd, \"x\", 1, 1<<42)` (4 TiB offset, below the kernel's s_maxbytes so it reaches the daemon).",
        "write_handle resizes the handle's Vec to 4 TiB and zero-fills it; the quarry process exhausts memory and is OOM-killed or aborts.",
        "The FUSE mount and the embedded HTTP/collab server in the same process die, denying service to all users of that daemon."
      ],
      "preconditions": [
        "The library is mounted read-write (read_only mounts reject the write at ensure_writable).",
        "The attacker controls a local process with access to the mountpoint (by default the mounting user, e.g. an agent granted document access through the mount).",
        "The target document path exists in the store so open-for-write succeeds."
      ],
      "recommendations": [
        "Enforce a maximum document size in write_handle (and its siblings) before resizing: reject any offset or offset+data.len() above a configured cap (e.g. the store's existing PayloadTooLarge limit) with QuarryError::PayloadTooLarge -> EFBIG instead of growing the buffer.",
        "Hardening: apply the same cap at open time (document content loaded fully into memory) and account total memory across all open handles, so many concurrent sparse writes cannot exhaust memory below the per-file cap.",
        "Regression test: open a document for write and call write_handle with an offset above the cap (and set_handle_len with a huge size); assert an error is returned and the handle buffer length is unchanged."
      ]
    },
    {
      "category": "crypto-and-secrets",
      "title": "Unbounded ftruncate/truncate size resizes in-memory Vec, crashing the quarry daemon",
      "rationale": "A local process on the mountpoint fully controls the u64 size passed through the FUSE setattr handler to set_handle_len (open file) or set_len (path truncate); the only guard (usize::try_from) accepts every 64-bit value and no size cap exists anywhere on the path, so the in-memory Vec is resized and zero-filled to an attacker-chosen size, OOM-killing or aborting the daemon that also hosts the embedded HTTP/collab server.",
      "file": "crates/quarry-fuse/src/lib.rs",
      "line": 334,
      "symbol": "set_handle_len",
      "ruleId": "dos.unbounded-allocation",
      "identity": {
        "anchor": "truncate-size-buffer-growth"
      },
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "HIGH",
      "impact": "Any local process able to reach the mountpoint can crash the quarry daemon with a single ftruncate(2)/truncate(2) to a huge size: set_handle_len (and the no-fh sibling set_len at :598) resizes an in-memory Vec to the attacker-chosen u64 size, zero-filling it, so the process is OOM-killed or aborts. The crash takes down the FUSE mount and the embedded HTTP/collab server sharing the process (crates/quarry-cli/src/lib.rs:516-533).",
      "evidence": [
        "crates/quarry-fuse/src/lib.rs:1059: the FUSE `setattr` handler takes the kernel-supplied `set_attr.size` — an attacker-controlled u64 from ftruncate/truncate issued by any local process on the mountpoint — and routes it to `set_handle_len(fh, size)` when the file is open (:1061) or `set_len(&path, size)` otherwise (:1063), with no size validation.",
        "crates/quarry-fuse/src/lib.rs:334: `handle.content.resize(usize::try_from(size)..., 0)` grows the open handle's in-memory Vec to the attacker-chosen size; the `usize::try_from` guard is ineffective on 64-bit because every u64 up to 2^64-1 converts, and no per-file or global size cap exists.",
        "crates/quarry-fuse/src/lib.rs:337: the resize value `0` means every newly allocated page is zero-filled and therefore touched, so the allocation consumes real memory and triggers the OOM killer or an allocator abort rather than a cheap overcommit reservation.",
        "crates/quarry-fuse/src/lib.rs:603: the sibling no-fh path `set_len` loads the full document via `get_document` (:601) and applies the same unbounded `document.content.resize(size, 0)` before any store interaction, so truncate(2) on a path reaches the identical sink.",
        "crates/quarry-fuse/src/lib.rs:1066: the only other guards in setattr concern mode/mtime metadata; nothing bounds `size`, and `QuarryError::PayloadTooLarge` (mapped to EFBIG at :1501) is never raised on this path."
      ],
      "snippet": "        handle.content.resize(\n            usize::try_from(size)\n                .map_err(|_| QuarryError::InvalidPath(\"file size too large\".to_string()))?,\n            0,\n        );",
      "exploitScenarios": [
        "Mount a library read-write and open any document, e.g. `fd = open('/mnt/quarry/doc.md', O_WRONLY)`.",
        "Call `ftruncate(fd, 1<<42)` (4 TiB, below the kernel's s_maxbytes so it reaches the daemon); alternatively `truncate('/mnt/quarry/doc.md', 1<<42)` on the path hits set_len.",
        "set_handle_len/set_len resizes the in-memory Vec to 4 TiB and zero-fills it.",
        "The quarry process exhausts memory and is OOM-killed or aborts in the allocator.",
        "The FUSE mount and the embedded HTTP/collab server in the same process die for all clients."
      ],
      "preconditions": [
        "The library is mounted read-write (ensure_writable rejects on read-only mounts).",
        "The attacker controls a local process with access to the mountpoint (by default the mounting user).",
        "For the set_handle_len path the file must be open; for the set_len path the document must exist in the store."
      ],
      "recommendations": [
        "Enforce a maximum document size in setattr before calling set_handle_len/set_len (and inside both functions): reject sizes above a configured cap with QuarryError::PayloadTooLarge -> EFBIG, reusing the store's existing payload limit as the single source of truth.",
        "Hardening: bound total memory held by all open handles, not just per-file size, so many concurrent truncates cannot collectively exhaust memory.",
        "Regression test: call set_handle_len and set_len with a size above the cap and assert an error is returned without the buffer being resized."
      ]
    }
  ]
}