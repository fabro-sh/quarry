use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod release;

#[derive(Debug, Parser)]
#[command(
    name = "quarry-dev",
    version,
    about = "Internal development tooling for Quarry"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate, version, commit, tag, and push a Quarry release.
    Release(release::ReleaseArgs),
}

impl Command {
    fn run(self) -> Result<()> {
        match self {
            Self::Release(args) => release::release(args),
        }
    }
}

pub fn run() -> ExitCode {
    match Cli::parse().command.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("quarry-dev failed");
            for cause in error.chain() {
                eprintln!("  caused by: {cause}");
            }
            ExitCode::FAILURE
        }
    }
}
