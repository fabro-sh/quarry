I've completed the review of `crates/quarry-fuse` through the cryptography-and-secrets lens. Summary of what I checked:

**Full read of `crates/quarry-fuse/src/lib.rs` (all 1517 lines)** plus the storage-side inode/path resolution it depends on (`crates/quarry-storage/src/directories.rs:305-343`, `crates/quarry-storage/src/lib.rs:1597-1636`) and `Cargo.toml`.

Candidates examined and rejected through the lens:

- **No crypto at all in the crate**: no hashing, encryption, signing, MAC, or key material anywhere in `quarry-fuse`; dependencies (`fuse3`, `libc`, `chrono`, `mime_guess`) add no cryptographic code paths in this component. Nothing to misuse.
- **Sequential inodes (`allocate_inode_conn`, quarry-storage/src/lib.rs:1618)**: inodes are identifiers, not capabilities. `path_for_inode` (directories.rs:324) resolves inode→path within the same library namespace, and the mount performs no per-inode authorization that an inode guess could bypass — every document is already equally reachable by path through `lookup`. No additional capability is granted, so weak-randomness does not apply.
- **Predictable file handles (`insert_handle`, lib.rs:641-645, `AtomicU64` counter from 1)**: a FUSE `fh` is not a security token; the kernel echoes it back, and any process able to issue requests to the mount can open its own handle to the same file (the mount is deliberately single-tenant with `default_permissions(false)` and no uid checks). Guessing another handle's `fh` confers no capability the attacker doesn't already have, so there is no complete attack path attributable to weak randomness.
- **Hardcoded secrets / credential handling**: none present; grep for `secret|token|password|key|hmac|nonce|uuid|rand` over the crate finds only `HashMap` and comment text. Mount goes through `fusermount3`/privileged mount with no credentials.
- **Timing side channels**: no secret comparisons; `path_exists` and error mapping (`to_errno`, lib.rs:1487) compare only non-secret error variants.
- **Info disclosure via logs/mount table**: `tracing` emits paths, doc/version/tx IDs (lib.rs:733-744) and `fs_name("quarry-{library}")` (lib.rs:968) appears in the mount table — these are identifiers, not secrets, and library slugs are not credentials in this system. Below the bar for a concrete attacker capability.

No complete source-to-sink path exists in this component under the crypto/secrets lens.

```json
{
  "findings": []
}
```