use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, Utc};
use clap::{Args, ValueEnum};
use semver::{BuildMetadata, Prerelease, Version};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum VersionBump {
    Patch,
    Minor,
    Major,
}

#[derive(Debug, Args)]
pub(crate) struct ReleaseArgs {
    /// Cut a nightly prerelease instead of a stable release.
    #[arg(long)]
    nightly: bool,
    /// Stable version component to increment.
    #[arg(long, value_enum, default_value = "patch")]
    bump: VersionBump,
    /// Print the release without tests or repository mutations.
    #[arg(long)]
    dry_run: bool,
    /// Skip the Rust and browser release smoke.
    #[arg(long)]
    skip_tests: bool,
    /// UTC date used in nightly versions.
    #[arg(long, value_name = "YYYY-MM-DD", env = "QUARRY_RELEASE_DATE")]
    release_date: Option<NaiveDate>,
    /// Repository root to release.
    #[arg(long, hide = true)]
    root: Option<PathBuf>,
}

pub(crate) fn release(args: ReleaseArgs) -> Result<()> {
    let root = args.root.unwrap_or_else(workspace_root);
    let release_date = args.release_date.unwrap_or_else(|| Utc::now().date_naive());

    if !args.dry_run {
        require_release_checkout(&root)?;
        run_command(
            &root,
            &PlannedCommand::new("git")
                .arg("fetch")
                .arg("origin")
                .arg("--tags"),
        )?;
        require_synced_main(&root)?;
    }

    let cargo_toml = root.join("Cargo.toml");
    let current_version = read_workspace_version(&cargo_toml)?;
    let tags = release_tags(&root)?;
    let version = next_release_version(
        &current_version,
        &tags,
        args.bump,
        args.nightly,
        release_date,
    )?;
    let tag = format!("v{version}");

    println!("Current version: {current_version}");
    println!("Releasing {version} (tag {tag})");

    if args.dry_run {
        print_dry_run(&version, &tag, args.skip_tests);
        return Ok(());
    }

    if !args.skip_tests {
        verify_release(&root)?;
        require_release_checkout(&root)?;
    }

    let cargo_toml_source = std::fs::read_to_string(&cargo_toml)
        .with_context(|| format!("reading {}", cargo_toml.display()))?;
    let updated = replace_workspace_version(&cargo_toml_source, &version)?;
    std::fs::write(&cargo_toml, updated)
        .with_context(|| format!("writing {}", cargo_toml.display()))?;

    run_command(
        &root,
        &PlannedCommand::new("cargo")
            .arg("update")
            .arg("--workspace"),
    )?;
    run_command(
        &root,
        &PlannedCommand::new("git")
            .arg("add")
            .arg("Cargo.toml")
            .arg("Cargo.lock"),
    )?;
    run_command(
        &root,
        &PlannedCommand::new("git")
            .arg("commit")
            .arg("-m")
            .arg(format!("build: bump version to {version}")),
    )?;
    run_command(
        &root,
        &PlannedCommand::new("git")
            .arg("tag")
            .arg("-a")
            .arg(&tag)
            .arg("-m")
            .arg(&tag),
    )?;
    run_command(
        &root,
        &PlannedCommand::new("git")
            .arg("push")
            .arg("--atomic")
            .arg("origin")
            .arg("main")
            .arg(&tag),
    )?;

    println!("Released {tag}");
    println!("Watch the build: https://github.com/fabro-sh/quarry/actions");
    Ok(())
}

fn workspace_root() -> PathBuf {
    let mut root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    root.pop();
    root.pop();
    root
}

