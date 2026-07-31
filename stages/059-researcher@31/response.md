I've completed a full read of the crypto/secrets-relevant surface in `crates/quarry-collab-codec`. Summary of what the review established:

- **No crypto primitives exist in the crate.** `Cargo.toml` has no crypto dependencies (only chrono, indexmap, pulldown-cmark, regex, serde, serde_json, serde_yaml, similar, thiserror, yrs), and no hashing, encryption, signing, MAC, or comparison of secrets appears anywhere in `src/`.
- **No randomness generation.** All block-ID creation is via caller-injected closures (`mint_block_id: impl FnMut() -> String` at `reconcile.rs:255`, `rows.rs:97`, `mint_id` at `session_doc.rs:502`). The crate never mints IDs itself, so weak-randomness claims would attach to callers outside this component, not here.
- **No credentials or secrets.** The review endmatter fields (`by`, `at`, `kind`, `editedAt`, `body`, `re`, `status`, `resolved` at `review.rs:52-70`) are attribution/display metadata, not credentials; the `userId` written into Yjs marks (`session_doc.rs:373,469`, `review.rs:629`) is a display name. Spoofing it is an integrity/authorization concern, not credential exposure, and out of this lens.
- **No timing side channels.** There are no secret-bearing comparisons in the crate; the only `==` paths compare document shapes and mark maps.
- **No hardcoded secrets** in source or fixtures.
- Timestamp handling (`created_at_ms`, `review.rs:638`) uses `chrono` RFC3339 parsing with no security semantics.

The one deterministic-ID note (`block_id: "seed-empty"` at `session_doc.rs:325`) is a correctness/design choice for the empty seed paragraph, not a crypto or secrets issue, and carries no attacker impact under this lens.

No complete source-to-sink path exists for weak crypto, weak randomness, key/nonce reuse, timing side channels, hardcoded secrets, or credential mishandling in this component.

```json
{
  "findings": []
}
```