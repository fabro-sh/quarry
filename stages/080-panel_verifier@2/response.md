I've verified the claim against the actual files. Checking the decisive points:

**Workflow side** (`.github/workflows/release-nightly.yml`):
- Line 35: `persist-credentials: false` — but line 69-70 explicitly runs `git remote set-url origin "https://x-access-token:${RELEASE_TOKEN}@github.com/..."`, reintroducing the token into `.git/config`.
- Line 71: only `unset RELEASE_TOKEN` scrubs the environment variable; the on-disk credential in `.git/config` is untouched before `cargo --locked dev release --nightly` runs (line 72).

**Component side** (`crates/quarry-dev/src/release.rs`):
- Lines 44-54: non-dry-run path uses the stored credential for `git fetch origin --tags`, confirming it is live.
- Lines 76-77: `verify_release(&root)?` runs with no prior credential scrub (workflow passes `--nightly` without `--dry-run`/`--skip-tests`, so this path is taken).
- Lines 294-327: `verify_release` spawns `bun install --frozen-lockfile`, four `bun run` scripts, and two `cargo test` invocations — cargo unconditionally executes third-party dependency `build.rs` scripts and test binaries.
- Lines 368-377, 399-404: spawned processes inherit the environment and run with `current_dir` at the checkout root/`ui/`, giving them direct read access to `.git/config`.
- Lines 406-421: the only scrub (`scrub_nested_cargo_env`) removes `CARGO_*` env vars from cargo's environment — it does nothing about the on-disk git credential.
- Lines 116-124: the same stored credential is used for `git push --atomic origin main {tag}`, and with `persist-credentials: false` there is no other credential available — so the App token must be contents-write-capable for the workflow to function at all.

**Reachability assessment**: Every cited guard is confirmed ineffective as claimed — `persist-credentials: false` is overridden by the explicit `set-url`, `unset` only clears the env var, and `scrub_nested_cargo_env` never touches `.git/config`. Third-party dependency build code (cargo `build.rs`, test binaries) is a genuine untrusted code source that executes during the exposure window while the token is valid, with no mitigation between source and sink. I cannot disprove the chain.

```json
{
  "job_id": "panel:F1:reachability",
  "candidate_id": "F1",
  "verdict": "TRUE_POSITIVE",
  "severity": "HIGH",
  "reasoning": "Reachability confirmed end-to-end. Source: untrusted third-party dependency build code executes during the release smoke — verify_release (crates/quarry-dev/src/release.rs:294-327) runs `bun install --frozen-lockfile`, four `bun run` scripts, and `cargo test --locked --workspace`, and cargo unconditionally runs dependency build.rs scripts and test binaries; run_command/capture_command (release.rs:368-377, 399-404) spawn them with the checkout root as cwd and inherited environment. Sink: a live write-capable GitHub App token sits in the checkout's .git/config — the workflow stores it via `git remote set-url origin \"https://x-access-token:${RELEASE_TOKEN}@...\"` (.github/workflows/release-nightly.yml:69-70), and release.rs:116-124 relies on that stored credential for `git push --atomic origin main {tag}`; since the checkout used persist-credentials: false (workflow line 35) and no other credential exists, the App token must have contents-write scope for the workflow to function. The exposure window is real: the token is written to disk BEFORE `cargo --locked dev release --nightly` runs (workflow lines 69-72), release() takes the non-dry-run, non-skip-tests path (release.rs:44-54, 76-77) so verify_release executes while the token is on disk and valid. Every cited guard verified ineffective: persist-credentials: false is overridden by the explicit set-url (workflow line 35 vs 69-70); `unset RELEASE_TOKEN` (workflow line 71) clears only the env var, not .git/config; scrub_nested_cargo_env (release.rs:406-421) removes only CARGO_* environment variables from cargo's env and never touches the on-disk credential. No mitigation exists between the untrusted code execution and the readable repository-write credential.",
  "confidence": "high",
  "evidence": [
    ".github/workflows/release-nightly.yml:69-72",
    ".github/workflows/release-nightly.yml:35",
    "crates/quarry-dev/src/release.rs:76-77",
    "crates/quarry-dev/src/release.rs:294-327",
    "crates/quarry-dev/src/release.rs:368-377",
    "crates/quarry-dev/src/release.rs:406-421",
    "crates/quarry-dev/src/release.rs:116-124"
  ]
}
```