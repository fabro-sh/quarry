{
  "job_id": "threat:010-dev-tooling-b2ecb3ec",
  "component": "dev-tooling",
  "summary": "quarry-dev is a small internal release-automation CLI (single `release` subcommand). It parses CLI args via clap, reads Cargo.toml, shells out to git/cargo/bun with argv-style invocations (no shell interpolation), mutates the workspace manifest, and pushes commits/tags to origin. Untrusted input surface is narrow: CLI flags, the QUARRY_RELEASE_DATE env var, repository contents (Cargo.toml, git tags), and PATH-resolved binaries.",
  "entryPoints": [
    "crates/quarry-dev/src/lib.rs:34 — clap parses argv into Cli/ReleaseArgs; all CLI flags (--bump, --nightly, --dry-run, --skip-tests, --release-date, --root) enter here",
    "crates/quarry-dev/src/release.rs:33 — QUARRY_RELEASE_DATE env var consumed via clap env binding and parsed as NaiveDate",
    "crates/quarry-dev/src/release.rs:36 — hidden --root arg selects an arbitrary repository root to operate on; subsequent reads, writes, and pushes follow this path",
    "crates/quarry-dev/src/release.rs:57 — workspace Cargo.toml read from the chosen root; version string parsed by workspace_version_value at line 246",
    "crates/quarry-dev/src/release.rs:58 — git tag names from `git tag --list v*` enter as untrusted strings and feed version computation in next_release_version (line 169)",
    "crates/quarry-dev/src/release.rs:406 — external commands (git, cargo, bun) resolved via PATH via Command::new without absolute paths"
  ],
  "sinks": [
    "crates/quarry-dev/src/release.rs:84 — std::fs::write overwrites the workspace Cargo.toml with computed content",
    "crates/quarry-dev/src/release.rs:368 — run_command executes planned git/cargo/bun commands via Command::status",
    "crates/quarry-dev/src/release.rs:399 — capture_command executes git plumbing and captures stdout/stderr via Command::output",
    "crates/quarry-dev/src/release.rs:100 — git commit with release-derived message",
    "crates/quarry-dev/src/release.rs:107 — git tag -a creation",
    "crates/quarry-dev/src/release.rs:116 — git push --atomic origin main <tag>; irreversible outward-facing sink publishing to the remote",
    "crates/quarry-dev/src/release.rs:46 — git fetch origin --tags; network I/O against the configured remote",
    "crates/quarry-dev/src/release.rs:294 — verify_release runs bun install/run and cargo test, executing build/test code from the checkout"
  ],
  "assumptions": [
    "crates/quarry-dev/src/release.rs:407 — assumes PATH yields trusted git/cargo/bun binaries; no absolute paths or verification",
    "crates/quarry-dev/src/release.rs:138 — assumes branch name 'main', clean porcelain status, and HEAD == origin/main (line 155) are sufficient integrity checks for the checkout",
    "crates/quarry-dev/src/release.rs:246 — assumes line-based string parsing of Cargo.toml identifies the real [workspace.package] version; no TOML parser used",
    "crates/quarry-dev/src/release.rs:176 — assumes git tags beginning with 'v' are semver or benign; malformed tags silently filtered out",
    "crates/quarry-dev/src/release.rs:415 — assumes scrubbing the enumerated CARGO_* env vars makes nested cargo invocations safe; all other environment variables (PATH, proxies, etc.) are inherited"
  ],
  "trustBoundaries": [
    "crates/quarry-dev/src/lib.rs:34 — operator-controlled CLI/env input crosses into a binary that mutates the repository and pushes to origin",
    "crates/quarry-dev/src/release.rs:81 — on-disk Cargo.toml content (potentially attacker-influenced) is read, transformed, and written back",
    "crates/quarry-dev/src/release.rs:164 — git tag output (remote-derived data after fetch) crosses into version arithmetic and tag/commit strings",
    "crates/quarry-dev/src/release.rs:368 — computed values (version, tag) cross into child-process argv; argv form prevents shell injection but values reach git unsanitized",
    "crates/quarry-dev/src/release.rs:116 — local repository state crosses to the shared remote via authenticated push"
  ],
  "hotFiles": [
    "crates/quarry-dev/src/release.rs:1 — entire release flow: args, checkout checks, manifest rewrite, command execution, push",
    "crates/quarry-dev/src/lib.rs:1 — CLI surface and error handling"
  ]
}