{
  "findings": [
    {
      "title": "Nightly release job executes repository code with a write-capable GitHub App token persisted in .git/config",
      "category": "authorization",
      "ruleId": "privilege-escalation.ci-write-token-exposure",
      "identity": {
        "anchor": "nightly-release-token-in-git-remote"
      },
      "severity": "MEDIUM",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "file": ".github/workflows/release-nightly.yml",
      "line": 69,
      "symbol": "tag-nightly job, 'Cut nightly tag' step",
      "snippet": "          git remote set-url origin \\",
      "rationale": "The scheduled nightly workflow mints a contents:write GitHub App token and stores it in the checkout's .git/config via git remote set-url, then runs cargo dev release --nightly, which executes repository-controlled code (bun scripts, cargo tests, build scripts, proc macros) in that same checkout. The only guard, unset RELEASE_TOKEN, clears the environment variable but leaves the token readable in .git/config, so any code merged to main can steal or directly wield a token that can push to main and create v* tags, which in turn trigger the full release pipeline. This is a complete source-to-sink path from merged-PR-level trust to release/publish-level trust.",
      "impact": "Any code that lands on main (e.g., a malicious PR merged after cursory review, or a compromised contributor account) is executed by the nightly job in a checkout whose .git/config contains a GitHub App installation token with contents:write. That code can read the token from .git/config and exfiltrate it (network egress is unrestricted in the job) or use it in place to push arbitrary commits to main — bypassing PR review — and push forged annotated vX.Y.Z tags. A forged tag matching the workspace version triggers release.yml, which publishes GitHub release assets and GHCR images under the project's identity, and (subject only to out-of-repo 'production' environment approval) deploys to production. This converts 'merged PR' level trust into release/publish level trust.",
      "evidence": [
        ".github/workflows/release-nightly.yml:4 — the workflow runs daily on the default branch (cron schedule) and via workflow_dispatch, so code on main is executed automatically within 24 hours.",
        ".github/workflows/release-nightly.yml:26 — actions/create-github-app-token mints a GitHub App installation token from secrets.FABRO_RELEASES_APP_PRIVATE_KEY; the token is later used to push main and a tag, proving it carries contents:write.",
        ".github/workflows/release-nightly.yml:69 — 'git remote set-url origin \"https://x-access-token:${RELEASE_TOKEN}@github.com/${GITHUB_REPOSITORY}.git\"' writes the raw token into the checkout's .git/config, readable by any process running in that directory.",
        ".github/workflows/release-nightly.yml:71 — 'unset RELEASE_TOKEN' is the only guard; it removes the environment variable but leaves the token persisted in .git/config (readable via 'git remote get-url origin' or a direct file read), so the guard is ineffective.",
        ".github/workflows/release-nightly.yml:72 — 'cargo --locked dev release --nightly' runs in the token-bearing checkout; .cargo/config.toml:2 defines 'dev = run --package quarry-dev --', so this compiles and executes repository code (crates/quarry-dev) including build scripts and proc macros.",
        "crates/quarry-dev/src/release.rs:76 — release() calls verify_release(), and verify_release (release.rs:294-327) runs 'bun install', 'bun run fixtures:check/typecheck/test/build', and 'cargo test --locked --workspace', executing repository-controlled package.json scripts, test code, build.rs files, and proc macros in the same working directory that holds the token in .git/config.",
        "crates/quarry-dev/src/release.rs:116 — 'git push --atomic origin main <tag>' authenticates through the token-bearing origin URL, demonstrating that any code in the job can push arbitrary refs (including forged v* tags) with the App token.",
        ".github/workflows/release.yml:4 — any pushed tag matching 'v*' triggers the full release pipeline (GitHub release publish with contents:write at line 140, GHCR push with packages:write at line 150, and production deploy gated only by the externally configured 'production' environment at line 201), completing the privilege-escalation chain."
      ],
      "exploitScenarios": [
        "Attacker opens a PR that adds an innocuous-looking payload (a Rust test, build.rs tweak, proc-macro helper, or a ui/package.json script) that, when executed, reads .git/config, extracts the x-access-token credential, and either POSTs it to an attacker endpoint or immediately runs 'git push origin HEAD:refs/tags/v9.9.9' plus a version-bump commit to main.",
        "Attacker gets the PR merged through normal review (the payload is disguised in test/build code) or uses a compromised account with merge rights.",
        "The scheduled nightly run (or a workflow_dispatch run) checks out main, mints the App token, stores it in .git/config, and executes the attacker's code via cargo/bun during the release smoke.",
        "The payload exfiltrates the token (valid for the App token's lifetime) or uses the authenticated remote directly within the job to push a forged annotated release tag whose Cargo.toml version matches the tag.",
        "The forged v* tag triggers release.yml, which publishes attacker-influenced artifacts to the GitHub release and GHCR, and proceeds to the production deploy stage (blocked only if the out-of-repo 'production' environment requires manual approval)."
      ],
      "preconditions": [
        "Attacker can get code merged to main (reviewed PR) or otherwise trigger a run of the nightly workflow on attacker-influenced code.",
        "The FABRO_RELEASES_APP installation token retains contents:write on the repository (required for the nightly push in release.rs to function).",
        "Network egress from the runner is unrestricted (default for GitHub-hosted runners) if the token is exfiltrated rather than used in place.",
        "Full release-pipeline impact additionally depends on the 'production' environment protection rules, which are configured outside the repository and could not be verified."
      ],
      "recommendations": [
        "Root cause: never persist a write-capable token in the checkout while repository code executes. Reorder the step so all code execution (the verify_release smoke) happens before any credential is introduced, and perform the push with a non-persisted credential — e.g. 'git -c http.extraheader=\"AUTHORIZATION: bearer ${RELEASE_TOKEN}\" push ...' or GIT_ASKPASS — so the token never lands in .git/config.",
        "Hardening: split the nightly job into an untrusted test/build job (no secrets, permissions {}) and a minimal tag-and-push job that runs only git commands; scope the GitHub App to the smallest permission set and this repository only; restrict the 'nightly' environment to the main branch.",
        "Regression test: add a CI check (or workflow lint) that fails if '.git/config' contains an x-access-token after checkout, and a test asserting the nightly workflow performs all cargo/bun execution before any token-bearing remote is configured."
      ]
    }
  ]
}