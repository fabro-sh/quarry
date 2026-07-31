## Review summary

I read `crates/quarry-core/src/lib.rs` (the crate's only source file, all 724 lines) in full through the **cryptography and secrets** lens and traced every candidate:

- **Randomness/key material**: none. The crate performs no random or key generation. `uuid` is declared in `Cargo.toml` but never used in the source — no weak-RNG sink exists.
- **Secrets/credentials**: `CollabInviteToken` (lib.rs:399) is a plain DTO; it carries no secret material, and no comparison, hashing, or validation of it happens in this crate — token generation/verification lives in consumers outside this component's scope.
- **Hashing**: `content_hash` (lib.rs:331, 390) is an opaque `Option<String>` produced by callers; no hash is computed or verified here, so there is no weak-crypto or unverified-digest operation inside this crate.
- **Timing side channels**: the only comparisons are enum-string parses (`from_str` impls) and path validation — none operate on secrets.
- **Hardcoded secrets**: none present.
- **`now_timestamp`** (lib.rs:579) is wall-clock RFC3339 with no security claim — not a crypto misuse.

The threat model's flagged sinks (`normalize_path`, `render_markdown_frontmatter`) fall under path-traversal/injection lenses, not this one, and I found no complete attacker source-to-sink path in this component under the crypto/secrets category. Per instructions, an empty array is preferable to a padded finding.

```json
{
  "findings": []
}
```