fn require_release_checkout(root: &Path) -> Result<()> {
    let branch = capture_stdout(root, "git", &["branch", "--show-current"])?;
    if branch.trim() != "main" {
        bail!("releases must be cut from main; current branch is {branch:?}");
    }

    let status = capture_stdout(
        root,
        "git",
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    if !status.is_empty() {
        bail!("working tree is dirty; commit or stash changes before releasing");
    }
    Ok(())
}

fn require_synced_main(root: &Path) -> Result<()> {
    let head = capture_stdout(root, "git", &["rev-parse", "HEAD"])?;
    let remote = capture_stdout(root, "git", &["rev-parse", "origin/main"])?;
    if head != remote {
        bail!("local main must exactly match origin/main before releasing");
    }
    Ok(())
}

fn release_tags(root: &Path) -> Result<Vec<String>> {
    let output = capture_stdout(root, "git", &["tag", "--list", "v*"])?;
    Ok(output.lines().map(str::to_owned).collect())
}

fn next_release_version(
    current_version: &Version,
    tags: &[String],
    bump: VersionBump,
    nightly: bool,
    release_date: NaiveDate,
) -> Result<Version> {
    let stable_versions = tags
        .iter()
        .filter_map(|tag| tag.strip_prefix('v'))
        .filter_map(|raw| Version::parse(raw).ok())
        .filter(|version| version.pre.is_empty());
    let latest_stable = stable_versions.max().unwrap_or_else(|| {
        let mut fallback = current_version.clone();
        fallback.pre = Prerelease::EMPTY;
        fallback.build = BuildMetadata::EMPTY;
        fallback
    });
    let base = bump_version(latest_stable, bump)?;

    if !nightly {
        let tag = format!("v{base}");
        if tags.contains(&tag) {
            bail!("release tag {tag} already exists");
        }
        return Ok(base);
    }

    let date = release_date.format("%Y%m%d");
    let first = with_prerelease(&base, &format!("nightly.{date}"))?;
    if !tags.contains(&format!("v{first}")) {
        return Ok(first);
    }

    for sequence in 2_u32.. {
        let candidate = with_prerelease(&base, &format!("nightly.{date}.{sequence}"))?;
        if !tags.contains(&format!("v{candidate}")) {
            return Ok(candidate);
        }
    }
    unreachable!("the nightly sequence has an unbounded candidate space")
}

fn bump_version(mut version: Version, bump: VersionBump) -> Result<Version> {
    version.pre = Prerelease::EMPTY;
    version.build = BuildMetadata::EMPTY;
    match bump {
        VersionBump::Patch => {
            version.patch = version.patch.checked_add(1).context("patch overflow")?;
        }
        VersionBump::Minor => {
            version.minor = version.minor.checked_add(1).context("minor overflow")?;
            version.patch = 0;
        }
        VersionBump::Major => {
            version.major = version.major.checked_add(1).context("major overflow")?;
            version.minor = 0;
            version.patch = 0;
        }
    }
    Ok(version)
}

fn with_prerelease(base: &Version, prerelease: &str) -> Result<Version> {
    let mut version = base.clone();
    version.pre = Prerelease::new(prerelease)
        .with_context(|| format!("constructing prerelease {prerelease}"))?;
    Ok(version)
}

fn read_workspace_version(path: &Path) -> Result<Version> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw = workspace_version_value(&source)?;
    Version::parse(raw).with_context(|| format!("parsing workspace version {raw}"))
}

fn workspace_version_value(source: &str) -> Result<&str> {
    let mut in_workspace_package = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_workspace_package = trimmed == "[workspace.package]";
            continue;
        }
        if in_workspace_package && let Some(raw) = trimmed.strip_prefix("version = ") {
            return raw
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .context("workspace package version must be a quoted string");
        }
    }
    bail!("Cargo.toml is missing [workspace.package] version")
}

fn replace_workspace_version(source: &str, version: &Version) -> Result<String> {
    let mut output = String::with_capacity(source.len());
    let mut in_workspace_package = false;
    let mut replaced = false;

    for line in source.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim();
        if trimmed.starts_with('[') {
            in_workspace_package = trimmed == "[workspace.package]";
        }

        if in_workspace_package && trimmed.starts_with("version = ") {
            if replaced {
                bail!("Cargo.toml has multiple [workspace.package] version entries");
            }
            let indent_len = content.len() - content.trim_start().len();
            writeln!(output, "{}version = \"{version}\"", &content[..indent_len])?;
            replaced = true;
        } else {
            output.push_str(line);
        }
    }

    if !replaced {
        bail!("Cargo.toml is missing [workspace.package] version");
    }
    Ok(output)
}

