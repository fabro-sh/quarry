use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use quarry_store::{Actor, ActorKind, LocalStore};
use std::fs;
use std::io::{self, Read};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "quarry")]
#[command(about = "Local-first collaborative storage substrate")]
struct Cli {
    #[arg(long, env = "QUARRY_DATA_DIR", default_value = ".quarry")]
    data_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Doctor,
    Server(ServerCommand),
    Status,
    Sync(GitMaterializeCommand),
    Compact,
    Refs,
    Write(WriteCommand),
    Read(ReadCommand),
    Delete(DeleteCommand),
    Comment(CommentCommand),
    Annotations(AnnotationsCommand),
    Events(EventsCommand),
    Snapshots(SnapshotsCommand),
    Restore(RestoreCommand),
    Document(DocumentCommand),
    Draft(DraftCommand),
    Binary(BinaryCommand),
    Export(ExportCommand),
    Git(GitCommand),
    Inspect(InspectCommand),
}

#[derive(Debug, Args)]
struct ServerCommand {
    #[arg(long, default_value = "127.0.0.1:7831")]
    addr: SocketAddr,
}

#[derive(Debug, Args)]
struct WriteCommand {
    path: String,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[arg(long, conflicts_with_all = ["content_file", "stdin"])]
    content: Option<String>,

    #[arg(long, conflicts_with_all = ["content", "stdin"])]
    content_file: Option<PathBuf>,

    #[arg(long, conflicts_with_all = ["content", "content_file"])]
    stdin: bool,

