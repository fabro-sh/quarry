{
  "job_id": "research:005-fuse-mount-9a6e73fc:crypto-and-secrets:2",
  "findings": [
    {
      "category": "dos",
      "title": "Unbounded in-memory buffer growth from attacker-controlled FUSE write offset and truncate size",
      "rationale": "The FUSE write and setattr callbacks forward an attacker-chosen u64 offset/size into Vec::resize with no maximum file size enforced anywhere on the path; a single pwrite or ftruncate from any process running as the mounting user forces a multi-terabyte allocation that aborts or OOM-kills the daemon hosting both the mount and the embedded server. This is a complete source-to-sink path, not a hardening note: every guard on the path (read_only flag, fh existence, usize conversion, 1 MiB per-request data cap) was checked and none bounds the allocation.",
      "file": "crates/quarry-fuse/src/lib.rs",
      "line": 317,
      "symbol": "FuseProjection::write_handle",
      "ruleId": "dos.unbounded-allocation",
      "identity": {
        "anchor": "fuse-handle-buffer-growth"
      },
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "HIGH",
      "impact": "Any local process running as the mounting user can crash the quarry daemon with a single pwrite or ftruncate syscall: a huge offset forces Vec::resize to attempt a multi-terabyte allocation, which aborts the process on allocation failure or triggers the OOM killer. Because the CLI Mount command hosts the embedded HTTP/WebSocket server in the same process (crates/quarry-cli/src/lib.rs:516-533), this kills the mount and the server, denying service to every connected client.",
      "evidence": [
        "crates/quarry-fuse/src/lib.rs:1164 — the FUSE write callback receives offset:u64 and data from the kernel on behalf of any local process that opened a file on the mountpoint, and passes both unchanged to write_handle (lines 1174-1177).",
        "crates/quarry-fuse/src/lib.rs:1126 — open() issues a numeric fh to any caller with a writable acmode; the only access barrier is that the mount sets no allow_other (lines 966-970), so the attacker merely needs to run as the mounting user, the exact trust boundary the threat model declares untrusted.",
        "crates/quarry-fuse/src/lib.rs:308 — write_handle's guards are ensure_writable (checks only the mount-wide read_only flag, lines 748-753) and fh existence in the handles map; neither constrains offset or data size.",
        "crates/quarry-fuse/src/lib.rs:314 — usize::try_from(offset) rejects only offsets above usize::MAX, so on 64-bit every attacker-chosen u64 offset passes.",
        "crates/quarry-fuse/src/lib.rs:316-317 — sink: handle.content.resize(offset, 0) grows the in-memory buffer to the attacker offset; MAX_WRITE_BYTES (lines 951-952) caps only per-request data length at 1 MiB, never the offset or total file size, so one pwrite(fd, buf, 1, 1<<40) forces a ~1 TiB allocation.",
        "crates/quarry-fuse/src/lib.rs:1059-1061 — the same missing cap is reachable via setattr: a caller-supplied size with an fh is routed to set_handle_len.",
        "crates/quarry-fuse/src/lib.rs:334-338 — set_handle_len resizes the handle buffer to the attacker-chosen u64 size after only a usize conversion check, so ftruncate(fd, HUGE) hits the identical allocation sink.",
        "crates/quarry-fuse/src/lib.rs:603-607 — set_len (path-level truncate, setattr without fh) applies the same unbounded Vec::resize to the loaded document buffer, a third route to the same root control."
      ],
      "snippet": "            handle.content.resize(offset, 0);",
      "exploitScenarios": [
        "As any process running as the user who mounted the library, open (or create) any writable file on the mountpoint to obtain a file handle.",
        "Call pwrite(fd, \"A\", 1, 0x10000000000) — a 1-byte write at a 1 TiB offset.",
        "The FUSE write callback forwards the offset to write_handle, which resizes the handle's Vec to ~1 TiB; the allocation aborts the process or the memset of zeroed pages triggers the OOM killer.",
        "The quarry CLI process hosting both the FUSE mount and the embedded server dies, dropping the mount and all connected browser sessions. (Equivalent: ftruncate(fd, 0x10000000000) reaches the same sink via setattr -> set_handle_len.)"
      ],
      "preconditions": [
        "Linux host with an active read-write quarry FUSE mount (read_only = false).",
        "Attacker controls a local process running as the mounting user (the mount sets default_permissions(false) and no allow_other, so other uids cannot reach the mountpoint).",
        "64-bit platform so the attacker offset survives usize::try_from."
      ],
      "recommendations": [
        "Root cause: enforce a maximum file size at the trust boundary — in write_handle, set_handle_len, and set_len reject offset.saturating_add(data.len()) or size above a fixed MAX_FILE_BYTES with an error mapped to EFBIG before any resize, so no attacker-chosen u64 ever reaches Vec::resize.",
        "Hardening: replace resize-after-implicit-reserve with try_reserve plus explicit error mapping (ENOMEM) so even unexpected allocation failure cannot abort the daemon.",
        "Regression test: a projection test that opens a handle, calls write_handle at a huge offset and set_handle_len with a huge size, asserts both return PayloadTooLarge/EFBIG-class errors, and asserts the projection (and process) remains usable afterwards."
      ]
    }
  ]
}