fn verify_release(root: &Path) -> Result<()> {
    let ui = root.join("ui");
    for command in [
        PlannedCommand::new("bun")
            .arg("install")
            .arg("--frozen-lockfile"),
        PlannedCommand::new("bun").arg("run").arg("fixtures:check"),
        PlannedCommand::new("bun").arg("run").arg("typecheck"),
        PlannedCommand::new("bun").arg("run").arg("test"),
        PlannedCommand::new("bun").arg("run").arg("build"),
    ] {
        run_command(&ui, &command)?;
    }
    run_command(
        root,
        &PlannedCommand::new("cargo")
            .arg("test")
            .arg("--locked")
            .arg("--workspace"),
    )?;
    run_command(
        root,
        &PlannedCommand::new("cargo")
            .arg("test")
            .arg("--locked")
            .arg("-p")
            .arg("quarry-server")
            .arg("-p")
            .arg("quarry-cli")
            .arg("-p")
            .arg("quarry")
            .arg("--all-features"),
    )
}

fn print_dry_run(version: &Version, tag: &str, skip_tests: bool) {
    if skip_tests {
        println!("DRY RUN: would skip the release smoke");
    } else {
        println!("DRY RUN: would run the Rust and browser release smoke");
    }
    println!("DRY RUN: would update Cargo.toml and Cargo.lock to {version}");
    println!("git commit -m 'build: bump version to {version}'");
    println!("git tag -a {tag} -m {tag}");
    println!("git push --atomic origin main {tag}");
}

struct PlannedCommand {
    program: String,
    args: Vec<String>,
}

impl PlannedCommand {
    fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    fn display(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|part| format!("{part:?}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn run_command(root: &Path, planned: &PlannedCommand) -> Result<()> {
    let status = prepared_command(planned)
        .current_dir(root)
        .status()
        .with_context(|| format!("running {}", planned.display()))?;
    if !status.success() {
        bail!("command failed with {status}: {}", planned.display());
    }
    Ok(())
}

fn capture_stdout(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let planned = args
        .iter()
        .fold(PlannedCommand::new(program), |command, arg| {
            command.arg(*arg)
        });
    let output = capture_command(root, &planned)?;
    if !output.status.success() {
        bail!(
            "command failed with {}: {}\n{}",
            output.status,
            planned.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout)
        .context("command output was not UTF-8")
        .map(|output| output.trim_end().to_string())
}

fn capture_command(root: &Path, planned: &PlannedCommand) -> Result<Output> {
    prepared_command(planned)
        .current_dir(root)
        .output()
        .with_context(|| format!("running {}", planned.display()))
}

fn prepared_command(planned: &PlannedCommand) -> Command {
    let mut command = Command::new(&planned.program);
    command.args(&planned.args);
    if planned.program == "cargo" {
        scrub_nested_cargo_env(&mut command);
    }
    command
}

fn scrub_nested_cargo_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if is_cargo_build_env(&key) {
            command.env_remove(key);
        }
    }
}

fn is_cargo_build_env(key: &OsStr) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    matches!(
        key,
        "CARGO_BIN_NAME"
            | "CARGO_CRATE_NAME"
            | "CARGO_MANIFEST_DIR"
            | "CARGO_MANIFEST_PATH"
            | "CARGO_PRIMARY_PACKAGE"
            | "DEBUG"
            | "HOST"
            | "NUM_JOBS"
            | "OPT_LEVEL"
            | "OUT_DIR"
            | "PROFILE"
            | "RUSTC"
            | "RUSTDOC"
            | "TARGET"
    ) || key.starts_with("CARGO_BIN_EXE_")
        || key.starts_with("CARGO_CFG_")
        || key.starts_with("CARGO_FEATURE_")
        || key.starts_with("CARGO_PKG_")
        || key.starts_with("DEP_")
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use chrono::NaiveDate;
    use semver::Version;

