use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use quarry_core::{DocumentSource, WritePrecondition};
use quarry_fuse::mount_library;
use quarry_git::{
    export_worktree, import_worktree, pull_peer, push_peer, sync_peer, GitExportOptions,
};
use quarry_server::serve;
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::json;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "quarry")]
#[command(about = "Local-first document substrate for agents and developer tools")]
pub struct Cli {
    #[arg(long, env = "QUARRY_ROOT", default_value = ".quarry")]
    root: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init(InitCommand),
    Serve(ServeCommand),
    Mount(MountCommand),
    Get(DocumentPathCommand),
    Put(PutCommand),
    List(ListCommand),
    Move(MoveCommand),
    Delete(DocumentPathCommand),
    Tx(TxCommand),
    Git(GitCommand),
    Conflicts(ConflictsCommand),
    Gc,
    Backup { destination: PathBuf },
    Restore { source: PathBuf },
}

#[derive(Debug, Args)]
struct InitCommand {
    server_root: PathBuf,
}

#[derive(Debug, Args)]
struct ServeCommand {
    #[arg(long)]
    db: Option<PathBuf>,

    #[arg(long)]
    cas: Option<PathBuf>,

    #[arg(long, default_value = "127.0.0.1:7831")]
    addr: SocketAddr,
}

#[derive(Debug, Args)]
struct MountCommand {
    library: String,
    mountpoint: PathBuf,

    #[arg(long)]
    read_only: bool,

    #[arg(long)]
    serve_addr: Option<SocketAddr>,
}

#[derive(Debug, Args)]
struct DocumentPathCommand {
    library: String,
    path: String,
}

#[derive(Debug, Args)]
struct PutCommand {
    library: String,
    path: String,
    file: PathBuf,
}

#[derive(Debug, Args)]
struct ListCommand {
    library: String,

    #[arg(long)]
    prefix: Option<String>,
}

#[derive(Debug, Args)]
struct MoveCommand {
    library: String,
    from_path: String,
    to_path: String,
}

#[derive(Debug, Args)]
struct TxCommand {
    #[command(subcommand)]
    command: TxSubcommand,
}

#[derive(Debug, Subcommand)]
enum TxSubcommand {
    Begin { library: String },
    Commit { tx: String },
    Rollback { tx: String },
}

#[derive(Debug, Args)]
struct GitCommand {
    #[command(subcommand)]
    command: GitSubcommand,
}

#[derive(Debug, Subcommand)]
enum GitSubcommand {
    Peer(GitPeerCommand),
    Import {
        library: String,
        repo: PathBuf,
    },
    Export {
        library: String,
        repo: PathBuf,
        #[arg(long, default_value = "main")]
        branch: String,
        #[arg(long)]
        force_large: bool,
    },
    Sync {
        library: String,
        peer: String,
    },
    Pull {
        library: String,
        peer: String,
    },
    Push {
        library: String,
        peer: String,
    },
}

#[derive(Debug, Args)]
struct GitPeerCommand {
    #[command(subcommand)]
    command: GitPeerSubcommand,
}

#[derive(Debug, Subcommand)]
enum GitPeerSubcommand {
    Add {
        library: String,
        repo: PathBuf,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long, default_value = "main")]
        branch: String,
    },
    List {
        library: String,
    },
}

#[derive(Debug, Args)]
struct ConflictsCommand {
    #[command(subcommand)]
    command: ConflictsSubcommand,
}

#[derive(Debug, Subcommand)]
enum ConflictsSubcommand {
    List { library: String },
    Resolve { library: String, conflict: String },
}

