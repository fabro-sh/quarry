All cited evidence verified against the source. The impact mechanism holds: `resize` zero-fills to the attacker-chosen size, and the only guard is a no-op `usize::try_from` on 64-bit.

```json
{
  "job_id": "panel:F18:impact",
  "candidate_id": "F18",
  "verdict": "TRUE_POSITIVE",
  "confidence": "HIGH",
  "severity": "MEDIUM",
  "reasoning": "IMPACT lens: the claimed consequence is real and accurately described. At crates/quarry-fuse/src/lib.rs:316-321, write_handle resizes handle.content to the kernel-supplied u64 offset (guarded only by usize::try_from at :314-315, a no-op on 64-bit) and Vec::resize(_, 0) zero-fills, so a small write at offset 2^40 either aborts on allocation failure or forces ~1 TiB of page-touching -> OOM kill; both paths terminate the daemon process. The same unbounded growth is reachable with no write payload at all via setattr -> set_handle_len (:334-338) or set_len (:603-607), so a plain ftruncate(fd, 1<<40) suffices, bypassing even MAX_WRITE_BYTES (:951-952, advertised as ReplyInit.max_write at :1015), which caps only per-write data length, never offset or resulting size. ensure_writable (:748-753) only rejects read-only mounts. The blast radius extends beyond the FUSE loop: quarry-cli/src/lib.rs:516-533 runs mount_library_with_shutdown and serve_state_with_shutdown in one tokio::try_join! in a single process, so the crash also kills the embedded HTTP server. Impact caveat supporting (not exceeding) the reported MEDIUM: MountOptions (:966-970) set no allow_other, so the mount is reachable only by the mounting local user, making this a local same-principal DoS rather than a remote one. Crash not executed per the read-only rule, but the resize semantics leave no doubt about the mechanism.",
  "evidence": [
    "crates/quarry-fuse/src/lib.rs:314-321",
    "crates/quarry-fuse/src/lib.rs:334-338",
    "crates/quarry-fuse/src/lib.rs:598-607",
    "crates/quarry-fuse/src/lib.rs:748-753",
    "crates/quarry-fuse/src/lib.rs:951-952",
    "crates/quarry-fuse/src/lib.rs:1015",
    "crates/quarry-fuse/src/lib.rs:1059-1064",
    "crates/quarry-fuse/src/lib.rs:1174-1177",
    "crates/quarry-cli/src/lib.rs:516-533"
  ]
}
```