    use super::{
        VersionBump, next_release_version, replace_workspace_version, require_release_checkout,
    };

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 11).expect("test date should be valid")
    }

    #[test]
    fn nightly_uses_next_patch_after_latest_stable() -> anyhow::Result<()> {
        let version = next_release_version(
            &Version::parse("0.1.3")?,
            &["v0.1.2".into(), "v0.1.3".into()],
            VersionBump::Patch,
            true,
            date(),
        )?;
        assert_eq!(version, Version::parse("0.1.4-nightly.20260711")?);
        Ok(())
    }

    #[test]
    fn nightly_increments_an_occupied_date() -> anyhow::Result<()> {
        let version = next_release_version(
            &Version::parse("0.1.4-nightly.20260711")?,
            &[
                "v0.1.3".into(),
                "v0.1.4-nightly.20260711".into(),
                "v0.1.4-nightly.20260711.2".into(),
            ],
            VersionBump::Patch,
            true,
            date(),
        )?;
        assert_eq!(version, Version::parse("0.1.4-nightly.20260711.3")?);
        Ok(())
    }

    #[test]
    fn stable_bump_can_promote_patch_or_choose_minor() -> anyhow::Result<()> {
        let tags = ["v0.1.3".into(), "v0.1.4-nightly.20260711".into()];
        let patch = next_release_version(
            &Version::parse("0.1.4-nightly.20260711")?,
            &tags,
            VersionBump::Patch,
            false,
            date(),
        )?;
        let minor = next_release_version(
            &Version::parse("0.1.4-nightly.20260711")?,
            &tags,
            VersionBump::Minor,
            false,
            date(),
        )?;
        assert_eq!(patch, Version::parse("0.1.4")?);
        assert_eq!(minor, Version::parse("0.2.0")?);
        Ok(())
    }

    #[test]
    fn manifest_update_only_changes_workspace_package_version() -> anyhow::Result<()> {
        let source = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.3\"\n\n[dependencies]\nversion = \"1\"\n";
        let updated = replace_workspace_version(source, &Version::parse("0.1.4")?)?;
        assert_eq!(
            updated,
            "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.4\"\n\n[dependencies]\nversion = \"1\"\n"
        );
        Ok(())
    }

    #[test]
    fn release_checkout_rejects_dirty_main() -> anyhow::Result<()> {
        let fixture = git_fixture()?;
        std::fs::write(fixture.path().join("dirty.txt"), "dirty\n")?;

        let error = require_release_checkout(fixture.path())
            .expect_err("dirty release checkout should be rejected");
        assert!(error.to_string().contains("working tree is dirty"));
        Ok(())
    }

    #[test]
    fn release_checkout_rejects_non_main_branch() -> anyhow::Result<()> {
        let fixture = git_fixture()?;
        git(fixture.path(), &["switch", "-c", "feature"])?;

        let error = require_release_checkout(fixture.path())
            .expect_err("non-main release checkout should be rejected");
        assert!(error.to_string().contains("releases must be cut from main"));
        Ok(())
    }

    fn git_fixture() -> anyhow::Result<tempfile::TempDir> {
        let fixture = tempfile::tempdir()?;
        git(fixture.path(), &["init", "-b", "main"])?;
        git(fixture.path(), &["config", "user.name", "Release Test"])?;
        git(
            fixture.path(),
            &["config", "user.email", "release@example.com"],
        )?;
        std::fs::write(fixture.path().join("README.md"), "fixture\n")?;
        git(fixture.path(), &["add", "README.md"])?;
        git(fixture.path(), &["commit", "-m", "initial"])?;
        Ok(fixture)
    }

    fn git(root: &Path, args: &[&str]) -> anyhow::Result<()> {
        let output = Command::new("git").args(args).current_dir(root).output()?;
        anyhow::ensure!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
}