pub async fn run() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Init(command) => {
            let store = open_at(&command.server_root, None, None).await?;
            drop(store);
            println!("{}", command.server_root.display());
            Ok(())
        }
        Command::Serve(command) => {
            let store = open_at(&cli.root, command.db, command.cas).await?;
            serve(store, command.addr).await?;
            Ok(())
        }
        Command::Mount(command) => {
            let store = open_at(&cli.root, None, None).await?;
            if let Some(addr) = command.serve_addr {
                let mount_store = store.clone();
                tokio::select! {
                    result = mount_library(
                        mount_store,
                        &command.library,
                        &command.mountpoint,
                        command.read_only,
                    ) => result?,
                    result = serve(store, addr) => result?,
                }
            } else {
                mount_library(
                    store,
                    &command.library,
                    &command.mountpoint,
                    command.read_only,
                )
                .await?;
            }
            Ok(())
        }
        Command::Get(command) => {
            let store = open_at(&cli.root, None, None).await?;
            let document = store.get_document(&command.library, &command.path).await?;
            print!("{}", String::from_utf8_lossy(&document.content));
            Ok(())
        }
        Command::Put(command) => {
            let store = open_at(&cli.root, None, None).await?;
            ensure_library(&store, &command.library).await?;
            let bytes = fs::read(&command.file)
                .with_context(|| format!("read {}", command.file.display()))?;
            let content_type = mime_guess::from_path(&command.file)
                .first_or_octet_stream()
                .essence_str()
                .to_string();
            let outcome = store
                .put_document(
                    &command.library,
                    &command.path,
                    bytes,
                    json!({"content_type": content_type}),
                    &content_type,
                    DocumentSource::Cli,
                    WritePrecondition::None,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&outcome)?);
            Ok(())
        }
        Command::List(command) => {
            let store = open_at(&cli.root, None, None).await?;
            let documents = store
                .list_documents(&command.library, command.prefix.as_deref(), None)
                .await?;
            println!("{}", serde_json::to_string_pretty(&documents)?);
            Ok(())
        }
        Command::Move(command) => {
            let store = open_at(&cli.root, None, None).await?;
            let tx = store
                .move_document(
                    &command.library,
                    &command.from_path,
                    &command.to_path,
                    DocumentSource::Cli,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&tx)?);
            Ok(())
        }
        Command::Delete(command) => {
            let store = open_at(&cli.root, None, None).await?;
            let tx = store
                .delete_document(&command.library, &command.path, DocumentSource::Cli)
                .await?;
            println!("{}", serde_json::to_string_pretty(&tx)?);
            Ok(())
        }
        Command::Tx(command) => run_tx(&cli.root, command).await,
        Command::Git(command) => run_git(&cli.root, command).await,
        Command::Conflicts(command) => run_conflicts(&cli.root, command).await,
        Command::Gc => {
            let store = open_at(&cli.root, None, None).await?;
            println!("{}", serde_json::to_string_pretty(&store.gc().await?)?);
            Ok(())
        }
        Command::Backup { destination } => {
            copy_dir(&cli.root, &destination)?;
            println!("{}", destination.display());
            Ok(())
        }
        Command::Restore { source } => {
            if cli.root.exists() {
                fs::remove_dir_all(&cli.root)?;
            }
            copy_dir(&source, &cli.root)?;
            println!("{}", cli.root.display());
            Ok(())
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

async fn run_tx(root: &Path, command: TxCommand) -> Result<()> {
    let store = open_at(root, None, None).await?;
    match command.command {
        TxSubcommand::Begin { library } => {
            ensure_library(&store, &library).await?;
            let tx = store
                .begin_transaction(
                    &library,
                    DocumentSource::Cli,
                    None,
                    Some("cli transaction".to_string()),
                    json!({}),
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&tx)?);
        }
        TxSubcommand::Commit { tx } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.commit_transaction(&tx).await?)?
            );
        }
        TxSubcommand::Rollback { tx } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.rollback_transaction(&tx).await?)?
            );
        }
    }
    Ok(())
}

