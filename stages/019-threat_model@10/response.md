{
  "job_id": "threat:011-ci-workflows-98c071e8",
  "component": "ci-workflows",
  "entryPoints": [
    ".github/workflows/release.yml:4 - push of tag 'v*' triggers the full release pipeline; tag name (github.ref_name) is attacker-controllable by anyone with tag-push rights",
    ".github/workflows/release-nightly.yml:6 - workflow_dispatch manual trigger; runs with the nightly environment and mints a GitHub App token",
    ".github/workflows/release-nightly.yml:29 - FABRO_RELEASES_APP_PRIVATE_KEY secret materializes a token via actions/create-github-app-token",
    ".github/workflows/release-build.yml:4 - workflow_call reusable build; inherits caller context and gains id-token:write and attestations:write",
    ".github/workflows/rust.yml:15 - pull_request trigger: untrusted PR code executes on the runner (cargo build/test, bun install of PR-controlled lockfile/scripts)",
    ".github/workflows/typescript.yml:11 - pull_request trigger: untrusted PR code executes bun install and tests on the runner",
    ".github/workflows/e2e-live.yml:16 - pull_request trigger: builds and runs PR-controlled Rust and Playwright code on the runner",
    ".github/workflows/release.yml:34 - github.ref_name (TAG) consumed in shell; validated only against Cargo.toml version and regexes",
    ".github/actions/release-docker/action.yml:27 - inputs.tag / inputs.image-name enter composite action shell contexts"
  ],
  "sinks": [
    ".github/workflows/release.yml:140 - gh release create publishes dist/* with contents:write token",
    ".github/workflows/release.yml:259 - remote sed -i on production compose.yaml, interpolating ${image} built from IMAGE_DIGEST; runs on the production EC2 via SSM",
    ".github/workflows/release.yml:280 - aws ssm send-command executes a runner-constructed remote bash script on the production host (AWS-RunShellScript)",
    ".github/workflows/release.yml:218 - aws-actions/configure-aws-credentials assumes production deploy role arn:aws:iam::449957914122 via OIDC id-token",
    ".github/workflows/release.yml:459 - git remote set-url embeds GH_TOKEN in the origin URL; token with contents:write used to push to main",
    ".github/workflows/release.yml:463 - ruby scripts/update-homebrew-formula.rb executes with tag and SHA values; edits and pushes Formula/quarry.rb to main",
    ".github/workflows/release.yml:481 - git pull --rebase + git push origin HEAD:main from CI with write token",
    ".github/workflows/release-nightly.yml:69 - git remote set-url embeds the minted GitHub App RELEASE_TOKEN in the origin URL",
    ".github/workflows/release-nightly.yml:72 - cargo dev release --nightly runs repository code with the App token in the remote URL, able to push tags that trigger release.yml",
    ".github/actions/release-docker/action.yml:50 - tar -xzf of downloaded artifacts into the build context (archive extraction of cross-job data)",
    ".github/actions/release-docker/action.yml:65 - GHCR login with inputs.github-token granting packages:write",
    ".github/actions/release-docker/action.yml:149 - docker build-push-action pushes multi-arch image to GHCR from Dockerfile context containing staged binaries",
    ".github/actions/release-docker/action.yml:160 - attest-build-provenance pushes signed provenance for the pushed image digest",
    ".github/workflows/release-build.yml:62 - cargo build --release of repository code in the privileged release path (build scripts/proc macros execute at build time)",
    ".github/workflows/release-build.yml:79 - attest-build-provenance signs release tarballs that users download",
    ".github/workflows/release.yml:191 - docker buildx imagetools create retags the mutable ghcr.io/fabro-sh/quarry:nightly and :latest aliases"
  ],
  "assumptions": [
    ".github/workflows/release.yml:40 - assumes the tag pusher is trusted: tag name must equal the workspace version in Cargo.toml, but nothing verifies who pushed the tag or that the commit is on a protected branch",
    ".github/workflows/release.yml:231 - assumes IMAGE_DIGEST starts with sha256:; the digest comes from a prior job's step output and is trusted in the SSM remote script",
    ".github/workflows/release.yml:123 - assumes artifacts-quarry-* downloaded via merge-multiple are exactly what release-build uploaded; no digest verification before gh release create ships them",
    ".github/workflows/release.yml:454 - assumes .sha256 files in dist contain well-formed single-field hashes (awk '{print $1}' into the Homebrew formula)",
    ".github/workflows/release-nightly.yml:72 - assumes `cargo dev release --nightly` (in-repo code) handles credentials and tag creation safely with the App token",
    ".github/workflows/rust.yml:15 - assumes default token permissions for pull_request runs are read-only and that PR code cannot reach secrets (no environment/secrets used in PR workflows)",
    ".github/actions/release-docker/action.yml:74 - assumes inputs.tag has a v-prefix and contains no shell metacharacters; TAG is interpolated into GITHUB_OUTPUT heredocs",
    ".github/workflows/release.yml:45 - assumes annotated-tag check plus version match is sufficient release provenance; assumes branch protection on main prevents forged version bumps"
  ],
  "trustBoundaries": [
    ".github/workflows/release.yml:251 - runner-constructed bash crosses onto the production EC2 host via AWS SSM send-command (CI -> production execution boundary)",
    ".github/workflows/release.yml:206 - GitHub OIDC id-token exchanged for the production AWS IAM role (CI identity -> cloud control plane)",
    ".github/workflows/release.yml:119 - artifacts cross from the release-build matrix jobs (id-token/attestations scope) into the publish job that ships them to users",
    ".github/workflows/release-nightly.yml:24 - scheduled workflow crosses from default GITHUB_TOKEN to a long-lived GitHub App identity able to push tags that trigger the release pipeline",
    ".github/workflows/release-build.yml:17 - reusable workflow grants id-token:write to builds of repository code; repo content crosses into a signing identity",
    ".github/workflows/rust.yml:15 - untrusted pull_request code executes on the runner with contents:read token (public contributor -> CI runner boundary)",
    ".github/actions/release-docker/action.yml:34 - downloaded artifacts enter the Docker build context and are baked into published GHCR images (build output -> distribution registry)",
    ".github/workflows/release.yml:201 - production environment gate: jobs before it run without environment approval; deploy-production and promote-stable run after"
  ],
  "hotFiles": [
    ".github/workflows/release.yml",
    ".github/workflows/release-nightly.yml",
    ".github/actions/release-docker/action.yml",
    ".github/workflows/release-build.yml",
    "scripts/update-homebrew-formula.rb",
    ".github/workflows/rust.yml",
    ".github/workflows/typescript.yml",
    ".github/workflows/e2e-live.yml"
  ]
}