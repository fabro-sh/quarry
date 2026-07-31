{
  "component": "dev-release-tooling",
  "job_id": "threat:010-dev-release-tooling-b4f86a67",
  "internetFacing": false,
  "summary": "Internal release tooling: the Rust quarry-dev crate validates a clean/synced main checkout, computes the next semver from git tags, rewrites Cargo.toml, and shells out to git/cargo/bun to test, commit, tag, and push a release; a Ruby script rewrites the Homebrew formula from argv-supplied tag/checksums; the Formula downloads and installs release binaries over HTTPS with sha256 pinning.",
  "entryPoints": [
    "crates/quarry-dev/src/lib.rs:34 (CLI argv parsed via clap: release subcommand flags --nightly/--bump/--dry-run/--skip-tests/--release-date/--root)",
    "crates/quarry-dev/src/release.rs:33 (QUARRY_RELEASE_DATE env var feeds NaiveDate)",
    "crates/quarry-dev/src/release.rs:41 (args.root overrides the repository root all commands run in)",
    "crates/quarry-dev/src/release.rs:57 (workspace Cargo.toml read as untrusted file content)",
    "crates/quarry-dev/src/release.rs:164 (git tag --list output parsed as version source)",
    "scripts/update-homebrew-formula.rb:14 (ARGV: formula_path, release_tag, sha256 values)",
    "scripts/update-homebrew-formula.rb:47 (formula file read from argv-controlled path)",
    "Formula/quarry.rb:9 (install-time network fetch of release tarballs from GitHub)"
  ],
  "sinks": [
    "crates/quarry-dev/src/release.rs:368-377 (run_command spawns arbitrary program via std::process::Command with argv array)",
    "crates/quarry-dev/src/release.rs:87-124 (git fetch/commit/tag/push --atomic origin main — mutates local and remote repository state)",
    "crates/quarry-dev/src/release.rs:294-327 (verify_release executes bun install/run and cargo test in the checkout, executing repo-controlled build scripts)",
    "crates/quarry-dev/src/release.rs:84 (std::fs::write overwrites root/Cargo.toml with rewritten version)",
    "crates/quarry-dev/src/release.rs:406-421 (prepared_command: PATH-resolved program lookup; env scrub only applied to cargo)",
    "scripts/update-homebrew-formula.rb:56 (File.write overwrites the argv-named formula path with regenerated content)",
    "Formula/quarry.rb:28-34 (install executes downloaded 'quarry' binary or cargo install; test block runs #{bin}/quarry --help)",
    "Formula/quarry.rb:9-24 (HTTPS downloads authenticated only by embedded sha256 checksums)"
  ],
  "assumptions": [
    "crates/quarry-dev/src/release.rs:138-162 (assumes git branch/status/rev-parse checks suffice to prove a safe release checkout; no verification of remote URL or tag authenticity/signing)",
    "crates/quarry-dev/src/release.rs:165 (assumes 'v*' tag names parse as semver; malformed tags silently filtered at :178-180)",
    "crates/quarry-dev/src/release.rs:246-262 (assumes [workspace.package] version is a unique quoted string via hand-rolled line parsing, not a TOML parser)",
    "crates/quarry-dev/src/release.rs:407 (assumes PATH resolves git/cargo/bun to trusted binaries in the release environment)",
    "scripts/update-homebrew-formula.rb:16 (assumes release_tag starting with 'v' is otherwise safe to interpolate into URL strings; no further charset validation)",
    "scripts/update-homebrew-formula.rb:20 (assumes 64-hex checksums genuinely correspond to published release assets — supplied entirely by caller)",
    "scripts/update-homebrew-formula.rb:48-51 (assumes the formula file matches the expected homepage/license/head/install regex structure)",
    "Formula/quarry.rb:9 (assumes GitHub release assets are uncompromised; integrity rests solely on committed sha256 values)"
  ],
  "trustBoundaries": [
    "crates/quarry-dev/src/release.rs:41 (operator-supplied --root moves from CLI input to cwd for all spawned processes and file writes)",
    "crates/quarry-dev/src/release.rs:56-65 (repository content — Cargo.toml version and git tags — crosses into release decisions and git push of new tags)",
    "crates/quarry-dev/src/release.rs:294-306 (repo-controlled bun/cargo build scripts execute with the releasing operator's credentials, including push-capable git credentials in the environment)",
    "scripts/update-homebrew-formula.rb:14-45 (CI/operator argv (tag, checksums) crosses into formula source that Homebrew later executes server-side on user machines)",
    "Formula/quarry.rb:7-25 (network-downloaded tarballs cross into bin.install on end-user systems, gated only by sha256)"
  ],
  "hotFiles": [
    "crates/quarry-dev/src/release.rs",
    "scripts/update-homebrew-formula.rb",
    "Formula/quarry.rb",
    "crates/quarry-dev/src/lib.rs"
  ],
  "notes": "Component is not internet-facing; primary risks are supply-chain/credential-abuse: release tool runs with push-capable credentials while executing repo-controlled build/test scripts, Ruby script interpolates a weakly-validated release_tag into Ruby source, and formula integrity depends on caller-supplied checksums. No workflow invoking update-homebrew-formula.rb was found in-repo. No vulnerabilities reported at this stage."
}