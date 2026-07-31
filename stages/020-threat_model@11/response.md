{
  "job_id": "threat:011-ci-and-build-config-c77fe400",
  "component": "ci-and-build-config",
  "entryPoints": [
    ".github/workflows/release.yml:4 — push of tag v* triggers the full release pipeline (validate, build, publish, docker, production deploy); anyone able to push tags is an entry actor",
    ".github/workflows/release.yml:34 — github.ref_name (tag name) enters shell via TAG env var",
    ".github/workflows/release-nightly.yml:4 — schedule cron and workflow_dispatch entry; runs with GitHub App release token and nightly environment",
    ".github/workflows/release-nightly.yml:72 — cargo --locked dev release --nightly executes repository code (crates/quarry-dev) with a token-bearing git remote configured",
    ".github/workflows/rust.yml:15 — pull_request trigger: untrusted PR code is compiled and tested on CI runners (build scripts and proc macros execute at build time)",
    ".github/workflows/e2e-live.yml:16 — pull_request trigger running full server plus Playwright against PR code",
    ".github/actions/release-docker/action.yml:27 — composite action input 'tag' interpolated into shell env",
    "Dockerfile:40 — ARG QUARRY_FEATURES build-time input fed into cargo build command line",
    "Dockerfile:72 — runtime CMD interpolates env QUARRY_ROOT and PORT into sh -c command line"
  ],
  "sinks": [
    ".github/workflows/release.yml:37 — shell pipeline cargo metadata | jq derives version that gates the release",
    ".github/workflows/release.yml:140 — gh release create publishes artifacts with a contents:write token",
    ".github/workflows/release.yml:218 — aws-actions/configure-aws-credentials assumes production IAM role via OIDC id-token:write",
    ".github/workflows/release.yml:231 — IMAGE_DIGEST validated only for sha256: prefix before production use",
    ".github/workflows/release.yml:251 — remote_script heredoc sent to production via aws ssm send-command: remote shell execution on production host, interpolating ${image}",
    ".github/workflows/release.yml:259 — sudo sed -i rewrites /opt/quarry/compose.yaml on the production host, followed by docker compose pull/up",
    ".github/workflows/release.yml:459 — git remote set-url embeds GH_TOKEN (contents:write) into origin URL and pushes to main",
    ".github/workflows/release.yml:463 — ruby scripts/update-homebrew-formula.rb executed with tag and SHA arguments",
    ".github/workflows/release-nightly.yml:69 — git remote set-url embeds GitHub App token; repository code then runs with a push-capable remote",
    ".github/workflows/release-build.yml:79 — actions/attest-build-provenance with id-token:write signs published artifacts",
    ".github/actions/release-docker/action.yml:50 — tar -xzf extracts downloaded workflow artifacts into the Docker build context",
    ".github/actions/release-docker/action.yml:149 — docker/build-push-action push:true publishes the multi-arch image to GHCR",
    ".github/actions/release-docker/action.yml:160 — attest-build-provenance push-to-registry attests the image digest",
    "Dockerfile:41 — cargo build --release inside the image with a cache mount of the cargo registry",
    "Dockerfile:78 — COPY of the CI-produced binary into the published runtime image (implicit trust in artifact integrity)"
  ],
  "assumptions": [
    ".github/workflows/release.yml:50 — assumes tag format/annotation checks plus tag-push permission suffice to authorize production deploy; enforcement depends on the 'production' environment protection configured outside the repo",
    ".github/workflows/release.yml:246 — assumes exactly one running EC2 instance matches the tags; ambiguity only partially guarded",
    ".github/workflows/release-build.yml:84 — assumes upload-artifact/download-artifact round-trip preserves artifact integrity across jobs; no digest verification before docker packaging, release publishing, or formula SHA extraction",
    ".github/workflows/release.yml:454 — assumes dist/*.sha256 files downloaded from artifacts are authentic; Homebrew formula SHAs come from the same unverified artifact blob",
    ".github/workflows/release-nightly.yml:28 — assumes FABRO_RELEASES_APP_PRIVATE_KEY is scoped to only this repo with minimal rights",
    ".github/workflows/rust.yml:15 — assumes running untrusted PR builds is safe because permissions:{} and no secrets are present; relies on GitHub fork-PR runner isolation",
    "Dockerfile:41 — assumes QUARRY_FEATURES build arg contains no shell metacharacters; it flows through an unquoted shell test context",
    ".github/workflows/e2e-live.yml:30 — assumes permissions:{} is sufficient isolation for PR-triggered live server tests"
  ],
  "trustBoundaries": [
    ".github/workflows/release.yml:105 — caller workflow grants id-token:write and attestations:write to reusable release-build.yml",
    ".github/workflows/release.yml:119 — build artifacts cross from contents:read matrix build jobs into publish/docker jobs holding write tokens",
    ".github/workflows/release.yml:278 — workflow-generated shell crosses into production EC2 via SSM JSON parameters",
    ".github/workflows/release-nightly.yml:65 — GitHub App token crosses into a job that executes repository code (cargo dev release)",
    ".github/workflows/rust.yml:15 — untrusted PR content crosses onto shared CI runners executing cargo/bun builds",
    ".github/actions/release-docker/action.yml:65 — GITHUB_TOKEN crosses into the composite action as the GHCR password",
    "Dockerfile:82 — CI-built binary crosses into the published runtime image executed by end users"
  ],
  "hotFiles": [
    ".github/workflows/release.yml",
    ".github/workflows/release-build.yml",
    ".github/workflows/release-nightly.yml",
    ".github/actions/release-docker/action.yml",
    "Dockerfile",
    "scripts/update-homebrew-formula.rb",
    "crates/quarry-dev (release subcommand invoked by release-nightly.yml)",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    "Formula/quarry.rb"
  ]
}