    #[arg(long)]
    message: Option<String>,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct ReadCommand {
    path: String,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,
}

#[derive(Debug, Args)]
struct DeleteCommand {
    path: String,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct CommentCommand {
    #[arg(long)]
    target: String,

    #[arg(long)]
    body: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct AnnotationsCommand {
    #[arg(long)]
    target: Option<String>,
}

#[derive(Debug, Args)]
struct EventsCommand {
    #[arg(long, default_value_t = 50)]
    limit: u64,

    #[arg(long)]
    target: Option<String>,
}

#[derive(Debug, Args)]
struct SnapshotsCommand {
    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[arg(long, default_value_t = 50)]
    limit: u64,
}

#[derive(Debug, Args)]
struct RestoreCommand {
    snapshot_id: String,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct DocumentCommand {
    #[command(subcommand)]
    command: DocumentSubcommand,
}

#[derive(Debug, Subcommand)]
enum DocumentSubcommand {
    Create(DocumentCreateCommand),
    State { id: String },
    Op(DocumentOpCommand),
    Presence(DocumentPresenceCommand),
    Snapshots(DocumentSnapshotsCommand),
    Restore(DocumentRestoreCommand),
}

#[derive(Debug, Args)]
struct DocumentCreateCommand {
    path: String,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[arg(long)]
    title: Option<String>,

    #[arg(long)]
    text: Option<String>,

    #[arg(long)]
    snapshot_json: Option<String>,

    #[arg(long)]
    message: Option<String>,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct DocumentOpCommand {
    id: String,

    #[arg(long)]
    op_json: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct DocumentPresenceCommand {
    id: String,

    #[arg(long)]
    cursor_json: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct DocumentSnapshotsCommand {
    id: String,

    #[arg(long, default_value_t = 50)]
    limit: u64,
}

#[derive(Debug, Args)]
struct DocumentRestoreCommand {
    id: String,
    snapshot_id: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct DraftCommand {
    #[command(subcommand)]
    command: DraftSubcommand,
}

#[derive(Debug, Subcommand)]
enum DraftSubcommand {
    Start(DraftStartCommand),
    Publish(DraftPublishCommand),
}

#[derive(Debug, Args)]
struct DraftStartCommand {
    #[arg(long = "base", default_value = "published/main")]
    base_ref: String,

    #[arg(long)]
    name: Option<String>,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct DraftPublishCommand {
    source_ref: String,

    #[arg(long = "target", default_value = "published/main")]
    target_ref: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct BinaryCommand {
    #[command(subcommand)]
    command: BinarySubcommand,
}

#[derive(Debug, Subcommand)]
enum BinarySubcommand {
    Add(BinaryAddCommand),
    List,
}

#[derive(Debug, Args)]
struct BinaryAddCommand {
    path: String,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[arg(long)]
    file: Option<PathBuf>,

    #[arg(long)]
    hash: Option<String>,

    #[arg(long)]
    size: Option<u64>,

    #[arg(long)]
    media_type: Option<String>,

    #[arg(long)]
    external_url: Option<String>,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct ExportCommand {
    out_dir: PathBuf,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,
}

#[derive(Debug, Args)]
struct GitCommand {
    #[command(subcommand)]
    command: GitSubcommand,
}

#[derive(Debug, Subcommand)]
enum GitSubcommand {
    Materialize(GitMaterializeCommand),
    Ingest(GitIngestCommand),
}

#[derive(Debug, Args)]
struct GitMaterializeCommand {
    repo_dir: PathBuf,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[arg(long, default_value = "main")]
    branch: String,

    #[arg(long)]
    message: Option<String>,
}

#[derive(Debug, Args)]
struct GitIngestCommand {
    repo_dir: PathBuf,

    #[arg(long = "ref", default_value = "published/main")]
    ref_name: String,

    #[command(flatten)]
    actor: ActorOptions,
}

#[derive(Debug, Args)]
struct InspectCommand {
    #[command(subcommand)]
    target: InspectTarget,
}

#[derive(Debug, Subcommand)]
enum InspectTarget {
    Transaction { id: String },
}

#[derive(Clone, Debug, Args)]
struct ActorOptions {
    #[arg(long, default_value = "local")]
    actor_id: String,

    #[arg(long)]
    actor_name: Option<String>,

    #[arg(long, value_enum, default_value_t = ActorKindArg::Human)]
    actor_kind: ActorKindArg,

    #[arg(long)]
    actor_avatar_url: Option<String>,
}

#[derive(Clone, Debug, ValueEnum)]
enum ActorKindArg {
    Human,
    Agent,
    GitImport,
    System,
    Integration,
}

impl ActorOptions {
    fn into_actor(self) -> Actor {
        let kind = match self.actor_kind {
            ActorKindArg::Human => ActorKind::Human,
            ActorKindArg::Agent => ActorKind::Agent,
            ActorKindArg::GitImport => ActorKind::GitImport,
            ActorKindArg::System => ActorKind::System,
            ActorKindArg::Integration => ActorKind::Integration,
        };

        Actor {
            display_name: self.actor_name.unwrap_or_else(|| self.actor_id.clone()),
            id: self.actor_id,
            kind,
            avatar_url: self.actor_avatar_url,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.status()?)?;
        }
        Command::Doctor => {
            let store = LocalStore::open(&cli.data_dir)?;
            let status = store.status()?;
            eprintln!("quarry data dir: {}", store.data_dir().display());
            eprintln!("workspace id: {}", status.workspace_id);
            eprintln!("sqlite: ok");
            eprintln!("wal: enabled");
        }
        Command::Server(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            eprintln!("quarry server listening on http://{}", command.addr);
            quarry_api::serve(store, command.addr).await?;
        }
        Command::Status => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.status()?)?;
        }
        Command::Sync(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.materialize_git(
                &command.ref_name,
                command.repo_dir,
                &command.branch,
                command.message.as_deref(),
            )?)?;
        }
        Command::Compact => {
            let store = LocalStore::open(&cli.data_dir)?;
            store.compact()?;
            print_json(&serde_json::json!({ "compacted": true }))?;
        }
        Command::Refs => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.list_refs()?)?;
        }
        Command::Write(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            let content = read_content(command.content, command.content_file, command.stdin)?;
            let result = store.write_text(
                &command.ref_name,
                &command.path,
                &content,
                command.actor.into_actor(),
                command.message,
            )?;
            print_json(&result)?;
        }
        Command::Read(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            print!("{}", store.read_text(&command.ref_name, &command.path)?);
        }
        Command::Delete(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.delete_path(
                &command.ref_name,
                &command.path,
                command.actor.into_actor(),
            )?)?;
        }
        Command::Comment(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            let annotation = store.create_annotation(
                &command.target,
                &command.body,
                command.actor.into_actor(),
            )?;
            print_json(&annotation)?;
        }
        Command::Annotations(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.list_annotations(command.target.as_deref())?)?;
        }
        Command::Events(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.list_events(command.limit, command.target.as_deref())?)?;
        }
        Command::Snapshots(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.list_ref_snapshots(&command.ref_name, command.limit)?)?;
        }
        Command::Restore(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            print_json(&store.restore_ref_snapshot(
                &command.ref_name,
                &command.snapshot_id,
                command.actor.into_actor(),
            )?)?;
        }
        Command::Document(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            match command.command {
                DocumentSubcommand::Create(command) => {
                    let snapshot = if let Some(snapshot_json) = command.snapshot_json {
                        serde_json::from_str(&snapshot_json)?
                    } else {
                        let text = command.text.unwrap_or_default();
                        serde_json::json!({
                            "schema": "quarry.structured_doc.v1",
                            "format": "plain_text",
                            "text": text.clone(),
                            "blocks": [{ "type": "p", "children": [{ "text": text.clone() }] }]
                        })
                    };
                    print_json(&store.create_document(
                        &command.ref_name,
                        &command.path,
                        command.title.as_deref(),
                        snapshot,
                        command.actor.into_actor(),
                        command.message,
                    )?)?;
                }
                DocumentSubcommand::State { id } => {
                    print_json(&store.document_state(&id)?)?;
                }
                DocumentSubcommand::Op(command) => {
                    let op = serde_json::from_str(&command.op_json)?;
                    print_json(&store.append_document_op(
                        &command.id,
                        op,
                        command.actor.into_actor(),
                    )?)?;
                }
                DocumentSubcommand::Presence(command) => {
                    let cursor = serde_json::from_str(&command.cursor_json)?;
                    print_json(&store.upsert_presence(
                        &command.id,
                        command.actor.into_actor(),
                        cursor,
                    )?)?;
                }
                DocumentSubcommand::Snapshots(command) => {
                    print_json(&store.list_document_snapshots(&command.id, command.limit)?)?;
                }
                DocumentSubcommand::Restore(command) => {
                    print_json(&store.restore_document_snapshot(
                        &command.id,
                        &command.snapshot_id,
                        command.actor.into_actor(),
                    )?)?;
                }
            }
        }
        Command::Draft(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            match command.command {
                DraftSubcommand::Start(command) => {
                    print_json(&store.create_draft(
                        &command.base_ref,
                        command.name.as_deref(),
                        command.actor.into_actor(),
                    )?)?;
                }
                DraftSubcommand::Publish(command) => {
                    print_json(&store.publish_ref(
                        &command.source_ref,
                        &command.target_ref,
                        command.actor.into_actor(),
                    )?)?;
                }
            }
        }
        Command::Binary(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            match command.command {
                BinarySubcommand::Add(command) => {
                    let actor = command.actor.into_actor();
                    if let Some(file) = command.file {
                        print_json(&store.add_binary_file(
                            &command.ref_name,
                            &command.path,
                            file,
                            command.media_type.as_deref(),
                            actor,
                        )?)?;
                    } else {
                        let hash = command
                            .hash
                            .ok_or_else(|| anyhow::anyhow!("--hash is required without --file"))?;
                        let size = command
                            .size
                            .ok_or_else(|| anyhow::anyhow!("--size is required without --file"))?;
                        print_json(&store.add_binary_pointer(
                            &command.ref_name,
                            &command.path,
                            &hash,
                            size,
                            command.media_type.as_deref(),
                            None,
                            command.external_url.as_deref(),
                            actor,
                        )?)?;
                    }
                }
                BinarySubcommand::List => {
                    print_json(&store.list_binary_objects()?)?;
                }
            }
        }
        Command::Export(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            store.export_ref(&command.ref_name, &command.out_dir)?;
            print_json(&serde_json::json!({
                "ref": command.ref_name,
                "out_dir": command.out_dir,
                "mode": "raw_export"
            }))?;
        }
        Command::Git(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            match command.command {
                GitSubcommand::Materialize(command) => {
                    print_json(&store.materialize_git(
                        &command.ref_name,
                        command.repo_dir,
                        &command.branch,
                        command.message.as_deref(),
                    )?)?;
                }
                GitSubcommand::Ingest(command) => {
                    print_json(&store.ingest_git(
                        command.repo_dir,
                        &command.ref_name,
                        command.actor.into_actor(),
                    )?)?;
                }
            }
        }
        Command::Inspect(command) => {
            let store = LocalStore::open(&cli.data_dir)?;
            match command.target {
                InspectTarget::Transaction { id } => {
                    print_json(&store.get_transaction(&id)?)?;
                }
            }
        }
    }

    Ok(())
}

fn read_content(
    content: Option<String>,
    content_file: Option<PathBuf>,
    read_stdin: bool,
) -> Result<String> {
    match (content, content_file, read_stdin) {
        (Some(content), None, false) => Ok(content),
        (None, Some(path), false) => {
            fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))
        }
        (None, None, true) => {
            let mut buffer = String::new();
            io::stdin().read_to_string(&mut buffer)?;
            Ok(buffer)
        }
        (None, None, false) => bail!("provide --content, --content-file, or --stdin"),
        _ => bail!("provide only one content source"),
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
