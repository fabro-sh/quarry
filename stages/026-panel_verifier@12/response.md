```json
{
  "job_id": "panel:F3:reachability",
  "candidate_id": "F3",
  "finding_id": "csf_c49d8697491bf04551a0001b",
  "occurrence_id": "occ_8e88d82a42d8c99f94d5ee71",
  "lens": "REACHABILITY",
  "verdict": "FALSE_POSITIVE",
  "confidence": "high",
  "reasoning": "The data flow itself is real: create_git_peer stores the caller-supplied `remote` verbatim (crates/quarry-server/src/git_handlers.rs:64-65), and pull/push later fetch/clone/push against it with no scheme allowlist (crates/quarry-git/src/lib.rs:296-297, 243-244, 1332-1334, 1360-1362, 1407-1408). However, the claimed unauthenticated-attacker reachability fails in the target deployment. The entire git route group is compiled only under the non-default `lib-documents` cargo feature: install_git_routes is wrapped in `#[cfg(feature = \"lib-documents\")]` (crates/quarry-server/src/lib.rs:453-455), and neither quarry-server, quarry-cli, nor the `quarry` binary enables it by default (`default = [\"tmp-documents\"]` in crates/quarry-server/Cargo.toml, crates/quarry-cli/Cargo.toml, crates/quarry/Cargo.toml). The shipped artifacts confirm this: the release workflow builds `cargo build --locked --release -p quarry` with default features (.github/workflows/release-build.yml:62), and the Dockerfile builds with default features unless the operator explicitly sets the QUARRY_FEATURES build-arg. The project's own threat model states the shipped image is tmp-documents-only, that the `/v1/libraries/**` surface 'has no access control at all' and 'must not be internet-exposed', and explicitly lists 'git sync — quarry-git' as out of scope because 'its handlers take request-controlled local paths and remote URLs (arbitrary FS + SSRF). Reachable only in a `lib-documents` build' (docs/security/threat-model.md:162-166, 324). Reaching the sink therefore requires (a) a non-default, explicitly opt-in build that the threat model declares out of scope and unsafe to expose, and (b) non-loopback binding, since the server defaults to 127.0.0.1:7831 (crates/quarry-cli/src/lib.rs:261) and loudly warns on non-loopback binds (crates/quarry-server/src/lib.rs:692-705). In the actual target deployment an unauthenticated remote attacker cannot reach the route at all — the code is absent from the binary. The finding as written ('an unauthenticated attacker can coerce the server into outbound connections') does not hold for the deployed artifact; the weakness is a documented, feature-gated, out-of-scope configuration rather than a reachable vulnerability.",
  "evidence": [
    "crates/quarry-server/src/lib.rs:453-455",
    "crates/quarry-server/Cargo.toml:12-15",
    "crates/quarry/Cargo.toml:11-14",
    ".github/workflows/release-build.yml:62",
    "Dockerfile:45-50",
    "docs/security/threat-model.md:162-166",
    "docs/security/threat-model.md:324",
    "crates/quarry-cli/src/lib.rs:261",
    "crates/quarry-server/src/lib.rs:692-705",
    "crates/quarry-server/src/git_handlers.rs:64-65",
    "crates/quarry-git/src/lib.rs:1332-1334"
  ]
}
```