use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use quarry_core::{DocumentSource, WritePrecondition};
use quarry_fuse::mount_library_with_shutdown;
use quarry_git::{
    export_worktree, import_worktree, pull_peer, push_peer, sync_peer, GitExportOptions,
};
use quarry_server::{serve, serve_with_shutdown, shutdown_signal};
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::json;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

mod logging {
    use tracing_subscriber::EnvFilter;

    pub const DEVELOPMENT_FILTER: &str = "warn,quarry=debug,quarry_cli=debug,quarry_server=debug,quarry_storage=debug,quarry_git=debug,quarry_fuse=debug,quarry_cas=debug,quarry_collab_codec=debug";

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LogConfig {
        pub filter: String,
        pub format: LogFormat,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LogFormat {
        Pretty,
        Json,
    }

    impl LogConfig {
        pub fn from_env() -> Self {
            let rust_log = std::env::var("RUST_LOG").ok();
            let format = std::env::var("QUARRY_LOG_FORMAT").ok();
            Self::from_env_values(rust_log.as_deref(), format.as_deref())
        }

        pub fn from_env_values(rust_log: Option<&str>, format: Option<&str>) -> Self {
            let requested_filter = rust_log
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEVELOPMENT_FILTER);
            let filter = if EnvFilter::try_new(requested_filter).is_ok() {
                requested_filter.to_string()
            } else {
                DEVELOPMENT_FILTER.to_string()
            };

            Self {
                filter,
                format: LogFormat::from_env_value(format),
            }
        }

        fn env_filter(&self) -> EnvFilter {
            EnvFilter::try_new(&self.filter).unwrap_or_else(|_| EnvFilter::new(DEVELOPMENT_FILTER))
        }
    }

    impl LogFormat {
        fn from_env_value(value: Option<&str>) -> Self {
            match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
                Some("json") => Self::Json,
                _ => Self::Pretty,
            }
        }

        fn as_str(self) -> &'static str {
            match self {
                Self::Pretty => "pretty",
                Self::Json => "json",
            }
        }
    }

    pub fn init() {
        let config = LogConfig::from_env();
        let result = match config.format {
            LogFormat::Pretty => tracing_subscriber::fmt()
                .with_env_filter(config.env_filter())
                .with_writer(std::io::stderr)
                .pretty()
                .try_init(),
            LogFormat::Json => tracing_subscriber::fmt()
                .with_env_filter(config.env_filter())
                .with_writer(std::io::stderr)
                .json()
                .flatten_event(true)
                .try_init(),
        };

        if result.is_ok() {
            tracing::debug!(
                event = "logging.initialized",
                log_format = config.format.as_str(),
                filter = %config.filter,
                "logging initialized"
            );
        }
    }
}

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
    Share(ShareCommand),
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
struct ShareCommand {
    library: String,
    path: String,

    #[arg(long, default_value = "editor")]
    role: String,

    #[arg(long)]
    by: Option<String>,
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
                let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
                tokio::spawn(async move {
                    shutdown_signal().await;
                    let _ = shutdown_tx.send(true);
                });
                let mount_store = store.clone();
                let mount_shutdown = wait_for_shutdown(shutdown_rx.clone());
                let server_shutdown = wait_for_shutdown(shutdown_rx);
                tokio::try_join!(
                    async {
                        mount_library_with_shutdown(
                            mount_store,
                            &command.library,
                            &command.mountpoint,
                            command.read_only,
                            mount_shutdown,
                        )
                        .await
                        .map_err(anyhow::Error::from)
                    },
                    async {
                        serve_with_shutdown(store, addr, server_shutdown)
                            .await
                            .map_err(anyhow::Error::from)
                    },
                )?;
            } else {
                mount_library_with_shutdown(
                    store,
                    &command.library,
                    &command.mountpoint,
                    command.read_only,
                    shutdown_signal(),
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
        Command::Share(command) => {
            let store = open_at(&cli.root, None, None).await?;
            let token = store
                .create_collab_invite_token(
                    &command.library,
                    &command.path,
                    &command.role,
                    command.by,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&token)?);
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
    logging::init();
}

async fn wait_for_shutdown(mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
    while !*shutdown_rx.borrow_and_update() {
        if shutdown_rx.changed().await.is_err() {
            break;
        }
    }
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
    fn default_logging_config_enables_quarry_crates_at_debug_and_dependencies_at_warn() {
        let config = logging::LogConfig::from_env_values(None, None);

        assert_eq!(config.format, logging::LogFormat::Pretty);
        assert!(config
            .filter
            .split(',')
            .any(|directive| directive == "warn"));
        for crate_name in [
            "quarry",
            "quarry_cli",
            "quarry_server",
            "quarry_storage",
            "quarry_git",
            "quarry_fuse",
            "quarry_cas",
            "quarry_collab_codec",
        ] {
            assert!(
                config
                    .filter
                    .split(',')
                    .any(|directive| directive == format!("{crate_name}=debug")),
                "default filter should enable {crate_name} at debug: {}",
                config.filter
            );
        }
    }

    #[test]
    fn rust_log_overrides_development_default_filter() {
        let config = logging::LogConfig::from_env_values(Some("info,quarry_storage=trace"), None);

        assert_eq!(config.filter, "info,quarry_storage=trace");
    }

    #[test]
    fn json_log_format_is_selected_from_env() {
        let config = logging::LogConfig::from_env_values(None, Some("json"));

        assert_eq!(config.format, logging::LogFormat::Json);
    }

    #[test]
    fn invalid_log_format_falls_back_to_pretty() {
        let config = logging::LogConfig::from_env_values(None, Some("yaml"));

        assert_eq!(config.format, logging::LogFormat::Pretty);
    }

    #[test]
    fn invalid_rust_log_falls_back_to_development_default_filter() {
        let config = logging::LogConfig::from_env_values(Some("quarry=debug,="), None);

        assert_eq!(config.filter, logging::DEVELOPMENT_FILTER);
    }

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

    #[test]
    fn share_command_mints_editor_tokens_by_default() {
        let cli =
            Cli::try_parse_from(["quarry", "share", "notes", "live.md", "--by", "Avery"]).unwrap();
        let Command::Share(command) = cli.command else {
            panic!("expected share command");
        };
        assert_eq!(command.library, "notes");
        assert_eq!(command.path, "live.md");
        assert_eq!(command.role, "editor");
        assert_eq!(command.by.as_deref(), Some("Avery"));
    }
}