async fn run_git(root: &Path, command: GitCommand) -> Result<()> {
    let store = open_at(root, None, None).await?;
    match command.command {
        GitSubcommand::Peer(command) => run_git_peer(&store, command).await?,
        GitSubcommand::Import { library, repo } => {
            ensure_library(&store, &library).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&import_worktree(&store, &library, &repo).await?)?
            );
        }
        GitSubcommand::Export {
            library,
            repo,
            branch,
            force_large,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &export_worktree(
                        &store,
                        &library,
                        &repo,
                        GitExportOptions {
                            branch,
                            force_large,
                            frontmatter_markdown: true,
                        },
                    )
                    .await?,
                )?
            );
        }
        GitSubcommand::Sync { library, peer } => {
            ensure_library(&store, &library).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&sync_peer(&store, &library, &peer).await?)?
            );
        }
        GitSubcommand::Pull { library, peer } => {
            ensure_library(&store, &library).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&pull_peer(&store, &library, &peer).await?)?
            );
        }
        GitSubcommand::Push { library, peer } => {
            ensure_library(&store, &library).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&push_peer(&store, &library, &peer).await?)?
            );
        }
    }
    Ok(())
}

async fn run_git_peer(store: &QuarryStore, command: GitPeerCommand) -> Result<()> {
    match command.command {
        GitPeerSubcommand::Add {
            library,
            repo,
            remote,
            branch,
        } => {
            ensure_library(store, &library).await?;
            let mut config = json!({
                "repo": repo,
                "branch": branch,
            });
            if let (Some(remote), Some(object)) = (remote, config.as_object_mut()) {
                object.insert("remote".to_string(), json!(remote));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&store.create_git_peer(&library, config).await?)?
            );
        }
        GitPeerSubcommand::List { library } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.list_git_peers(&library).await?)?
            );
        }
    }
    Ok(())
}

async fn run_conflicts(root: &Path, command: ConflictsCommand) -> Result<()> {
    let store = open_at(root, None, None).await?;
    match command.command {
        ConflictsSubcommand::List { library } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.list_conflicts(&library).await?)?
            );
        }
        ConflictsSubcommand::Resolve { library, conflict } => {
            let library = store.get_library(&library).await?;
            let conflict_record = store.get_conflict(&conflict).await?;
            if conflict_record.library_id != library.id {
                bail!("conflict {conflict} not found in library {}", library.slug);
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&store.resolve_conflict(&conflict).await?)?
            );
        }
    }
    Ok(())
}

async fn ensure_library(store: &QuarryStore, library: &str) -> Result<()> {
    if store.get_library(library).await.is_err() {
        store.create_library(library).await?;
    }
    Ok(())
}

async fn open_at(root: &Path, db: Option<PathBuf>, cas: Option<PathBuf>) -> Result<QuarryStore> {
    fs::create_dir_all(root)?;
    QuarryStore::open(StoreConfig {
        db_path: db.unwrap_or_else(|| root.join("quarry.db")),
        cas_path: cas.unwrap_or_else(|| root.join("cas")),
        lock_path: None,
    })
    .await
    .map_err(Into::into)
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_addr_defaults_to_loopback_and_can_be_overridden() {
        let cli = Cli::try_parse_from(["quarry", "serve"]).unwrap();
        let Command::Serve(command) = cli.command else {
            panic!("expected serve command");
        };
        assert_eq!(command.addr, "127.0.0.1:7831".parse().unwrap());

        let cli = Cli::try_parse_from(["quarry", "serve", "--addr", "127.0.0.1:9000"]).unwrap();
        let Command::Serve(command) = cli.command else {
            panic!("expected serve command");
        };
        assert_eq!(command.addr, "127.0.0.1:9000".parse().unwrap());
    }

    #[test]
    fn mount_can_expose_rest_api_from_same_process() {
        let cli = Cli::try_parse_from([
            "quarry",
            "mount",
            "notes",
            "/tmp/quarry-mount",
            "--serve-addr",
            "127.0.0.1:9000",
        ])
        .unwrap();
        let Command::Mount(command) = cli.command else {
            panic!("expected mount command");
        };
        assert_eq!(command.serve_addr, Some("127.0.0.1:9000".parse().unwrap()));
    }
}
