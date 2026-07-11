# Quarry Release Alignment Plan

## Goal

Fabro-style tag-first releases; correct SemVer; retain Quarry deploy safety.

## Plan

1. Add `.cargo/config.toml` alias `dev`; add private `crates/quarry-dev` CLI.
2. Implement/test `cargo dev release`: `--nightly`, `--bump patch|minor|major`, `--dry-run`, `--skip-tests`; require clean `main`; fetch tags; compute next SemVer; update `Cargo.toml`/`Cargo.lock`; run release smoke; commit; annotated tag; push branch+tag.
3. Version rules: after `v0.1.3`, nightlies use `0.1.4-nightly.YYYYMMDD[.N]`; stable promotes nightly base or bumps latest stable; tag must equal workspace version.
4. Replace publishing in `release-nightly.yml` with skip probe + GitHub App token + `cargo dev release --nightly`; add `nightly` environment.
5. Replace `release-stable.yml` with one `release.yml` on `v*` tag push; build once; classify `*-nightly.*`; validate tag/version; create GitHub release; publish Docker. Stable-only: formula + production deploy.
6. Keep local `cargo dev release --bump ...` as stable entrypoint. Document dry-run, required GitHub App, recovery, rerun rules.
7. Strengthen gates: Rust/UI release smoke before tag; packaged binary smoke; archive provenance; checksum verification. Keep pinned actions, least privilege, Docker per-arch health, digest-pinned AWS deploy, ALB/public checks.
8. Split immutable/mutable Docker publication: push version tag/digest; deploy+verify stable digest; then move `latest`. Keep `nightly` only for successful prereleases.
9. Make Homebrew update idempotent after immutable release succeeds; test generator; decide same-repo formula versus shared tap.
10. Delete obsolete workflow paths after one nightly and one stable dry run/test tag; update `AGENTS.md` release commands.

## Verification

- `cargo dev release --dry-run --nightly`; stable patch/minor fixtures; dirty tree, duplicate tag, non-main, tag/version mismatch failures.
- Workflow syntax/actionlint if available; Rust/UI CI; artifact checksums/attestations; amd64+arm64 image smoke.
- Test prerelease: no `latest`, no Homebrew stable, no deploy. Test stable: release, digest deploy, health, then `latest`/formula.

## Decisions

- Stable releases use `--bump` only; no explicit `--version`.
- Keep `Formula/quarry.rb` in this repository.
- Do not add mandatory production approval.
