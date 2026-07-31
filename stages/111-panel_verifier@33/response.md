{
  "job_id": "panel:F20:defenses",
  "candidate_id": "F20",
  "verdict": "TRUE_POSITIVE",
  "confidence": "high",
  "reasoning": "Every link of the claimed chain verified against the source, and the DEFENSES lens found no effective mitigation. (1) Sensitive data exists and is stored verbatim: quarry-git sets no git2 credential callbacks — FetchOptions::new() at crates/quarry-git/src/lib.rs:1331 and PushOptions::new() at crates/quarry-git/src/lib.rs:1406 — so credential-bearing URLs are the only HTTPS auth path, and redact_remote_url (crates/quarry-git/src/lib.rs:1442, tested with 'https://token:secret@example.com/...' at line 1840) proves credentialed URLs are an anticipated input. (2) Storage is plaintext and unredacted: create_git_peer inserts the caller-supplied `remote` string verbatim into config JSON (crates/quarry-server/src/git_handlers.rs:64-66) and persists it as config_json (crates/quarry-storage/src/sync.rs:39-44). (3) The sink discloses it verbatim: list_git_peers returns the stored config with no redaction (crates/quarry-storage/src/sync.rs:72; crates/quarry-server/src/git_handlers.rs:83, exact snippet match), and GitPeer.config is a plain JsonValue serialized as-is (crates/quarry-core/src/lib.rs:447-452). (4) No defensive layer: the route GET /v1/libraries/{library}/git/peers is registered at crates/quarry-server/src/lib.rs:457-458 with no handler-level guard, and the only router layers are error-envelope, tracing, and security-headers middleware (crates/quarry-server/src/lib.rs:215-217) — grep across quarry-server/src confirms no authn/authz middleware on library routes (tmp-document secrets are a separate mechanism that does not cover /v1/libraries/*). (5) The redact_remote_url guard applies only to tracing log fields (crates/quarry-git/src/lib.rs:1340, 1367, 1414), never to storage or API output, so it is ineffective for this sink. (6) The only deployment-level mitigation is loopback-by-default plus a warning: warn_if_non_loopback (crates/quarry-server/src/lib.rs:692-706, invoked at line 582) merely prints 'warning: Quarry phase one has no auth' for non-loopback binds, confirming unauthenticated access is the designed posture and non-loopback exposure is a supported, warned-about configuration. The compile-time lib-documents feature gate (crates/quarry-server/src/lib.rs:453-455) scopes the route's existence but is not a defense in the deployment where the feature is enabled — which is the only deployment where the finding applies. Severity MEDIUM is reasonable: disclosure requires a reachable (non-loopback or local) attacker and an operator who stored a credentialed remote, but the consequence is leak of third-party git credentials to any unauthenticated caller.",
  "evidence": [
    "crates/quarry-server/src/git_handlers.rs:83",
    "crates/quarry-server/src/git_handlers.rs:64-66",
    "crates/quarry-storage/src/sync.rs:39-44",
    "crates/quarry-storage/src/sync.rs:55-76",
    "crates/quarry-core/src/lib.rs:447-452",
    "crates/quarry-server/src/lib.rs:215-217",
    "crates/quarry-server/src/lib.rs:457-458",
    "crates/quarry-server/src/lib.rs:692-706",
    "crates/quarry-git/src/lib.rs:1331",
    "crates/quarry-git/src/lib.rs:1406",
    "crates/quarry-git/src/lib.rs:1442-1454"
  ]
}