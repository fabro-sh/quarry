use git2::{
    FetchOptions, IndexAddOption, ObjectType, PushOptions, Repository, Signature,
    build::{CheckoutBuilder, RepoBuilder},
};
use quarry_core::{
    ConflictRecord, DocumentListEntry, DocumentSource, GIT_BINARY_WARN_THRESHOLD, Library,
    QuarryError, Result, SyncStateEntry, WriteOutcome, normalize_path, render_markdown_frontmatter,
};
use quarry_storage::{
    BlockMarkdownWrite, BlockMarkdownWriteOutcome, BlockWriteBase, DocumentKind, DocumentScopeRef,
    PutDocumentRequest, QuarryStore, TransactionMetadata, split_markdown_frontmatter,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Semaphore;
use utoipa::ToSchema;
use uuid::Uuid;
use walkdir::WalkDir;

const LOCAL_GIT_OPERATION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REMOTE_GIT_OPERATION_TIMEOUT: Duration = Duration::from_secs(2 * 60);

// Git peer operations are already serialized by QuarryStore's global operation
// gate. Keeping the blocking lane single-file preserves that invariant for the
// public import/export helpers too, and keeps a timed-out spawn_blocking job
// from racing a later job while the first closure finishes in the background.
static GIT_BLOCKING_LANE: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("path is outside the git worktree: {0}")]
    WorktreePath(#[from] std::path::StripPrefixError),
    #[error("Git blocking lane closed during {operation}")]
    BlockingLaneClosed { operation: &'static str },
    #[error("Git blocking task failed during {operation}")]
    BlockingTask {
        operation: &'static str,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("Git operation {operation} timed out after {timeout_seconds} seconds")]
    OperationTimedOut {
        operation: &'static str,
        timeout_seconds: u64,
    },
}

impl From<GitError> for QuarryError {
    fn from(err: GitError) -> Self {
        Self::GitSource {
            source: Box::new(err),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitExportOptions {
    pub branch: String,
    pub force_large: bool,
    pub frontmatter_markdown: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct GitImportResult {
    pub imported_paths: Vec<String>,
    pub transaction_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct GitExportResult {
    pub exported_paths: Vec<String>,
    pub commit_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct GitSyncResult {
    pub imported_paths: Vec<String>,
    pub exported_paths: Vec<String>,
    pub conflict_paths: Vec<String>,
    pub conflicts: Vec<ConflictRecord>,
    pub commit_id: Option<String>,
}

#[derive(Clone, Debug)]
struct PeerConfig {
    repo: PathBuf,
    branch: String,
    remote: Option<String>,
    max_delete_percent: u8,
}

#[derive(Clone, Debug)]
struct GitFile {
    content: Vec<u8>,
    metadata: JsonValue,
    content_type: String,
    oid: String,
}

struct WorktreeImportFile {
    path: String,
    file: GitFile,
}

struct WorktreeExportFile {
    path: String,
    content: Vec<u8>,
    metadata: JsonValue,
}

struct WorktreeExportPlan {
    repo_dir: PathBuf,
    library_id: String,
    library_slug: String,
    branch: String,
    frontmatter_markdown: bool,
    files: Vec<WorktreeExportFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Marker {
    library_id: String,
    library_slug: String,
}

async fn run_git_blocking<T, F>(
    operation: &'static str,
    repo_dir: PathBuf,
    timeout_duration: Duration,
    work: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let repo = repo_dir.display().to_string();
    let operation_future = async move {
        let permit = GIT_BLOCKING_LANE
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| GitError::BlockingLaneClosed { operation })?;
        let span = tracing::debug_span!(
            "git.operation",
            operation,
            repo,
            timeout_ms = timeout_duration.as_millis() as u64
        );
        let task = tokio::task::spawn_blocking(move || {
            // The permit deliberately lives in the closure. spawn_blocking jobs
            // cannot be cancelled once started, so a caller timeout must not
            // admit a later Git job until this one has actually stopped.
            let _permit = permit;
            span.in_scope(|| {
                let started = Instant::now();
                let result = work();
                match &result {
                    Ok(_) => tracing::debug!(
                        event = "git.blocking.completed",
                        duration_ms = started.elapsed().as_millis() as u64,
                        "Git blocking operation completed"
                    ),
                    Err(error) => tracing::warn!(
                        event = "git.blocking.failed",
                        error = ?error,
                        duration_ms = started.elapsed().as_millis() as u64,
                        "Git blocking operation failed"
                    ),
                }
                result
            })
        });
        task.await
            .map_err(|source| GitError::BlockingTask { operation, source })?
    };

    match tokio::time::timeout(timeout_duration, operation_future).await {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(
                event = "git.blocking.timed_out",
                operation,
                repo = %repo_dir.display(),
                timeout_ms = timeout_duration.as_millis() as u64,
                "Git blocking operation timed out"
            );
            Err(GitError::OperationTimedOut {
                operation,
                timeout_seconds: timeout_duration.as_secs(),
            }
            .into())
        }
    }
}

pub async fn push_peer(store: &QuarryStore, library: &str, peer_id: &str) -> Result<GitSyncResult> {
    run_peer_operation(
        store,
        library,
        peer_id,
        |store, library, peer_id| async move { push_peer_inner(&store, &library, &peer_id).await },
    )
    .await
}

async fn push_peer_inner(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
) -> Result<GitSyncResult> {
    let started = Instant::now();
    let peer = peer_config(store, library, peer_id).await?;
    tracing::debug!(
        event = "git.push.started",
        library,
        peer_id,
        branch = %peer.branch,
        repo = %peer.repo.display(),
        remote_present = peer.remote.is_some(),
        "Git push started"
    );
    let branch = peer.branch.clone();
    let export = export_worktree(
        store,
        library,
        &peer.repo,
        GitExportOptions {
            branch: branch.clone(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await?;
    if let Some(remote) = &peer.remote {
        push_remote(&peer.repo, remote, &branch).await?;
    }
    record_exported_sync_state(store, library, peer_id, &peer.repo).await?;
    tracing::info!(
        event = "git.push.completed",
        library,
        peer_id,
        branch,
        exported_paths = export.exported_paths.len(),
        remote_url = peer
            .remote
            .as_deref()
            .map(redact_remote_url)
            .unwrap_or_default(),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git push completed"
    );
    Ok(GitSyncResult {
        imported_paths: Vec::new(),
        exported_paths: export.exported_paths,
        conflict_paths: Vec::new(),
        conflicts: Vec::new(),
        commit_id: export.commit_id,
    })
}

pub async fn pull_peer(store: &QuarryStore, library: &str, peer_id: &str) -> Result<GitSyncResult> {
    run_peer_operation(
        store,
        library,
        peer_id,
        |store, library, peer_id| async move { pull_peer_inner(&store, &library, &peer_id).await },
    )
    .await
}

async fn pull_peer_inner(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
) -> Result<GitSyncResult> {
    let started = Instant::now();
    let peer = peer_config(store, library, peer_id).await?;
    tracing::debug!(
        event = "git.pull.started",
        library,
        peer_id,
        branch = %peer.branch,
        repo = %peer.repo.display(),
        remote_present = peer.remote.is_some(),
        "Git pull started"
    );
    if let Some(remote) = &peer.remote {
        fetch_remote_worktree(&peer.repo, remote, &peer.branch).await?;
    }
    verify_marker(&peer.repo, &store.get_library(library).await?.id).await?;
    let import = import_worktree(store, library, &peer.repo).await?;
    record_exported_sync_state(store, library, peer_id, &peer.repo).await?;
    tracing::info!(
        event = "git.pull.completed",
        library,
        peer_id,
        branch = peer.branch,
        imported_paths = import.imported_paths.len(),
        remote_url = peer
            .remote
            .as_deref()
            .map(redact_remote_url)
            .unwrap_or_default(),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git pull completed"
    );
    Ok(GitSyncResult {
        imported_paths: import.imported_paths,
        exported_paths: Vec::new(),
        conflict_paths: Vec::new(),
        conflicts: Vec::new(),
        commit_id: None,
    })
}

pub async fn sync_peer(store: &QuarryStore, library: &str, peer_id: &str) -> Result<GitSyncResult> {
    run_peer_operation(
        store,
        library,
        peer_id,
        |store, library, peer_id| async move { sync_peer_inner(&store, &library, &peer_id).await },
    )
    .await
}

async fn run_peer_operation<F, Fut>(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
    operation: F,
) -> Result<GitSyncResult>
where
    F: FnOnce(QuarryStore, String, String) -> Fut,
    Fut: Future<Output = Result<GitSyncResult>> + Send + 'static,
{
    let store_clone = store.clone();
    let library = library.to_string();
    let peer_id = peer_id.to_string();
    store
        .run_global_operation(operation(store_clone, library, peer_id))
        .await
}

async fn sync_peer_inner(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
) -> Result<GitSyncResult> {
    let started = Instant::now();
    let peer = peer_config(store, library, peer_id).await?;
    tracing::debug!(
        event = "git.sync.started",
        library,
        peer_id,
        branch = %peer.branch,
        repo = %peer.repo.display(),
        remote_present = peer.remote.is_some(),
        "Git sync started"
    );
    if let Some(remote) = &peer.remote {
        fetch_remote_worktree(&peer.repo, remote, &peer.branch).await?;
    }
    let library_record = store.get_library(library).await?;
    verify_marker(&peer.repo, &library_record.id).await?;

    let docs = store
        .list_documents(&library_record.slug, None, Some(10_000))
        .await?;
    let doc_map: HashMap<String, DocumentListEntry> = docs
        .into_iter()
        .map(|doc| (doc.path.clone(), doc))
        .collect();
    let git_map = worktree_snapshot(&peer.repo).await?;
    let mut paths: BTreeSet<String> = doc_map.keys().cloned().collect();
    paths.extend(git_map.keys().cloned());
    let sync_states: HashMap<String, SyncStateEntry> = store
        .list_sync_state(peer_id)
        .await?
        .into_iter()
        .map(|state| (state.path.clone(), state))
        .collect();
    paths.extend(sync_states.keys().cloned());

    // Pure git-side renames: a clean delete and a clean create whose bytes
    // match exactly — unique on both sides, the reconciler's no-guess move
    // rule — pair into an identity-preserving document move instead of
    // delete + create. Block ids, review anchors, version history, and the
    // peer's shadow base all ride the document id.
    let renames = pair_renames(
        store,
        &library_record.slug,
        &paths,
        &doc_map,
        &git_map,
        &sync_states,
    )
    .await?;
    let renamed_from: BTreeSet<String> = renames.iter().map(|rename| rename.from.clone()).collect();

    enforce_delete_safety(
        &paths,
        &doc_map,
        &git_map,
        &sync_states,
        &renamed_from,
        peer.max_delete_percent,
    )?;

    let sync_paths = SyncPathReconciler {
        store,
        library: &library_record,
        peer_id,
        paths,
        doc_map: &doc_map,
        git_map: &git_map,
        sync_states: &sync_states,
        renames: &renames,
        renamed_from,
    }
    .run()
    .await?;

    let export = export_worktree(
        store,
        &library_record.slug,
        &peer.repo,
        GitExportOptions {
            branch: peer.branch.clone(),
            force_large: false,
            frontmatter_markdown: true,
        },
    )
    .await?;
    if let Some(remote) = &peer.remote {
        push_remote(&peer.repo, remote, &peer.branch).await?;
    }
    record_exported_sync_state(store, &library_record.slug, peer_id, &peer.repo).await?;
    for path in &sync_paths.deleted_sync_paths {
        store.upsert_sync_state(peer_id, path, None, None).await?;
    }
    tracing::info!(
        event = "git.sync.completed",
        library = library_record.slug,
        library_id = %library_record.id,
        peer_id,
        branch = peer.branch,
        imported_paths = sync_paths.imported_paths.len(),
        exported_paths = export.exported_paths.len(),
        conflicts = sync_paths.conflicts.len(),
        remote_url = peer.remote.as_deref().map(redact_remote_url).unwrap_or_default(),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git sync completed"
    );

    Ok(GitSyncResult {
        imported_paths: sync_paths.imported_paths,
        exported_paths: export.exported_paths,
        conflict_paths: sync_paths.conflict_paths,
        conflicts: sync_paths.conflicts,
        commit_id: export.commit_id,
    })
}

struct SyncPathReconciler<'a> {
    store: &'a QuarryStore,
    library: &'a Library,
    peer_id: &'a str,
    paths: BTreeSet<String>,
    doc_map: &'a HashMap<String, DocumentListEntry>,
    git_map: &'a HashMap<String, GitFile>,
    sync_states: &'a HashMap<String, SyncStateEntry>,
    renames: &'a [RenamePair],
    renamed_from: BTreeSet<String>,
}

struct SyncPathOutcome {
    imported_paths: Vec<String>,
    conflict_paths: Vec<String>,
    conflicts: Vec<ConflictRecord>,
    deleted_sync_paths: BTreeSet<String>,
}

struct SyncPathAccumulator {
    imported_paths: Vec<String>,
    conflict_paths: Vec<String>,
    conflicts: Vec<ConflictRecord>,
    deleted_sync_paths: BTreeSet<String>,
    renamed_paths: BTreeSet<String>,
}

impl From<SyncPathAccumulator> for SyncPathOutcome {
    fn from(accumulator: SyncPathAccumulator) -> Self {
        Self {
            imported_paths: accumulator.imported_paths,
            conflict_paths: accumulator.conflict_paths,
            conflicts: accumulator.conflicts,
            deleted_sync_paths: accumulator.deleted_sync_paths,
        }
    }
}

impl SyncPathReconciler<'_> {
    async fn run(self) -> Result<SyncPathOutcome> {
        let mut accumulator = SyncPathAccumulator {
            imported_paths: Vec::new(),
            conflict_paths: Vec::new(),
            conflicts: Vec::new(),
            deleted_sync_paths: BTreeSet::new(),
            renamed_paths: self.renamed_from.clone(),
        };
        self.apply_renames(&mut accumulator).await?;
        for path in self.paths.iter().cloned() {
            self.process_path(path, &mut accumulator).await?;
        }
        Ok(accumulator.into())
    }

    async fn apply_renames(&self, accumulator: &mut SyncPathAccumulator) -> Result<()> {
        for rename in self.renames {
            self.store
                .move_document(
                    &self.library.slug,
                    &rename.from,
                    &rename.to,
                    DocumentSource::Git,
                )
                .await?;
            tracing::info!(
                event = "git.sync.rename_paired",
                library = self.library.slug,
                peer_id = self.peer_id,
                from = %rename.from,
                to = %rename.to,
                "git-side rename paired into an identity-preserving move"
            );
            // The old path's sync state clears below; the moved document exports
            // at its new path, which records the new state.
            accumulator.deleted_sync_paths.insert(rename.from.clone());
            accumulator.renamed_paths.insert(rename.to.clone());
            accumulator.imported_paths.push(rename.to.clone());
        }
        Ok(())
    }

    async fn process_path(
        &self,
        path: String,
        accumulator: &mut SyncPathAccumulator,
    ) -> Result<()> {
        if accumulator.renamed_paths.contains(&path) {
            return Ok(());
        }
        let doc = self.doc_map.get(&path);
        let git = self.git_map.get(&path);
        let state = self.sync_states.get(&path);
        let last_doc = state.and_then(|state| state.last_synced_doc_version_id.as_deref());
        let last_git = state.and_then(|state| state.last_synced_git_oid.as_deref());
        let doc_changed = doc
            .map(|doc| Some(doc.head_version_id.as_str()) != last_doc)
            .unwrap_or(last_doc.is_some());
        let git_changed = git
            .map(|git| Some(git.oid.as_str()) != last_git)
            .unwrap_or(last_git.is_some());

        match (doc, git, doc_changed, git_changed) {
            (Some(doc), Some(git), true, true) => {
                self.import_both_changed_path(&path, doc, git, last_doc, accumulator)
                    .await?;
            }
            (Some(doc), None, true, true) => {
                let conflict = self
                    .store
                    .record_conflict(
                        &self.library.slug,
                        &path,
                        Some(doc.head_version_id.to_string()),
                        None,
                    )
                    .await?;
                log_git_conflict_recorded(self.library, self.peer_id, &path, None, &conflict);
                accumulator.conflicts.push(conflict);
            }
            (Some(_doc), None, false, true) => {
                self.store
                    .delete_document(&self.library.slug, &path, DocumentSource::Git)
                    .await?;
                accumulator.deleted_sync_paths.insert(path.clone());
                accumulator.imported_paths.push(path);
            }
            (None, Some(git), true, true) if last_doc.is_some() => {
                self.record_delete_vs_create_conflict(&path, git, accumulator)
                    .await?;
            }
            (None, Some(_git), true, false) if last_doc.is_some() => {
                accumulator.deleted_sync_paths.insert(path);
            }
            (None, None, true, true) | (None, None, true, false) | (None, None, false, true) => {
                accumulator.deleted_sync_paths.insert(path);
            }
            (None, Some(git), _, true) | (None, Some(git), _, false) => {
                self.import_git_file(&path, git, quarry_core::WritePrecondition::None)
                    .await?;
                accumulator.imported_paths.push(path);
            }
            (Some(_), None, true, _) => {
                // Quarry changed or created the path; export publishes it below.
            }
            (Some(doc), Some(git), false, true) => {
                self.import_git_file(
                    &path,
                    git,
                    quarry_core::WritePrecondition::IfMatch(doc.head_version_id.to_string()),
                )
                .await?;
                accumulator.imported_paths.push(path);
            }
            _ => {}
        }
        Ok(())
    }

    async fn import_both_changed_path(
        &self,
        path: &str,
        doc: &DocumentListEntry,
        git: &GitFile,
        last_doc: Option<&str>,
        accumulator: &mut SyncPathAccumulator,
    ) -> Result<()> {
        let current = self.store.get_document(&self.library.slug, path).await?;
        if current.content == git.content {
            return Ok(());
        }
        if is_block_file(path, &git.content_type) {
            // Phase 4: both sides changed a Markdown document — diff3
            // against the peer's shadow base. Without one, the last-synced
            // version's content is the common ancestor; without even that
            // (both sides created the path independently) an EMPTY base keeps
            // it conservative: differences conflict instead of silently
            // overwriting. Non-conflicting hunks from both sides land; true
            // conflicts become review items, never sibling files or sync
            // failures.
            let ancestor = match last_doc {
                Some(version_id) => {
                    let version = self
                        .store
                        .document_version(&self.library.slug, path, version_id)
                        .await?;
                    BlockWriteBase::Markdown {
                        markdown: version.content,
                        version_id: Some(version_id.to_string()),
                    }
                }
                None => BlockWriteBase::Markdown {
                    markdown: String::new(),
                    version_id: None,
                },
            };
            write_markdown_file(
                self.store,
                &self.library.slug,
                Some(self.peer_id),
                path,
                git,
                ancestor,
            )
            .await?;
            accumulator.imported_paths.push(path.to_string());
            return Ok(());
        }

        let conflict_path = conflict_sibling_path(path);
        let outcome = self
            .store
            .put_document(PutDocumentRequest {
                library: self.library.slug.clone(),
                path: conflict_path.clone(),
                content: git.content.clone(),
                metadata: git.metadata.clone(),
                content_type: git.content_type.clone(),
                source: DocumentSource::Git,
                precondition: quarry_core::WritePrecondition::None,
                origin_id: None,
                transaction: TransactionMetadata::default(),
            })
            .await?;
        let conflict = self
            .store
            .record_conflict(
                &self.library.slug,
                path,
                Some(doc.head_version_id.to_string()),
                Some(outcome.version.id.to_string()),
            )
            .await?;
        log_git_conflict_recorded(
            self.library,
            self.peer_id,
            path,
            Some(&conflict_path),
            &conflict,
        );
        accumulator.conflict_paths.push(conflict_path);
        accumulator.conflicts.push(conflict);
        Ok(())
    }

    async fn record_delete_vs_create_conflict(
        &self,
        path: &str,
        git: &GitFile,
        accumulator: &mut SyncPathAccumulator,
    ) -> Result<()> {
        let conflict_path = conflict_sibling_path(path);
        // Delete-vs-create: Quarry deleted the path, Git changed it. The Git
        // side is preserved as a sibling document; Markdown siblings import
        // through the block writer (a first import — fresh ids, no base) so
        // they are ordinary BlockDocuments, not raw bytes with a cleared
        // projection.
        let sibling_version_id = write_git_file_to_document(
            self.store,
            &self.library.slug,
            Some(self.peer_id),
            &conflict_path,
            git,
            BlockWriteBase::CurrentCanonical,
            quarry_core::WritePrecondition::None,
        )
        .await?
        .version
        .id;
        let conflict = self
            .store
            .record_conflict(
                &self.library.slug,
                path,
                None,
                Some(sibling_version_id.to_string()),
            )
            .await?;
        log_git_conflict_recorded(
            self.library,
            self.peer_id,
            path,
            Some(&conflict_path),
            &conflict,
        );
        accumulator.deleted_sync_paths.insert(path.to_string());
        accumulator.conflict_paths.push(conflict_path);
        accumulator.conflicts.push(conflict);
        Ok(())
    }

    async fn import_git_file(
        &self,
        path: &str,
        git: &GitFile,
        precondition: quarry_core::WritePrecondition,
    ) -> Result<()> {
        // When the Quarry document is unchanged since the last sync, the
        // current canonical state is the common ancestor.
        write_git_file_to_document(
            self.store,
            &self.library.slug,
            Some(self.peer_id),
            path,
            git,
            BlockWriteBase::CurrentCanonical,
            precondition,
        )
        .await?;
        Ok(())
    }
}

async fn write_git_file_to_document(
    store: &QuarryStore,
    library: &str,
    peer_id: Option<&str>,
    path: &str,
    git: &GitFile,
    block_base: BlockWriteBase,
    raw_precondition: quarry_core::WritePrecondition,
) -> Result<WriteOutcome> {
    if is_block_file(path, &git.content_type) {
        return Ok(
            write_markdown_file(store, library, peer_id, path, git, block_base)
                .await?
                .outcome,
        );
    }

    store
        .put_document(PutDocumentRequest {
            library: library.to_string(),
            path: path.to_string(),
            content: git.content.clone(),
            metadata: git.metadata.clone(),
            content_type: git.content_type.clone(),
            source: DocumentSource::Git,
            precondition: raw_precondition,
            origin_id: None,
            transaction: TransactionMetadata::default(),
        })
        .await
}

/// Imports a Git worktree into a library.
///
/// **Atomicity (changed in Phase 4):** Markdown files commit PER DOCUMENT
/// through the reconciling writer (two-way merge against the current
/// canonical state; byte-identical files are no-ops) and therefore escape
/// the staged transaction — an import that fails midway leaves the markdown
/// documents already imported in place. Raw files keep the staged
/// multi-document transaction and roll back together on failure.
pub async fn import_worktree(
    store: &QuarryStore,
    library: &str,
    repo_dir: &Path,
) -> Result<GitImportResult> {
    ensure_worktree_exists(repo_dir).await?;
    let library_record = store.get_library(library).await?;
    let tx = store
        .begin_transaction(
            &library_record.slug,
            DocumentSource::Git,
            Some("git".to_string()),
            Some(format!("import from {}", repo_dir.display())),
            serde_json::json!({"repo": repo_dir.display().to_string()}),
        )
        .await?;

    let result = async {
        let import_files = read_worktree_import_files(repo_dir).await?;
        import_worktree_transaction(store, &library_record.slug, import_files, &tx.id).await
    }
    .await;
    if result.is_err() {
        let _ = store.rollback_transaction(&tx.id).await;
    }
    result
}

async fn ensure_worktree_exists(repo_dir: &Path) -> Result<()> {
    let repo_dir = repo_dir.to_path_buf();
    run_git_blocking(
        "worktree.import.validate",
        repo_dir.clone(),
        LOCAL_GIT_OPERATION_TIMEOUT,
        move || {
            if repo_dir.exists() {
                Ok(())
            } else {
                Err(QuarryError::NotFound(repo_dir.display().to_string()))
            }
        },
    )
    .await
}

async fn read_worktree_import_files(repo_dir: &Path) -> Result<Vec<WorktreeImportFile>> {
    let repo_dir = repo_dir.to_path_buf();
    run_git_blocking(
        "worktree.import.scan",
        repo_dir.clone(),
        LOCAL_GIT_OPERATION_TIMEOUT,
        move || scan_worktree_import_files(&repo_dir),
    )
    .await
}

fn scan_worktree_import_files(repo_dir: &Path) -> Result<Vec<WorktreeImportFile>> {
    if !repo_dir.exists() {
        return Err(QuarryError::NotFound(repo_dir.display().to_string()));
    }

    let mut import_files = Vec::new();
    for entry in WalkDir::new(repo_dir).into_iter().filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        name != ".git" && name != ".quarry"
    }) {
        let entry = entry.map_err(|err| QuarryError::Io(err.into()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(repo_dir)
            .map_err(GitError::from)?;
        if is_sidecar(relative) {
            continue;
        }
        let path = normalize_path(&relative.to_string_lossy())?;
        let bytes = fs::read(entry.path())?;
        let (content, mut metadata) = if path.ends_with(".md") {
            let (_, metadata) = split_frontmatter(&bytes)?;
            (bytes, metadata)
        } else {
            (bytes, serde_json::json!({}))
        };
        merge_metadata(&mut metadata, sidecar_metadata(repo_dir, &path)?);
        ensure_content_type(&mut metadata, &path);
        let content_type = metadata
            .get("content_type")
            .and_then(JsonValue::as_str)
            .unwrap_or("application/octet-stream")
            .to_string();
        import_files.push(WorktreeImportFile {
            path,
            file: GitFile {
                content,
                metadata,
                content_type,
                oid: String::new(),
            },
        });
    }
    import_files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(import_files)
}

async fn import_worktree_transaction(
    store: &QuarryStore,
    library: &str,
    import_files: Vec<WorktreeImportFile>,
    tx_id: &str,
) -> Result<GitImportResult> {
    let mut imported_paths = Vec::new();
    for WorktreeImportFile { path, file } in import_files {
        if is_block_file(&path, &file.content_type) {
            // Phase 4: Markdown imports reconcile per document (two-way —
            // plain `git import` has no peer scope, so the base is the
            // current canonical state). Byte-identical files are no-ops, so
            // re-imports do not churn versions. Raw files keep the staged
            // multi-document transaction below.
            write_markdown_file(
                store,
                library,
                None,
                &path,
                &file,
                BlockWriteBase::CurrentCanonical,
            )
            .await?;
        } else {
            let GitFile {
                content,
                metadata,
                content_type,
                ..
            } = file;
            store
                .stage_put(tx_id, &path, content, metadata, &content_type)
                .await?;
        }
        imported_paths.push(path);
    }

    imported_paths.sort();
    store.commit_transaction(tx_id).await?;
    Ok(GitImportResult {
        imported_paths,
        transaction_id: tx_id.to_string(),
    })
}

pub async fn export_worktree(
    store: &QuarryStore,
    library: &str,
    repo_dir: &Path,
    options: GitExportOptions,
) -> Result<GitExportResult> {
    let library_record = store.get_library(library).await?;
    let documents = store
        .list_documents(&library_record.slug, None, Some(10_000))
        .await?;
    let mut files = Vec::with_capacity(documents.len());
    for entry in documents {
        if is_reserved_git_metadata_path(&entry.path) {
            return Err(QuarryError::InvalidPath(format!(
                "{} is reserved for Git metadata sidecars",
                entry.path
            )));
        }
        let document = store
            .get_document(&library_record.slug, &entry.path)
            .await?;
        if document.content.len() > GIT_BINARY_WARN_THRESHOLD && !options.force_large {
            return Err(QuarryError::Conflict(format!(
                "{} is larger than the 5 MiB phase-one Git export threshold",
                document.path
            )));
        }
        files.push(WorktreeExportFile {
            path: document.path,
            content: document.content,
            metadata: document.metadata,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let repo_dir = repo_dir.to_path_buf();
    let plan = WorktreeExportPlan {
        repo_dir: repo_dir.clone(),
        library_id: library_record.id,
        library_slug: library_record.slug,
        branch: options.branch,
        frontmatter_markdown: options.frontmatter_markdown,
        files,
    };
    run_git_blocking(
        "worktree.export",
        repo_dir,
        LOCAL_GIT_OPERATION_TIMEOUT,
        move || execute_worktree_export(plan),
    )
    .await
}

fn execute_worktree_export(plan: WorktreeExportPlan) -> Result<GitExportResult> {
    fs::create_dir_all(&plan.repo_dir)?;
    verify_or_write_marker(&plan.repo_dir, &plan.library_id, &plan.library_slug)?;
    clean_worktree(&plan.repo_dir)?;

    let mut exported_paths = Vec::with_capacity(plan.files.len());
    for file in plan.files {
        let output = plan.repo_dir.join(&file.path);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        if plan.frontmatter_markdown && file.path.ends_with(".md") {
            write_atomic(
                &output,
                &markdown_with_frontmatter(&file.metadata, &file.content)?,
            )?;
        } else {
            write_atomic(&output, &file.content)?;
            write_sidecar(&plan.repo_dir, &file.path, &file.metadata)?;
        }
        exported_paths.push(file.path);
    }
    write_marker(&plan.repo_dir, &plan.library_id, &plan.library_slug)?;

    let commit_id = commit_all(&plan.repo_dir, &plan.branch, "Quarry export")?;
    Ok(GitExportResult {
        exported_paths,
        commit_id,
    })
}

fn split_frontmatter(bytes: &[u8]) -> Result<(Vec<u8>, JsonValue)> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok((bytes.to_vec(), serde_json::json!({})));
    };
    let (text, bom_len) = text
        .strip_prefix('\u{feff}')
        .map(|text| (text, 3))
        .unwrap_or((text, 0));
    let Some(open_len) = frontmatter_open_len(text) else {
        return Ok((bytes.to_vec(), serde_json::json!({})));
    };
    let body = &text[open_len..];
    let Some((end, close_len)) = frontmatter_close(body) else {
        return Ok((bytes.to_vec(), serde_json::json!({})));
    };
    let yaml = &body[..end];
    let body_start = bom_len + open_len + end + close_len;
    let metadata: JsonValue =
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(yaml)?)?;
    Ok((bytes[body_start..].to_vec(), metadata))
}

fn frontmatter_open_len(text: &str) -> Option<usize> {
    if text.starts_with("---\n") {
        Some(4)
    } else if text.starts_with("---\r\n") {
        Some(5)
    } else {
        None
    }
}

fn frontmatter_close(text: &str) -> Option<(usize, usize)> {
    ["\n---\n", "\r\n---\r\n", "\n---\r\n", "\r\n---\n"]
        .into_iter()
        .filter_map(|marker| text.find(marker).map(|index| (index, marker.len())))
        .min_by_key(|(index, _)| *index)
}

fn sidecar_metadata(repo_dir: &Path, path: &str) -> Result<Option<JsonValue>> {
    let sidecar = repo_dir.join(format!("{path}.quarrymeta.yaml"));
    if !sidecar.exists() {
        return Ok(None);
    }
    let yaml = fs::read_to_string(sidecar)?;
    let metadata: JsonValue =
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(&yaml)?)?;
    Ok(Some(metadata))
}

fn merge_metadata(target: &mut JsonValue, patch: Option<JsonValue>) {
    let Some(patch) = patch else {
        return;
    };
    match (target, patch) {
        (JsonValue::Object(target), JsonValue::Object(patch)) => {
            for (key, value) in patch {
                target.insert(key, value);
            }
        }
        (target, value) => *target = value,
    }
}

fn ensure_content_type(metadata: &mut JsonValue, path: &str) {
    if let JsonValue::Object(object) = metadata {
        object.entry("content_type".to_string()).or_insert_with(|| {
            JsonValue::String(
                mime_guess::from_path(path)
                    .first_or_octet_stream()
                    .essence_str()
                    .to_string(),
            )
        });
    }
}

fn markdown_with_frontmatter(metadata: &JsonValue, content: &[u8]) -> Result<Vec<u8>> {
    // Documents written through the block gateway store their normalized
    // text WITH frontmatter already; exporting must not double it.
    if content.starts_with(b"---\n") || content.starts_with(b"---\r\n") {
        return Ok(content.to_vec());
    }
    let header = render_markdown_frontmatter(metadata)?;
    if header.is_empty() {
        return Ok(content.to_vec());
    }
    let mut output = header.into_bytes();
    output.extend_from_slice(content);
    Ok(output)
}

/// Writes via a sibling temp file plus rename so a crash mid-write leaves
/// either the old file or the new one, never a truncated hybrid.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let Some(file_name) = path.file_name() else {
        return Err(QuarryError::InvalidPath(path.display().to_string()));
    };
    let tmp = path.with_file_name(format!(
        "{}.quarry-tmp-{}",
        file_name.to_string_lossy(),
        Uuid::new_v4()
    ));
    fs::write(&tmp, contents)?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

fn write_sidecar(repo_dir: &Path, path: &str, metadata: &JsonValue) -> Result<()> {
    if metadata == &serde_json::json!({}) {
        return Ok(());
    }
    let sidecar = repo_dir.join(format!("{path}.quarrymeta.yaml"));
    if let Some(parent) = sidecar.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&sidecar, serde_yaml::to_string(metadata)?.as_bytes())?;
    Ok(())
}

fn is_sidecar(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".quarrymeta.yaml")
}

fn is_reserved_git_metadata_path(path: &str) -> bool {
    path.ends_with(".quarrymeta.yaml")
}

fn marker_path(repo_dir: &Path) -> PathBuf {
    repo_dir.join(".quarry").join("marker.json")
}

fn verify_or_write_marker(repo_dir: &Path, library_id: &str, library_slug: &str) -> Result<()> {
    let path = marker_path(repo_dir);
    if path.exists() {
        let marker: Marker = serde_json::from_slice(&fs::read(&path)?)?;
        if marker.library_id != library_id {
            return Err(QuarryError::Conflict(format!(
                "Git marker belongs to library {} not {library_id}",
                marker.library_id
            )));
        }
    } else {
        write_marker(repo_dir, library_id, library_slug)?;
    }
    Ok(())
}

async fn verify_marker(repo_dir: &Path, library_id: &str) -> Result<()> {
    let repo_dir = repo_dir.to_path_buf();
    let library_id = library_id.to_string();
    run_git_blocking(
        "worktree.marker.verify",
        repo_dir.clone(),
        LOCAL_GIT_OPERATION_TIMEOUT,
        move || verify_marker_blocking(&repo_dir, &library_id),
    )
    .await
}

fn verify_marker_blocking(repo_dir: &Path, library_id: &str) -> Result<()> {
    let path = marker_path(repo_dir);
    if !path.exists() {
        return Err(QuarryError::Conflict(format!(
            "Git marker is missing at {}",
            path.display()
        )));
    }
    let marker: Marker = serde_json::from_slice(&fs::read(&path)?)?;
    if marker.library_id != library_id {
        return Err(QuarryError::Conflict(format!(
            "Git marker belongs to library {} not {library_id}",
            marker.library_id
        )));
    }
    Ok(())
}

fn write_marker(repo_dir: &Path, library_id: &str, library_slug: &str) -> Result<()> {
    let path = marker_path(repo_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let marker = Marker {
        library_id: library_id.to_string(),
        library_slug: library_slug.to_string(),
    };
    write_atomic(&path, serde_json::to_string_pretty(&marker)?.as_bytes())?;
    Ok(())
}

fn clean_worktree(repo_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(repo_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn commit_all(repo_dir: &Path, branch: &str, message: &str) -> Result<Option<String>> {
    let repo = Repository::open(repo_dir)
        .or_else(|_| Repository::init(repo_dir))
        .map_err(map_git)?;
    let head_ref = format!("refs/heads/{branch}");
    if repo.head().is_err() {
        repo.set_head(&head_ref).map_err(map_git)?;
    }
    let mut index = repo.index().map_err(map_git)?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(map_git)?;
    index.write().map_err(map_git)?;
    let tree_id = index.write_tree().map_err(map_git)?;
    let tree = repo.find_tree(tree_id).map_err(map_git)?;
    let signature = Signature::now("Quarry", "quarry@local").map_err(map_git)?;
    let parent = repo
        .head()
        .ok()
        .and_then(|head| head.target())
        .and_then(|oid| repo.find_commit(oid).ok());
    if parent
        .as_ref()
        .map(|commit| commit.tree_id() == tree_id)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    let oid = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .map_err(map_git)?;
    Ok(Some(oid.to_string()))
}

async fn fetch_remote_worktree(repo_dir: &Path, remote_url: &str, branch: &str) -> Result<()> {
    let repo_dir = repo_dir.to_path_buf();
    let remote_url = remote_url.to_string();
    let branch = branch.to_string();
    run_git_blocking(
        "remote.fetch",
        repo_dir.clone(),
        REMOTE_GIT_OPERATION_TIMEOUT,
        move || fetch_remote_worktree_blocking(&repo_dir, &remote_url, &branch),
    )
    .await
}

fn fetch_remote_worktree_blocking(repo_dir: &Path, remote_url: &str, branch: &str) -> Result<()> {
    let started = Instant::now();
    if repo_dir.join(".git").exists() {
        let repo = Repository::open(repo_dir).map_err(map_git)?;
        let mut remote = ensure_remote(&repo, remote_url)?;
        let mut options = FetchOptions::new();
        remote
            .fetch(&[branch], Some(&mut options), None)
            .map_err(map_git)?;
        checkout_remote_branch(&repo, branch)?;
        tracing::debug!(
            event = "git.remote.fetch.completed",
            repo = %repo_dir.display(),
            branch,
            remote_url = %redact_remote_url(remote_url),
            duration_ms = started.elapsed().as_millis() as u64,
            "Git remote fetch completed"
        );
        return Ok(());
    }

    if repo_dir.exists() && fs::read_dir(repo_dir)?.next().is_some() {
        return Err(QuarryError::Conflict(format!(
            "{} is not a Git repository and is not empty",
            repo_dir.display()
        )));
    }
    if repo_dir.exists() {
        fs::remove_dir(repo_dir)?;
    }
    if let Some(parent) = repo_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut builder = RepoBuilder::new();
    builder.branch(branch);
    builder.clone(remote_url, repo_dir).map_err(map_git)?;
    tracing::debug!(
        event = "git.remote.fetch.completed",
        repo = %repo_dir.display(),
        branch,
        remote_url = %redact_remote_url(remote_url),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git remote clone completed"
    );
    Ok(())
}

fn checkout_remote_branch(repo: &Repository, branch: &str) -> Result<()> {
    let remote_ref = format!("refs/remotes/origin/{branch}");
    let object = repo.revparse_single(&remote_ref).map_err(map_git)?;
    let commit = object.peel_to_commit().map_err(map_git)?;
    let local_ref = format!("refs/heads/{branch}");
    repo.reference(&local_ref, commit.id(), true, "Quarry remote fetch")
        .map_err(map_git)?;
    repo.set_head(&local_ref).map_err(map_git)?;
    let mut checkout = CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout)).map_err(map_git)?;
    Ok(())
}

async fn push_remote(repo_dir: &Path, remote_url: &str, branch: &str) -> Result<()> {
    let repo_dir = repo_dir.to_path_buf();
    let remote_url = remote_url.to_string();
    let branch = branch.to_string();
    run_git_blocking(
        "remote.push",
        repo_dir.clone(),
        REMOTE_GIT_OPERATION_TIMEOUT,
        move || push_remote_blocking(&repo_dir, &remote_url, &branch),
    )
    .await
}

fn push_remote_blocking(repo_dir: &Path, remote_url: &str, branch: &str) -> Result<()> {
    let started = Instant::now();
    let repo = Repository::open(repo_dir).map_err(map_git)?;
    let mut remote = ensure_remote(&repo, remote_url)?;
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let mut options = PushOptions::new();
    remote
        .push(&[&refspec], Some(&mut options))
        .map_err(map_git)?;
    tracing::debug!(
        event = "git.remote.push.completed",
        repo = %repo_dir.display(),
        branch,
        remote_url = %redact_remote_url(remote_url),
        duration_ms = started.elapsed().as_millis() as u64,
        "Git remote push completed"
    );
    Ok(())
}

fn log_git_conflict_recorded(
    library: &quarry_core::Library,
    peer_id: &str,
    path: &str,
    conflict_path: Option<&str>,
    conflict: &ConflictRecord,
) {
    tracing::debug!(
        event = "git.conflict.recorded",
        library = %library.slug,
        library_id = %library.id,
        peer_id,
        path,
        conflict_path = conflict_path.unwrap_or(""),
        conflict_id = %conflict.id,
        ours_version_id = conflict.ours_version_id.as_deref().unwrap_or(""),
        theirs_version_id = conflict.theirs_version_id.as_deref().unwrap_or(""),
        "Git conflict recorded"
    );
}

fn redact_remote_url(remote_url: &str) -> String {
    let Some((scheme, rest)) = remote_url.split_once("://") else {
        return remote_url.to_string();
    };
    let Some((userinfo, host_and_path)) = rest.split_once('@') else {
        return remote_url.to_string();
    };
    if userinfo.is_empty() {
        remote_url.to_string()
    } else {
        format!("{scheme}://<redacted>@{host_and_path}")
    }
}

fn ensure_remote<'repo>(repo: &'repo Repository, remote_url: &str) -> Result<git2::Remote<'repo>> {
    match repo.find_remote("origin") {
        Ok(remote) => {
            let existing_url = remote.url().ok().map(str::to_string);
            drop(remote);
            if existing_url.as_deref() != Some(remote_url) {
                repo.remote_set_url("origin", remote_url).map_err(map_git)?;
            }
        }
        Err(_) => {
            repo.remote("origin", remote_url).map_err(map_git)?;
        }
    }
    Ok(repo.find_remote("origin").map_err(map_git)?)
}

fn map_git(err: git2::Error) -> GitError {
    GitError::Git(err)
}

async fn peer_config(store: &QuarryStore, library: &str, peer_id: &str) -> Result<PeerConfig> {
    let peer = store
        .list_git_peers(library)
        .await?
        .into_iter()
        .find(|peer| peer.id == peer_id)
        .ok_or_else(|| QuarryError::NotFound(format!("git peer {peer_id}")))?;
    let repo = peer
        .config
        .get("repo")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| QuarryError::InvalidPath("git peer missing repo".to_string()))?;
    let branch = peer
        .config
        .get("branch")
        .and_then(JsonValue::as_str)
        .unwrap_or("main");
    let remote = peer
        .config
        .get("remote")
        .or_else(|| peer.config.get("remote_url"))
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);
    Ok(PeerConfig {
        repo: PathBuf::from(repo),
        branch: branch.to_string(),
        remote,
        max_delete_percent: peer
            .config
            .get("max_delete_percent")
            .and_then(JsonValue::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(100),
    })
}

async fn record_exported_sync_state(
    store: &QuarryStore,
    library: &str,
    peer_id: &str,
    repo_dir: &Path,
) -> Result<()> {
    let docs = store.list_documents(library, None, Some(10_000)).await?;
    let git = worktree_snapshot(repo_dir).await?;
    for doc in docs {
        let git_oid = git.get(&doc.path).map(|file| file.oid.clone());
        // Phase 4 shadow-base bookkeeping: what this peer just saw (the
        // exported canonical body) is the diff3 base for its next write.
        let content_type = git
            .get(&doc.path)
            .map(|file| file.content_type.clone())
            .unwrap_or_default();
        if is_block_file(&doc.path, &content_type) {
            // Skip the full-content load when the recorded base already
            // names the current head: the base only advances with content,
            // so an unchanged document needs no rewrite (keeps no-op syncs
            // from reading every Markdown document's body).
            let recorded = store.block_shadow_base("git", peer_id, &doc.id).await?;
            let recorded_head = recorded.and_then(|base| base.base_version_id);
            if recorded_head.as_deref() != Some(doc.head_version_id.as_str()) {
                let document = store.get_document(library, &doc.path).await?;
                if let Ok(text) = String::from_utf8(document.content) {
                    let (_, body) = split_markdown_frontmatter(&text)?;
                    store
                        .put_block_shadow_base(
                            "git",
                            peer_id,
                            &document.id,
                            body,
                            Some(doc.head_version_id.to_string()),
                        )
                        .await?;
                }
            }
        }
        store
            .upsert_sync_state(
                peer_id,
                &doc.path,
                Some(doc.head_version_id.to_string()),
                git_oid,
            )
            .await?;
    }
    Ok(())
}

async fn worktree_snapshot(repo_dir: &Path) -> Result<HashMap<String, GitFile>> {
    let repo_dir = repo_dir.to_path_buf();
    run_git_blocking(
        "worktree.snapshot",
        repo_dir.clone(),
        LOCAL_GIT_OPERATION_TIMEOUT,
        move || worktree_snapshot_blocking(&repo_dir),
    )
    .await
}

fn worktree_snapshot_blocking(repo_dir: &Path) -> Result<HashMap<String, GitFile>> {
    let mut files = HashMap::new();
    if !repo_dir.exists() {
        return Ok(files);
    }
    for entry in WalkDir::new(repo_dir).into_iter().filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        name != ".git" && name != ".quarry"
    }) {
        let entry = entry.map_err(|err| QuarryError::Io(err.into()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(repo_dir)
            .map_err(GitError::from)?;
        if is_sidecar(relative) {
            continue;
        }
        let path = normalize_path(&relative.to_string_lossy())?;
        let raw = fs::read(entry.path())?;
        let oid = git2::Oid::hash_object(ObjectType::Blob, &raw)
            .map_err(map_git)?
            .to_string();
        // Markdown keeps its raw text (frontmatter included): the Phase 4
        // reconciled writer splits the frontmatter itself, and byte
        // comparisons against stored content (which carries frontmatter)
        // stay like-for-like. The parsed metadata still feeds sidecar
        // merging and content-type detection.
        let (content, mut metadata) = if path.ends_with(".md") {
            let (_, metadata) = split_frontmatter(&raw)?;
            (raw, metadata)
        } else {
            (raw, serde_json::json!({}))
        };
        merge_metadata(&mut metadata, sidecar_metadata(repo_dir, &path)?);
        ensure_content_type(&mut metadata, &path);
        let content_type = metadata
            .get("content_type")
            .and_then(JsonValue::as_str)
            .unwrap_or("application/octet-stream")
            .to_string();
        files.insert(
            path,
            GitFile {
                content,
                metadata,
                content_type,
                oid,
            },
        );
    }
    Ok(files)
}

/// Whether a worktree file participates in the block model (Phase 4
/// reconciled writes) or stays on the raw byte path.
fn is_block_file(path: &str, content_type: &str) -> bool {
    quarry_storage::document_kind(path, content_type) == DocumentKind::BlockDocument
}

/// One reconciled Markdown write from a Git worktree file, with per-peer
/// shadow-base bookkeeping: the diff3 base is the canonical text this peer
/// last synced (recorded at export/import); a missing base degrades to the
/// two-way merge. After the write the peer's base advances to the new
/// canonical text. Merge conflicts never fail the sync — they surface as
/// conflict review items on the document. CriticMarkup (content the codec
/// rejects outright) DOES fail the file's import with the typed
/// unsupported-markdown error.
async fn write_markdown_file(
    store: &QuarryStore,
    library: &str,
    peer_id: Option<&str>,
    path: &str,
    file: &GitFile,
    fallback_base: BlockWriteBase,
) -> Result<BlockMarkdownWriteOutcome> {
    let markdown = String::from_utf8(file.content.clone())
        .map_err(|_| QuarryError::InvalidInput(format!("{path} is not valid UTF-8 markdown")))?;
    let base = match peer_id {
        Some(peer) => match store.head_document(library, path).await {
            Ok(head) => match store.block_shadow_base("git", peer, &head.id).await? {
                Some(shadow) => BlockWriteBase::Markdown {
                    markdown: shadow.base_markdown,
                    version_id: shadow.base_version_id,
                },
                None => fallback_base,
            },
            Err(QuarryError::NotFound(_)) => fallback_base,
            Err(error) => return Err(error),
        },
        None => fallback_base,
    };
    let outcome = store
        .write_block_markdown(BlockMarkdownWrite {
            scope: DocumentScopeRef::library(library),
            path: path.to_string(),
            markdown,
            metadata: file.metadata.clone(),
            base,
            source: DocumentSource::Git,
            surface: "git".to_string(),
            actor_label: peer_id.map(|peer| format!("Git sync ({peer})")),
        })
        .await?;
    if outcome.conflicts > 0 {
        tracing::info!(
            event = "git.sync.merge_conflicts_recorded",
            library,
            path,
            conflicts = outcome.conflicts,
            "git import merged with conflict review items"
        );
    }
    if let Some(peer) = peer_id {
        store
            .put_block_shadow_base(
                "git",
                peer,
                &outcome.outcome.document.id,
                &outcome.canonical_body,
                Some(outcome.outcome.version.id.to_string()),
            )
            .await?;
    }
    Ok(outcome)
}

fn conflict_sibling_path(path: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    format!("{path}.conflict-git-{timestamp}")
}

struct RenamePair {
    from: String,
    to: String,
}

/// Pairs clean git-side deletes (document unchanged since the last sync,
/// file gone) with clean creates (a path this peer never synced) whose bytes
/// match exactly. Duplicate content on either side makes the pairing
/// ambiguous — no pairing happens, delete + create proceeds as before.
async fn pair_renames(
    store: &QuarryStore,
    library: &str,
    paths: &BTreeSet<String>,
    doc_map: &HashMap<String, DocumentListEntry>,
    git_map: &HashMap<String, GitFile>,
    sync_states: &HashMap<String, SyncStateEntry>,
) -> Result<Vec<RenamePair>> {
    let mut deletes_by_content: HashMap<Vec<u8>, Vec<String>> = HashMap::new();
    let mut creates_by_content: HashMap<Vec<u8>, Vec<String>> = HashMap::new();
    for path in paths {
        let state = sync_states.get(path);
        let last_doc = state.and_then(|state| state.last_synced_doc_version_id.as_deref());
        let last_git = state.and_then(|state| state.last_synced_git_oid.as_deref());
        match (doc_map.get(path), git_map.get(path)) {
            (Some(doc), None) => {
                let doc_unchanged = Some(doc.head_version_id.as_str()) == last_doc;
                if doc_unchanged && last_git.is_some() {
                    let content = store.get_document(library, path).await?.content;
                    deletes_by_content
                        .entry(content)
                        .or_default()
                        .push(path.clone());
                }
            }
            (None, Some(git)) if last_doc.is_none() => {
                creates_by_content
                    .entry(git.content.clone())
                    .or_default()
                    .push(path.clone());
            }
            _ => {}
        }
    }

    let mut pairs: Vec<RenamePair> = deletes_by_content
        .into_iter()
        .filter_map(|(content, from_paths)| {
            let to_paths = creates_by_content.get(&content)?;
            match (&from_paths[..], &to_paths[..]) {
                ([from], [to]) => Some(RenamePair {
                    from: from.clone(),
                    to: to.clone(),
                }),
                _ => None,
            }
        })
        .collect();
    pairs.sort_by(|left, right| left.from.cmp(&right.from));
    Ok(pairs)
}

fn enforce_delete_safety(
    paths: &BTreeSet<String>,
    doc_map: &HashMap<String, DocumentListEntry>,
    git_map: &HashMap<String, GitFile>,
    sync_states: &HashMap<String, SyncStateEntry>,
    renamed_from: &BTreeSet<String>,
    max_delete_percent: u8,
) -> Result<()> {
    let tracked = sync_states
        .values()
        .filter(|state| {
            state.last_synced_doc_version_id.is_some() || state.last_synced_git_oid.is_some()
        })
        .count();
    if tracked == 0 {
        return Ok(());
    }

    let delete_candidates = paths
        .iter()
        .filter(|path| {
            if renamed_from.contains(*path) {
                return false;
            }
            let Some(state) = sync_states.get(*path) else {
                return false;
            };
            let doc = doc_map.get(*path);
            let git = git_map.get(*path);
            let last_doc = state.last_synced_doc_version_id.as_deref();
            let last_git = state.last_synced_git_oid.as_deref();
            let doc_changed = doc
                .map(|doc| Some(doc.head_version_id.as_str()) != last_doc)
                .unwrap_or(last_doc.is_some());
            let git_changed = git
                .map(|git| Some(git.oid.as_str()) != last_git)
                .unwrap_or(last_git.is_some());

            matches!(
                (doc, git, doc_changed, git_changed),
                (Some(_), None, false, true) | (None, Some(_), true, false)
            )
        })
        .count();

    if delete_candidates * 100 > tracked * usize::from(max_delete_percent) {
        return Err(QuarryError::Conflict(format!(
            "delete safety abort: {delete_candidates} of {tracked} tracked paths would be deleted, exceeding {max_delete_percent}%"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_error_converts_through_the_crate_error_boundary() {
        let err = QuarryError::from(GitError::Git(git2::Error::from_str("boom")));

        let QuarryError::GitSource { source } = err else {
            panic!("expected git source error");
        };
        assert!(source.to_string().contains("git error: boom"));
    }

    #[test]
    fn redacts_url_userinfo_without_touching_plain_remotes() {
        assert_eq!(
            redact_remote_url("https://token:secret@example.com/acme/repo.git"),
            "https://<redacted>@example.com/acme/repo.git"
        );
        assert_eq!(
            redact_remote_url("https://example.com/acme/repo.git"),
            "https://example.com/acme/repo.git"
        );
        assert_eq!(
            redact_remote_url("git@example.com:acme/repo.git"),
            "git@example.com:acme/repo.git"
        );
    }

    // A read-only destination distinguishes rename from open-and-truncate:
    // rename(2) only needs directory write permission, while an in-place
    // write fails — and would leave a truncated file on a crash mid-write.
    #[test]
    fn write_atomic_replaces_a_read_only_destination() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("doc.md");
        fs::write(&dest, "old")?;
        let mut permissions = fs::metadata(&dest)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&dest, permissions)?;

        write_atomic(&dest, b"new")?;

        assert_eq!(fs::read(&dest)?, b"new");
        Ok(())
    }

    #[test]
    fn write_atomic_leaves_no_temp_file_behind() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("doc.md");

        write_atomic(&dest, b"content")?;

        let names: Vec<_> = fs::read_dir(dir.path())?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<_>>()?;
        assert_eq!(names, ["doc.md"]);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn slow_git_blocking_phase_does_not_delay_tokio_heartbeat() -> Result<()> {
        let slow_git_work = run_git_blocking(
            "test.slow_git_work",
            PathBuf::from("latency-test-worktree"),
            Duration::from_secs(2),
            || {
                std::thread::sleep(Duration::from_millis(400));
                Ok(())
            },
        );
        let heartbeat = async {
            let started = Instant::now();
            tokio::time::sleep(Duration::from_millis(20)).await;
            started.elapsed()
        };

        let (git_result, heartbeat_latency) = tokio::join!(slow_git_work, heartbeat);

        git_result?;
        assert!(
            heartbeat_latency < Duration::from_millis(200),
            "Tokio heartbeat was delayed by blocking Git work: {heartbeat_latency:?}"
        );
        Ok(())
    }
}
