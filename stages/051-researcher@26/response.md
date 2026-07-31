I've completed the review. Summary of what I traced under the crypto/secrets lens for `cli-and-binary`:

- **Invite token minting** (`lib.rs:655-663` → `quarry-storage/src/documents.rs:273-316`): token IDs are `Uuid::new_v4()` with the workspace `uuid` crate's `v4` feature (getrandom-backed, cryptographically secure). No weak randomness.
- **Tmp document secret handling** (`lib.rs:948-990`): the secret is server-generated, returned to the same origin (path-segment interpolation cannot redirect authority), printed to stdout and passed to the browser by design — both output branches (agent prompt and browser URL) carry the secret, so `detect_agent` env spoofing yields no confidentiality delta.
- **Hardcoded secrets**: none; `https://quarry.lithos.computer` is a public default endpoint, not a credential. `COPILOT_GITHUB_TOKEN` is only checked for presence, never read or logged.
- **TLS/transport**: reqwest default client (certificate verification on); cleartext `http://` is only by explicit operator choice for their own data.
- **Logging** (`RUST_LOG`/`QUARRY_LOG_FORMAT`): no secret values logged by the CLI; only the agent name at debug level.
- **Timing/comparison paths**: none exist in this component.

No complete attacker-controlled path to a crypto/secrets violation exists in this component. Reporting zero findings.

```json
{
  "job_id": "research:008-cli-and-binary-55d4ef1b:crypto-and-secrets",
  "findings": []
}
```