mod blocks;

pub use blocks::{
    BlockMarkdownWrite, BlockMarkdownWriteOutcome, BlockMarkdownWriter, BlockMutationCommit,
    BlockMutationOutcome, BlockMutationState, BlockReviewItem, BlockReviewKind, BlockReviewState,
    BlockShadowBase, BlockTransactionRecord, BlockWriteBase, DocumentKind, DocumentScopeRef,
    NewBlockReviewItem, SessionSeedState, document_kind,
};
/// Re-exported because the store's block APIs speak it.
pub use quarry_collab_codec::BlockRow;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use quarry_cas::DiskCas;
use quarry_core::{
    ChangeType, CollabInviteToken, ConflictRecord, ConflictStatus, Document, DocumentHistoryEntry,
    DocumentLink, DocumentListEntry, DocumentSource, DocumentVersion, DocumentVersionContent,
    GcReport, GitPeer, GraphEdge, GraphNode, GraphResponse, INLINE_CONTENT_THRESHOLD, Library,
    LinkCollection, QuarryError, ReindexReport, Result, SearchResponse, SearchResult,
    SearchSuggestion, SyncStateEntry, TransactionRecord, TransactionState, VersionDiff,
    WriteOutcome, WritePrecondition, normalize_path, now_timestamp, parent_dirs,
};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::process;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, MutexGuard, OwnedMutexGuard, broadcast};
use turso::{Builder, Connection, Database, Row, Rows, Value, params};
use uuid::Uuid;

const TMP_TRANSACTION_LIBRARY_ID: &str = "__tmp__";
pub const TMP_DOCUMENT_SECRET_LEN: usize = 32;
pub const TMP_DOCUMENT_MARKDOWN_MAX_BYTES: usize = 1024 * 1024;
pub const TMP_DOCUMENT_DEFAULT_CONTENT_TYPE: &str = "text/markdown";

type WriteTransactionFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocumentLookupScope<'a> {
    Library { library_id: &'a str },
    Tmp,
}

/// True when `value` has the exact shape of a tmp document capability secret.
pub fn is_tmp_document_secret(value: &str) -> bool {
    value.len() == TMP_DOCUMENT_SECRET_LEN
        && value.chars().all(|character| character.is_ascii_hexdigit())
}

