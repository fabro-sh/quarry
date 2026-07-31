All cited code verified against the repository. The DEFENSES lens found no effective mitigation:

- `write_handle` (crates/quarry-fuse/src/lib.rs:314-321): the only guard is `usize::try_from(offset)`, which is infallible for u64→usize on 64-bit; `handle.content.resize(offset, 0)` then grows the in-memory Vec to the kernel-supplied offset with no cap.
- `set_handle_len` (:334-338) and `set_len` (:603-607): same unchecked `usize::try_from` guard before `resize`.
- `MAX_WRITE_BYTES` (:951-952, advertised at :1015) caps only the per-write payload length via `ReplyInit.max_write`; it never bounds offset or truncate size.
- The store-side `PayloadTooLarge` check (crates/quarry-storage/src/tmp_documents.rs:617-623) applies only to tmp Markdown and only at commit time — after `resize` has already consumed/aborted on the memory.
- `ensure_writable` (:748-753) only rejects read-only mounts.
- Rust `Vec::resize` aborts on allocation failure, and the mount shares one process with the HTTP server (crates/quarry-cli/src/lib.rs:516-533), so the crash takes down both.

No framework default, middleware, or caller check interrupts the path from the kernel-supplied u64 offset/size to the unbounded `resize`.

```json
{
  "job_id": "panel:F18:defenses",
  "candidate_id": "F18",
  "verdict": "TRUE_POSITIVE",
  "confidence": "HIGH",
  "reasoning": "DEFENSES lens: verified every claimed guard is absent or ineffective. crates/quarry-fuse/src/lib.rs:314-321 (write_handle) forwards the FUSE write() handler's kernel-supplied u64 offset (lib.rs:1164-1177) into resize with only usize::try_from as a guard, which is a no-op on 64-bit; a 1 MiB write at offset 2^40 materializes ~1 TiB via zero-filled Vec growth. The same unchecked pattern exists in set_handle_len (lib.rs:334-338) and set_len (lib.rs:603-607), reachable from a plain ftruncate via setattr (lib.rs:1059-1064). MAX_WRITE_BYTES (lib.rs:951-952, advertised at :1015) caps only per-write payload length, never offset or resulting size. The only PayloadTooLarge check (crates/quarry-storage/src/tmp_documents.rs:617-623) covers tmp Markdown only and runs at commit/flush, after the memory is already consumed. ensure_writable (lib.rs:748-753) only rejects read-only mounts. On allocation failure Rust's Vec growth aborts the process, and quarry-cli/src/lib.rs:516-533 runs the mount and HTTP server in one process via try_join!, so the abort kills both. No defense could be found or invented that interrupts the source-to-sink path."
}
```