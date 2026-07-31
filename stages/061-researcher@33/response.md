I've completed a crypto-and-secrets review of `crates/quarry-collab-codec`. The evidence:

- **Dependencies** (`Cargo.toml`): only `chrono`, `indexmap`, `pulldown-cmark`, `regex`, `serde`, `serde_json`, `serde_yaml`, `similar`, `thiserror`, `yrs` — no crypto, hashing, or RNG crates.
- **No RNG/ID generation in-crate**: all identifier minting (`mint_block_id` in `rows.rs:97`, `reconcile.rs:255`; `mint_id` in `session_doc.rs:502`) is caller-injected closures; the codec never generates identifiers itself (the only constant is the structural `"seed-empty"` scaffold id, not a token). The browser-side `nanoid` mentioned in `review.rs:147` is out of scope.
- **No hardcoded secrets**: greps for URLs, `Bearer`, API keys, private-key markers, base64/hex material, and credentials across `src/` returned nothing.
- **No secret comparisons**: no hashing, verification, or equality checks over sensitive material — no timing-side-channel surface exists. The only `chrono` use is RFC3339 parsing of review timestamps (`review.rs:639`).
- **No credential handling**: the codec transforms document content only; it neither stores, redacts, nor transmits credentials.

There is no crypto to misuse and no secrets to expose in this component. Under this lens there is no complete source-to-sink attack path to report.

```json
{
  "findings": []
}
```