/// Returns the canonical Markdown media type for tmp-document writes.
///
/// Parameters such as `; charset=utf-8` are tolerated, but tmp documents are
/// intentionally Markdown-only. Library documents keep the broader raw/binary
/// content path.
pub fn normalize_tmp_markdown_content_type(content_type: &str) -> Result<&'static str> {
    normalize_markdown_content_type(content_type).ok_or_else(|| {
        QuarryError::UnsupportedMediaType(format!(
            "tmp documents are Markdown-only; unsupported content type {content_type}"
        ))
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TmpDocumentSecret(String);

impl TmpDocumentSecret {
    fn generate() -> Self {
        Self(Uuid::new_v4().simple().to_string())
    }

    fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if !is_tmp_document_secret(value) {
            return Err(QuarryError::InvalidPath(
                "invalid tmp document secret".to_string(),
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub db_path: PathBuf,
    pub cas_path: PathBuf,
    pub lock_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct TransactionMetadata {
    pub actor: Option<String>,
    pub message: Option<String>,
    /// `None` means the write path supplies its own default provenance.
    pub provenance: Option<JsonValue>,
}

#[derive(Clone, Debug)]
pub struct PutDocumentRequest {
    pub library: String,
    pub path: String,
    pub content: Vec<u8>,
    pub metadata: JsonValue,
    pub content_type: String,
    pub source: DocumentSource,
    pub precondition: WritePrecondition,
    pub origin_id: Option<String>,
    pub transaction: TransactionMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TmpTtl {
    Default,
    Unchanged,
    ExpiresAt(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryMetadata {
    pub path: String,
    pub mode: Option<i64>,
    pub mtime: String,
    pub inode: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollabDocumentSeed {
    pub document_id: String,
    pub head_version_id: String,
    pub content_type: String,
    pub content: Vec<u8>,
}

pub struct GlobalOperationGuard {
    _guard: OwnedMutexGuard<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreEventKind {
    DocumentPut,
    DocumentDelete,
    DocumentMove,
    LinksIndexed,
    DirectoryPut,
    DirectoryDelete,
    DirectoryMove,
    ConflictCreated,
    ConflictResolved,
    LibraryReindexed,
    GitSyncCompleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreEvent {
    kind: StoreEventKind,
    library_id: String,
    path: Option<String>,
    new_path: Option<String>,
    source: Option<DocumentSource>,
    tx_id: Option<String>,
    doc_id: Option<String>,
    version_id: Option<String>,
    conflict_id: Option<String>,
    peer_id: Option<String>,
    applied: Option<usize>,
    conflicts: Option<usize>,
    origin_id: Option<String>,
}

impl StoreEvent {
    fn new(kind: StoreEventKind, library_id: String) -> Self {
        Self {
            kind,
            library_id,
            path: None,
            new_path: None,
            source: None,
            tx_id: None,
            doc_id: None,
            version_id: None,
            conflict_id: None,
            peer_id: None,
            applied: None,
            conflicts: None,
            origin_id: None,
        }
    }

    pub fn document_put(
        library_id: String,
        path: String,
        source: DocumentSource,
        tx_id: String,
        doc_id: String,
        version_id: String,
        origin_id: Option<String>,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DocumentPut, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event.tx_id = Some(tx_id);
        event.doc_id = Some(doc_id);
        event.version_id = Some(version_id);
        event.origin_id = origin_id;
        event
    }

    pub fn document_delete(
        library_id: String,
        path: String,
        source: DocumentSource,
        tx_id: String,
        doc_id: Option<String>,
        origin_id: Option<String>,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DocumentDelete, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event.tx_id = Some(tx_id);
        event.doc_id = doc_id;
        event.origin_id = origin_id;
        event
    }

    pub fn document_move(
        library_id: String,
        path: String,
        new_path: String,
        source: DocumentSource,
        tx_id: String,
        doc_id: Option<String>,
        origin_id: Option<String>,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DocumentMove, library_id);
        event.path = Some(path);
        event.new_path = Some(new_path);
        event.source = Some(source);
        event.tx_id = Some(tx_id);
        event.doc_id = doc_id;
        event.origin_id = origin_id;
        event
    }

    pub fn links_indexed(library_id: String, path: String) -> Self {
        let mut event = Self::new(StoreEventKind::LinksIndexed, library_id);
        event.path = Some(path);
        event
    }

    pub fn directory_put(library_id: String, path: String, source: DocumentSource) -> Self {
        let mut event = Self::new(StoreEventKind::DirectoryPut, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event
    }

    pub fn directory_delete(library_id: String, path: String, source: DocumentSource) -> Self {
        let mut event = Self::new(StoreEventKind::DirectoryDelete, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event
    }

    pub fn directory_move(
        library_id: String,
        path: String,
        new_path: String,
        source: DocumentSource,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DirectoryMove, library_id);
        event.path = Some(path);
        event.new_path = Some(new_path);
        event.source = Some(source);
        event
    }

    pub fn conflict_created(library_id: String, path: String, conflict_id: String) -> Self {
        let mut event = Self::new(StoreEventKind::ConflictCreated, library_id);
        event.path = Some(path);
        event.conflict_id = Some(conflict_id);
        event
    }

    pub fn conflict_resolved(library_id: String, path: String, conflict_id: String) -> Self {
        let mut event = Self::new(StoreEventKind::ConflictResolved, library_id);
        event.path = Some(path);
        event.conflict_id = Some(conflict_id);
        event
    }

    pub fn library_reindexed(library_id: String) -> Self {
        Self::new(StoreEventKind::LibraryReindexed, library_id)
    }

    pub fn git_sync_completed(
        library_id: String,
        peer_id: String,
        applied: usize,
        conflicts: usize,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::GitSyncCompleted, library_id);
        event.source = Some(DocumentSource::Git);
        event.peer_id = Some(peer_id);
        event.applied = Some(applied);
        event.conflicts = Some(conflicts);
        event
    }

    pub fn kind(&self) -> StoreEventKind {
        self.kind
    }

    pub fn library_id(&self) -> &str {
        &self.library_id
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn new_path(&self) -> Option<&str> {
        self.new_path.as_deref()
    }

    pub fn source(&self) -> Option<&DocumentSource> {
        self.source.as_ref()
    }

    pub fn tx_id(&self) -> Option<&str> {
        self.tx_id.as_deref()
    }

    pub fn doc_id(&self) -> Option<&str> {
        self.doc_id.as_deref()
    }

    pub fn version_id(&self) -> Option<&str> {
        self.version_id.as_deref()
    }

    pub fn conflict_id(&self) -> Option<&str> {
        self.conflict_id.as_deref()
    }

    pub fn peer_id(&self) -> Option<&str> {
        self.peer_id.as_deref()
    }

    pub fn applied(&self) -> Option<usize> {
        self.applied
    }

    pub fn conflicts(&self) -> Option<usize> {
        self.conflicts
    }

    pub fn origin_id(&self) -> Option<&str> {
        self.origin_id.as_deref()
    }
}

#[derive(Clone)]
pub struct QuarryStore {
    db: Database,
    cas: DiskCas,
    write_lock: Arc<Mutex<()>>,
    operation_lock: Arc<Mutex<()>>,
    event_tx: broadcast::Sender<StoreEvent>,
    /// Phase 4: the whole-file Markdown write path for BlockDocuments,
    /// installed by the serving process (quarry-server owns the single
    /// reconciliation implementation and the session mode switch). Shared
    /// across store clones. Weak: the writer itself holds store clones, so a
    /// strong ref here would cycle and keep the store (and its lock file)
    /// alive past shutdown — the installer keeps the strong handle for the
    /// serving lifetime.
    block_markdown_writer: Arc<std::sync::RwLock<std::sync::Weak<dyn BlockMarkdownWriter>>>,
    _lock_guard: Arc<LockGuard>,
}

tokio::task_local! {
    static GLOBAL_OPERATION_ACTIVE: ();
}

struct LockGuard {
    path: Option<PathBuf>,
    file: Option<File>,
}

struct StagedChange {
    path: String,
    change_type: ChangeType,
    old_version_id: Option<String>,
    new_version_id: Option<String>,
    new_path: Option<String>,
}

fn log_store_event(event: &StoreEvent) {
    tracing::debug!(
        event = "storage.event.emitted",
        store_event = %store_event_name(event.kind()),
        library_id = %event.library_id(),
        path = event.path().unwrap_or(""),
        new_path = event.new_path().unwrap_or(""),
        tx_id = event.tx_id().unwrap_or(""),
        doc_id = event.doc_id().unwrap_or(""),
        version_id = event.version_id().unwrap_or(""),
        source = event.source().map(DocumentSource::as_str).unwrap_or(""),
        conflict_id = event.conflict_id().unwrap_or(""),
        peer_id = event.peer_id().unwrap_or(""),
        applied = event.applied().unwrap_or(0),
        conflicts = event.conflicts().unwrap_or(0),
        origin_id = event.origin_id().unwrap_or(""),
        "store event emitted"
    );
}

fn store_event_name(kind: StoreEventKind) -> &'static str {
    match kind {
        StoreEventKind::DocumentPut => "document.put.committed",
        StoreEventKind::DocumentDelete => "document.delete.committed",
        StoreEventKind::DocumentMove => "document.move.committed",
        StoreEventKind::LinksIndexed => "links.indexed",
        StoreEventKind::DirectoryPut => "directory.put.committed",
        StoreEventKind::DirectoryDelete => "directory.delete.committed",
        StoreEventKind::DirectoryMove => "directory.move.committed",
        StoreEventKind::ConflictCreated => "conflict.created",
        StoreEventKind::ConflictResolved => "conflict.resolved",
        StoreEventKind::LibraryReindexed => "library.reindexed",
        StoreEventKind::GitSyncCompleted => "git.sync.completed",
    }
}

fn precondition_name(precondition: &WritePrecondition) -> &'static str {
    match precondition {
        WritePrecondition::None => "none",
        WritePrecondition::IfMatch(_) => "if_match",
        WritePrecondition::IfNoneMatch => "if_none_match",
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
        if let Some(file) = &self.file {
            let _ = FileExt::unlock(file);
        }
    }
}

impl QuarryStore {
    pub async fn open(config: StoreConfig) -> Result<Self> {
        let started = Instant::now();
        tracing::debug!(
            event = "storage.open.started",
            db_path = %config.db_path.display(),
            cas_path = %config.cas_path.display(),
            "opening Quarry store"
        );
        if let Some(parent) = config.db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&config.cas_path)?;

        let lock_guard = acquire_lock(&config)?;
        let db_path = config.db_path.to_string_lossy().to_string();
        let db = Builder::new_local(&db_path)
            .build()
            .await
            .map_err(map_turso_error)?;
        let cas = DiskCas::open(config.cas_path)?;
        let (event_tx, _) = broadcast::channel(1024);
        let store = Self {
            db,
            cas,
            write_lock: Arc::new(Mutex::new(())),
            operation_lock: Arc::new(Mutex::new(())),
            event_tx,
            block_markdown_writer: Arc::new(std::sync::RwLock::new(std::sync::Weak::<
                blocks::NoBlockMarkdownWriter,
            >::new())),
            _lock_guard: Arc::new(lock_guard),
        };
        store.migrate().await?;
        tracing::debug!(
            event = "storage.open.completed",
            db_path = %db_path,
            cas_path = %store.cas.root().display(),
            duration_ms = started.elapsed().as_millis() as u64,
            "opened Quarry store"
        );
        Ok(store)
    }

    pub fn cas(&self) -> &DiskCas {
        &self.cas
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<StoreEvent> {
        self.event_tx.subscribe()
    }

    fn emit_event(&self, event: StoreEvent) {
        log_store_event(&event);
        let _ = self.event_tx.send(event);
    }

    pub async fn acquire_global_operation_lock(&self) -> GlobalOperationGuard {
        tracing::debug!(
            event = "storage.global_operation.waiting",
            "waiting for global operation lock"
        );
        let guard = self.operation_lock.clone().lock_owned().await;
        tracing::debug!(
            event = "storage.global_operation.acquired",
            "acquired global operation lock"
        );
        GlobalOperationGuard { _guard: guard }
    }

    async fn acquire_write_lock(&self) -> MutexGuard<'_, ()> {
        tracing::debug!(
            event = "storage.write_lock.waiting",
            "waiting for storage write lock"
        );
        let guard = self.write_lock.lock().await;
        tracing::debug!(
            event = "storage.write_lock.acquired",
            "acquired storage write lock"
        );
        guard
    }

    pub async fn run_global_operation<F, T>(&self, future: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        if GLOBAL_OPERATION_ACTIVE.try_with(|_| ()).is_ok() {
            return future.await;
        }
        let _guard = self.acquire_global_operation_lock().await;
        GLOBAL_OPERATION_ACTIVE.scope((), future).await
    }

    async fn normal_write_gate(&self) -> Option<GlobalOperationGuard> {
        if GLOBAL_OPERATION_ACTIVE.try_with(|_| ()).is_ok() {
            None
        } else {
            Some(self.acquire_global_operation_lock().await)
        }
    }

    async fn run_normal_write<F, T>(&self, future: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        let _guard = self.normal_write_gate().await;
        GLOBAL_OPERATION_ACTIVE.scope((), future).await
    }

    pub(crate) async fn write_transaction<T, F>(&self, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(&'a QuarryStore, &'a Connection) -> WriteTransactionFuture<'a, T>,
    {
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.acquire_write_lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = f(self, &conn).await;
        finish_tx(&conn, result).await
    }

    pub async fn create_library(&self, slug: &str) -> Result<Library> {
        validate_slug(slug)?;
        let slug = slug.to_string();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
                if let Some(existing) = Self::library_by_slug_or_id_conn(conn, &slug).await? {
                    return Ok(existing);
                }
                let now = now_timestamp();
                let library = Library {
                    id: Uuid::new_v4().to_string(),
                    slug,
                    created_at: now.into(),
                    settings: serde_json::json!({}),
                };
                conn.execute(
                    "INSERT INTO libraries (id, slug, created_at, settings_json) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        library.id.clone(),
                        library.slug.clone(),
                        library.created_at.to_string(),
                        library.settings.to_string()
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                ensure_inode_conn(conn, &library.id, "").await?;
                Ok(library)
            })
        })
        .await
    }

    pub async fn list_libraries(&self) -> Result<Vec<Library>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, slug, created_at, settings_json FROM libraries ORDER BY slug",
                (),
            )
            .await
            .map_err(map_turso_error)?;
        let mut libraries = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            libraries.push(library_from_row(&row)?);
        }
        Ok(libraries)
    }

    pub async fn get_library(&self, slug_or_id: &str) -> Result<Library> {
        let conn = self.conn()?;
        Self::library_by_slug_or_id_conn(&conn, slug_or_id)
            .await?
            .ok_or_else(|| QuarryError::NotFound(format!("library {slug_or_id}")))
    }

    pub async fn ensure_directory(
        &self,
        library: &str,
        path: &str,
        mode: Option<i64>,
    ) -> Result<DirectoryMetadata> {
        let path = normalize_directory_path(path)?;
        let library = library.to_string();
        let event_path = path.clone();
        let (metadata, library_id) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    ensure_inode_conn(conn, &library.id, "").await?;
                    if !path.is_empty() {
                        for dir in directory_path_and_parents(&path) {
                            ensure_inode_conn(conn, &library.id, &dir).await?;
                            conn.execute(
                                "INSERT INTO dir_metadata (library_id, path, mode, mtime)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(library_id, path) DO UPDATE SET
                           mode = COALESCE(excluded.mode, dir_metadata.mode),
                           mtime = excluded.mtime",
                                vec![
                                    Value::Text(library.id.clone()),
                                    Value::Text(dir),
                                    mode.map(Value::Integer).unwrap_or(Value::Null),
                                    Value::Text(now_timestamp()),
                                ],
                            )
                            .await
                            .map_err(map_turso_error)?;
                        }
                    }
                    let metadata = store
                        .directory_metadata_conn(conn, &library.id, &path)
                        .await?;
                    Ok((metadata, library_id))
                })
            })
            .await?;
        self.emit_event(StoreEvent::directory_put(
            library_id,
            event_path,
            DocumentSource::Fuse,
        ));
        Ok(metadata)
    }

    pub async fn update_directory_metadata(
        &self,
        library: &str,
        path: &str,
        mode: Option<i64>,
        mtime: Option<&str>,
        source: DocumentSource,
    ) -> Result<DirectoryMetadata> {
        let path = normalize_directory_path(path)?;
        if path.is_empty() {
            return Err(QuarryError::Conflict(
                "cannot update root directory metadata".to_string(),
            ));
        }
        let library = library.to_string();
        let event_path = path.clone();
        let mtime = mtime.map(str::to_string);
        let source_for_event = source.clone();
        let (metadata, library_id) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    let updated = conn
                        .execute(
                            "UPDATE dir_metadata
                     SET mode = COALESCE(?1, mode),
                         mtime = COALESCE(?2, mtime)
                     WHERE library_id = ?3 AND path = ?4",
                            vec![
                                mode.map(Value::Integer).unwrap_or(Value::Null),
                                mtime.map(Value::Text).unwrap_or(Value::Null),
                                Value::Text(library.id.clone()),
                                Value::Text(path.clone()),
                            ],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    if updated == 0 {
                        return Err(QuarryError::NotFound(path.clone()));
                    }
                    let metadata = store
                        .directory_metadata_conn(conn, &library.id, &path)
                        .await?;
                    Ok((metadata, library_id))
                })
            })
            .await?;
        self.emit_event(StoreEvent::directory_put(
            library_id,
            event_path,
            source_for_event,
        ));
        Ok(metadata)
    }

    pub async fn move_directory(
        &self,
        library: &str,
        from_path: &str,
        to_path: &str,
        source: DocumentSource,
    ) -> Result<()> {
        let from_path = normalize_directory_path(from_path)?;
        let to_path = normalize_directory_path(to_path)?;
        if from_path.is_empty() || to_path.is_empty() {
            return Err(QuarryError::Conflict(
                "cannot rename root directory".to_string(),
            ));
        }
        if to_path == from_path || to_path.starts_with(&format!("{from_path}/")) {
            return Err(QuarryError::Conflict(format!(
                "cannot move {from_path} into itself"
            )));
        }
        let library = library.to_string();
        let event_from_path = from_path.clone();
        let event_to_path = to_path.clone();
        let source_for_event = source.clone();
        let library_id = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    let from_prefix = format!("{from_path}/");
                    let mut rows = conn
                        .query(
                            "SELECT path, mode, mtime FROM dir_metadata
                     WHERE library_id = ?1 AND (path = ?2 OR path LIKE ?3)
                     ORDER BY length(path)",
                            params![
                                library.id.clone(),
                                from_path.clone(),
                                format!("{from_prefix}%")
                            ],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    let mut directories = Vec::new();
                    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
                        directories.push((text(&row, 0)?, opt_int(&row, 1)?, text(&row, 2)?));
                    }
                    let mut document_rows = conn
                        .query(
                            "SELECT 1 FROM documents
                     WHERE document_scope = 'library'
                       AND library_id = ?1
                       AND deleted_at IS NULL
                       AND head_version_id IS NOT NULL
                       AND (expires_at IS NULL OR expires_at > ?2)
                       AND path LIKE ?3
                     LIMIT 1",
                            params![
                                library.id.clone(),
                                now_timestamp(),
                                format!("{from_prefix}%")
                            ],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    let has_documents = document_rows
                        .next()
                        .await
                        .map_err(map_turso_error)?
                        .is_some();
                    if directories.is_empty() && !has_documents {
                        return Err(QuarryError::NotFound(from_path.clone()));
                    }
                    move_path_prefix_inodes_conn(conn, &library.id, &from_path, &to_path).await?;
                    for (old_path, _, _) in &directories {
                        let new_path = replace_path_prefix(old_path, &from_path, &to_path);
                        conn.execute(
                            "DELETE FROM dir_metadata WHERE library_id = ?1 AND path = ?2",
                            params![library.id.clone(), new_path],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    }
                    for (old_path, _, _) in &directories {
                        conn.execute(
                            "DELETE FROM dir_metadata WHERE library_id = ?1 AND path = ?2",
                            params![library.id.clone(), old_path.clone()],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    }
                    for (old_path, mode, mtime) in directories {
                        conn.execute(
                            "INSERT INTO dir_metadata (library_id, path, mode, mtime)
                     VALUES (?1, ?2, ?3, ?4)",
                            vec![
                                Value::Text(library.id.clone()),
                                Value::Text(replace_path_prefix(&old_path, &from_path, &to_path)),
                                mode.map(Value::Integer).unwrap_or(Value::Null),
                                Value::Text(mtime),
                            ],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    }
                    Ok(library_id)
                })
            })
            .await?;
        self.emit_event(StoreEvent::directory_move(
            library_id,
            event_from_path,
            event_to_path,
            source_for_event,
        ));
        Ok(())
    }

    pub async fn remove_directory(&self, library: &str, path: &str) -> Result<()> {
        let path = normalize_directory_path(path)?;
        if path.is_empty() {
            return Err(QuarryError::Conflict(
                "cannot remove root directory".to_string(),
            ));
        }
        let library = library.to_string();
        let event_path = path.clone();
        let library_id = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    conn.execute(
                        "DELETE FROM dir_metadata WHERE library_id = ?1 AND path = ?2",
                        params![library.id, path.clone()],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    Ok(library_id)
                })
            })
            .await?;
        self.emit_event(StoreEvent::directory_delete(
            library_id,
            event_path,
            DocumentSource::Fuse,
        ));
        Ok(())
    }

    pub async fn list_directories(
        &self,
        library: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<DirectoryMetadata>> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let normalized_prefix = match prefix {
            Some("") | None => None,
            Some(prefix) => Some(normalize_directory_path(prefix)?),
        };
        let (sql, params) = if let Some(prefix) = normalized_prefix {
            (
                "SELECT dm.path, dm.mode, dm.mtime, i.inode
                 FROM dir_metadata dm
                 JOIN inodes i ON i.library_id = dm.library_id AND i.path = dm.path
                 WHERE dm.library_id = ?1 AND dm.path LIKE ?2
                 ORDER BY dm.path",
                vec![Value::Text(library.id), Value::Text(format!("{prefix}%"))],
            )
        } else {
            (
                "SELECT dm.path, dm.mode, dm.mtime, i.inode
                 FROM dir_metadata dm
                 JOIN inodes i ON i.library_id = dm.library_id AND i.path = dm.path
                 WHERE dm.library_id = ?1
                 ORDER BY dm.path",
                vec![Value::Text(library.id)],
            )
        };
        let mut rows = conn.query(sql, params).await.map_err(map_turso_error)?;
        let mut directories = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            directories.push(directory_metadata_from_row(&row)?);
        }
        Ok(directories)
    }

    pub async fn inode_for_path(&self, library: &str, path: &str) -> Result<i64> {
        let path = normalize_directory_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let mut rows = conn
            .query(
                "SELECT inode FROM inodes WHERE library_id = ?1 AND path = ?2 LIMIT 1",
                params![library.id, path.clone()],
            )
            .await
            .map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| int(&row, 0))
            .transpose()?
            .ok_or(QuarryError::NotFound(path))
    }

    pub async fn path_for_inode(&self, library: &str, inode: i64) -> Result<String> {
        if inode <= 0 {
            return Err(QuarryError::InvalidPath(format!("invalid inode {inode}")));
        }
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let mut rows = conn
            .query(
                "SELECT path FROM inodes WHERE library_id = ?1 AND inode = ?2 LIMIT 1",
                params![library.id, inode],
            )
            .await
            .map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| text(&row, 0))
            .transpose()?
            .ok_or_else(|| QuarryError::NotFound(format!("inode {inode}")))
    }

    pub async fn put_document(&self, request: PutDocumentRequest) -> Result<WriteOutcome> {
        let outcome = self
            .commit_document_without_events_with_transaction(
                &request.library,
                &request.path,
                request.content,
                request.metadata,
                &request.content_type,
                request.source,
                request.precondition,
                request.transaction,
            )
            .await?;
        self.emit_document_put_events(&outcome, request.origin_id);
        Ok(outcome)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "legacy write API still passes document fields explicitly"
    )]
    pub async fn commit_document_without_events(
        &self,
        library: &str,
        path: &str,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
        source: DocumentSource,
        precondition: WritePrecondition,
    ) -> Result<WriteOutcome> {
        self.commit_document_without_events_with_transaction(
            library,
            path,
            content,
            metadata,
            content_type,
            source,
            precondition,
            TransactionMetadata::default(),
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "origin metadata extends the legacy write API"
    )]
    pub async fn commit_document_without_events_with_transaction(
        &self,
        library: &str,
        path: &str,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
        source: DocumentSource,
        precondition: WritePrecondition,
        transaction: TransactionMetadata,
    ) -> Result<WriteOutcome> {
        let path = normalize_path(path)?;
        let content_bytes = content.len();
        let started = Instant::now();
        tracing::debug!(
            event = "document.put.started",
            library,
            path = %path,
            source = source.as_str(),
            precondition = precondition_name(&precondition),
            content_type,
            content_bytes,
            "document put started"
        );
        let library = library.to_string();
        let content_type = content_type.to_string();
        let outcome = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    store
                        .check_precondition_conn(conn, &library.id, &path, &precondition)
                        .await?;
                    let tx = insert_transaction_conn(
                        conn,
                        &library.id,
                        source,
                        transaction.actor,
                        transaction.message,
                        transaction
                            .provenance
                            .unwrap_or_else(|| serde_json::json!({ "mode": "auto_commit" })),
                    )
                    .await?;
                    let (doc_id, old_version_id) =
                        ensure_document_conn(conn, &library.id, &path, &now_timestamp()).await?;
                    let version = store
                        .insert_version_conn(
                            conn,
                            &doc_id,
                            &tx.id,
                            content,
                            metadata,
                            &content_type,
                        )
                        .await?;
                    insert_change_conn(
                        conn,
                        &tx.id,
                        &path,
                        ChangeType::Put,
                        old_version_id.as_deref(),
                        Some(&version.id),
                        None,
                    )
                    .await?;
                    publish_put_conn(conn, &doc_id, &version.id).await?;
                    // A legacy put bypasses the block import path, so any block
                    // projection for this document is now stale: drop it fail-closed
                    // (see the `blocks` module docs).
                    blocks::clear_block_state_conn(conn, &doc_id).await?;
                    ensure_path_inodes_conn(conn, &library.id, &path).await?;
                    store.reindex_links_conn(conn, &library.id).await?;
                    commit_transaction_record_conn(conn, &tx.id).await?;
                    let document = store.document_entry_conn(conn, &library.id, &path).await?;
                    let tx = Self::transaction_conn(conn, &tx.id).await?;
                    Ok(WriteOutcome {
                        document,
                        version,
                        transaction: tx,
                    })
                })
            })
            .await?;
        tracing::debug!(
            event = "document.put.committed",
            library_id = %outcome.transaction.library_id,
            path = %outcome.document.path,
            tx_id = %outcome.transaction.id,
            doc_id = %outcome.document.id,
            version_id = %outcome.version.id,
            source = outcome.transaction.source.as_str(),
            content_type = %outcome.version.content_type,
            content_bytes = outcome.version.byte_size,
            duration_ms = started.elapsed().as_millis() as u64,
            "document put committed"
        );
        Ok(outcome)
    }

    pub fn emit_document_put_events(&self, outcome: &WriteOutcome, origin_id: Option<String>) {
        self.emit_event(StoreEvent::document_put(
            outcome.transaction.library_id.clone(),
            outcome.document.path.clone(),
            outcome.transaction.source.clone(),
            outcome.transaction.id.clone(),
            outcome.document.id.to_string(),
            outcome.version.id.to_string(),
            origin_id,
        ));
        self.emit_event(StoreEvent::links_indexed(
            outcome.transaction.library_id.clone(),
            outcome.document.path.clone(),
        ));
    }

    pub async fn get_document(&self, library: &str, path: &str) -> Result<Document> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        self.document_conn(&conn, &library.id, &path).await
    }

    pub async fn get_document_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
    ) -> Result<Document> {
        match scope {
            DocumentScopeRef::Library { slug } => self.get_document(slug, path).await,
            DocumentScopeRef::Tmp => self.get_tmp_document(path).await,
        }
    }

    pub async fn head_document_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
    ) -> Result<DocumentListEntry> {
        match scope {
            DocumentScopeRef::Library { slug } => self.head_document(slug, path).await,
            DocumentScopeRef::Tmp => self.head_tmp_document(path).await,
        }
    }

    pub async fn document_version_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
        version_id: &str,
    ) -> Result<DocumentVersionContent> {
        match scope {
            DocumentScopeRef::Library { slug } => {
                self.document_version(slug, path, version_id).await
            }
            DocumentScopeRef::Tmp => self.tmp_document_version(path, version_id).await,
        }
    }

    pub async fn put_tmp_document(
        &self,
        path: &str,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
        ttl: TmpTtl,
        precondition: WritePrecondition,
    ) -> Result<WriteOutcome> {
        self.put_tmp_document_with_transaction(
            path,
            content,
            metadata,
            content_type,
            ttl,
            precondition,
            None,
            TransactionMetadata::default(),
        )
        .await
    }

    pub async fn create_tmp_document(
        &self,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
        ttl: TmpTtl,
    ) -> Result<WriteOutcome> {
        let secret = TmpDocumentSecret::generate();
        self.put_tmp_document(
            secret.as_str(),
            content,
            metadata,
            content_type,
            ttl,
            WritePrecondition::IfNoneMatch,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "transaction staging needs document and CAS metadata"
    )]
    pub async fn put_tmp_document_with_transaction(
        &self,
        path: &str,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
        ttl: TmpTtl,
        precondition: WritePrecondition,
        origin_id: Option<String>,
        transaction: TransactionMetadata,
    ) -> Result<WriteOutcome> {
        let secret = TmpDocumentSecret::parse(path)?;
        let path = secret.as_str().to_string();
        let (content, metadata, content_type) =
            validate_tmp_markdown_write(content, metadata, content_type)?;
        let outcome = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let provenance = transaction
                        .provenance
                        .unwrap_or_else(|| serde_json::json!({ "mode": "tmp_document" }));
                    store
                        .check_tmp_precondition_conn(conn, &path, &precondition)
                        .await?;
                    let expires_at = match ttl {
                        TmpTtl::Default => default_tmp_expires_at(),
                        TmpTtl::Unchanged => store
                            .tmp_document_expires_at_conn(conn, &path)
                            .await?
                            .unwrap_or_else(default_tmp_expires_at),
                        TmpTtl::ExpiresAt(expires_at) => expires_at,
                    };
                    let tx = insert_transaction_conn(
                        conn,
                        TMP_TRANSACTION_LIBRARY_ID,
                        DocumentSource::Rest,
                        transaction.actor,
                        transaction.message,
                        provenance,
                    )
                    .await?;
                    let (doc_id, old_version_id) =
                        ensure_tmp_document_conn(conn, &path, &expires_at, &now_timestamp())
                            .await?;
                    let version = store
                        .insert_version_conn(
                            conn,
                            &doc_id,
                            &tx.id,
                            content,
                            metadata,
                            &content_type,
                        )
                        .await?;
                    insert_change_conn(
                        conn,
                        &tx.id,
                        &path,
                        ChangeType::Put,
                        old_version_id.as_deref(),
                        Some(&version.id),
                        None,
                    )
                    .await?;
                    publish_put_conn(conn, &doc_id, &version.id).await?;
                    conn.execute(
                        "UPDATE documents SET expires_at = ?1 WHERE id = ?2",
                        params![expires_at, doc_id.clone()],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    commit_transaction_record_conn(conn, &tx.id).await?;
                    let document = store.tmp_document_entry_conn(conn, &path).await?;
                    let tx = Self::transaction_conn(conn, &tx.id).await?;
                    Ok(WriteOutcome {
                        document,
                        version,
                        transaction: tx,
                    })
                })
            })
            .await?;
        self.emit_document_put_events(&outcome, origin_id);
        Ok(outcome)
    }

    pub async fn get_tmp_document(&self, path: &str) -> Result<Document> {
        let secret = TmpDocumentSecret::parse(path)?;
        let conn = self.conn()?;
        self.tmp_document_conn(&conn, secret.as_str()).await
    }

    pub async fn head_tmp_document(&self, path: &str) -> Result<DocumentListEntry> {
        let secret = TmpDocumentSecret::parse(path)?;
        let conn = self.conn()?;
        self.tmp_document_entry_conn(&conn, secret.as_str()).await
    }

    pub async fn raw_tmp_version_history(&self, path: &str) -> Result<Vec<DocumentVersion>> {
        let secret = TmpDocumentSecret::parse(path)?;
        let path = secret.as_str().to_string();
        let conn = self.conn()?;
        let document_id = self
            .tmp_document_id_conn(&conn, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        self.raw_version_history_for_document_conn(&conn, &document_id)
            .await
    }

    pub async fn tmp_version_history(&self, path: &str) -> Result<Vec<DocumentHistoryEntry>> {
        let raw = self.raw_tmp_version_history(path).await?;
        Ok(group_version_history(raw))
    }

    pub async fn tmp_document_version(
        &self,
        path: &str,
        version_id: &str,
    ) -> Result<DocumentVersionContent> {
        let secret = TmpDocumentSecret::parse(path)?;
        let path = secret.as_str().to_string();
        let conn = self.conn()?;
        let document_id = self
            .tmp_document_id_conn(&conn, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        let (version, content) = self
            .version_content_conn(&conn, &document_id, version_id)
            .await?;
        Ok(DocumentVersionContent {
            version,
            content: String::from_utf8_lossy(&content).into_owned(),
        })
    }

    pub async fn delete_tmp_document(&self, path: &str) -> Result<TransactionRecord> {
        let secret = TmpDocumentSecret::parse(path)?;
        let path = secret.as_str().to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let (doc_id, head_version_id) = store
                    .tmp_document_identity_conn(conn, &path)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
                let tx = insert_transaction_conn(
                    conn,
                    TMP_TRANSACTION_LIBRARY_ID,
                    DocumentSource::Rest,
                    None,
                    None,
                    serde_json::json!({ "mode": "tmp_document" }),
                )
                .await?;
                insert_change_conn(
                    conn,
                    &tx.id,
                    &path,
                    ChangeType::Delete,
                    head_version_id.as_deref(),
                    None,
                    None,
                )
                .await?;
                conn.execute(
                    "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                    params![now_timestamp(), doc_id],
                )
                .await
                .map_err(map_turso_error)?;
                commit_transaction_record_conn(conn, &tx.id).await?;
                Self::transaction_conn(conn, &tx.id).await
            })
        })
        .await
    }

    pub async fn set_tmp_document_ttl(
        &self,
        path: &str,
        expires_at: Option<String>,
    ) -> Result<DocumentListEntry> {
        let Some(expires_at) = expires_at else {
            return Err(QuarryError::InvalidInput(
                "tmp document TTL cannot be removed".to_string(),
            ));
        };
        let secret = TmpDocumentSecret::parse(path)?;
        let path = secret.as_str().to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let (doc_id, _) = store
                    .tmp_document_identity_conn(conn, &path)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
                conn.execute(
                    "UPDATE documents SET expires_at = ?1, updated_at = ?2 WHERE id = ?3",
                    params![expires_at, now_timestamp(), doc_id],
                )
                .await
                .map_err(map_turso_error)?;
                store.tmp_document_entry_any_conn(conn, &path).await
            })
        })
        .await
    }

    pub async fn set_document_ttl(
        &self,
        library: &str,
        path: &str,
        expires_at: Option<String>,
    ) -> Result<DocumentListEntry> {
        let path = normalize_path(path)?;
        let library = library.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let library = Self::require_library_conn(conn, &library).await?;
                let doc_id = store
                    .library_document_id_any_conn(conn, &library.id, &path)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
                conn.execute(
                    "UPDATE documents SET expires_at = ?1, updated_at = ?2 WHERE id = ?3",
                    vec![
                        opt_value(expires_at),
                        Value::Text(now_timestamp()),
                        Value::Text(doc_id),
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                store
                    .document_entry_any_conn(conn, &library.id, &path)
                    .await
            })
        })
        .await
    }

    pub async fn promote_tmp_document(
        &self,
        tmp_path: &str,
        library: &str,
        target_path: &str,
        precondition: WritePrecondition,
    ) -> Result<DocumentListEntry> {
        let tmp_secret = TmpDocumentSecret::parse(tmp_path)?;
        let tmp_path = tmp_secret.as_str().to_string();
        let target_path = normalize_path(target_path)?;
        let library = library.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let library = Self::require_library_conn(conn, &library).await?;
                store
                    .check_tmp_precondition_conn(conn, &tmp_path, &precondition)
                    .await?;
                if store
                    .document_identity_conn(conn, &library.id, &target_path)
                    .await?
                    .is_some()
                {
                    return Err(QuarryError::Conflict(format!(
                        "{target_path} already exists"
                    )));
                }
                let (doc_id, _) = store
                    .tmp_document_identity_conn(conn, &tmp_path)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(tmp_path.clone()))?;
                conn.execute(
                    "UPDATE documents
                 SET library_id = ?1,
                     document_scope = 'library',
                     path = ?2,
                     expires_at = NULL,
                     updated_at = ?3
                 WHERE id = ?4",
                    params![
                        library.id.clone(),
                        target_path.clone(),
                        now_timestamp(),
                        doc_id
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                ensure_path_inodes_conn(conn, &library.id, &target_path).await?;
                store.reindex_links_conn(conn, &library.id).await?;
                store
                    .document_entry_conn(conn, &library.id, &target_path)
                    .await
            })
        })
        .await
    }

    pub async fn collab_document_seed(
        &self,
        document_id: &str,
    ) -> Result<Option<CollabDocumentSeed>> {
        let conn = self.conn()?;
        self.collab_document_seed_conn(&conn, document_id).await
    }

    pub async fn create_collab_invite_token(
        &self,
        library: &str,
        path: &str,
        role: &str,
        by_hint: Option<String>,
    ) -> Result<CollabInviteToken> {
        let role = normalize_collab_invite_role(role)?;
        let path = normalize_path(path)?;
        let library = library.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let library = Self::require_library_conn(conn, &library).await?;
                let (document_id, _) = store
                    .document_identity_conn(conn, &library.id, &path)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
                let token = CollabInviteToken {
                    id: Uuid::new_v4().to_string(),
                    document_id: document_id.into(),
                    role,
                    by_hint: by_hint.filter(|value| !value.trim().is_empty()),
                    created_at: now_timestamp().into(),
                    revoked_at: None,
                };
                conn.execute(
                    "INSERT INTO collab_invite_tokens
                 (id, document_id, role, by_hint, created_at, revoked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                    vec![
                        Value::Text(token.id.clone()),
                        Value::Text(token.document_id.to_string()),
                        Value::Text(token.role.clone()),
                        opt_value(token.by_hint.clone()),
                        Value::Text(token.created_at.to_string()),
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                Ok(token)
            })
        })
        .await
    }

    pub async fn collab_invite_tokens(
        &self,
        library: &str,
        path: &str,
    ) -> Result<Vec<CollabInviteToken>> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let (document_id, _) = self
            .document_identity_conn(&conn, &library.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        self.collab_invite_tokens_for_document_conn(&conn, &document_id)
            .await
    }

    pub async fn revoke_collab_invite_token(&self, token_id: &str) -> Result<CollabInviteToken> {
        let token_id = token_id.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let revoked_at = now_timestamp();
                let changed = conn
                    .execute(
                        "UPDATE collab_invite_tokens
                     SET revoked_at = COALESCE(revoked_at, ?2)
                     WHERE id = ?1",
                        params![token_id.clone(), revoked_at],
                    )
                    .await
                    .map_err(map_turso_error)?;
                if changed == 0 {
                    return Err(QuarryError::NotFound(format!("invite token {token_id}")));
                }
                store
                    .collab_invite_token_conn(conn, &token_id)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(format!("invite token {token_id}")))
            })
        })
        .await
    }

    pub async fn head_document(&self, library: &str, path: &str) -> Result<DocumentListEntry> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        self.document_entry_conn(&conn, &library.id, &path).await
    }

    pub async fn list_documents(
        &self,
        library: &str,
        prefix: Option<&str>,
        limit: Option<u64>,
    ) -> Result<Vec<DocumentListEntry>> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let normalized_prefix = match prefix {
            Some("") | None => None,
            Some(prefix) => Some(normalize_prefix(prefix)?),
        };
        let limit = limit.unwrap_or(1000).min(10_000) as i64;
        let now = now_timestamp();

        let (sql, params) = if let Some(prefix) = normalized_prefix {
            (
                "SELECT d.id, d.library_id, d.path, d.head_version_id, v.content_type, v.byte_size, v.content_hash, v.metadata_json, d.expires_at, d.updated_at
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.document_scope = 'library'
                   AND d.library_id = ?1
                   AND d.deleted_at IS NULL
                   AND d.head_version_id IS NOT NULL
                   AND (d.expires_at IS NULL OR d.expires_at > ?2)
                   AND d.path LIKE ?3
                 ORDER BY d.path LIMIT ?4",
                vec![
                    Value::Text(library.id),
                    Value::Text(now),
                    Value::Text(format!("{prefix}%")),
                    Value::Integer(limit),
                ],
            )
        } else {
            (
                "SELECT d.id, d.library_id, d.path, d.head_version_id, v.content_type, v.byte_size, v.content_hash, v.metadata_json, d.expires_at, d.updated_at
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.document_scope = 'library'
                   AND d.library_id = ?1
                   AND d.deleted_at IS NULL
                   AND d.head_version_id IS NOT NULL
                   AND (d.expires_at IS NULL OR d.expires_at > ?2)
                 ORDER BY d.path LIMIT ?3",
                vec![Value::Text(library.id), Value::Text(now), Value::Integer(limit)],
            )
        };

        let mut rows = conn.query(sql, params).await.map_err(map_turso_error)?;
        let mut documents = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            documents.push(document_entry_from_row(&row)?);
        }
        Ok(documents)
    }

    pub async fn search_documents(
        &self,
        library: &str,
        query: &str,
        limit: Option<u64>,
    ) -> Result<SearchResponse> {
        let query = query.trim();
        let query_lc = query.to_lowercase();
        let tag_query_lc = query.trim_start_matches('#').to_lowercase();
        let limit = limit.unwrap_or(50).min(100) as usize;
        let conn = self.conn()?;
        let library_record = Self::require_library_conn(&conn, library).await?;
        let documents = self
            .document_entries_for_library_conn(&conn, &library_record.id, 10_000)
            .await?;
        let mut results = Vec::new();

        for entry in documents {
            let title = title_for_entry(&entry);
            let mut matched_fields = Vec::new();
            let mut score = 0.0;
            let mut snippet = None;

            if query.is_empty() || entry.path.to_lowercase().contains(&query_lc) {
                push_unique(&mut matched_fields, "path");
                score += 3.0;
            }
            if query.is_empty() || title.to_lowercase().contains(&query_lc) {
                push_unique(&mut matched_fields, "title");
                score += 2.0;
            }
            if !query.is_empty()
                && metadata_aliases(&entry.metadata)
                    .iter()
                    .any(|alias| alias.to_lowercase().contains(&query_lc))
            {
                push_unique(&mut matched_fields, "alias");
                score += 2.5;
            }
            if !query.is_empty() && is_textual_content_type(&entry.content_type) {
                let document = self.get_document(library, &entry.path).await?;
                let body = String::from_utf8_lossy(&document.content);
                if let Some(index) = body.to_lowercase().find(&query_lc) {
                    push_unique(&mut matched_fields, "body");
                    score += 1.0;
                    snippet = Some(make_snippet(&body, index, query.len()));
                }
            }
            if !tag_query_lc.is_empty() {
                let tag_match = self
                    .links_for_source_conn(&conn, &library_record.id, &entry.id)
                    .await?
                    .into_iter()
                    .filter(|link| link.target_kind == "tag")
                    .any(|link| link.target_text.to_lowercase().contains(&tag_query_lc));
                if tag_match {
                    push_unique(&mut matched_fields, "tag");
                    score += 2.5;
                }
            }

            if score > 0.0 {
                results.push(SearchResult {
                    document_id: entry.id,
                    path: entry.path,
                    title,
                    content_type: entry.content_type,
                    score,
                    snippet,
                    matched_fields,
                    head_version_id: entry.head_version_id,
                });
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        results.truncate(limit);

        Ok(SearchResponse {
            results,
            cursor: None,
        })
    }

    pub async fn suggest_documents(
        &self,
        library: &str,
        query: &str,
        limit: Option<u64>,
    ) -> Result<Vec<SearchSuggestion>> {
        let query_lc = query.trim().to_lowercase();
        let limit = limit.unwrap_or(20).min(100) as usize;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let documents = self
            .document_entries_for_library_conn(&conn, &library.id, 10_000)
            .await?;
        let mut suggestions = Vec::new();

        for entry in documents {
            let title = title_for_entry(&entry);
            let title_match = query_lc.is_empty() || title.to_lowercase().contains(&query_lc);
            let path_match = query_lc.is_empty() || entry.path.to_lowercase().contains(&query_lc);
            if title_match || path_match {
                suggestions.push(SearchSuggestion {
                    path: entry.path.clone(),
                    title,
                    match_type: if title_match { "title" } else { "path" }.to_string(),
                    head_version_id: entry.head_version_id.clone(),
                    matched_text: Some(if title_match {
                        title_for_entry(&entry)
                    } else {
                        entry.path.clone()
                    }),
                    target_anchor: None,
                });
            }

            for alias in metadata_aliases(&entry.metadata) {
                if query_lc.is_empty() || alias.to_lowercase().contains(&query_lc) {
                    suggestions.push(SearchSuggestion {
                        path: entry.path.clone(),
                        title: title_for_entry(&entry),
                        match_type: "alias".to_string(),
                        head_version_id: entry.head_version_id.clone(),
                        matched_text: Some(alias),
                        target_anchor: None,
                    });
                }
            }

            if is_textual_content_type(&entry.content_type) {
                for link in self
                    .links_for_source_conn(&conn, &library.id, &entry.id)
                    .await?
                    .into_iter()
                    .filter(|link| link.target_kind == "heading")
                {
                    if query_lc.is_empty() || link.target_text.to_lowercase().contains(&query_lc) {
                        suggestions.push(SearchSuggestion {
                            path: entry.path.clone(),
                            title: title_for_entry(&entry),
                            match_type: "heading".to_string(),
                            head_version_id: entry.head_version_id.clone(),
                            matched_text: Some(link.target_text.clone()),
                            target_anchor: Some(link.target_text),
                        });
                    }
                }
            }
        }

        suggestions.sort_by(|a, b| {
            suggestion_match_rank(&a.match_type)
                .cmp(&suggestion_match_rank(&b.match_type))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.matched_text.cmp(&b.matched_text))
        });
        suggestions.truncate(limit);
        Ok(suggestions)
    }

    pub async fn reindex_library(&self, library: &str) -> Result<ReindexReport> {
        let library = library.to_string();
        let (library_id, report) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    let indexed_documents = store.reindex_links_conn(conn, &library.id).await?;
                    Ok((
                        library_id,
                        ReindexReport {
                            ok: true,
                            indexed_documents,
                        },
                    ))
                })
            })
            .await?;
        self.emit_event(StoreEvent::library_reindexed(library_id));
        Ok(report)
    }

    pub async fn emit_git_sync_completed(
        &self,
        library: &str,
        peer_id: &str,
        applied: usize,
        conflicts: usize,
    ) -> Result<()> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        self.emit_event(StoreEvent::git_sync_completed(
            library.id,
            peer_id.to_string(),
            applied,
            conflicts,
        ));
        Ok(())
    }

    pub async fn outgoing_links(&self, library: &str, path: &str) -> Result<LinkCollection> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let document = self.document_entry_conn(&conn, &library.id, &path).await?;
        Ok(LinkCollection {
            path: document.path.clone(),
            links: self
                .links_for_source_conn(&conn, &library.id, &document.id)
                .await?,
        })
    }

    pub async fn backlinks(&self, library: &str, path: &str) -> Result<LinkCollection> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let target = self.document_entry_conn(&conn, &library.id, &path).await?;
        Ok(LinkCollection {
            path: target.path,
            links: self
                .links_for_target_conn(&conn, &library.id, &target.id)
                .await?,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "link graph query exposes independent filters"
    )]
    pub async fn graph(
        &self,
        library: &str,
        root: Option<&str>,
        depth: Option<u64>,
        limit: Option<u64>,
        folder: Option<&str>,
        tag: Option<&str>,
        link_kind: Option<&str>,
        resolved: Option<bool>,
    ) -> Result<GraphResponse> {
        let limit = limit.unwrap_or(500).min(10_000) as usize;
        let depth = depth.unwrap_or(1).min(32);
        let root = root.map(normalize_path).transpose()?;
        let folder = folder
            .map(normalize_graph_folder)
            .transpose()?
            .filter(|folder| !folder.is_empty());
        let tag = tag.map(normalize_graph_tag).filter(|tag| !tag.is_empty());
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let documents = self
            .document_entries_for_library_conn(&conn, &library.id, 10_000)
            .await?;
        let document_by_id: HashMap<String, &DocumentListEntry> = documents
            .iter()
            .map(|entry| (entry.id.to_string(), entry))
            .collect();
        let mut node_map: HashMap<String, GraphNode> = HashMap::new();
        let mut edges = Vec::new();
        let mut candidate_nodes = 0usize;

        let mut add_node = |entry: &DocumentListEntry| {
            if node_map.contains_key(entry.id.as_str()) {
                return;
            }
            candidate_nodes += 1;
            if node_map.len() < limit {
                node_map.insert(entry.id.to_string(), graph_node_from_entry(entry));
            }
        };

        let document_matches_folder = |entry: &DocumentListEntry| {
            folder
                .as_deref()
                .is_none_or(|folder| path_is_in_folder(&entry.path, folder))
        };
        let link_matches_folder = |link: &DocumentLink| {
            folder.as_deref().is_none_or(|folder| {
                path_is_in_folder(&link.src_path, folder)
                    && link
                        .target_path
                        .as_deref()
                        .is_none_or(|path| path_is_in_folder(path, folder))
            })
        };
        let link_matches_tag = |link: &DocumentLink| {
            tag.as_deref().is_none_or(|tag| {
                link.target_kind == "tag" && link.target_text.eq_ignore_ascii_case(tag)
            })
        };
        let links: Vec<DocumentLink> = self
            .links_for_library_conn(&conn, &library.id)
            .await?
            .into_iter()
            .filter(|link| {
                link_kind.is_none_or(|kind| link.target_kind == kind)
                    && resolved.is_none_or(|expected| link.resolved == expected)
                    && link_matches_folder(link)
                    && link_matches_tag(link)
            })
            .collect();
        let mut included_ids: HashSet<String> = HashSet::new();
        let has_edge_filter = link_kind.is_some() || resolved.is_some() || tag.is_some();

        if root.is_none() {
            if has_edge_filter {
                for link in &links {
                    if let Some(source) = document_by_id.get(link.src_doc_id.as_str())
                        && document_matches_folder(source)
                        && included_ids.insert(source.id.to_string())
                    {
                        add_node(source);
                    }
                    if let Some(target_id) = link.target_doc_id.as_deref()
                        && let Some(target) = document_by_id.get(target_id)
                        && document_matches_folder(target)
                        && included_ids.insert(target.id.to_string())
                    {
                        add_node(target);
                    }
                }
            } else {
                for entry in &documents {
                    if document_matches_folder(entry) {
                        included_ids.insert(entry.id.to_string());
                        add_node(entry);
                    }
                }
            }
        } else if let Some(root_path) = root.as_deref()
            && let Some(root_entry) = documents.iter().find(|entry| entry.path == root_path)
            && document_matches_folder(root_entry)
        {
            included_ids.insert(root_entry.id.to_string());
            add_node(root_entry);
            let mut queue = VecDeque::from([(root_entry.id.to_string(), 0u64)]);
            while let Some((document_id, distance)) = queue.pop_front() {
                if distance >= depth {
                    continue;
                }
                for link in &links {
                    let neighbor_id = if link.src_doc_id.as_str() == document_id {
                        link.target_doc_id.as_deref()
                    } else if link.target_doc_id.as_deref() == Some(document_id.as_str()) {
                        Some(link.src_doc_id.as_str())
                    } else {
                        None
                    };
                    let Some(neighbor_id) = neighbor_id else {
                        continue;
                    };
                    if included_ids.insert(neighbor_id.to_string())
                        && let Some(neighbor) = document_by_id.get(neighbor_id)
                    {
                        add_node(neighbor);
                        queue.push_back((neighbor_id.to_string(), distance + 1));
                    }
                }
            }
        }

        for link in links {
            if root.is_some() || folder.is_some() || has_edge_filter {
                let source_included = included_ids.contains(link.src_doc_id.as_str());
                let target_included = link
                    .target_doc_id
                    .as_deref()
                    .is_some_and(|target_id| included_ids.contains(target_id));
                if link.target_doc_id.is_some() {
                    if !source_included || !target_included {
                        continue;
                    }
                } else if !source_included {
                    continue;
                }
            }
            edges.push(GraphEdge {
                id: format!(
                    "{}:{}:{}",
                    link.src_doc_id, link.start_offset, link.end_offset
                ),
                source: link.src_doc_id,
                source_path: link.src_path,
                target: link.target_doc_id,
                target_path: link.target_path,
                target_kind: link.target_kind,
                target_text: link.target_text,
                resolved: link.resolved,
                resolution_status: link.resolution_status,
            });
        }

        let truncated = candidate_nodes > limit;
        let nodes = node_map.into_values().collect();
        Ok(GraphResponse {
            nodes,
            edges,
            truncated,
        })
    }

    pub async fn version_history(
        &self,
        library: &str,
        path: &str,
    ) -> Result<Vec<DocumentHistoryEntry>> {
        let raw = self.raw_version_history(library, path).await?;
        Ok(group_version_history(raw))
    }

    pub async fn raw_version_history(
        &self,
        library: &str,
        path: &str,
    ) -> Result<Vec<DocumentVersion>> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        self.raw_version_history_for_document_conn(&conn, &document_id)
            .await
    }

    async fn raw_version_history_for_document_conn(
        &self,
        conn: &Connection,
        document_id: &str,
    ) -> Result<Vec<DocumentVersion>> {
        let mut rows = conn
            .query(
                "SELECT v.id, v.document_id, v.tx_id, v.content_hash, v.inline_content, v.metadata_json,
                        v.content_type, v.byte_size, v.created_at, t.source, t.actor, t.message, t.provenance_json
                 FROM document_versions v
                 JOIN transactions t ON t.id = v.tx_id
                 WHERE v.document_id = ?1 ORDER BY v.created_at DESC, v.id DESC",
                params![document_id],
            )
            .await
            .map_err(map_turso_error)?;
        let mut versions = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            versions.push(version_from_row(&row)?);
        }
        Ok(versions)
    }

    pub async fn document_version(
        &self,
        library: &str,
        path: &str,
        version_id: &str,
    ) -> Result<DocumentVersionContent> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        let (version, content) = self
            .version_content_conn(&conn, &document_id, version_id)
            .await?;
        Ok(DocumentVersionContent {
            version,
            content: String::from_utf8_lossy(&content).into_owned(),
        })
    }

    pub async fn version_diff(
        &self,
        library: &str,
        path: &str,
        version_id: &str,
        against: Option<&str>,
    ) -> Result<VersionDiff> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library_record = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library_record.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        let (_, base_content) = self
            .version_content_conn(&conn, &document_id, version_id)
            .await?;
        let against_id = if let Some(against) = against {
            against.to_string()
        } else {
            self.document_entry_conn(&conn, &library_record.id, &path)
                .await?
                .head_version_id
                .to_string()
        };
        let (_, against_content) = self
            .version_content_conn(&conn, &document_id, &against_id)
            .await?;

        Ok(VersionDiff {
            base_version_id: version_id.into(),
            against_version_id: against_id.into(),
            unified_diff: unified_line_diff(
                &String::from_utf8_lossy(&base_content),
                &String::from_utf8_lossy(&against_content),
            ),
        })
    }

    /// Restores a version through the legacy byte path. RawDocuments only:
    /// the server routes BlockDocument restores through the reconciling
    /// gateway (`markdown_write::restore_block_document_version`).
    pub async fn restore_document_version_with_origin(
        &self,
        library: &str,
        path: &str,
        version_id: &str,
        origin_id: Option<String>,
        actor: Option<String>,
    ) -> Result<WriteOutcome> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library_record = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library_record.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        let (version, content) = self
            .version_content_conn(&conn, &document_id, version_id)
            .await?;
        drop(conn);

        self.put_document(PutDocumentRequest {
            library: library.to_string(),
            path,
            content,
            metadata: version.metadata,
            content_type: version.content_type,
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id,
            transaction: TransactionMetadata {
                actor,
                message: Some(format!("Restore version {version_id}")),
                provenance: Some(serde_json::json!({
                    "mode": "auto_commit",
                    "history": {"kind": "checkpoint", "reason": "restore"}
                })),
            },
        })
        .await
    }

    pub async fn delete_document(
        &self,
        library: &str,
        path: &str,
        source: DocumentSource,
    ) -> Result<TransactionRecord> {
        self.delete_document_with_origin(library, path, source, None, None)
            .await
    }

    pub async fn delete_document_with_origin(
        &self,
        library: &str,
        path: &str,
        source: DocumentSource,
        origin_id: Option<String>,
        actor: Option<String>,
    ) -> Result<TransactionRecord> {
        let path = normalize_path(path)?;
        let source_for_event = source.clone();
        let path_for_event = path.clone();
        let library = library.to_string();
        let (tx, doc_id) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let (doc_id, head_version_id) = store
                        .document_identity_conn(conn, &library.id, &path)
                        .await?
                        .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
                    let tx = insert_transaction_conn(
                        conn,
                        &library.id,
                        source,
                        actor,
                        None,
                        serde_json::json!({ "mode": "auto_commit" }),
                    )
                    .await?;
                    insert_change_conn(
                        conn,
                        &tx.id,
                        &path,
                        ChangeType::Delete,
                        head_version_id.as_deref(),
                        None,
                        None,
                    )
                    .await?;
                    conn.execute(
                        "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                        params![now_timestamp(), doc_id.clone()],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    blocks::clear_block_state_conn(conn, &doc_id).await?;
                    delete_path_inode_conn(conn, &library.id, &path).await?;
                    store.reindex_links_conn(conn, &library.id).await?;
                    commit_transaction_record_conn(conn, &tx.id).await?;
                    let tx = Self::transaction_conn(conn, &tx.id).await?;
                    Ok((tx, doc_id))
                })
            })
            .await?;
        self.emit_event(StoreEvent::document_delete(
            tx.library_id.clone(),
            path_for_event.clone(),
            source_for_event,
            tx.id.clone(),
            Some(doc_id),
            origin_id,
        ));
        self.emit_event(StoreEvent::links_indexed(
            tx.library_id.clone(),
            path_for_event,
        ));
        Ok(tx)
    }

    pub async fn move_document(
        &self,
        library: &str,
        from_path: &str,
        to_path: &str,
        source: DocumentSource,
    ) -> Result<TransactionRecord> {
        self.move_document_with_origin(library, from_path, to_path, source, None, None)
            .await
    }

    pub async fn move_document_with_origin(
        &self,
        library: &str,
        from_path: &str,
        to_path: &str,
        source: DocumentSource,
        origin_id: Option<String>,
        actor: Option<String>,
    ) -> Result<TransactionRecord> {
        let from_path = normalize_path(from_path)?;
        let to_path = normalize_path(to_path)?;
        let source_for_event = source.clone();
        let from_path_for_event = from_path.clone();
        let to_path_for_event = to_path.clone();
        let library = library.to_string();
        let (tx, doc_id) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let (doc_id, head_version_id) = store
                        .document_identity_conn(conn, &library.id, &from_path)
                        .await?
                        .ok_or_else(|| QuarryError::NotFound(from_path.clone()))?;
                    if store
                        .document_identity_conn(conn, &library.id, &to_path)
                        .await?
                        .is_some()
                    {
                        return Err(QuarryError::Conflict(format!("{to_path} already exists")));
                    }
                    let tx = insert_transaction_conn(
                        conn,
                        &library.id,
                        source,
                        actor,
                        None,
                        serde_json::json!({ "mode": "auto_commit" }),
                    )
                    .await?;
                    insert_change_conn(
                        conn,
                        &tx.id,
                        &from_path,
                        ChangeType::Move,
                        head_version_id.as_deref(),
                        head_version_id.as_deref(),
                        Some(&to_path),
                    )
                    .await?;
                    conn.execute(
                        "UPDATE documents SET path = ?1, updated_at = ?2 WHERE id = ?3",
                        params![to_path.clone(), now_timestamp(), doc_id.clone()],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    move_path_inode_conn(conn, &library.id, &from_path, &to_path).await?;
                    store.reindex_links_conn(conn, &library.id).await?;
                    commit_transaction_record_conn(conn, &tx.id).await?;
                    let tx = Self::transaction_conn(conn, &tx.id).await?;
                    Ok((tx, doc_id))
                })
            })
            .await?;
        self.emit_event(StoreEvent::document_move(
            tx.library_id.clone(),
            from_path_for_event.clone(),
            to_path_for_event.clone(),
            source_for_event,
            tx.id.clone(),
            Some(doc_id),
            origin_id,
        ));
        self.emit_event(StoreEvent::links_indexed(
            tx.library_id.clone(),
            to_path_for_event,
        ));
        Ok(tx)
    }

    pub async fn replace_document(
        &self,
        library: &str,
        from_path: &str,
        to_path: &str,
        source: DocumentSource,
    ) -> Result<TransactionRecord> {
        let from_path = normalize_path(from_path)?;
        let to_path = normalize_path(to_path)?;
        if from_path == to_path {
            return Err(QuarryError::Conflict(
                "cannot replace a document with itself".to_string(),
            ));
        }
        let source_for_event = source.clone();
        let from_path_for_event = from_path.clone();
        let to_path_for_event = to_path.clone();
        let library = library.to_string();
        let (tx, doc_id) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let from_document = store.document_conn(conn, &library.id, &from_path).await?;
                    let (to_doc_id, old_to_version_id) = store
                        .document_identity_conn(conn, &library.id, &to_path)
                        .await?
                        .ok_or_else(|| QuarryError::NotFound(to_path.clone()))?;
                    let tx = insert_transaction_conn(
                        conn,
                        &library.id,
                        source,
                        None,
                        None,
                        serde_json::json!({ "mode": "auto_commit", "replace": true }),
                    )
                    .await?;
                    insert_change_conn(
                        conn,
                        &tx.id,
                        &to_path,
                        ChangeType::Delete,
                        old_to_version_id.as_deref(),
                        None,
                        None,
                    )
                    .await?;
                    conn.execute(
                        "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                        params![now_timestamp(), to_doc_id],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    insert_change_conn(
                        conn,
                        &tx.id,
                        &from_path,
                        ChangeType::Move,
                        Some(&from_document.version.id),
                        Some(&from_document.version.id),
                        Some(&to_path),
                    )
                    .await?;
                    conn.execute(
                        "UPDATE documents SET path = ?1, updated_at = ?2 WHERE id = ?3",
                        params![to_path.clone(), now_timestamp(), from_document.id.clone()],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    move_path_inode_conn(conn, &library.id, &from_path, &to_path).await?;
                    store.reindex_links_conn(conn, &library.id).await?;
                    commit_transaction_record_conn(conn, &tx.id).await?;
                    let tx = Self::transaction_conn(conn, &tx.id).await?;
                    Ok((tx, from_document.id))
                })
            })
            .await?;
        self.emit_event(StoreEvent::document_move(
            tx.library_id.clone(),
            from_path_for_event.clone(),
            to_path_for_event.clone(),
            source_for_event,
            tx.id.clone(),
            Some(doc_id.to_string()),
            None,
        ));
        self.emit_event(StoreEvent::links_indexed(
            tx.library_id.clone(),
            to_path_for_event,
        ));
        Ok(tx)
    }

    pub async fn patch_metadata(
        &self,
        library: &str,
        path: &str,
        patch: JsonValue,
        source: DocumentSource,
        precondition: WritePrecondition,
    ) -> Result<WriteOutcome> {
        self.run_normal_write(async {
            let current = self.get_document(library, path).await?;
            let mut metadata = current.metadata;
            merge_json(&mut metadata, patch);
            self.put_document(PutDocumentRequest {
                library: library.to_string(),
                path: path.to_string(),
                content: current.content,
                metadata,
                content_type: current.version.content_type,
                source,
                precondition,
                origin_id: None,
                transaction: TransactionMetadata::default(),
            })
            .await
        })
        .await
    }

    pub async fn begin_transaction(
        &self,
        library: &str,
        source: DocumentSource,
        actor: Option<String>,
        message: Option<String>,
        provenance: JsonValue,
    ) -> Result<TransactionRecord> {
        let library = library.to_string();
        let tx = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    insert_transaction_conn(conn, &library.id, source, actor, message, provenance)
                        .await
                })
            })
            .await?;
        tracing::debug!(
            event = "storage.transaction.begin",
            library_id = %tx.library_id,
            tx_id = %tx.id,
            source = tx.source.as_str(),
            "storage transaction began"
        );
        Ok(tx)
    }

    pub async fn list_transactions(&self, library: &str) -> Result<Vec<TransactionRecord>> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let mut rows = conn
            .query(
                "SELECT id, library_id, state, actor, source, message, provenance_json, created_at, committed_at
                 FROM transactions WHERE library_id = ?1 ORDER BY created_at, id",
                params![library.id],
            )
            .await
            .map_err(map_turso_error)?;
        let mut transactions = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            transactions.push(transaction_from_row(&row)?);
        }
        Ok(transactions)
    }

    pub async fn get_transaction(&self, tx_id: &str) -> Result<TransactionRecord> {
        let conn = self.conn()?;
        Self::transaction_conn(&conn, tx_id).await
    }

    pub async fn stage_put(
        &self,
        tx_id: &str,
        path: &str,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
    ) -> Result<DocumentVersion> {
        let path = normalize_path(path)?;
        let tx_id = tx_id.to_string();
        let content_type = content_type.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let tx = Self::transaction_conn(conn, &tx_id).await?;
                ensure_open(&tx)?;
                let (doc_id, old_version_id) =
                    ensure_document_conn(conn, &tx.library_id, &path, &now_timestamp()).await?;
                delete_staged_change_conn(conn, &tx_id, &path).await?;
                let version = store
                    .insert_version_conn(conn, &doc_id, &tx_id, content, metadata, &content_type)
                    .await?;
                insert_change_conn(
                    conn,
                    &tx_id,
                    &path,
                    ChangeType::Put,
                    old_version_id.as_deref(),
                    Some(&version.id),
                    None,
                )
                .await?;
                Ok(version)
            })
        })
        .await
    }

    pub async fn stage_delete(&self, tx_id: &str, path: &str) -> Result<()> {
        let path = normalize_path(path)?;
        let tx_id = tx_id.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let tx = Self::transaction_conn(conn, &tx_id).await?;
                ensure_open(&tx)?;
                let (_, old_version_id) = store
                    .document_identity_conn(conn, &tx.library_id, &path)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
                delete_staged_change_conn(conn, &tx_id, &path).await?;
                insert_change_conn(
                    conn,
                    &tx_id,
                    &path,
                    ChangeType::Delete,
                    old_version_id.as_deref(),
                    None,
                    None,
                )
                .await
            })
        })
        .await
    }

    pub async fn stage_metadata(
        &self,
        tx_id: &str,
        path: &str,
        patch: JsonValue,
    ) -> Result<DocumentVersion> {
        let path = normalize_path(path)?;
        let tx_id = tx_id.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let tx = Self::transaction_conn(conn, &tx_id).await?;
                ensure_open(&tx)?;
                let current = store.document_conn(conn, &tx.library_id, &path).await?;
                let mut metadata = current.metadata;
                merge_json(&mut metadata, patch);
                delete_staged_change_conn(conn, &tx_id, &path).await?;
                let version = store
                    .insert_version_conn(
                        conn,
                        &current.id,
                        &tx_id,
                        current.content,
                        metadata,
                        &current.version.content_type,
                    )
                    .await?;
                insert_change_conn(
                    conn,
                    &tx_id,
                    &path,
                    ChangeType::Metadata,
                    Some(&current.version.id),
                    Some(&version.id),
                    None,
                )
                .await?;
                Ok(version)
            })
        })
        .await
    }

    pub async fn stage_move(&self, tx_id: &str, from_path: &str, to_path: &str) -> Result<()> {
        let from_path = normalize_path(from_path)?;
        let to_path = normalize_path(to_path)?;
        let tx_id = tx_id.to_string();
        self.write_transaction(move |store, conn| {
            Box::pin(async move {
                let tx = Self::transaction_conn(conn, &tx_id).await?;
                ensure_open(&tx)?;
                let (_, old_version_id) = store
                    .document_identity_conn(conn, &tx.library_id, &from_path)
                    .await?
                    .ok_or_else(|| QuarryError::NotFound(from_path.clone()))?;
                if store
                    .document_identity_conn(conn, &tx.library_id, &to_path)
                    .await?
                    .is_some()
                {
                    return Err(QuarryError::Conflict(format!("{to_path} already exists")));
                }
                insert_change_conn(
                    conn,
                    &tx_id,
                    &from_path,
                    ChangeType::Move,
                    old_version_id.as_deref(),
                    old_version_id.as_deref(),
                    Some(&to_path),
                )
                .await
            })
        })
        .await
    }

    pub async fn commit_transaction(&self, tx_id: &str) -> Result<TransactionRecord> {
        let started = Instant::now();
        tracing::debug!(
            event = "storage.transaction.commit.started",
            tx_id,
            "storage transaction commit started"
        );
        let tx_id = tx_id.to_string();
        let (tx, events) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let tx = Self::transaction_conn(conn, &tx_id).await?;
                    ensure_open(&tx)?;
                    let mut events = Vec::new();
                    let mut changes = Vec::new();
                    let mut rows = conn
                        .query(
                            "SELECT path, change_type, old_version_id, new_version_id, new_path
                             FROM transaction_changes
                             WHERE tx_id = ?1 ORDER BY rowid",
                            params![tx_id.clone()],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
                        changes.push(StagedChange {
                            path: text(&row, 0)?,
                            change_type: parse_storage_enum(&text(&row, 1)?)?,
                            old_version_id: opt_text(&row, 2)?,
                            new_version_id: opt_text(&row, 3)?,
                            new_path: opt_text(&row, 4)?,
                        });
                    }
                    for change in &changes {
                        match &change.change_type {
                            ChangeType::Put | ChangeType::Metadata => {
                                store
                                    .ensure_staged_head_unchanged_conn(
                                        conn,
                                        &tx.library_id,
                                        &change.path,
                                        change.old_version_id.as_deref(),
                                    )
                                    .await?;
                            }
                            ChangeType::Delete => {
                                store
                                    .ensure_staged_head_unchanged_conn(
                                        conn,
                                        &tx.library_id,
                                        &change.path,
                                        change.old_version_id.as_deref(),
                                    )
                                    .await?;
                            }
                            ChangeType::Move => {
                                let new_path = change.new_path.as_deref().ok_or_else(|| {
                                    QuarryError::Invariant("move change missing new path".to_string())
                                })?;
                                store
                                    .ensure_staged_head_unchanged_conn(
                                        conn,
                                        &tx.library_id,
                                        &change.path,
                                        change.old_version_id.as_deref(),
                                    )
                                    .await?;
                                store
                                    .ensure_move_target_available_conn(
                                        conn,
                                        &tx.library_id,
                                        new_path,
                                    )
                                    .await?;
                            }
                        }
                    }
                    for change in changes {
                        match change.change_type {
                            ChangeType::Put | ChangeType::Metadata => {
                                let version_id = change.new_version_id.ok_or_else(|| {
                                    QuarryError::Invariant(
                                        "put change missing new version".to_string(),
                                    )
                                })?;
                                let doc_id =
                                    Self::document_id_for_version_conn(conn, &version_id).await?;
                                publish_put_conn(conn, &doc_id, &version_id).await?;
                                blocks::clear_block_state_conn(conn, &doc_id).await?;
                                ensure_path_inodes_conn(conn, &tx.library_id, &change.path)
                                    .await?;
                                events.push(StoreEvent::document_put(
                                    tx.library_id.clone(),
                                    change.path.clone(),
                                    tx.source.clone(),
                                    tx.id.clone(),
                                    doc_id,
                                    version_id,
                                    None,
                                ));
                                events.push(StoreEvent::links_indexed(
                                    tx.library_id.clone(),
                                    change.path.clone(),
                                ));
                            }
                            ChangeType::Delete => {
                                if let Some((doc_id, _)) = store
                                    .document_identity_conn(conn, &tx.library_id, &change.path)
                                    .await?
                                {
                                    conn.execute(
                                        "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                                        params![now_timestamp(), doc_id.clone()],
                                    )
                                    .await
                                    .map_err(map_turso_error)?;
                                    blocks::clear_block_state_conn(conn, &doc_id).await?;
                                    delete_path_inode_conn(conn, &tx.library_id, &change.path)
                                        .await?;
                                }
                                events.push(StoreEvent::document_delete(
                                    tx.library_id.clone(),
                                    change.path.clone(),
                                    tx.source.clone(),
                                    tx.id.clone(),
                                    None,
                                    None,
                                ));
                                events.push(StoreEvent::links_indexed(
                                    tx.library_id.clone(),
                                    change.path.clone(),
                                ));
                            }
                            ChangeType::Move => {
                                let new_path = change.new_path.ok_or_else(|| {
                                    QuarryError::Invariant("move change missing new path".to_string())
                                })?;
                                let (doc_id, _) = store
                                    .document_identity_conn(conn, &tx.library_id, &change.path)
                                    .await?
                                    .ok_or_else(|| QuarryError::NotFound(change.path.clone()))?;
                                conn.execute(
                                    "UPDATE documents SET path = ?1, updated_at = ?2 WHERE id = ?3",
                                    params![new_path.clone(), now_timestamp(), doc_id],
                                )
                                .await
                                .map_err(map_turso_error)?;
                                move_path_inode_conn(
                                    conn,
                                    &tx.library_id,
                                    &change.path,
                                    &new_path,
                                )
                                .await?;
                                events.push(StoreEvent::document_move(
                                    tx.library_id.clone(),
                                    change.path.clone(),
                                    new_path.clone(),
                                    tx.source.clone(),
                                    tx.id.clone(),
                                    None,
                                    None,
                                ));
                                events
                                    .push(StoreEvent::links_indexed(tx.library_id.clone(), new_path));
                            }
                        }
                    }
                    store.reindex_links_conn(conn, &tx.library_id).await?;
                    commit_transaction_record_conn(conn, &tx_id).await?;
                    let tx = Self::transaction_conn(conn, &tx_id).await?;
                    Ok((tx, events))
                })
            })
            .await?;
        for event in events {
            self.emit_event(event);
        }
        tracing::debug!(
            event = "storage.transaction.commit.completed",
            library_id = %tx.library_id,
            tx_id = %tx.id,
            source = tx.source.as_str(),
            duration_ms = started.elapsed().as_millis() as u64,
            "storage transaction commit completed"
        );
        Ok(tx)
    }

    pub async fn rollback_transaction(&self, tx_id: &str) -> Result<TransactionRecord> {
        let started = Instant::now();
        let tx_id = tx_id.to_string();
        let tx = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    let tx = Self::transaction_conn(conn, &tx_id).await?;
                    ensure_open(&tx)?;
                    conn.execute(
                        "UPDATE transactions SET state = ?1 WHERE id = ?2",
                        params![TransactionState::RolledBack.as_str(), tx_id.clone()],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    Self::transaction_conn(conn, &tx_id).await
                })
            })
            .await?;
        tracing::debug!(
            event = "storage.transaction.rollback",
            library_id = %tx.library_id,
            tx_id = %tx.id,
            source = tx.source.as_str(),
            duration_ms = started.elapsed().as_millis() as u64,
            "storage transaction rolled back"
        );
        Ok(tx)
    }

    pub async fn create_git_peer(&self, library: &str, config: JsonValue) -> Result<GitPeer> {
        let library = library.to_string();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
                let library = Self::require_library_conn(conn, &library).await?;
                let peer = GitPeer {
                    id: Uuid::new_v4().to_string(),
                    library_id: library.id,
                    kind: "git".to_string(),
                    config,
                };
                conn.execute(
                    "INSERT INTO sync_peers (id, library_id, kind, config_json) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        peer.id.clone(),
                        peer.library_id.clone(),
                        peer.kind.clone(),
                        peer.config.to_string()
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                Ok(peer)
            })
        })
        .await
    }

    pub async fn list_git_peers(&self, library: &str) -> Result<Vec<GitPeer>> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let mut rows = conn
            .query(
                "SELECT id, library_id, kind, config_json FROM sync_peers
                 WHERE library_id = ?1 AND kind = 'git' ORDER BY id",
                params![library.id],
            )
            .await
            .map_err(map_turso_error)?;
        let mut peers = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            peers.push(GitPeer {
                id: text(&row, 0)?,
                library_id: text(&row, 1)?,
                kind: text(&row, 2)?,
                config: serde_json::from_str(&text(&row, 3)?)?,
            });
        }
        Ok(peers)
    }

    pub async fn sync_state(&self, peer_id: &str, path: &str) -> Result<Option<SyncStateEntry>> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT peer_id, path, last_synced_doc_version_id, last_synced_git_oid
                 FROM sync_state WHERE peer_id = ?1 AND path = ?2 LIMIT 1",
                params![peer_id.to_string(), path],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            Ok(Some(sync_state_from_row(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_sync_state(&self, peer_id: &str) -> Result<Vec<SyncStateEntry>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT peer_id, path, last_synced_doc_version_id, last_synced_git_oid
                 FROM sync_state WHERE peer_id = ?1 ORDER BY path",
                params![peer_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            entries.push(sync_state_from_row(&row)?);
        }
        Ok(entries)
    }

    pub async fn upsert_sync_state(
        &self,
        peer_id: &str,
        path: &str,
        last_synced_doc_version_id: Option<String>,
        last_synced_git_oid: Option<String>,
    ) -> Result<SyncStateEntry> {
        let path = normalize_path(path)?;
        let peer_id = peer_id.to_string();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
                conn.execute(
                    "INSERT INTO sync_state
                     (peer_id, path, last_synced_doc_version_id, last_synced_git_oid)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(peer_id, path)
                     DO UPDATE SET
                        last_synced_doc_version_id = excluded.last_synced_doc_version_id,
                        last_synced_git_oid = excluded.last_synced_git_oid",
                    vec![
                        Value::Text(peer_id.clone()),
                        Value::Text(path.clone()),
                        opt_value(last_synced_doc_version_id.clone()),
                        opt_value(last_synced_git_oid.clone()),
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                Ok(SyncStateEntry {
                    peer_id,
                    path,
                    last_synced_doc_version_id: last_synced_doc_version_id.map(Into::into),
                    last_synced_git_oid,
                })
            })
        })
        .await
    }

    pub async fn list_conflicts(&self, library: &str) -> Result<Vec<ConflictRecord>> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let mut rows = conn
            .query(
                "SELECT c.id, c.library_id, c.path, c.ours_version_id, c.theirs_version_id,
                        c.status, c.discovered_at, c.resolved_at,
                        CASE WHEN d.path IS NOT NULL AND d.path <> c.path THEN d.path ELSE NULL END
                 FROM conflicts c
                 LEFT JOIN document_versions tv ON tv.id = c.theirs_version_id
                 LEFT JOIN documents d ON d.id = tv.document_id AND d.library_id = c.library_id
                 WHERE c.library_id = ?1
                 ORDER BY c.discovered_at DESC",
                params![library.id],
            )
            .await
            .map_err(map_turso_error)?;
        let mut conflicts = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            conflicts.push(conflict_from_row(&row)?);
        }
        Ok(conflicts)
    }

    pub async fn get_conflict(&self, conflict_id: &str) -> Result<ConflictRecord> {
        let conn = self.conn()?;
        Self::conflict_conn(&conn, conflict_id).await
    }

    pub async fn record_conflict(
        &self,
        library: &str,
        path: &str,
        ours_version_id: Option<String>,
        theirs_version_id: Option<String>,
    ) -> Result<ConflictRecord> {
        let path = normalize_path(path)?;
        let library = library.to_string();
        let conflict = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let conflict = ConflictRecord {
                        id: Uuid::new_v4().to_string(),
                        library_id: library.id,
                        path,
                        conflict_path: None,
                        ours_version_id: ours_version_id.map(Into::into),
                        theirs_version_id: theirs_version_id.map(Into::into),
                        status: ConflictStatus::Open,
                        discovered_at: now_timestamp().into(),
                        resolved_at: None,
                    };
                    conn.execute(
                        "INSERT INTO conflicts
                         (id, library_id, path, ours_version_id, theirs_version_id, status, discovered_at, resolved_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
                        vec![
                            Value::Text(conflict.id.clone()),
                            Value::Text(conflict.library_id.clone()),
                            Value::Text(conflict.path.clone()),
                            opt_value(conflict.ours_version_id.clone()),
                            opt_value(conflict.theirs_version_id.clone()),
                            Value::Text(conflict.status.as_str().to_string()),
                            Value::Text(conflict.discovered_at.to_string()),
                        ],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    Self::conflict_conn(conn, &conflict.id).await
                })
            })
            .await?;
        self.emit_event(StoreEvent::conflict_created(
            conflict.library_id.clone(),
            conflict.path.clone(),
            conflict.id.clone(),
        ));
        Ok(conflict)
    }

    pub async fn resolve_conflict(&self, conflict_id: &str) -> Result<ConflictRecord> {
        let conflict_id = conflict_id.to_string();
        let conflict = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    conn.execute(
                        "UPDATE conflicts SET status = ?1, resolved_at = ?2 WHERE id = ?3",
                        params![
                            ConflictStatus::Resolved.as_str(),
                            now_timestamp(),
                            conflict_id.clone()
                        ],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    Self::conflict_conn(conn, &conflict_id).await
                })
            })
            .await?;
        self.emit_event(StoreEvent::conflict_resolved(
            conflict.library_id.clone(),
            conflict.path.clone(),
            conflict.id.clone(),
        ));
        Ok(conflict)
    }

    pub async fn gc(&self) -> Result<GcReport> {
        self.run_global_operation(async { self.gc_inner().await })
            .await
    }

    async fn gc_inner(&self) -> Result<GcReport> {
        let _guard = self.acquire_write_lock().await;
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT dv.content_hash
                 FROM document_versions dv
                 JOIN documents d ON d.id = dv.document_id
                 JOIN transactions t ON t.id = dv.tx_id
                 WHERE dv.content_hash IS NOT NULL
                   AND t.state IN ('open', 'committed')
                   AND (
                     t.state = 'open'
                     OR (
                       d.deleted_at IS NULL
                       AND d.head_version_id IS NOT NULL
                       AND (d.expires_at IS NULL OR d.expires_at > ?1)
                     )
                   )",
                params![now_timestamp()],
            )
            .await
            .map_err(map_turso_error)?;
        let mut reachable = HashSet::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            if let Some(hash) = opt_text(&row, 0)? {
                reachable.insert(hash);
            }
        }
        let report = self.cas.gc(reachable)?;
        tracing::info!(
            event = "storage.gc.completed",
            reachable_blobs = report.reachable,
            removed_blobs = report.removed,
            "storage GC completed"
        );
        Ok(report)
    }

    async fn migrate(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(SCHEMA).await.map_err(map_turso_error)?;
        migrate_documents_scope_ttl(&conn).await?;
        ensure_document_indexes_conn(&conn).await?;
        ensure_links_resolution_status_column(&conn).await?;
        // Sessions are discardable (recovery is reseed-from-rows); the legacy
        // CRDT recovery-state table is dropped wholesale.
        conn.execute("DROP TABLE IF EXISTS collab_recovery_states", ())
            .await
            .map_err(map_turso_error)?;
        Ok(())
    }

    fn conn(&self) -> Result<Connection> {
        self.db.connect().map_err(map_turso_error)
    }

    async fn library_by_slug_or_id_conn(
        conn: &Connection,
        slug_or_id: &str,
    ) -> Result<Option<Library>> {
        let mut rows = conn
            .query(
                "SELECT id, slug, created_at, settings_json FROM libraries WHERE slug = ?1 OR id = ?1 LIMIT 1",
                params![slug_or_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            Ok(Some(library_from_row(&row)?))
        } else {
            Ok(None)
        }
    }

    async fn require_library_conn(conn: &Connection, slug_or_id: &str) -> Result<Library> {
        Self::library_by_slug_or_id_conn(conn, slug_or_id)
            .await?
            .ok_or_else(|| QuarryError::NotFound(format!("library {slug_or_id}")))
    }

    async fn check_precondition_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
        precondition: &WritePrecondition,
    ) -> Result<()> {
        let current = self.document_identity_conn(conn, library_id, path).await?;
        match precondition {
            WritePrecondition::None => {
                tracing::debug!(
                    event = "storage.precondition.checked",
                    library_id,
                    path,
                    precondition = "none",
                    outcome = "accepted",
                    "storage precondition checked"
                );
                Ok(())
            }
            WritePrecondition::IfNoneMatch => {
                if current.is_some() {
                    tracing::debug!(
                        event = "storage.precondition.rejected",
                        library_id,
                        path,
                        precondition = "if_none_match",
                        outcome = "rejected",
                        reason_code = "document_exists",
                        reason = "If-None-Match requires the document to be absent",
                        "storage precondition rejected"
                    );
                    Err(QuarryError::PreconditionFailed(format!("{path} exists")))
                } else {
                    tracing::debug!(
                        event = "storage.precondition.checked",
                        library_id,
                        path,
                        precondition = "if_none_match",
                        outcome = "accepted",
                        "storage precondition checked"
                    );
                    Ok(())
                }
            }
            WritePrecondition::IfMatch(expected) => {
                let actual = match current.and_then(|(_, version)| version) {
                    Some(actual) => actual,
                    None => {
                        tracing::debug!(
                            event = "storage.precondition.rejected",
                            library_id,
                            path,
                            precondition = "if_match",
                            outcome = "rejected",
                            expected_version_id = %expected,
                            reason_code = "document_missing",
                            reason = "If-Match requires an existing document head",
                            "storage precondition rejected"
                        );
                        return Err(QuarryError::PreconditionFailed(format!("{path} missing")));
                    }
                };
                if &actual == expected {
                    tracing::debug!(
                        event = "storage.precondition.checked",
                        library_id,
                        path,
                        precondition = "if_match",
                        outcome = "accepted",
                        expected_version_id = %expected,
                        version_id = %actual,
                        "storage precondition checked"
                    );
                    Ok(())
                } else {
                    tracing::debug!(
                        event = "storage.precondition.rejected",
                        library_id,
                        path,
                        precondition = "if_match",
                        outcome = "rejected",
                        expected_version_id = %expected,
                        version_id = %actual,
                        reason_code = "version_mismatch",
                        reason = "If-Match did not match the current document head",
                        "storage precondition rejected"
                    );
                    Err(QuarryError::PreconditionFailed(format!(
                        "{path} head is {actual}, expected {expected}"
                    )))
                }
            }
        }
    }

    async fn check_tmp_precondition_conn(
        &self,
        conn: &Connection,
        path: &str,
        precondition: &WritePrecondition,
    ) -> Result<()> {
        let current = self.tmp_document_identity_conn(conn, path).await?;
        match precondition {
            WritePrecondition::None => Ok(()),
            WritePrecondition::IfNoneMatch => {
                if current.is_some() {
                    Err(QuarryError::PreconditionFailed(format!("{path} exists")))
                } else {
                    Ok(())
                }
            }
            WritePrecondition::IfMatch(expected) => {
                let actual = current
                    .and_then(|(_, version)| version)
                    .ok_or_else(|| QuarryError::PreconditionFailed(format!("{path} missing")))?;
                if &actual == expected {
                    Ok(())
                } else {
                    Err(QuarryError::PreconditionFailed(format!(
                        "{path} head is {actual}, expected {expected}"
                    )))
                }
            }
        }
    }

    async fn ensure_staged_head_unchanged_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
        expected_version_id: Option<&str>,
    ) -> Result<()> {
        let actual_version_id = self
            .document_identity_conn(conn, library_id, path)
            .await?
            .and_then(|(_, version_id)| version_id);
        if actual_version_id.as_deref() == expected_version_id {
            return Ok(());
        }

        Err(QuarryError::PreconditionFailed(format!(
            "{path} changed since transaction was staged; current head is {}, expected {}",
            actual_version_id.as_deref().unwrap_or("<missing>"),
            expected_version_id.unwrap_or("<missing>")
        )))
    }

    async fn ensure_move_target_available_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<()> {
        if let Some((_, version_id)) = self.document_identity_conn(conn, library_id, path).await? {
            return Err(QuarryError::PreconditionFailed(format!(
                "{path} appeared since transaction was staged with head {}",
                version_id.unwrap_or_else(|| "<unknown>".to_string())
            )));
        }
        Ok(())
    }

    async fn directory_metadata_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<DirectoryMetadata> {
        if path.is_empty() {
            let inode = ensure_inode_conn(conn, library_id, "").await?;
            return Ok(DirectoryMetadata {
                path: String::new(),
                mode: None,
                mtime: now_timestamp(),
                inode,
            });
        }
        let mut rows = conn
            .query(
                "SELECT dm.path, dm.mode, dm.mtime, i.inode
                 FROM dir_metadata dm
                 JOIN inodes i ON i.library_id = dm.library_id AND i.path = dm.path
                 WHERE dm.library_id = ?1 AND dm.path = ?2 LIMIT 1",
                params![library_id.to_string(), path.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            directory_metadata_from_row(&row)
        } else {
            Err(QuarryError::NotFound(path.to_string()))
        }
    }

    async fn document_id_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<Option<String>> {
        self.scoped_document_identity_conn(conn, DocumentLookupScope::Library { library_id }, path)
            .await
            .map(|identity| identity.map(|(id, _)| id))
    }

    async fn document_identity_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        self.scoped_document_identity_conn(conn, DocumentLookupScope::Library { library_id }, path)
            .await
    }

    async fn scoped_document_identity_conn(
        &self,
        conn: &Connection,
        scope: DocumentLookupScope<'_>,
        path: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        let now = now_timestamp();
        let (scope_filter, binds) = match scope {
            DocumentLookupScope::Library { library_id } => (
                "document_scope = 'library'
                   AND library_id = ?1
                   AND path = ?2
                   AND (expires_at IS NULL OR expires_at > ?3)",
                vec![
                    Value::Text(library_id.to_string()),
                    Value::Text(path.to_string()),
                    Value::Text(now.clone()),
                ],
            ),
            DocumentLookupScope::Tmp => (
                "document_scope = 'tmp'
                   AND library_id IS NULL
                   AND path = ?1
                   AND expires_at > ?2",
                vec![Value::Text(path.to_string()), Value::Text(now.clone())],
            ),
        };
        let sql = format!(
            "SELECT id, head_version_id FROM documents
             WHERE {scope_filter}
               AND deleted_at IS NULL
               AND head_version_id IS NOT NULL
             LIMIT 1"
        );
        let mut rows = conn.query(&sql, binds).await.map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            return Ok(Some((text(&row, 0)?, opt_text(&row, 1)?)));
        }
        match scope {
            DocumentLookupScope::Library { library_id } => {
                self.error_if_library_document_expired_conn(conn, library_id, path, &now)
                    .await?;
            }
            DocumentLookupScope::Tmp => {
                self.error_if_tmp_document_expired_conn(conn, path, &now)
                    .await?;
            }
        }
        Ok(None)
    }

    async fn library_document_id_any_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<Option<String>> {
        let mut rows = conn
            .query(
                "SELECT id FROM documents
                 WHERE document_scope = 'library'
                   AND library_id = ?1
                   AND path = ?2
                   AND deleted_at IS NULL
                   AND head_version_id IS NOT NULL
                 LIMIT 1",
                params![library_id.to_string(), path.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| text(&row, 0))
            .transpose()
    }

    async fn tmp_document_id_conn(&self, conn: &Connection, path: &str) -> Result<Option<String>> {
        self.scoped_document_identity_conn(conn, DocumentLookupScope::Tmp, path)
            .await
            .map(|identity| identity.map(|(id, _)| id))
    }

    async fn tmp_document_identity_conn(
        &self,
        conn: &Connection,
        path: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        self.scoped_document_identity_conn(conn, DocumentLookupScope::Tmp, path)
            .await
    }

    async fn tmp_document_expires_at_conn(
        &self,
        conn: &Connection,
        path: &str,
    ) -> Result<Option<String>> {
        let mut rows = conn
            .query(
                "SELECT expires_at FROM documents
                 WHERE document_scope = 'tmp'
                   AND library_id IS NULL
                   AND path = ?1
                   AND deleted_at IS NULL
                   AND head_version_id IS NOT NULL
                   AND expires_at > ?2
                 LIMIT 1",
                params![path.to_string(), now_timestamp()],
            )
            .await
            .map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| opt_text(&row, 0))
            .transpose()
            .map(Option::flatten)
    }

    async fn error_if_library_document_expired_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
        now: &str,
    ) -> Result<()> {
        error_if_document_expired_conn(conn, DocumentLookupScope::Library { library_id }, path, now)
            .await
    }

    async fn error_if_tmp_document_expired_conn(
        &self,
        conn: &Connection,
        path: &str,
        now: &str,
    ) -> Result<()> {
        error_if_document_expired_conn(conn, DocumentLookupScope::Tmp, path, now).await
    }

    async fn collab_document_seed_conn(
        &self,
        conn: &Connection,
        document_id: &str,
    ) -> Result<Option<CollabDocumentSeed>> {
        let mut rows = conn
            .query(
                "SELECT d.id, v.id, v.content_type, v.content_hash, v.inline_content
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.id = ?1
                   AND d.document_scope = 'library'
                   AND d.deleted_at IS NULL
                   AND d.head_version_id IS NOT NULL
                   AND (d.expires_at IS NULL OR d.expires_at > ?2)
                 LIMIT 1",
                params![document_id.to_string(), now_timestamp()],
            )
            .await
            .map_err(map_turso_error)?;
        let Some(row) = rows.next().await.map_err(map_turso_error)? else {
            return Ok(None);
        };
        let content_hash = opt_text(&row, 3)?;
        let inline_content = opt_blob(&row, 4)?;
        let content = match (inline_content, content_hash) {
            (Some(bytes), None) => bytes,
            (None, Some(hash)) => self.cas.read(&hash)?,
            _ => {
                return Err(QuarryError::Invariant(format!(
                    "head version for document {document_id} violates inline/CAS invariant"
                )));
            }
        };
        Ok(Some(CollabDocumentSeed {
            document_id: text(&row, 0)?,
            head_version_id: text(&row, 1)?,
            content_type: text(&row, 2)?,
            content,
        }))
    }

    async fn collab_invite_token_conn(
        &self,
        conn: &Connection,
        token_id: &str,
    ) -> Result<Option<CollabInviteToken>> {
        let mut rows = conn
            .query(
                "SELECT id, document_id, role, by_hint, created_at, revoked_at
                 FROM collab_invite_tokens WHERE id = ?1 LIMIT 1",
                params![token_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| collab_invite_token_from_row(&row))
            .transpose()
    }

    async fn collab_invite_tokens_for_document_conn(
        &self,
        conn: &Connection,
        document_id: &str,
    ) -> Result<Vec<CollabInviteToken>> {
        let mut rows = conn
            .query(
                "SELECT id, document_id, role, by_hint, created_at, revoked_at
                 FROM collab_invite_tokens
                 WHERE document_id = ?1
                 ORDER BY created_at, id",
                params![document_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        let mut tokens = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            tokens.push(collab_invite_token_from_row(&row)?);
        }
        Ok(tokens)
    }

    async fn insert_version_conn(
        &self,
        conn: &Connection,
        document_id: &str,
        tx_id: &str,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
    ) -> Result<DocumentVersion> {
        let id = Uuid::new_v4().to_string();
        let created_at = now_timestamp();
        let byte_size = content.len() as u64;
        let metadata = merge_markdown_frontmatter_metadata(&content, metadata, content_type)?;
        let (content_hash, inline_content) = if content.len() <= INLINE_CONTENT_THRESHOLD {
            (None, Some(content))
        } else {
            let blob = self.cas.put(&content)?;
            conn.execute(
                "INSERT INTO blobs (hash, hash_alg, byte_size, storage_backend, created_at)
                 VALUES (?1, 'blake3', ?2, 'disk', ?3)
                 ON CONFLICT(hash) DO NOTHING",
                params![blob.hash.clone(), blob.byte_size as i64, created_at.clone()],
            )
            .await
            .map_err(map_turso_error)?;
            (Some(blob.hash), None)
        };
        let version = DocumentVersion {
            id: id.into(),
            document_id: document_id.into(),
            tx_id: tx_id.to_string(),
            transaction_source: None,
            transaction_actor: None,
            transaction_message: None,
            transaction_provenance: None,
            content_hash,
            inline_content,
            metadata,
            content_type: content_type.to_string(),
            byte_size,
            created_at: created_at.into(),
        };
        conn.execute(
            "INSERT INTO document_versions
             (id, document_id, tx_id, content_hash, inline_content, metadata_json, content_type, byte_size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            vec![
                Value::Text(version.id.to_string()),
                Value::Text(version.document_id.to_string()),
                Value::Text(version.tx_id.clone()),
                opt_value(version.content_hash.clone()),
                match &version.inline_content {
                    Some(bytes) => Value::Blob(bytes.clone()),
                    None => Value::Null,
                },
                Value::Text(version.metadata.to_string()),
                Value::Text(version.content_type.clone()),
                Value::Integer(version.byte_size as i64),
                Value::Text(version.created_at.to_string()),
            ],
        )
        .await
        .map_err(map_turso_error)?;
        Ok(version)
    }

    async fn document_entry_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<DocumentListEntry> {
        self.scoped_document_entry_conn(conn, DocumentLookupScope::Library { library_id }, path)
            .await
    }

    async fn document_entry_any_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<DocumentListEntry> {
        self.scoped_document_entry_any_conn(conn, DocumentLookupScope::Library { library_id }, path)
            .await
    }

    async fn tmp_document_entry_conn(
        &self,
        conn: &Connection,
        path: &str,
    ) -> Result<DocumentListEntry> {
        self.scoped_document_entry_conn(conn, DocumentLookupScope::Tmp, path)
            .await
    }

    async fn tmp_document_entry_any_conn(
        &self,
        conn: &Connection,
        path: &str,
    ) -> Result<DocumentListEntry> {
        self.scoped_document_entry_any_conn(conn, DocumentLookupScope::Tmp, path)
            .await
    }

    async fn scoped_document_entry_any_conn(
        &self,
        conn: &Connection,
        scope: DocumentLookupScope<'_>,
        path: &str,
    ) -> Result<DocumentListEntry> {
        let (scope_filter, binds) = match scope {
            DocumentLookupScope::Library { library_id } => (
                "d.document_scope = 'library'
                   AND d.library_id = ?1
                   AND d.path = ?2",
                vec![
                    Value::Text(library_id.to_string()),
                    Value::Text(path.to_string()),
                ],
            ),
            DocumentLookupScope::Tmp => (
                "d.document_scope = 'tmp'
                   AND d.library_id IS NULL
                   AND d.path = ?1",
                vec![Value::Text(path.to_string())],
            ),
        };
        let sql = format!(
            "SELECT d.id, d.library_id, d.path, d.head_version_id, v.content_type, v.byte_size, v.content_hash, v.metadata_json, d.expires_at, d.updated_at
             FROM documents d
             JOIN document_versions v ON v.id = d.head_version_id
             WHERE {scope_filter}
               AND d.deleted_at IS NULL
               AND d.head_version_id IS NOT NULL
             LIMIT 1"
        );
        let mut rows = conn.query(&sql, binds).await.map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| document_entry_from_row(&row))
            .transpose()?
            .ok_or_else(|| QuarryError::NotFound(path.to_string()))
    }

    async fn scoped_document_entry_conn(
        &self,
        conn: &Connection,
        scope: DocumentLookupScope<'_>,
        path: &str,
    ) -> Result<DocumentListEntry> {
        let now = now_timestamp();
        let (scope_filter, binds) = match scope {
            DocumentLookupScope::Library { library_id } => (
                "d.document_scope = 'library'
                   AND d.library_id = ?1
                   AND d.path = ?2
                   AND (d.expires_at IS NULL OR d.expires_at > ?3)",
                vec![
                    Value::Text(library_id.to_string()),
                    Value::Text(path.to_string()),
                    Value::Text(now.clone()),
                ],
            ),
            DocumentLookupScope::Tmp => (
                "d.document_scope = 'tmp'
                   AND d.library_id IS NULL
                   AND d.path = ?1
                   AND d.expires_at > ?2",
                vec![Value::Text(path.to_string()), Value::Text(now.clone())],
            ),
        };
        let sql = format!(
            "SELECT d.id, d.library_id, d.path, d.head_version_id, v.content_type, v.byte_size, v.content_hash, v.metadata_json, d.expires_at, d.updated_at
             FROM documents d
             JOIN document_versions v ON v.id = d.head_version_id
             WHERE {scope_filter}
               AND d.deleted_at IS NULL
               AND d.head_version_id IS NOT NULL
             LIMIT 1"
        );
        let mut rows = conn.query(&sql, binds).await.map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            document_entry_from_row(&row)
        } else {
            match scope {
                DocumentLookupScope::Library { library_id } => {
                    self.error_if_library_document_expired_conn(conn, library_id, path, &now)
                        .await?;
                }
                DocumentLookupScope::Tmp => {
                    self.error_if_tmp_document_expired_conn(conn, path, &now)
                        .await?;
                }
            }
            Err(QuarryError::NotFound(path.to_string()))
        }
    }

    async fn document_entries_for_library_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        limit: i64,
    ) -> Result<Vec<DocumentListEntry>> {
        let mut rows = conn
            .query(
                "SELECT d.id, d.library_id, d.path, d.head_version_id, v.content_type, v.byte_size, v.content_hash, v.metadata_json, d.expires_at, d.updated_at
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.document_scope = 'library'
                   AND d.library_id = ?1
                   AND d.deleted_at IS NULL
                   AND d.head_version_id IS NOT NULL
                   AND (d.expires_at IS NULL OR d.expires_at > ?2)
                 ORDER BY d.path LIMIT ?3",
                params![library_id.to_string(), now_timestamp(), limit],
            )
            .await
            .map_err(map_turso_error)?;
        let mut documents = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            documents.push(document_entry_from_row(&row)?);
        }
        Ok(documents)
    }

    async fn reindex_links_conn(&self, conn: &Connection, library_id: &str) -> Result<usize> {
        let documents = self
            .document_entries_for_library_conn(conn, library_id, 10_000)
            .await?;

        conn.execute(
            "DELETE FROM links WHERE library_id = ?1",
            params![library_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
        conn.execute(
            "DELETE FROM aliases WHERE library_id = ?1",
            params![library_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;

        for document in &documents {
            for alias in metadata_aliases(&document.metadata) {
                if alias.trim().is_empty() {
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO aliases (library_id, doc_id, alias, alias_source)
                     VALUES (?1, ?2, ?3, 'metadata')",
                    params![
                        library_id.to_string(),
                        document.id.clone(),
                        alias.trim().to_string()
                    ],
                )
                .await
                .map_err(map_turso_error)?;
            }
        }

        for entry in &documents {
            if !is_textual_content_type(&entry.content_type) {
                continue;
            }
            let document = self.document_conn(conn, library_id, &entry.path).await?;
            for link in extract_links_for_document(&document, &documents) {
                insert_link_conn(conn, library_id, &link).await?;
            }
        }

        Ok(documents.len())
    }

    async fn links_for_source_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        source_doc_id: &str,
    ) -> Result<Vec<DocumentLink>> {
        let mut rows = conn
            .query(
                "SELECT l.src_doc_id, l.src_version_id, sd.path,
                        l.target_kind, l.target_text, l.target_doc_id, td.path,
                        l.target_anchor, l.alias, l.start_offset, l.end_offset, l.resolution_status
                 FROM links l
                 JOIN documents sd ON sd.library_id = l.library_id AND sd.id = l.src_doc_id
                 LEFT JOIN documents td
                   ON td.library_id = l.library_id
                  AND td.id = l.target_doc_id
                  AND td.document_scope = 'library'
                  AND td.deleted_at IS NULL
                  AND td.head_version_id IS NOT NULL
                  AND (td.expires_at IS NULL OR td.expires_at > ?3)
                 WHERE l.library_id = ?1
                   AND l.src_doc_id = ?2
                   AND sd.document_scope = 'library'
                   AND sd.deleted_at IS NULL
                   AND sd.head_version_id IS NOT NULL
                   AND (sd.expires_at IS NULL OR sd.expires_at > ?3)
                 ORDER BY l.start_offset, l.end_offset, l.target_kind",
                params![
                    library_id.to_string(),
                    source_doc_id.to_string(),
                    now_timestamp()
                ],
            )
            .await
            .map_err(map_turso_error)?;
        links_from_rows(&mut rows).await
    }

    async fn links_for_target_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        target_doc_id: &str,
    ) -> Result<Vec<DocumentLink>> {
        let mut rows = conn
            .query(
                "SELECT l.src_doc_id, l.src_version_id, sd.path,
                        l.target_kind, l.target_text, l.target_doc_id, td.path,
                        l.target_anchor, l.alias, l.start_offset, l.end_offset, l.resolution_status
                 FROM links l
                 JOIN documents sd ON sd.library_id = l.library_id AND sd.id = l.src_doc_id
                 LEFT JOIN documents td
                   ON td.library_id = l.library_id
                  AND td.id = l.target_doc_id
                  AND td.document_scope = 'library'
                  AND td.deleted_at IS NULL
                  AND td.head_version_id IS NOT NULL
                  AND (td.expires_at IS NULL OR td.expires_at > ?3)
                 WHERE l.library_id = ?1
                   AND l.target_doc_id = ?2
                   AND l.target_kind <> 'heading'
                   AND sd.document_scope = 'library'
                   AND sd.deleted_at IS NULL
                   AND sd.head_version_id IS NOT NULL
                   AND (sd.expires_at IS NULL OR sd.expires_at > ?3)
                 ORDER BY l.start_offset, l.end_offset, l.target_kind",
                params![
                    library_id.to_string(),
                    target_doc_id.to_string(),
                    now_timestamp()
                ],
            )
            .await
            .map_err(map_turso_error)?;
        links_from_rows(&mut rows).await
    }

    async fn links_for_library_conn(
        &self,
        conn: &Connection,
        library_id: &str,
    ) -> Result<Vec<DocumentLink>> {
        let mut rows = conn
            .query(
                "SELECT l.src_doc_id, l.src_version_id, sd.path,
                        l.target_kind, l.target_text, l.target_doc_id, td.path,
                        l.target_anchor, l.alias, l.start_offset, l.end_offset, l.resolution_status
                 FROM links l
                 JOIN documents sd ON sd.library_id = l.library_id AND sd.id = l.src_doc_id
                 LEFT JOIN documents td
                   ON td.library_id = l.library_id
                  AND td.id = l.target_doc_id
                  AND td.document_scope = 'library'
                  AND td.deleted_at IS NULL
                  AND td.head_version_id IS NOT NULL
                  AND (td.expires_at IS NULL OR td.expires_at > ?2)
                 WHERE l.library_id = ?1
                   AND l.target_kind <> 'heading'
                   AND sd.document_scope = 'library'
                   AND sd.deleted_at IS NULL
                   AND sd.head_version_id IS NOT NULL
                   AND (sd.expires_at IS NULL OR sd.expires_at > ?2)
                 ORDER BY sd.path, l.start_offset, l.end_offset, l.target_kind",
                params![library_id.to_string(), now_timestamp()],
            )
            .await
            .map_err(map_turso_error)?;
        links_from_rows(&mut rows).await
    }

    async fn document_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<Document> {
        self.scoped_document_conn(conn, DocumentLookupScope::Library { library_id }, path)
            .await
    }

    async fn tmp_document_conn(&self, conn: &Connection, path: &str) -> Result<Document> {
        self.scoped_document_conn(conn, DocumentLookupScope::Tmp, path)
            .await
    }

    async fn scoped_document_conn(
        &self,
        conn: &Connection,
        scope: DocumentLookupScope<'_>,
        path: &str,
    ) -> Result<Document> {
        let now = now_timestamp();
        let (scope_filter, binds) = match scope {
            DocumentLookupScope::Library { library_id } => (
                "d.document_scope = 'library'
                   AND d.library_id = ?1
                   AND d.path = ?2
                   AND (d.expires_at IS NULL OR d.expires_at > ?3)",
                vec![
                    Value::Text(library_id.to_string()),
                    Value::Text(path.to_string()),
                    Value::Text(now.clone()),
                ],
            ),
            DocumentLookupScope::Tmp => (
                "d.document_scope = 'tmp'
                   AND d.library_id IS NULL
                   AND d.path = ?1
                   AND d.expires_at > ?2",
                vec![Value::Text(path.to_string()), Value::Text(now.clone())],
            ),
        };
        let sql = format!(
            "SELECT d.id, d.library_id, d.path, d.created_at, d.updated_at, d.expires_at,
                    v.id, v.document_id, v.tx_id, v.content_hash, v.inline_content,
                    v.metadata_json, v.content_type, v.byte_size, v.created_at
             FROM documents d
             JOIN document_versions v ON v.id = d.head_version_id
             WHERE {scope_filter}
               AND d.deleted_at IS NULL
               AND d.head_version_id IS NOT NULL
             LIMIT 1"
        );
        let mut rows = conn.query(&sql, binds).await.map_err(map_turso_error)?;
        let Some(row) = rows.next().await.map_err(map_turso_error)? else {
            match scope {
                DocumentLookupScope::Library { library_id } => {
                    self.error_if_library_document_expired_conn(conn, library_id, path, &now)
                        .await?;
                }
                DocumentLookupScope::Tmp => {
                    self.error_if_tmp_document_expired_conn(conn, path, &now)
                        .await?;
                }
            }
            return Err(QuarryError::NotFound(path.to_string()));
        };
        self.document_from_row(&row)
    }

    fn document_from_row(&self, row: &Row) -> Result<Document> {
        let version = DocumentVersion {
            id: text(row, 6)?.into(),
            document_id: text(row, 7)?.into(),
            tx_id: text(row, 8)?,
            transaction_source: None,
            transaction_actor: None,
            transaction_message: None,
            transaction_provenance: None,
            content_hash: opt_text(row, 9)?,
            inline_content: opt_blob(row, 10)?,
            metadata: serde_json::from_str(&text(row, 11)?)?,
            content_type: text(row, 12)?,
            byte_size: int(row, 13)? as u64,
            created_at: text(row, 14)?.into(),
        };
        let content = match (&version.inline_content, &version.content_hash) {
            (Some(bytes), None) => bytes.clone(),
            (None, Some(hash)) => self.cas.read(hash)?,
            _ => {
                return Err(QuarryError::Invariant(format!(
                    "version {} violates inline/CAS invariant",
                    version.id
                )));
            }
        };
        Ok(Document {
            id: text(row, 0)?.into(),
            library_id: opt_text(row, 1)?,
            path: text(row, 2)?,
            metadata: version.metadata.clone(),
            version,
            content,
            expires_at: opt_text(row, 5)?.map(Into::into),
            created_at: text(row, 3)?.into(),
            updated_at: text(row, 4)?.into(),
        })
    }

    async fn version_content_conn(
        &self,
        conn: &Connection,
        document_id: &str,
        version_id: &str,
    ) -> Result<(DocumentVersion, Vec<u8>)> {
        let mut rows = conn
            .query(
                "SELECT v.id, v.document_id, v.tx_id, v.content_hash, v.inline_content, v.metadata_json,
                        v.content_type, v.byte_size, v.created_at, t.source, t.actor, t.message, t.provenance_json
                 FROM document_versions v
                 JOIN transactions t ON t.id = v.tx_id
                 WHERE v.document_id = ?1 AND v.id = ?2 LIMIT 1",
                params![document_id.to_string(), version_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        let row = rows
            .next()
            .await
            .map_err(map_turso_error)?
            .ok_or_else(|| QuarryError::NotFound(format!("version {version_id}")))?;
        let version = version_from_row(&row)?;
        let content = match (&version.inline_content, &version.content_hash) {
            (Some(bytes), None) => bytes.clone(),
            (None, Some(hash)) => self.cas.read(hash)?,
            _ => {
                return Err(QuarryError::Invariant(format!(
                    "version {} violates inline/CAS invariant",
                    version.id
                )));
            }
        };
        Ok((version, content))
    }

    async fn document_id_for_version_conn(conn: &Connection, version_id: &str) -> Result<String> {
        let mut rows = conn
            .query(
                "SELECT document_id FROM document_versions WHERE id = ?1 LIMIT 1",
                params![version_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| text(&row, 0))
            .transpose()?
            .ok_or_else(|| QuarryError::NotFound(format!("version {version_id}")))
    }

    async fn transaction_conn(conn: &Connection, tx_id: &str) -> Result<TransactionRecord> {
        let mut rows = conn
            .query(
                "SELECT id, library_id, state, actor, source, message, provenance_json, created_at, committed_at
                 FROM transactions WHERE id = ?1 LIMIT 1",
                params![tx_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            transaction_from_row(&row)
        } else {
            Err(QuarryError::NotFound(format!("transaction {tx_id}")))
        }
    }

    async fn conflict_conn(conn: &Connection, conflict_id: &str) -> Result<ConflictRecord> {
        let mut rows = conn
            .query(
                "SELECT c.id, c.library_id, c.path, c.ours_version_id, c.theirs_version_id,
                        c.status, c.discovered_at, c.resolved_at,
                        CASE WHEN d.path IS NOT NULL AND d.path <> c.path THEN d.path ELSE NULL END
                 FROM conflicts c
                 LEFT JOIN document_versions tv ON tv.id = c.theirs_version_id
                 LEFT JOIN documents d ON d.id = tv.document_id AND d.library_id = c.library_id
                 WHERE c.id = ?1
                 LIMIT 1",
                params![conflict_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            conflict_from_row(&row)
        } else {
            Err(QuarryError::NotFound(format!("conflict {conflict_id}")))
        }
    }
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS libraries(
  id TEXT PRIMARY KEY,
  slug TEXT UNIQUE NOT NULL,
  created_at TEXT NOT NULL,
  settings_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS documents(
  id TEXT PRIMARY KEY,
  library_id TEXT,
  path TEXT NOT NULL,
  head_version_id TEXT,
  deleted_at TEXT,
  document_scope TEXT NOT NULL DEFAULT 'library',
  expires_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (document_scope IN ('library', 'tmp')),
  CHECK (
    (document_scope = 'library' AND library_id IS NOT NULL)
    OR (document_scope = 'tmp' AND library_id IS NULL AND expires_at IS NOT NULL)
  )
);

CREATE TABLE IF NOT EXISTS document_versions(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  tx_id TEXT NOT NULL,
  content_hash TEXT,
  inline_content BLOB,
  metadata_json TEXT NOT NULL,
  content_type TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  CHECK ((inline_content IS NULL) != (content_hash IS NULL))
);

CREATE TABLE IF NOT EXISTS transactions(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  state TEXT NOT NULL,
  actor TEXT,
  source TEXT NOT NULL,
  message TEXT,
  provenance_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  committed_at TEXT
);

CREATE TABLE IF NOT EXISTS transaction_changes(
  tx_id TEXT NOT NULL,
  path TEXT NOT NULL,
  change_type TEXT NOT NULL,
  old_version_id TEXT,
  new_version_id TEXT,
  new_path TEXT
);

CREATE TABLE IF NOT EXISTS blobs(
  hash TEXT PRIMARY KEY,
  hash_alg TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  storage_backend TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_peers(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  config_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_state(
  peer_id TEXT NOT NULL,
  path TEXT NOT NULL,
  last_synced_doc_version_id TEXT,
  last_synced_git_oid TEXT,
  PRIMARY KEY(peer_id, path)
);

CREATE TABLE IF NOT EXISTS conflicts(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  ours_version_id TEXT,
  theirs_version_id TEXT,
  status TEXT NOT NULL,
  discovered_at TEXT NOT NULL,
  resolved_at TEXT
);

CREATE TABLE IF NOT EXISTS dir_metadata(
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  mode INTEGER,
  mtime TEXT,
  PRIMARY KEY(library_id, path)
);

CREATE TABLE IF NOT EXISTS inodes(
  library_id TEXT NOT NULL,
  inode INTEGER NOT NULL,
  path TEXT NOT NULL,
  PRIMARY KEY(library_id, inode),
  UNIQUE(library_id, path)
);

CREATE TABLE IF NOT EXISTS inode_counters(
  library_id TEXT PRIMARY KEY,
  next_inode INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS links(
  library_id TEXT NOT NULL,
  src_doc_id TEXT NOT NULL,
  src_version_id TEXT NOT NULL,
  target_kind TEXT NOT NULL,
  target_text TEXT NOT NULL,
  target_doc_id TEXT,
  target_anchor TEXT,
  start_offset INTEGER NOT NULL,
  end_offset INTEGER NOT NULL,
  alias TEXT,
  resolution_status TEXT NOT NULL DEFAULT 'unresolved',
  PRIMARY KEY(library_id, src_doc_id, src_version_id, start_offset, end_offset, target_kind, target_text)
);

CREATE TABLE IF NOT EXISTS collab_invite_tokens(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  role TEXT NOT NULL,
  by_hint TEXT,
  created_at TEXT NOT NULL,
  revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS aliases(
  library_id TEXT NOT NULL,
  doc_id TEXT NOT NULL,
  alias TEXT NOT NULL,
  alias_source TEXT NOT NULL,
  PRIMARY KEY(library_id, alias, doc_id)
);

CREATE TABLE IF NOT EXISTS blocks(
  block_id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  parent_block_id TEXT,
  position INTEGER NOT NULL,
  block_type TEXT NOT NULL,
  attrs TEXT NOT NULL,
  text TEXT NOT NULL,
  marks TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS block_review_items(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  block_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  start_offset INTEGER NOT NULL,
  end_offset INTEGER NOT NULL,
  body TEXT,
  replacement TEXT,
  author TEXT,
  state TEXT NOT NULL,
  quote TEXT,
  context_before TEXT,
  context_after TEXT,
  parent_item_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS block_shadow_bases(
  surface TEXT NOT NULL,
  scope_key TEXT NOT NULL,
  document_id TEXT NOT NULL,
  base_markdown TEXT NOT NULL,
  base_version_id TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(surface, scope_key, document_id)
);

CREATE TABLE IF NOT EXISTS block_transactions(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  client_tx_id TEXT NOT NULL,
  actor_kind TEXT NOT NULL,
  actor_id TEXT,
  ops TEXT NOT NULL,
  resulting_version_id TEXT,
  created_at TEXT NOT NULL,
  UNIQUE(document_id, client_tx_id)
);

CREATE INDEX IF NOT EXISTS idx_versions_document ON document_versions(document_id, created_at);
CREATE INDEX IF NOT EXISTS idx_versions_content_type ON document_versions(content_type);
CREATE INDEX IF NOT EXISTS idx_versions_created_at ON document_versions(created_at);
CREATE INDEX IF NOT EXISTS idx_changes_tx ON transaction_changes(tx_id);
CREATE INDEX IF NOT EXISTS idx_links_src ON links(library_id, src_doc_id, src_version_id);
CREATE INDEX IF NOT EXISTS idx_links_target ON links(library_id, target_doc_id);
CREATE INDEX IF NOT EXISTS idx_aliases_lookup ON aliases(library_id, alias);
CREATE INDEX IF NOT EXISTS idx_collab_invite_tokens_document ON collab_invite_tokens(document_id);
CREATE INDEX IF NOT EXISTS idx_blocks_document ON blocks(document_id, parent_block_id, position);
CREATE INDEX IF NOT EXISTS idx_block_review_items_document ON block_review_items(document_id);
CREATE INDEX IF NOT EXISTS idx_block_review_items_block ON block_review_items(block_id);
"#;

fn title_for_entry(entry: &DocumentListEntry) -> String {
    entry
        .metadata
        .get("title")
        .and_then(JsonValue::as_str)
        .filter(|title| !title.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| display_name_from_path(&entry.path))
}

fn graph_node_from_entry(entry: &DocumentListEntry) -> GraphNode {
    GraphNode {
        id: entry.id.clone(),
        path: entry.path.clone(),
        title: title_for_entry(entry),
        content_type: entry.content_type.clone(),
    }
}

fn display_name_from_path(path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    file_name
        .strip_suffix(".md")
        .or_else(|| file_name.strip_suffix(".markdown"))
        .unwrap_or(file_name)
        .to_string()
}

fn is_textual_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json"
                | "application/markdown"
                | "application/x-markdown"
                | "application/yaml"
                | "application/x-yaml"
        )
}

fn push_unique(fields: &mut Vec<String>, field: &str) {
    if !fields.iter().any(|existing| existing == field) {
        fields.push(field.to_string());
    }
}

fn suggestion_match_rank(match_type: &str) -> u8 {
    match match_type {
        "title" => 0,
        "path" => 1,
        "alias" => 2,
        "heading" => 3,
        _ => 4,
    }
}

fn make_snippet(text: &str, index: usize, query_len: usize) -> String {
    let mut start = index.saturating_sub(60);
    let mut end = (index + query_len + 60).min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < text.len() { "..." } else { "" };
    format!("{prefix}{}{suffix}", text[start..end].replace('\n', " "))
}

fn extract_links_for_document(
    document: &Document,
    documents: &[DocumentListEntry],
) -> Vec<DocumentLink> {
    if !is_textual_content_type(&document.version.content_type) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&document.content);
    let mut links = Vec::new();
    extract_headings(&text, document, &mut links);
    extract_wikilinks(&text, document, documents, &mut links);
    extract_markdown_links(&text, document, documents, &mut links);
    extract_tags(&text, document, &mut links);
    links.sort_by_key(|link| link.start_offset);
    links
}

fn extract_headings(text: &str, document: &Document, links: &mut Vec<DocumentLink>) {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_body = line.trim_end_matches(['\r', '\n']);
        let trimmed_start = line_body.trim_start();
        let leading_whitespace = line_body.len() - trimmed_start.len();
        let heading_marks = trimmed_start
            .as_bytes()
            .iter()
            .take_while(|byte| **byte == b'#')
            .count();
        if !(1..=6).contains(&heading_marks) {
            offset += line.len();
            continue;
        }
        let after_marks = &trimmed_start[heading_marks..];
        if !after_marks.starts_with(' ') && !after_marks.starts_with('\t') {
            offset += line.len();
            continue;
        }
        let content_start_in_after_marks = after_marks.len() - after_marks.trim_start().len();
        let raw_text = after_marks.trim();
        let heading_text = raw_text.trim_end_matches('#').trim();
        if heading_text.is_empty() {
            offset += line.len();
            continue;
        }
        let start_offset =
            offset + leading_whitespace + heading_marks + content_start_in_after_marks;
        links.push(DocumentLink {
            src_doc_id: document.id.clone(),
            src_version_id: document.version.id.clone(),
            src_path: document.path.clone(),
            target_kind: "heading".to_string(),
            target_text: heading_text.to_string(),
            target_doc_id: Some(document.id.clone()),
            target_path: Some(document.path.clone()),
            target_anchor: Some(slugify_heading(heading_text)),
            alias: None,
            start_offset,
            end_offset: start_offset + heading_text.len(),
            resolved: true,
            resolution_status: "resolved".to_string(),
        });
        offset += line.len();
    }
}

fn extract_wikilinks(
    text: &str,
    document: &Document,
    documents: &[DocumentListEntry],
    links: &mut Vec<DocumentLink>,
) {
    let mut search_start = 0;
    while let Some(open_rel) = text[search_start..].find("[[") {
        let open = search_start + open_rel;
        let Some(close_rel) = text[open + 2..].find("]]") else {
            break;
        };
        let close = open + 2 + close_rel;
        let inner = &text[open + 2..close];
        let is_embed = open > 0 && text.as_bytes()[open - 1] == b'!';
        let start_offset = if is_embed { open - 1 } else { open };
        let (target_text, alias) = split_alias(inner);
        let (lookup_target, target_anchor) = split_anchor(&target_text);
        let resolution = resolve_link_target(&lookup_target, documents);
        links.push(DocumentLink {
            src_doc_id: document.id.clone(),
            src_version_id: document.version.id.clone(),
            src_path: document.path.clone(),
            target_kind: if is_embed { "embed" } else { "wiki_link" }.to_string(),
            target_text: lookup_target,
            target_doc_id: resolution.target.map(|entry| entry.id.clone()),
            target_path: resolution.target.map(|entry| entry.path.clone()),
            target_anchor,
            alias,
            start_offset,
            end_offset: close + 2,
            resolved: resolution.target.is_some(),
            resolution_status: resolution.status.to_string(),
        });
        search_start = close + 2;
    }
}

fn extract_markdown_links(
    text: &str,
    document: &Document,
    documents: &[DocumentListEntry],
    links: &mut Vec<DocumentLink>,
) {
    let mut search_start = 0;
    while let Some(open_rel) = text[search_start..].find('[') {
        let open = search_start + open_rel;
        if text[open..].starts_with("[[") {
            search_start = open + 2;
            continue;
        }
        let Some(label_end_rel) = text[open + 1..].find("](") else {
            search_start = open + 1;
            continue;
        };
        let target_start = open + 1 + label_end_rel + 2;
        let Some(close_rel) = text[target_start..].find(')') else {
            break;
        };
        let close = target_start + close_rel;
        let target = text[target_start..close].trim();
        if target.is_empty() {
            search_start = close + 1;
            continue;
        }
        let (lookup_target, target_anchor) = split_anchor(target);
        let resolution = if is_external_link(&lookup_target) || lookup_target.starts_with('#') {
            LinkResolution::external()
        } else {
            resolve_link_target(&lookup_target, documents)
        };
        links.push(DocumentLink {
            src_doc_id: document.id.clone(),
            src_version_id: document.version.id.clone(),
            src_path: document.path.clone(),
            target_kind: "markdown_link".to_string(),
            target_text: lookup_target,
            target_doc_id: resolution.target.map(|entry| entry.id.clone()),
            target_path: resolution.target.map(|entry| entry.path.clone()),
            target_anchor,
            alias: None,
            start_offset: open,
            end_offset: close + 1,
            resolved: resolution.target.is_some(),
            resolution_status: resolution.status.to_string(),
        });
        search_start = close + 1;
    }
}

fn extract_tags(text: &str, document: &Document, links: &mut Vec<DocumentLink>) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'#' {
            index += 1;
            continue;
        }
        let previous = index.checked_sub(1).map(|idx| bytes[idx] as char);
        if previous.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == ']') {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < bytes.len() {
            let ch = bytes[end] as char;
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '/' {
                end += 1;
            } else {
                break;
            }
        }
        if end > index + 1 {
            let tag = text[index + 1..end].to_string();
            links.push(DocumentLink {
                src_doc_id: document.id.clone(),
                src_version_id: document.version.id.clone(),
                src_path: document.path.clone(),
                target_kind: "tag".to_string(),
                target_text: tag,
                target_doc_id: None,
                target_path: None,
                target_anchor: None,
                alias: None,
                start_offset: index,
                end_offset: end,
                resolved: false,
                resolution_status: "unresolved".to_string(),
            });
        }
        index = end.max(index + 1);
    }
}

fn split_alias(text: &str) -> (String, Option<String>) {
    let (target, alias) = text
        .split_once('|')
        .map(|(target, alias)| (target, Some(alias)))
        .unwrap_or((text, None));
    (
        target.trim().to_string(),
        alias
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(ToOwned::to_owned),
    )
}

fn split_anchor(target: &str) -> (String, Option<String>) {
    if let Some((path, anchor)) = target.split_once('#') {
        return (
            path.trim().to_string(),
            Some(anchor.trim().trim_start_matches('#').to_string()),
        );
    }
    if let Some((path, anchor)) = target.split_once('^') {
        return (
            path.trim().to_string(),
            Some(format!("^{}", anchor.trim().trim_start_matches('^'))),
        );
    }
    (target.trim().to_string(), None)
}

fn is_external_link(target: &str) -> bool {
    target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("tel:")
}

struct LinkResolution<'a> {
    target: Option<&'a DocumentListEntry>,
    status: &'static str,
}

impl<'a> LinkResolution<'a> {
    fn resolved(target: &'a DocumentListEntry) -> Self {
        Self {
            target: Some(target),
            status: "resolved",
        }
    }

    fn unresolved() -> Self {
        Self {
            target: None,
            status: "unresolved",
        }
    }

    fn ambiguous() -> Self {
        Self {
            target: None,
            status: "ambiguous",
        }
    }

    /// The link does not reference a library document: an external URL
    /// (`https://…`, `mailto:`) or a same-document anchor (`#section`, empty target).
    fn external() -> Self {
        Self {
            target: None,
            status: "external",
        }
    }
}

fn resolve_link_target<'a>(target: &str, documents: &'a [DocumentListEntry]) -> LinkResolution<'a> {
    let normalized = target.trim().trim_start_matches('/');
    if normalized.is_empty() {
        // No document target intended (e.g. a bare `#anchor` or empty `[[]]`).
        return LinkResolution::external();
    }
    let normalized_lc = normalized.to_lowercase();
    let normalized_md_lc = format!("{normalized_lc}.md");
    let normalized_without_ext = strip_markdown_extension(&normalized_lc);
    let mut candidates: Vec<(&DocumentListEntry, u8)> = documents
        .iter()
        .filter_map(|entry| {
            let path_lc = entry.path.to_lowercase();
            let path_without_ext = strip_markdown_extension(&path_lc);
            let file_name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            let file_stem_lc = strip_markdown_extension(&file_name.to_lowercase());
            let rank = if path_lc == normalized_lc {
                0
            } else if path_lc == normalized_md_lc {
                1
            } else if path_without_ext == normalized_without_ext {
                2
            } else if file_stem_lc == normalized_without_ext {
                3
            } else if metadata_aliases(&entry.metadata)
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(normalized))
            {
                4
            } else {
                return None;
            };
            Some((entry, rank))
        })
        .collect();
    candidates.sort_by(|(a, a_rank), (b, b_rank)| {
        a_rank.cmp(b_rank).then_with(|| {
            a.path
                .len()
                .cmp(&b.path.len())
                .then_with(|| a.path.cmp(&b.path))
        })
    });
    let Some((first, rank)) = candidates.first().copied() else {
        return LinkResolution::unresolved();
    };
    let shortest_path_len = first.path.len();
    let ambiguous = candidates.iter().skip(1).any(|(entry, candidate_rank)| {
        *candidate_rank == rank && (rank == 4 || entry.path.len() == shortest_path_len)
    });
    if ambiguous {
        LinkResolution::ambiguous()
    } else {
        LinkResolution::resolved(first)
    }
}

fn strip_markdown_extension(path: &str) -> String {
    path.strip_suffix(".md")
        .or_else(|| path.strip_suffix(".markdown"))
        .unwrap_or(path)
        .to_string()
}

fn slugify_heading(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lowercase in ch.to_lowercase() {
                slug.push(lowercase);
            }
            last_was_dash = false;
        } else if !slug.is_empty() && !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    if last_was_dash {
        slug.pop();
    }
    slug
}

fn metadata_aliases(metadata: &JsonValue) -> Vec<String> {
    match metadata.get("aliases") {
        Some(JsonValue::String(alias)) => vec![alias.clone()],
        Some(JsonValue::Array(aliases)) => aliases
            .iter()
            .filter_map(JsonValue::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn unified_line_diff(base: &str, against: &str) -> String {
    let base_lines: Vec<&str> = base.lines().collect();
    let against_lines: Vec<&str> = against.lines().collect();
    let mut diff = String::from("--- base\n+++ against\n");
    let max = base_lines.len().max(against_lines.len());
    for index in 0..max {
        match (base_lines.get(index), against_lines.get(index)) {
            (Some(base_line), Some(against_line)) if base_line == against_line => {
                diff.push(' ');
                diff.push_str(base_line);
                diff.push('\n');
            }
            (Some(base_line), Some(against_line)) => {
                diff.push('-');
                diff.push_str(base_line);
                diff.push('\n');
                diff.push('+');
                diff.push_str(against_line);
                diff.push('\n');
            }
            (Some(base_line), None) => {
                diff.push('-');
                diff.push_str(base_line);
                diff.push('\n');
            }
            (None, Some(against_line)) => {
                diff.push('+');
                diff.push_str(against_line);
                diff.push('\n');
            }
            (None, None) => {}
        }
    }
    diff
}

fn acquire_lock(config: &StoreConfig) -> Result<LockGuard> {
    let path = config
        .lock_path
        .clone()
        .unwrap_or_else(|| config.db_path.with_extension("lock"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(QuarryError::Io)?;
    file.try_lock_exclusive().map_err(|err| {
        if err.kind() == ErrorKind::WouldBlock {
            QuarryError::Busy(format!(
                "another Quarry daemon appears to own {}",
                config.db_path.display()
            ))
        } else {
            QuarryError::Io(err)
        }
    })?;
    file.set_len(0)?;
    writeln!(&file, "{}", process::id())?;
    Ok(LockGuard {
        path: Some(path),
        file: Some(file),
    })
}

async fn begin_immediate(conn: &Connection) -> Result<()> {
    let mut delay = Duration::from_millis(5);
    for attempt in 0..6 {
        match conn.execute("BEGIN IMMEDIATE", ()).await {
            Ok(_) => return Ok(()),
            Err(err) if is_busy(&err) && attempt < 5 => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(err) => return Err(map_turso_error(err)),
        }
    }
    Err(QuarryError::Busy("database remained locked".to_string()))
}

async fn finish_tx<T>(conn: &Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute("COMMIT", ()).await.map_err(map_turso_error)?;
            Ok(value)
        }
        Err(err) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(err)
        }
    }
}

async fn insert_transaction_conn(
    conn: &Connection,
    library_id: &str,
    source: DocumentSource,
    actor: Option<String>,
    message: Option<String>,
    provenance: JsonValue,
) -> Result<TransactionRecord> {
    let tx = TransactionRecord {
        id: Uuid::new_v4().to_string(),
        library_id: library_id.to_string(),
        state: TransactionState::Open,
        actor,
        source,
        message,
        provenance,
        created_at: now_timestamp().into(),
        committed_at: None,
    };
    conn.execute(
        "INSERT INTO transactions
         (id, library_id, state, actor, source, message, provenance_json, created_at, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        vec![
            Value::Text(tx.id.clone()),
            Value::Text(tx.library_id.clone()),
            Value::Text(tx.state.as_str().to_string()),
            opt_value(tx.actor.clone()),
            Value::Text(tx.source.as_str().to_string()),
            opt_value(tx.message.clone()),
            Value::Text(tx.provenance.to_string()),
            Value::Text(tx.created_at.to_string()),
        ],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(tx)
}

async fn insert_change_conn(
    conn: &Connection,
    tx_id: &str,
    path: &str,
    change_type: ChangeType,
    old_version_id: Option<&str>,
    new_version_id: Option<&str>,
    new_path: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO transaction_changes
         (tx_id, path, change_type, old_version_id, new_version_id, new_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        vec![
            Value::Text(tx_id.to_string()),
            Value::Text(path.to_string()),
            Value::Text(change_type.as_str().to_string()),
            opt_value(old_version_id.map(ToOwned::to_owned)),
            opt_value(new_version_id.map(ToOwned::to_owned)),
            opt_value(new_path.map(ToOwned::to_owned)),
        ],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn insert_link_conn(conn: &Connection, library_id: &str, link: &DocumentLink) -> Result<()> {
    conn.execute(
        "INSERT INTO links
         (library_id, src_doc_id, src_version_id, target_kind, target_text, target_doc_id,
          target_anchor, start_offset, end_offset, alias, resolution_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        vec![
            Value::Text(library_id.to_string()),
            Value::Text(link.src_doc_id.to_string()),
            Value::Text(link.src_version_id.to_string()),
            Value::Text(link.target_kind.clone()),
            Value::Text(link.target_text.clone()),
            opt_value(link.target_doc_id.clone()),
            opt_value(link.target_anchor.clone()),
            Value::Integer(link.start_offset as i64),
            Value::Integer(link.end_offset as i64),
            opt_value(link.alias.clone()),
            Value::Text(link.resolution_status.clone()),
        ],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn links_from_rows(rows: &mut Rows) -> Result<Vec<DocumentLink>> {
    let mut links = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        links.push(link_from_row(&row)?);
    }
    Ok(links)
}

async fn delete_staged_change_conn(conn: &Connection, tx_id: &str, path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM transaction_changes WHERE tx_id = ?1 AND path = ?2",
        params![tx_id.to_string(), path.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

fn default_tmp_expires_at() -> String {
    (Utc::now() + chrono::Duration::days(30)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

async fn error_if_library_document_expired(
    conn: &Connection,
    library_id: &str,
    path: &str,
    now: &str,
) -> Result<()> {
    error_if_document_expired_conn(conn, DocumentLookupScope::Library { library_id }, path, now)
        .await
}

async fn error_if_tmp_document_expired(conn: &Connection, path: &str, now: &str) -> Result<()> {
    error_if_document_expired_conn(conn, DocumentLookupScope::Tmp, path, now).await
}

async fn error_if_document_expired_conn(
    conn: &Connection,
    scope: DocumentLookupScope<'_>,
    path: &str,
    now: &str,
) -> Result<()> {
    let (scope_filter, binds) = match scope {
        DocumentLookupScope::Library { library_id } => (
            "document_scope = 'library'
               AND library_id = ?1
               AND path = ?2
               AND expires_at IS NOT NULL
               AND expires_at <= ?3",
            vec![
                Value::Text(library_id.to_string()),
                Value::Text(path.to_string()),
                Value::Text(now.to_string()),
            ],
        ),
        DocumentLookupScope::Tmp => (
            "document_scope = 'tmp'
               AND library_id IS NULL
               AND path = ?1
               AND expires_at <= ?2",
            vec![Value::Text(path.to_string()), Value::Text(now.to_string())],
        ),
    };
    let sql = format!(
        "SELECT 1 FROM documents
         WHERE {scope_filter}
           AND deleted_at IS NULL
           AND head_version_id IS NOT NULL
         LIMIT 1"
    );
    let mut rows = conn.query(&sql, binds).await.map_err(map_turso_error)?;
    if rows.next().await.map_err(map_turso_error)?.is_some() {
        Err(QuarryError::Gone(path.to_string()))
    } else {
        Ok(())
    }
}

async fn ensure_document_conn(
    conn: &Connection,
    library_id: &str,
    path: &str,
    now: &str,
) -> Result<(String, Option<String>)> {
    if let Some(identity) =
        document_identity_conn(conn, DocumentLookupScope::Library { library_id }, path, now).await?
    {
        return Ok(identity);
    }
    error_if_library_document_expired(conn, library_id, path, now).await?;
    insert_document_conn(
        conn,
        DocumentLookupScope::Library { library_id },
        path,
        None,
        now,
    )
    .await
}

async fn ensure_tmp_document_conn(
    conn: &Connection,
    path: &str,
    expires_at: &str,
    now: &str,
) -> Result<(String, Option<String>)> {
    if let Some(identity) =
        document_identity_conn(conn, DocumentLookupScope::Tmp, path, now).await?
    {
        return Ok(identity);
    }
    error_if_tmp_document_expired(conn, path, now).await?;
    insert_document_conn(conn, DocumentLookupScope::Tmp, path, Some(expires_at), now).await
}

async fn document_identity_conn(
    conn: &Connection,
    scope: DocumentLookupScope<'_>,
    path: &str,
    now: &str,
) -> Result<Option<(String, Option<String>)>> {
    let (scope_filter, binds) = match scope {
        DocumentLookupScope::Library { library_id } => (
            "document_scope = 'library'
               AND library_id = ?1
               AND path = ?2
               AND (expires_at IS NULL OR expires_at > ?3)",
            vec![
                Value::Text(library_id.to_string()),
                Value::Text(path.to_string()),
                Value::Text(now.to_string()),
            ],
        ),
        DocumentLookupScope::Tmp => (
            "document_scope = 'tmp'
               AND library_id IS NULL
               AND path = ?1
               AND expires_at > ?2",
            vec![Value::Text(path.to_string()), Value::Text(now.to_string())],
        ),
    };
    let sql = format!(
        "SELECT id, head_version_id FROM documents
         WHERE {scope_filter}
           AND deleted_at IS NULL
           AND head_version_id IS NOT NULL
         LIMIT 1"
    );
    let mut rows = conn.query(&sql, binds).await.map_err(map_turso_error)?;
    if let Some(row) = rows.next().await.map_err(map_turso_error)? {
        Ok(Some((text(&row, 0)?, opt_text(&row, 1)?)))
    } else {
        Ok(None)
    }
}

async fn insert_document_conn(
    conn: &Connection,
    scope: DocumentLookupScope<'_>,
    path: &str,
    expires_at: Option<&str>,
    now: &str,
) -> Result<(String, Option<String>)> {
    let id = Uuid::new_v4().to_string();
    match scope {
        DocumentLookupScope::Library { library_id } => {
            conn.execute(
                "INSERT INTO documents
                 (id, library_id, path, head_version_id, deleted_at, created_at, updated_at, document_scope, expires_at)
                 VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?4, 'library', NULL)",
                params![
                    id.clone(),
                    library_id.to_string(),
                    path.to_string(),
                    now.to_string()
                ],
            )
            .await
            .map_err(map_turso_error)?;
        }
        DocumentLookupScope::Tmp => {
            let expires_at = expires_at.ok_or_else(|| {
                QuarryError::Invariant("tmp document inserts require expires_at".to_string())
            })?;
            conn.execute(
                "INSERT INTO documents
                 (id, library_id, path, head_version_id, deleted_at, created_at, updated_at, document_scope, expires_at)
                 VALUES (?1, NULL, ?2, NULL, NULL, ?3, ?3, 'tmp', ?4)",
                params![
                    id.clone(),
                    path.to_string(),
                    now.to_string(),
                    expires_at.to_string()
                ],
            )
            .await
            .map_err(map_turso_error)?;
        }
    }
    Ok((id, None))
}

async fn publish_put_conn(conn: &Connection, doc_id: &str, version_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE documents SET head_version_id = ?1, deleted_at = NULL, updated_at = ?2 WHERE id = ?3",
        params![version_id.to_string(), now_timestamp(), doc_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn commit_transaction_record_conn(conn: &Connection, tx_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE transactions SET state = ?1, committed_at = ?2 WHERE id = ?3",
        params![
            TransactionState::Committed.as_str(),
            now_timestamp(),
            tx_id.to_string()
        ],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn ensure_inode_conn(conn: &Connection, library_id: &str, path: &str) -> Result<i64> {
    let mut rows = conn
        .query(
            "SELECT inode FROM inodes WHERE library_id = ?1 AND path = ?2 LIMIT 1",
            params![library_id.to_string(), path.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    if let Some(row) = rows.next().await.map_err(map_turso_error)? {
        return int(&row, 0);
    }
    let inode = allocate_inode_conn(conn, library_id).await?;
    conn.execute(
        "INSERT INTO inodes (library_id, inode, path) VALUES (?1, ?2, ?3)",
        params![library_id.to_string(), inode, path.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(inode)
}

async fn allocate_inode_conn(conn: &Connection, library_id: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO inode_counters (library_id, next_inode)
         VALUES (?1, (SELECT COALESCE(MAX(inode), 0) + 1 FROM inodes WHERE library_id = ?1))
         ON CONFLICT(library_id) DO NOTHING",
        params![library_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    let mut rows = conn
        .query(
            "SELECT next_inode FROM inode_counters WHERE library_id = ?1 LIMIT 1",
            params![library_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    let inode = rows
        .next()
        .await
        .map_err(map_turso_error)?
        .map(|row| int(&row, 0))
        .transpose()?
        .ok_or_else(|| {
            QuarryError::Invariant(format!("inode counter missing for library {library_id}"))
        })?;
    conn.execute(
        "UPDATE inode_counters SET next_inode = next_inode + 1 WHERE library_id = ?1",
        params![library_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(inode)
}

async fn ensure_path_inodes_conn(conn: &Connection, library_id: &str, path: &str) -> Result<()> {
    ensure_inode_conn(conn, library_id, "").await?;
    for dir in parent_dirs(path) {
        ensure_inode_conn(conn, library_id, &dir).await?;
        conn.execute(
            "INSERT INTO dir_metadata (library_id, path, mode, mtime)
             VALUES (?1, ?2, NULL, ?3)
             ON CONFLICT(library_id, path) DO NOTHING",
            params![library_id.to_string(), dir, now_timestamp()],
        )
        .await
        .map_err(map_turso_error)?;
    }
    ensure_inode_conn(conn, library_id, path).await?;
    Ok(())
}

async fn ensure_parent_inodes_conn(conn: &Connection, library_id: &str, path: &str) -> Result<()> {
    ensure_inode_conn(conn, library_id, "").await?;
    for dir in parent_dirs(path) {
        ensure_inode_conn(conn, library_id, &dir).await?;
        conn.execute(
            "INSERT INTO dir_metadata (library_id, path, mode, mtime)
             VALUES (?1, ?2, NULL, ?3)
             ON CONFLICT(library_id, path) DO NOTHING",
            params![library_id.to_string(), dir, now_timestamp()],
        )
        .await
        .map_err(map_turso_error)?;
    }
    Ok(())
}

async fn delete_path_inode_conn(conn: &Connection, library_id: &str, path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM inodes WHERE library_id = ?1 AND path = ?2",
        params![library_id.to_string(), path.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn move_path_inode_conn(
    conn: &Connection,
    library_id: &str,
    from_path: &str,
    to_path: &str,
) -> Result<()> {
    ensure_parent_inodes_conn(conn, library_id, to_path).await?;
    let Some(inode) = inode_for_path_conn(conn, library_id, from_path).await? else {
        ensure_inode_conn(conn, library_id, to_path).await?;
        return Ok(());
    };
    conn.execute(
        "DELETE FROM inodes WHERE library_id = ?1 AND path = ?2 AND inode <> ?3",
        params![library_id.to_string(), to_path.to_string(), inode],
    )
    .await
    .map_err(map_turso_error)?;
    conn.execute(
        "UPDATE inodes SET path = ?1 WHERE library_id = ?2 AND inode = ?3",
        params![to_path.to_string(), library_id.to_string(), inode],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn move_path_prefix_inodes_conn(
    conn: &Connection,
    library_id: &str,
    from_path: &str,
    to_path: &str,
) -> Result<()> {
    ensure_parent_inodes_conn(conn, library_id, to_path).await?;
    let from_prefix = format!("{from_path}/");
    let mut rows = conn
        .query(
            "SELECT inode, path FROM inodes
             WHERE library_id = ?1 AND (path = ?2 OR path LIKE ?3)
             ORDER BY length(path)",
            params![
                library_id.to_string(),
                from_path.to_string(),
                format!("{from_prefix}%")
            ],
        )
        .await
        .map_err(map_turso_error)?;
    let mut moved = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        moved.push((int(&row, 0)?, text(&row, 1)?));
    }
    if moved.is_empty() {
        ensure_inode_conn(conn, library_id, to_path).await?;
        return Ok(());
    }
    for (inode, old_path) in &moved {
        let new_path = replace_path_prefix(old_path, from_path, to_path);
        conn.execute(
            "DELETE FROM inodes WHERE library_id = ?1 AND path = ?2 AND inode <> ?3",
            params![library_id.to_string(), new_path, *inode],
        )
        .await
        .map_err(map_turso_error)?;
    }
    for (inode, old_path) in moved {
        conn.execute(
            "UPDATE inodes SET path = ?1 WHERE library_id = ?2 AND inode = ?3",
            params![
                replace_path_prefix(&old_path, from_path, to_path),
                library_id.to_string(),
                inode
            ],
        )
        .await
        .map_err(map_turso_error)?;
    }
    Ok(())
}

fn replace_path_prefix(path: &str, from_path: &str, to_path: &str) -> String {
    if path == from_path {
        return to_path.to_string();
    }
    let suffix = path
        .strip_prefix(from_path)
        .unwrap_or(path)
        .trim_start_matches('/');
    if suffix.is_empty() {
        to_path.to_string()
    } else {
        format!("{to_path}/{suffix}")
    }
}

async fn inode_for_path_conn(
    conn: &Connection,
    library_id: &str,
    path: &str,
) -> Result<Option<i64>> {
    let mut rows = conn
        .query(
            "SELECT inode FROM inodes WHERE library_id = ?1 AND path = ?2 LIMIT 1",
            params![library_id.to_string(), path.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    rows.next()
        .await
        .map_err(map_turso_error)?
        .map(|row| int(&row, 0))
        .transpose()
}

fn library_from_row(row: &Row) -> Result<Library> {
    Ok(Library {
        id: text(row, 0)?,
        slug: text(row, 1)?,
        created_at: text(row, 2)?.into(),
        settings: serde_json::from_str(&text(row, 3)?)?,
    })
}

fn directory_metadata_from_row(row: &Row) -> Result<DirectoryMetadata> {
    Ok(DirectoryMetadata {
        path: text(row, 0)?,
        mode: opt_int(row, 1)?,
        mtime: text(row, 2)?,
        inode: int(row, 3)?,
    })
}

fn document_entry_from_row(row: &Row) -> Result<DocumentListEntry> {
    Ok(DocumentListEntry {
        id: text(row, 0)?.into(),
        library_id: opt_text(row, 1)?,
        path: text(row, 2)?,
        head_version_id: text(row, 3)?.into(),
        content_type: text(row, 4)?,
        byte_size: int(row, 5)? as u64,
        content_hash: opt_text(row, 6)?,
        metadata: serde_json::from_str(&text(row, 7)?)?,
        expires_at: opt_text(row, 8)?.map(Into::into),
        updated_at: text(row, 9)?.into(),
    })
}

fn link_from_row(row: &Row) -> Result<DocumentLink> {
    let target_doc_id = opt_text(row, 5)?;
    let target_path = opt_text(row, 6)?;
    let resolved = target_doc_id.is_some() && target_path.is_some();
    let stored_resolution_status = text(row, 11)?;
    let resolution_status = if resolved {
        "resolved".to_string()
    } else if stored_resolution_status == "ambiguous" {
        "ambiguous".to_string()
    } else if stored_resolution_status == "external" {
        "external".to_string()
    } else {
        "unresolved".to_string()
    };
    Ok(DocumentLink {
        src_doc_id: text(row, 0)?.into(),
        src_version_id: text(row, 1)?.into(),
        src_path: text(row, 2)?,
        target_kind: text(row, 3)?,
        target_text: text(row, 4)?,
        target_doc_id: if resolved {
            target_doc_id.map(Into::into)
        } else {
            None
        },
        target_path,
        target_anchor: opt_text(row, 7)?,
        alias: opt_text(row, 8)?,
        start_offset: int(row, 9)? as usize,
        end_offset: int(row, 10)? as usize,
        resolved,
        resolution_status,
    })
}

const AUTOSAVE_IDLE_SPLIT_SECONDS: i64 = 120;
const AUTOSAVE_MAX_SPAN_SECONDS: i64 = 600;

pub fn group_version_history(mut versions: Vec<DocumentVersion>) -> Vec<DocumentHistoryEntry> {
    versions.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut groups: Vec<Vec<DocumentVersion>> = Vec::new();
    for version in versions {
        if let Some(current) = groups.last_mut()
            && can_group_autosave(current, &version)
        {
            current.push(version);
            continue;
        }
        groups.push(vec![version]);
    }

    groups
        .into_iter()
        .rev()
        .map(history_entry_from_group)
        .collect()
}

fn can_group_autosave(group: &[DocumentVersion], next: &DocumentVersion) -> bool {
    let Some(first) = group.first() else {
        return false;
    };
    let Some(previous) = group.last() else {
        return false;
    };
    if !is_autosave_version(first) || !is_autosave_version(next) {
        return false;
    }
    if first.transaction_source != next.transaction_source
        || first.transaction_actor != next.transaction_actor
        || first.content_type != next.content_type
        || autosave_session_id(first) != autosave_session_id(next)
        || checkpoint_reason(first) != checkpoint_reason(next)
    {
        return false;
    }
    let Some(first_at) = parse_history_time(&first.created_at) else {
        return false;
    };
    let Some(previous_at) = parse_history_time(&previous.created_at) else {
        return false;
    };
    let Some(next_at) = parse_history_time(&next.created_at) else {
        return false;
    };
    next_at.signed_duration_since(previous_at).num_seconds() <= AUTOSAVE_IDLE_SPLIT_SECONDS
        && next_at.signed_duration_since(first_at).num_seconds() <= AUTOSAVE_MAX_SPAN_SECONDS
}

fn history_entry_from_group(group: Vec<DocumentVersion>) -> DocumentHistoryEntry {
    let earliest = group.first().expect("history group must contain a version");
    let latest = group.last().expect("history group must contain a version");
    DocumentHistoryEntry {
        id: latest.id.clone(),
        document_id: latest.document_id.clone(),
        latest_version_id: latest.id.clone(),
        earliest_version_id: earliest.id.clone(),
        raw_version_count: group.len() as u64,
        source: latest.transaction_source.clone(),
        actor: latest.transaction_actor.clone(),
        message: latest.transaction_message.clone(),
        provenance: latest.transaction_provenance.clone(),
        checkpoint_reason: checkpoint_reason(latest),
        content_type: latest.content_type.clone(),
        byte_size: latest.byte_size,
        created_at: earliest.created_at.clone(),
        updated_at: latest.created_at.clone(),
    }
}

fn is_autosave_version(version: &DocumentVersion) -> bool {
    version
        .transaction_provenance
        .as_ref()
        .and_then(|provenance| provenance.get("history"))
        .and_then(|history| history.get("kind"))
        .and_then(JsonValue::as_str)
        == Some("autosave")
}

fn autosave_session_id(version: &DocumentVersion) -> Option<String> {
    version
        .transaction_provenance
        .as_ref()
        .and_then(|provenance| provenance.get("history"))
        .and_then(|history| history.get("session_id"))
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

fn checkpoint_reason(version: &DocumentVersion) -> Option<String> {
    version
        .transaction_provenance
        .as_ref()
        .and_then(|provenance| provenance.get("history"))
        .and_then(|history| history.get("reason"))
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

fn parse_history_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn parse_storage_enum<T>(value: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|err| QuarryError::Invariant(err.to_string()))
}

fn version_from_row(row: &Row) -> Result<DocumentVersion> {
    let mut version = DocumentVersion {
        id: text(row, 0)?.into(),
        document_id: text(row, 1)?.into(),
        tx_id: text(row, 2)?,
        transaction_source: None,
        transaction_actor: None,
        transaction_message: None,
        transaction_provenance: None,
        content_hash: opt_text(row, 3)?,
        inline_content: opt_blob(row, 4)?,
        metadata: serde_json::from_str(&text(row, 5)?)?,
        content_type: text(row, 6)?,
        byte_size: int(row, 7)? as u64,
        created_at: text(row, 8)?.into(),
    };
    if let Some(source) = opt_text(row, 9)? {
        version.transaction_source = Some(parse_storage_enum(&source)?);
        version.transaction_actor = opt_text(row, 10)?;
        version.transaction_message = opt_text(row, 11)?;
        version.transaction_provenance = Some(serde_json::from_str(&text(row, 12)?)?);
    }
    Ok(version)
}

fn transaction_from_row(row: &Row) -> Result<TransactionRecord> {
    Ok(TransactionRecord {
        id: text(row, 0)?,
        library_id: text(row, 1)?,
        state: parse_storage_enum(&text(row, 2)?)?,
        actor: opt_text(row, 3)?,
        source: parse_storage_enum(&text(row, 4)?)?,
        message: opt_text(row, 5)?,
        provenance: serde_json::from_str(&text(row, 6)?)?,
        created_at: text(row, 7)?.into(),
        committed_at: opt_text(row, 8)?.map(Into::into),
    })
}

fn conflict_from_row(row: &Row) -> Result<ConflictRecord> {
    Ok(ConflictRecord {
        id: text(row, 0)?,
        library_id: text(row, 1)?,
        path: text(row, 2)?,
        conflict_path: opt_text(row, 8)?,
        ours_version_id: opt_text(row, 3)?.map(Into::into),
        theirs_version_id: opt_text(row, 4)?.map(Into::into),
        status: parse_storage_enum(&text(row, 5)?)?,
        discovered_at: text(row, 6)?.into(),
        resolved_at: opt_text(row, 7)?.map(Into::into),
    })
}

fn sync_state_from_row(row: &Row) -> Result<SyncStateEntry> {
    Ok(SyncStateEntry {
        peer_id: text(row, 0)?,
        path: text(row, 1)?,
        last_synced_doc_version_id: opt_text(row, 2)?.map(Into::into),
        last_synced_git_oid: opt_text(row, 3)?,
    })
}

fn collab_invite_token_from_row(row: &Row) -> Result<CollabInviteToken> {
    Ok(CollabInviteToken {
        id: text(row, 0)?,
        document_id: text(row, 1)?.into(),
        role: text(row, 2)?,
        by_hint: opt_text(row, 3)?,
        created_at: text(row, 4)?.into(),
        revoked_at: opt_text(row, 5)?.map(Into::into),
    })
}

fn ensure_open(tx: &TransactionRecord) -> Result<()> {
    if tx.state == TransactionState::Open {
        Ok(())
    } else {
        Err(QuarryError::Conflict(format!(
            "transaction {} is {:?}",
            tx.id, tx.state
        )))
    }
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || slug.contains('/')
        || slug.contains('\\')
        || slug == "."
        || slug == ".."
        || slug.chars().any(char::is_whitespace)
    {
        Err(QuarryError::InvalidPath(format!("library slug {slug}")))
    } else {
        Ok(())
    }
}

async fn ensure_links_resolution_status_column(conn: &Connection) -> Result<()> {
    let mut rows = conn
        .query("PRAGMA table_info(links)", ())
        .await
        .map_err(map_turso_error)?;
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        if text(&row, 1)? == "resolution_status" {
            return Ok(());
        }
    }
    conn.execute(
        "ALTER TABLE links ADD COLUMN resolution_status TEXT NOT NULL DEFAULT 'unresolved'",
        (),
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn migrate_documents_scope_ttl(conn: &Connection) -> Result<()> {
    let columns = table_columns_conn(conn, "documents").await?;
    let has_scope = columns.iter().any(|column| column.name == "document_scope");
    let has_expires_at = columns.iter().any(|column| column.name == "expires_at");
    let library_id_not_null = columns
        .iter()
        .find(|column| column.name == "library_id")
        .is_some_and(|column| column.not_null);
    if has_scope
        && has_expires_at
        && !library_id_not_null
        && !documents_has_legacy_path_unique_conn(conn).await?
    {
        return Ok(());
    }

    begin_immediate(conn).await?;
    let result = async {
        let scope_expr = if has_scope {
            "document_scope"
        } else {
            "'library'"
        };
        let expires_expr = if has_expires_at { "expires_at" } else { "NULL" };
        let insert_sql = format!(
            r#"
            INSERT INTO documents
              (id, library_id, path, head_version_id, deleted_at, created_at, updated_at, document_scope, expires_at)
            SELECT id, library_id, path, head_version_id, deleted_at, created_at, updated_at,
                   {scope_expr}, {expires_expr}
            FROM documents_scope_ttl_migration;
            "#
        );
        conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS documents_scope_ttl_migration;
            ALTER TABLE documents RENAME TO documents_scope_ttl_migration;
            CREATE TABLE documents(
              id TEXT PRIMARY KEY,
              library_id TEXT,
              path TEXT NOT NULL,
              head_version_id TEXT,
              deleted_at TEXT,
              document_scope TEXT NOT NULL DEFAULT 'library',
              expires_at TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              CHECK (document_scope IN ('library', 'tmp')),
              CHECK (
                (document_scope = 'library' AND library_id IS NOT NULL)
                OR (document_scope = 'tmp' AND library_id IS NULL AND expires_at IS NOT NULL)
              )
            );
            "#,
        )
        .await
        .map_err(map_turso_error)?;
        conn.execute_batch(&insert_sql).await.map_err(map_turso_error)?;
        conn.execute("DROP TABLE documents_scope_ttl_migration", ())
            .await
            .map_err(map_turso_error)?;
        Ok(())
    }
    .await;
    finish_tx(conn, result).await
}

struct TableColumn {
    name: String,
    not_null: bool,
}

async fn table_columns_conn(conn: &Connection, table: &str) -> Result<Vec<TableColumn>> {
    let mut rows = conn
        .query(
            format!("PRAGMA table_info({})", quote_sql_string(table)),
            (),
        )
        .await
        .map_err(map_turso_error)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        columns.push(TableColumn {
            name: text(&row, 1)?,
            not_null: int(&row, 3)? != 0,
        });
    }
    Ok(columns)
}

async fn documents_has_legacy_path_unique_conn(conn: &Connection) -> Result<bool> {
    let mut rows = conn
        .query("PRAGMA index_list('documents')", ())
        .await
        .map_err(map_turso_error)?;
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        let name = text(&row, 1)?;
        if name == "idx_documents_active_library_path" || int(&row, 2)? == 0 {
            continue;
        }
        if index_columns_conn(conn, &name).await? == ["library_id", "path"] {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn index_columns_conn(conn: &Connection, index_name: &str) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            format!("PRAGMA index_info({})", quote_sql_string(index_name)),
            (),
        )
        .await
        .map_err(map_turso_error)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        columns.push(text(&row, 2)?);
    }
    Ok(columns)
}

async fn ensure_document_indexes_conn(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        DROP INDEX IF EXISTS idx_documents_active_library_path;
        DROP INDEX IF EXISTS idx_documents_active_tmp_path;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_active_library_path
          ON documents(library_id, path)
          WHERE document_scope = 'library' AND deleted_at IS NULL AND head_version_id IS NOT NULL;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_active_tmp_path
          ON documents(path)
          WHERE document_scope = 'tmp' AND library_id IS NULL AND deleted_at IS NULL AND head_version_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_documents_library_path ON documents(library_id, path);
        CREATE INDEX IF NOT EXISTS idx_documents_scope_path ON documents(document_scope, path);
        CREATE INDEX IF NOT EXISTS idx_documents_expires_at ON documents(expires_at);
        CREATE INDEX IF NOT EXISTS idx_documents_created_at ON documents(created_at);
        CREATE INDEX IF NOT EXISTS idx_documents_updated_at ON documents(updated_at);
        "#,
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn normalize_prefix(prefix: &str) -> Result<String> {
    let trimmed = prefix.trim_start_matches('/');
    if trimmed.is_empty() {
        Ok(String::new())
    } else if trimmed.ends_with('/') {
        Ok(normalize_path(trimmed.trim_end_matches('/'))? + "/")
    } else {
        normalize_path(trimmed)
    }
}

fn normalize_graph_folder(folder: &str) -> Result<String> {
    let trimmed = folder.trim().trim_matches('/');
    if trimmed.is_empty() {
        Ok(String::new())
    } else {
        normalize_path(trimmed)
    }
}

fn normalize_graph_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('#').to_string()
}

fn path_is_in_folder(path: &str, folder: &str) -> bool {
    path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn normalize_directory_path(path: &str) -> Result<String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        Ok(String::new())
    } else {
        normalize_path(trimmed)
    }
}

fn directory_path_and_parents(path: &str) -> Vec<String> {
    let mut dirs = parent_dirs(&format!("{path}/.dir"));
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Deep-merges `patch` into `target` (objects merge recursively, scalars
/// replace). Shared with the Phase 4 reconciled write path in quarry-server.
pub fn merge_json(target: &mut JsonValue, patch: JsonValue) {
    match (target, patch) {
        (JsonValue::Object(target), JsonValue::Object(patch)) => {
            for (key, value) in patch {
                merge_json(target.entry(key).or_insert(JsonValue::Null), value);
            }
        }
        (target, value) => *target = value,
    }
}

fn validate_tmp_markdown_bytes(content: &[u8]) -> Result<()> {
    if content.len() > TMP_DOCUMENT_MARKDOWN_MAX_BYTES {
        return Err(QuarryError::PayloadTooLarge(format!(
            "tmp Markdown documents are limited to {} bytes",
            TMP_DOCUMENT_MARKDOWN_MAX_BYTES
        )));
    }
    std::str::from_utf8(content).map_err(|_| {
        QuarryError::InvalidInput("tmp Markdown documents must be valid UTF-8".to_string())
    })?;
    Ok(())
}

fn validate_tmp_markdown_text(markdown: &str) -> Result<()> {
    validate_tmp_markdown_bytes(markdown.as_bytes())
}

fn tmp_metadata_with_content_type(mut metadata: JsonValue, content_type: &str) -> JsonValue {
    match &mut metadata {
        JsonValue::Object(object) => {
            object.insert(
                "content_type".to_string(),
                JsonValue::String(content_type.to_string()),
            );
            metadata
        }
        _ => serde_json::json!({ "content_type": content_type }),
    }
}

fn validate_tmp_markdown_write(
    content: Vec<u8>,
    metadata: JsonValue,
    content_type: &str,
) -> Result<(Vec<u8>, JsonValue, String)> {
    let content_type = normalize_tmp_markdown_content_type(content_type)?.to_string();
    validate_tmp_markdown_bytes(&content)?;
    let metadata = tmp_metadata_with_content_type(metadata, &content_type);
    Ok((content, metadata, content_type))
}

fn merge_markdown_frontmatter_metadata(
    content: &[u8],
    metadata: JsonValue,
    content_type: &str,
) -> Result<JsonValue> {
    if !is_markdown_content_type(content_type) {
        return Ok(metadata);
    }
    let mut frontmatter = markdown_frontmatter_metadata(content)?;
    merge_json(&mut frontmatter, metadata);
    Ok(frontmatter)
}

fn markdown_frontmatter_metadata(content: &[u8]) -> Result<JsonValue> {
    let Ok(text) = std::str::from_utf8(content) else {
        return Ok(serde_json::json!({}));
    };
    Ok(split_markdown_frontmatter(text)?.0)
}

/// Splits leading YAML frontmatter from a Markdown document: the parsed
/// frontmatter (an empty object when absent) and the body after it.
/// Splits leading YAML frontmatter from a Markdown text: `(frontmatter
/// metadata, body)`. Shared with the Phase 4 reconciled write path.
pub fn split_markdown_frontmatter(text: &str) -> Result<(JsonValue, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(open_len) = markdown_frontmatter_open_len(text) else {
        return Ok((serde_json::json!({}), text));
    };
    let rest = &text[open_len..];
    let Some((end, close_len)) = markdown_frontmatter_close(rest) else {
        return Ok((serde_json::json!({}), text));
    };
    let yaml = &rest[..end];
    let frontmatter = serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(yaml)?)?;
    Ok((frontmatter, &rest[end + close_len..]))
}

fn markdown_frontmatter_open_len(text: &str) -> Option<usize> {
    if text.starts_with("---\n") {
        Some(4)
    } else if text.starts_with("---\r\n") {
        Some(5)
    } else {
        None
    }
}

fn markdown_frontmatter_close(text: &str) -> Option<(usize, usize)> {
    ["\n---\n", "\r\n---\r\n", "\n---\r\n", "\r\n---\n"]
        .into_iter()
        .filter_map(|marker| text.find(marker).map(|index| (index, marker.len())))
        .min_by_key(|(index, _)| *index)
}

fn normalize_markdown_content_type(content_type: &str) -> Option<&'static str> {
    match content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "text/markdown" => Some("text/markdown"),
        "text/x-markdown" => Some("text/x-markdown"),
        "application/markdown" => Some("application/markdown"),
        "application/x-markdown" => Some("application/x-markdown"),
        _ => None,
    }
}

fn is_markdown_content_type(content_type: &str) -> bool {
    normalize_markdown_content_type(content_type).is_some()
}

fn normalize_collab_invite_role(role: &str) -> Result<String> {
    let role = role.trim().to_ascii_lowercase();
    match role.as_str() {
        "viewer" | "editor" => Ok(role),
        _ => Err(QuarryError::InvalidPath(format!(
            "unsupported collab invite role {role}"
        ))),
    }
}

fn opt_value<T>(value: Option<T>) -> Value
where
    T: Into<String>,
{
    value
        .map(Into::into)
        .map(Value::Text)
        .unwrap_or(Value::Null)
}

fn text(row: &Row, index: usize) -> Result<String> {
    row.get::<String>(index).map_err(map_turso_error)
}

fn opt_text(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Text(value) => Ok(Some(value)),
        other => Err(QuarryError::Invariant(format!(
            "expected text/null at column {index}, got {other:?}"
        ))),
    }
}

fn opt_blob(row: &Row, index: usize) -> Result<Option<Vec<u8>>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Blob(value) => Ok(Some(value)),
        other => Err(QuarryError::Invariant(format!(
            "expected blob/null at column {index}, got {other:?}"
        ))),
    }
}

fn opt_int(row: &Row, index: usize) -> Result<Option<i64>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Integer(value) => Ok(Some(value)),
        other => Err(QuarryError::Invariant(format!(
            "expected integer/null at column {index}, got {other:?}"
        ))),
    }
}

fn int(row: &Row, index: usize) -> Result<i64> {
    row.get::<i64>(index).map_err(map_turso_error)
}

fn is_busy(err: &turso::Error) -> bool {
    matches!(err, turso::Error::Busy(_) | turso::Error::BusySnapshot(_))
}

fn map_turso_error(err: turso::Error) -> QuarryError {
    if is_busy(&err) {
        QuarryError::Busy(err.to_string())
    } else {
        QuarryError::StorageSource {
            source: Box::new(err),
        }
    }
}

#[cfg(test)]
mod tmp_secret_tests {
    use super::*;

    #[test]
    fn store_event_constructors_populate_only_relevant_fields() {
        let event = StoreEvent::document_put(
            "lib".to_string(),
            "docs/readme.md".to_string(),
            DocumentSource::Rest,
            "tx1".to_string(),
            "doc1".to_string(),
            "v1".to_string(),
            Some("origin-1".to_string()),
        );
        assert_eq!(event.kind(), StoreEventKind::DocumentPut);
        assert_eq!(event.library_id(), "lib");
        assert_eq!(event.path(), Some("docs/readme.md"));
        assert_eq!(event.new_path(), None);
        assert_eq!(event.source(), Some(&DocumentSource::Rest));
        assert_eq!(event.tx_id(), Some("tx1"));
        assert_eq!(event.doc_id(), Some("doc1"));
        assert_eq!(event.version_id(), Some("v1"));
        assert_eq!(event.conflict_id(), None);
        assert_eq!(event.peer_id(), None);
        assert_eq!(event.applied(), None);
        assert_eq!(event.conflicts(), None);
        assert_eq!(event.origin_id(), Some("origin-1"));

        let event = StoreEvent::document_delete(
            "lib".to_string(),
            "docs/readme.md".to_string(),
            DocumentSource::Rest,
            "tx2".to_string(),
            Some("doc1".to_string()),
            Some("origin-2".to_string()),
        );
        assert_eq!(event.kind(), StoreEventKind::DocumentDelete);
        assert_eq!(event.path(), Some("docs/readme.md"));
        assert_eq!(event.new_path(), None);
        assert_eq!(event.source(), Some(&DocumentSource::Rest));
        assert_eq!(event.tx_id(), Some("tx2"));
        assert_eq!(event.doc_id(), Some("doc1"));
        assert_eq!(event.version_id(), None);
        assert_eq!(event.origin_id(), Some("origin-2"));

        let event = StoreEvent::document_move(
            "lib".to_string(),
            "docs/readme.md".to_string(),
            "docs/archive.md".to_string(),
            DocumentSource::Rest,
            "tx3".to_string(),
            Some("doc1".to_string()),
            Some("origin-3".to_string()),
        );
        assert_eq!(event.kind(), StoreEventKind::DocumentMove);
        assert_eq!(event.path(), Some("docs/readme.md"));
        assert_eq!(event.new_path(), Some("docs/archive.md"));
        assert_eq!(event.source(), Some(&DocumentSource::Rest));
        assert_eq!(event.tx_id(), Some("tx3"));
        assert_eq!(event.doc_id(), Some("doc1"));
        assert_eq!(event.origin_id(), Some("origin-3"));

        let event = StoreEvent::links_indexed("lib".to_string(), "docs/readme.md".to_string());
        assert_eq!(event.kind(), StoreEventKind::LinksIndexed);
        assert_eq!(event.path(), Some("docs/readme.md"));
        assert_eq!(event.source(), None);

        let event =
            StoreEvent::directory_put("lib".to_string(), "docs".to_string(), DocumentSource::Rest);
        assert_eq!(event.kind(), StoreEventKind::DirectoryPut);
        assert_eq!(event.path(), Some("docs"));
        assert_eq!(event.source(), Some(&DocumentSource::Rest));

        let event = StoreEvent::directory_delete(
            "lib".to_string(),
            "docs".to_string(),
            DocumentSource::Rest,
        );
        assert_eq!(event.kind(), StoreEventKind::DirectoryDelete);
        assert_eq!(event.path(), Some("docs"));
        assert_eq!(event.source(), Some(&DocumentSource::Rest));

        let event = StoreEvent::directory_move(
            "lib".to_string(),
            "docs".to_string(),
            "archive".to_string(),
            DocumentSource::Rest,
        );
        assert_eq!(event.kind(), StoreEventKind::DirectoryMove);
        assert_eq!(event.path(), Some("docs"));
        assert_eq!(event.new_path(), Some("archive"));
        assert_eq!(event.source(), Some(&DocumentSource::Rest));

        let event = StoreEvent::conflict_created(
            "lib".to_string(),
            "docs/readme.md".to_string(),
            "conflict-1".to_string(),
        );
        assert_eq!(event.kind(), StoreEventKind::ConflictCreated);
        assert_eq!(event.path(), Some("docs/readme.md"));
        assert_eq!(event.conflict_id(), Some("conflict-1"));
        assert_eq!(event.source(), None);

        let event = StoreEvent::conflict_resolved(
            "lib".to_string(),
            "docs/readme.md".to_string(),
            "conflict-1".to_string(),
        );
        assert_eq!(event.kind(), StoreEventKind::ConflictResolved);
        assert_eq!(event.path(), Some("docs/readme.md"));
        assert_eq!(event.conflict_id(), Some("conflict-1"));
        assert_eq!(event.source(), None);

        let event = StoreEvent::library_reindexed("lib".to_string());
        assert_eq!(event.kind(), StoreEventKind::LibraryReindexed);
        assert_eq!(event.library_id(), "lib");
        assert_eq!(event.path(), None);
        assert_eq!(event.source(), None);
        assert_eq!(event.conflict_id(), None);

        let event = StoreEvent::git_sync_completed("lib".to_string(), "peer".to_string(), 3, 1);
        assert_eq!(event.kind(), StoreEventKind::GitSyncCompleted);
        assert_eq!(event.library_id(), "lib");
        assert_eq!(event.path(), None);
        assert_eq!(event.source(), Some(&DocumentSource::Git));
        assert_eq!(event.peer_id(), Some("peer"));
        assert_eq!(event.applied(), Some(3));
        assert_eq!(event.conflicts(), Some(1));
    }

    #[test]
    fn generated_tmp_secret_is_url_safe_hex() {
        let secret = TmpDocumentSecret::generate();

        assert_eq!(secret.as_str().len(), TMP_DOCUMENT_SECRET_LEN);
        assert!(
            secret
                .as_str()
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
        assert!(!secret.as_str().contains('/'));
    }

    #[test]
    fn tmp_secret_rejects_path_like_values() {
        let error = TmpDocumentSecret::parse("scratch/note.md")
            .expect_err("path-like tmp identifiers should be rejected");

        assert!(matches!(
            error,
            QuarryError::InvalidPath(message) if message == "invalid tmp document secret"
        ));
    }

    #[test]
    fn tmp_secret_normalizes_uppercase_hex() -> Result<()> {
        let secret = TmpDocumentSecret::parse("ABCDEF0123456789ABCDEF0123456789")?;

        assert_eq!(secret.as_str(), "abcdef0123456789abcdef0123456789");
        Ok(())
    }
}
