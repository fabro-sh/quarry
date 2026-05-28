use quarry_cas::DiskCas;
use quarry_core::{
    normalize_path, now_timestamp, parent_dirs, ChangeType, ConflictRecord, ConflictStatus,
    Document, DocumentListEntry, DocumentSource, DocumentVersion, GcReport, GitPeer, Library,
    QuarryError, Result, SyncStateEntry, TransactionRecord, TransactionState, WriteOutcome,
    WritePrecondition, INLINE_CONTENT_THRESHOLD,
};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex, OwnedMutexGuard};
use turso::{params, Builder, Connection, Database, Row, Value};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub db_path: PathBuf,
    pub cas_path: PathBuf,
    pub lock_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryMetadata {
    pub path: String,
    pub mode: Option<i64>,
    pub mtime: String,
    pub inode: i64,
}

pub struct GlobalOperationGuard {
    _guard: OwnedMutexGuard<()>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreEventKind {
    DocumentPut,
    DocumentDelete,
    DocumentMove,
    DirectoryPut,
    DirectoryDelete,
    DirectoryMove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreEvent {
    pub kind: StoreEventKind,
    pub library_id: String,
    pub path: Option<String>,
    pub new_path: Option<String>,
    pub source: Option<DocumentSource>,
    pub tx_id: Option<String>,
}

#[derive(Clone)]
pub struct QuarryStore {
    db: Database,
    cas: DiskCas,
    write_lock: Arc<Mutex<()>>,
    operation_lock: Arc<Mutex<()>>,
    event_tx: broadcast::Sender<StoreEvent>,
    _lock_guard: Arc<LockGuard>,
}

tokio::task_local! {
    static GLOBAL_OPERATION_ACTIVE: ();
}

struct LockGuard {
    path: Option<PathBuf>,
    _file: Option<File>,
}

struct StagedChange {
    path: String,
    change_type: String,
    old_version_id: Option<String>,
    new_version_id: Option<String>,
    new_path: Option<String>,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

impl QuarryStore {
    pub async fn open(config: StoreConfig) -> Result<Self> {
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
            _lock_guard: Arc::new(lock_guard),
        };
        store.migrate().await?;
        Ok(store)
    }

    pub fn cas(&self) -> &DiskCas {
        &self.cas
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<StoreEvent> {
        self.event_tx.subscribe()
    }

    fn emit_event(&self, event: StoreEvent) {
        let _ = self.event_tx.send(event);
    }

    pub async fn acquire_global_operation_lock(&self) -> GlobalOperationGuard {
        GlobalOperationGuard {
            _guard: self.operation_lock.clone().lock_owned().await,
        }
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

    pub async fn create_library(&self, slug: &str) -> Result<Library> {
        validate_slug(slug)?;
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            if let Some(existing) = self.library_by_slug_or_id_conn(&conn, slug).await? {
                return Ok(existing);
            }
            let now = now_timestamp();
            let library = Library {
                id: Uuid::new_v4().to_string(),
                slug: slug.to_string(),
                created_at: now,
                settings: serde_json::json!({}),
            };
            conn.execute(
                "INSERT INTO libraries (id, slug, created_at, settings_json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    library.id.clone(),
                    library.slug.clone(),
                    library.created_at.clone(),
                    library.settings.to_string()
                ],
            )
            .await
            .map_err(map_turso_error)?;
            ensure_inode_conn(&conn, &library.id, "").await?;
            Ok(library)
        }
        .await;
        finish_tx(&conn, result).await
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
        self.library_by_slug_or_id_conn(&conn, slug_or_id)
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
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            let library_id = library.id.clone();
            ensure_inode_conn(&conn, &library.id, "").await?;
            if !path.is_empty() {
                for dir in directory_path_and_parents(&path) {
                    ensure_inode_conn(&conn, &library.id, &dir).await?;
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
            let metadata = self
                .directory_metadata_conn(&conn, &library.id, &path)
                .await?;
            Ok((metadata, library_id))
        }
        .await;
        let (metadata, library_id) = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DirectoryPut,
            library_id,
            path: Some(path),
            new_path: None,
            source: Some(DocumentSource::Fuse),
            tx_id: None,
        });
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
        let source_for_event = source.clone();
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            let library_id = library.id.clone();
            let updated = conn
                .execute(
                    "UPDATE dir_metadata
                     SET mode = COALESCE(?1, mode),
                         mtime = COALESCE(?2, mtime)
                     WHERE library_id = ?3 AND path = ?4",
                    vec![
                        mode.map(Value::Integer).unwrap_or(Value::Null),
                        mtime
                            .map(|value| Value::Text(value.to_string()))
                            .unwrap_or(Value::Null),
                        Value::Text(library.id.clone()),
                        Value::Text(path.clone()),
                    ],
                )
                .await
                .map_err(map_turso_error)?;
            if updated == 0 {
                return Err(QuarryError::NotFound(path.clone()));
            }
            let metadata = self
                .directory_metadata_conn(&conn, &library.id, &path)
                .await?;
            Ok((metadata, library_id))
        }
        .await;
        let (metadata, library_id) = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DirectoryPut,
            library_id,
            path: Some(path),
            new_path: None,
            source: Some(source_for_event),
            tx_id: None,
        });
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
        let source_for_event = source.clone();
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
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
                     WHERE library_id = ?1 AND deleted_at IS NULL AND path LIKE ?2
                     LIMIT 1",
                    params![library.id.clone(), format!("{from_prefix}%")],
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
            move_path_prefix_inodes_conn(&conn, &library.id, &from_path, &to_path).await?;
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
        }
        .await;
        let library_id = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DirectoryMove,
            library_id,
            path: Some(from_path),
            new_path: Some(to_path),
            source: Some(source_for_event),
            tx_id: None,
        });
        Ok(())
    }

    pub async fn remove_directory(&self, library: &str, path: &str) -> Result<()> {
        let path = normalize_directory_path(path)?;
        if path.is_empty() {
            return Err(QuarryError::Conflict(
                "cannot remove root directory".to_string(),
            ));
        }
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            let library_id = library.id.clone();
            conn.execute(
                "DELETE FROM dir_metadata WHERE library_id = ?1 AND path = ?2",
                params![library.id, path.clone()],
            )
            .await
            .map_err(map_turso_error)?;
            Ok(library_id)
        }
        .await;
        let library_id = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DirectoryDelete,
            library_id,
            path: Some(path),
            new_path: None,
            source: Some(DocumentSource::Fuse),
            tx_id: None,
        });
        Ok(())
    }

    pub async fn list_directories(
        &self,
        library: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<DirectoryMetadata>> {
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
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
        let library = self.require_library_conn(&conn, library).await?;
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
            .ok_or_else(|| QuarryError::NotFound(path))
    }

    pub async fn path_for_inode(&self, library: &str, inode: i64) -> Result<String> {
        if inode <= 0 {
            return Err(QuarryError::InvalidPath(format!("invalid inode {inode}")));
        }
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
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

    pub async fn put_document(
        &self,
        library: &str,
        path: &str,
        content: Vec<u8>,
        metadata: JsonValue,
        content_type: &str,
        source: DocumentSource,
        precondition: WritePrecondition,
    ) -> Result<WriteOutcome> {
        let path = normalize_path(path)?;
        let source_for_event = source.clone();
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            self.check_precondition_conn(&conn, &library.id, &path, &precondition)
                .await?;
            let tx = insert_transaction_conn(
                &conn,
                &library.id,
                source,
                None,
                None,
                serde_json::json!({ "mode": "auto_commit" }),
            )
            .await?;
            let (doc_id, old_version_id) =
                ensure_document_conn(&conn, &library.id, &path, &now_timestamp()).await?;
            let version = self
                .insert_version_conn(&conn, &doc_id, &tx.id, content, metadata, content_type)
                .await?;
            insert_change_conn(
                &conn,
                &tx.id,
                &path,
                ChangeType::Put,
                old_version_id.as_deref(),
                Some(&version.id),
                None,
            )
            .await?;
            publish_put_conn(&conn, &doc_id, &version.id).await?;
            commit_transaction_record_conn(&conn, &tx.id).await?;
            ensure_path_inodes_conn(&conn, &library.id, &path).await?;
            let document = self.document_entry_conn(&conn, &library.id, &path).await?;
            let tx = self.transaction_conn(&conn, &tx.id).await?;
            Ok(WriteOutcome {
                document,
                version,
                transaction: tx,
            })
        }
        .await;
        let outcome = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DocumentPut,
            library_id: outcome.transaction.library_id.clone(),
            path: Some(outcome.document.path.clone()),
            new_path: None,
            source: Some(source_for_event),
            tx_id: Some(outcome.transaction.id.clone()),
        });
        Ok(outcome)
    }

    pub async fn get_document(&self, library: &str, path: &str) -> Result<Document> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
        self.document_conn(&conn, &library.id, &path).await
    }

    pub async fn head_document(&self, library: &str, path: &str) -> Result<DocumentListEntry> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
        self.document_entry_conn(&conn, &library.id, &path).await
    }

    pub async fn list_documents(
        &self,
        library: &str,
        prefix: Option<&str>,
        limit: Option<u64>,
    ) -> Result<Vec<DocumentListEntry>> {
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
        let normalized_prefix = match prefix {
            Some("") | None => None,
            Some(prefix) => Some(normalize_prefix(prefix)?),
        };
        let limit = limit.unwrap_or(1000).min(10_000) as i64;

        let (sql, params) = if let Some(prefix) = normalized_prefix {
            (
                "SELECT d.id, d.path, d.head_version_id, v.content_type, v.byte_size, v.metadata_json, d.updated_at
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.library_id = ?1 AND d.deleted_at IS NULL AND d.head_version_id IS NOT NULL AND d.path LIKE ?2
                 ORDER BY d.path LIMIT ?3",
                vec![
                    Value::Text(library.id),
                    Value::Text(format!("{prefix}%")),
                    Value::Integer(limit),
                ],
            )
        } else {
            (
                "SELECT d.id, d.path, d.head_version_id, v.content_type, v.byte_size, v.metadata_json, d.updated_at
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.library_id = ?1 AND d.deleted_at IS NULL AND d.head_version_id IS NOT NULL
                 ORDER BY d.path LIMIT ?2",
                vec![Value::Text(library.id), Value::Integer(limit)],
            )
        };

        let mut rows = conn.query(sql, params).await.map_err(map_turso_error)?;
        let mut documents = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            documents.push(document_entry_from_row(&row)?);
        }
        Ok(documents)
    }

    pub async fn version_history(&self, library: &str, path: &str) -> Result<Vec<DocumentVersion>> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        let mut rows = conn
            .query(
                "SELECT id, document_id, tx_id, content_hash, inline_content, metadata_json, content_type, byte_size, created_at
                 FROM document_versions WHERE document_id = ?1 ORDER BY created_at, id",
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

    pub async fn delete_document(
        &self,
        library: &str,
        path: &str,
        source: DocumentSource,
    ) -> Result<TransactionRecord> {
        let path = normalize_path(path)?;
        let source_for_event = source.clone();
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            let (doc_id, head_version_id) = self
                .document_identity_conn(&conn, &library.id, &path)
                .await?
                .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
            let tx = insert_transaction_conn(
                &conn,
                &library.id,
                source,
                None,
                None,
                serde_json::json!({ "mode": "auto_commit" }),
            )
            .await?;
            insert_change_conn(
                &conn,
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
            commit_transaction_record_conn(&conn, &tx.id).await?;
            self.transaction_conn(&conn, &tx.id).await
        }
        .await;
        let tx = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DocumentDelete,
            library_id: tx.library_id.clone(),
            path: Some(path),
            new_path: None,
            source: Some(source_for_event),
            tx_id: Some(tx.id.clone()),
        });
        Ok(tx)
    }

    pub async fn move_document(
        &self,
        library: &str,
        from_path: &str,
        to_path: &str,
        source: DocumentSource,
    ) -> Result<TransactionRecord> {
        let from_path = normalize_path(from_path)?;
        let to_path = normalize_path(to_path)?;
        let source_for_event = source.clone();
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            let (doc_id, head_version_id) = self
                .document_identity_conn(&conn, &library.id, &from_path)
                .await?
                .ok_or_else(|| QuarryError::NotFound(from_path.clone()))?;
            if self
                .document_identity_conn(&conn, &library.id, &to_path)
                .await?
                .is_some()
            {
                return Err(QuarryError::Conflict(format!("{to_path} already exists")));
            }
            if let Some((to_doc_id, old_to_version_id)) = self
                .document_any_identity_conn(&conn, &library.id, &to_path)
                .await?
            {
                let from_document = self.document_conn(&conn, &library.id, &from_path).await?;
                let content_type = from_document.version.content_type.clone();
                let from_version_id = from_document.version.id.clone();
                let tx = insert_transaction_conn(
                    &conn,
                    &library.id,
                    source,
                    None,
                    None,
                    serde_json::json!({ "mode": "auto_commit", "move_to_deleted_target": true }),
                )
                .await?;
                let version = self
                    .insert_version_conn(
                        &conn,
                        &to_doc_id,
                        &tx.id,
                        from_document.content,
                        from_document.metadata,
                        &content_type,
                    )
                    .await?;
                insert_change_conn(
                    &conn,
                    &tx.id,
                    &to_path,
                    ChangeType::Put,
                    old_to_version_id.as_deref(),
                    Some(&version.id),
                    None,
                )
                .await?;
                publish_put_conn(&conn, &to_doc_id, &version.id).await?;
                conn.execute(
                    "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                    params![now_timestamp(), doc_id.clone()],
                )
                .await
                .map_err(map_turso_error)?;
                insert_change_conn(
                    &conn,
                    &tx.id,
                    &from_path,
                    ChangeType::Delete,
                    Some(&from_version_id),
                    None,
                    None,
                )
                .await?;
                move_path_inode_conn(&conn, &library.id, &from_path, &to_path).await?;
                commit_transaction_record_conn(&conn, &tx.id).await?;
                return self.transaction_conn(&conn, &tx.id).await;
            }
            let tx = insert_transaction_conn(
                &conn,
                &library.id,
                source,
                None,
                None,
                serde_json::json!({ "mode": "auto_commit" }),
            )
            .await?;
            insert_change_conn(
                &conn,
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
                params![to_path.clone(), now_timestamp(), doc_id],
            )
            .await
            .map_err(map_turso_error)?;
            move_path_inode_conn(&conn, &library.id, &from_path, &to_path).await?;
            commit_transaction_record_conn(&conn, &tx.id).await?;
            self.transaction_conn(&conn, &tx.id).await
        }
        .await;
        let tx = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DocumentMove,
            library_id: tx.library_id.clone(),
            path: Some(from_path),
            new_path: Some(to_path),
            source: Some(source_for_event),
            tx_id: Some(tx.id.clone()),
        });
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
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            let from_document = self.document_conn(&conn, &library.id, &from_path).await?;
            let (to_doc_id, old_to_version_id) = self
                .document_identity_conn(&conn, &library.id, &to_path)
                .await?
                .ok_or_else(|| QuarryError::NotFound(to_path.clone()))?;
            let tx = insert_transaction_conn(
                &conn,
                &library.id,
                source,
                None,
                None,
                serde_json::json!({ "mode": "auto_commit", "replace": true }),
            )
            .await?;
            let version = self
                .insert_version_conn(
                    &conn,
                    &to_doc_id,
                    &tx.id,
                    from_document.content,
                    from_document.metadata,
                    &from_document.version.content_type,
                )
                .await?;
            insert_change_conn(
                &conn,
                &tx.id,
                &to_path,
                ChangeType::Put,
                old_to_version_id.as_deref(),
                Some(&version.id),
                None,
            )
            .await?;
            publish_put_conn(&conn, &to_doc_id, &version.id).await?;
            conn.execute(
                "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now_timestamp(), from_document.id],
            )
            .await
            .map_err(map_turso_error)?;
            insert_change_conn(
                &conn,
                &tx.id,
                &from_path,
                ChangeType::Delete,
                Some(&from_document.version.id),
                None,
                None,
            )
            .await?;
            move_path_inode_conn(&conn, &library.id, &from_path, &to_path).await?;
            commit_transaction_record_conn(&conn, &tx.id).await?;
            self.transaction_conn(&conn, &tx.id).await
        }
        .await;
        let tx = finish_tx(&conn, result).await?;
        self.emit_event(StoreEvent {
            kind: StoreEventKind::DocumentMove,
            library_id: tx.library_id.clone(),
            path: Some(from_path),
            new_path: Some(to_path),
            source: Some(source_for_event),
            tx_id: Some(tx.id.clone()),
        });
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
            self.put_document(
                library,
                path,
                current.content,
                metadata,
                &current.version.content_type,
                source,
                precondition,
            )
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
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            insert_transaction_conn(&conn, &library.id, source, actor, message, provenance).await
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn list_transactions(&self, library: &str) -> Result<Vec<TransactionRecord>> {
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
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
        self.transaction_conn(&conn, tx_id).await
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
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let tx = self.transaction_conn(&conn, tx_id).await?;
            ensure_open(&tx)?;
            let (doc_id, old_version_id) =
                ensure_document_conn(&conn, &tx.library_id, &path, &now_timestamp()).await?;
            delete_staged_change_conn(&conn, tx_id, &path).await?;
            let version = self
                .insert_version_conn(&conn, &doc_id, tx_id, content, metadata, content_type)
                .await?;
            insert_change_conn(
                &conn,
                tx_id,
                &path,
                ChangeType::Put,
                old_version_id.as_deref(),
                Some(&version.id),
                None,
            )
            .await?;
            Ok(version)
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn stage_delete(&self, tx_id: &str, path: &str) -> Result<()> {
        let path = normalize_path(path)?;
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let tx = self.transaction_conn(&conn, tx_id).await?;
            ensure_open(&tx)?;
            let (_, old_version_id) = self
                .document_identity_conn(&conn, &tx.library_id, &path)
                .await?
                .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
            delete_staged_change_conn(&conn, tx_id, &path).await?;
            insert_change_conn(
                &conn,
                tx_id,
                &path,
                ChangeType::Delete,
                old_version_id.as_deref(),
                None,
                None,
            )
            .await
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn stage_metadata(
        &self,
        tx_id: &str,
        path: &str,
        patch: JsonValue,
    ) -> Result<DocumentVersion> {
        let path = normalize_path(path)?;
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let tx = self.transaction_conn(&conn, tx_id).await?;
            ensure_open(&tx)?;
            let current = self.document_conn(&conn, &tx.library_id, &path).await?;
            let mut metadata = current.metadata;
            merge_json(&mut metadata, patch);
            delete_staged_change_conn(&conn, tx_id, &path).await?;
            let version = self
                .insert_version_conn(
                    &conn,
                    &current.id,
                    tx_id,
                    current.content,
                    metadata,
                    &current.version.content_type,
                )
                .await?;
            insert_change_conn(
                &conn,
                tx_id,
                &path,
                ChangeType::Metadata,
                Some(&current.version.id),
                Some(&version.id),
                None,
            )
            .await?;
            Ok(version)
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn stage_move(&self, tx_id: &str, from_path: &str, to_path: &str) -> Result<()> {
        let from_path = normalize_path(from_path)?;
        let to_path = normalize_path(to_path)?;
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let tx = self.transaction_conn(&conn, tx_id).await?;
            ensure_open(&tx)?;
            let (_, old_version_id) = self
                .document_identity_conn(&conn, &tx.library_id, &from_path)
                .await?
                .ok_or_else(|| QuarryError::NotFound(from_path.clone()))?;
            if self
                .document_identity_conn(&conn, &tx.library_id, &to_path)
                .await?
                .is_some()
            {
                return Err(QuarryError::Conflict(format!("{to_path} already exists")));
            }
            insert_change_conn(
                &conn,
                tx_id,
                &from_path,
                ChangeType::Move,
                old_version_id.as_deref(),
                old_version_id.as_deref(),
                Some(&to_path),
            )
            .await
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn commit_transaction(&self, tx_id: &str) -> Result<TransactionRecord> {
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let tx = self.transaction_conn(&conn, tx_id).await?;
            ensure_open(&tx)?;
            let mut events = Vec::new();
            let mut changes = Vec::new();
            let mut rows = conn
                .query(
                    "SELECT path, change_type, old_version_id, new_version_id, new_path
                     FROM transaction_changes
                     WHERE tx_id = ?1 ORDER BY rowid",
                    params![tx_id.to_string()],
                )
                .await
                .map_err(map_turso_error)?;
            while let Some(row) = rows.next().await.map_err(map_turso_error)? {
                changes.push(StagedChange {
                    path: text(&row, 0)?,
                    change_type: text(&row, 1)?,
                    old_version_id: opt_text(&row, 2)?,
                    new_version_id: opt_text(&row, 3)?,
                    new_path: opt_text(&row, 4)?,
                });
            }
            for change in &changes {
                match change.change_type.as_str() {
                    "put" | "metadata" => {
                        self.ensure_staged_head_unchanged_conn(
                            &conn,
                            &tx.library_id,
                            &change.path,
                            change.old_version_id.as_deref(),
                        )
                        .await?;
                    }
                    "delete" => {
                        self.ensure_staged_head_unchanged_conn(
                            &conn,
                            &tx.library_id,
                            &change.path,
                            change.old_version_id.as_deref(),
                        )
                        .await?;
                    }
                    "move" => {
                        let new_path = change.new_path.as_deref().ok_or_else(|| {
                            QuarryError::Storage("move change missing new path".to_string())
                        })?;
                        self.ensure_staged_head_unchanged_conn(
                            &conn,
                            &tx.library_id,
                            &change.path,
                            change.old_version_id.as_deref(),
                        )
                        .await?;
                        self.ensure_move_target_available_conn(&conn, &tx.library_id, new_path)
                            .await?;
                    }
                    other => {
                        return Err(QuarryError::Storage(format!("unknown change type {other}")));
                    }
                }
            }
            for change in changes {
                match change.change_type.as_str() {
                    "put" | "metadata" => {
                        let version_id = change.new_version_id.ok_or_else(|| {
                            QuarryError::Storage("put change missing new version".to_string())
                        })?;
                        let doc_id = self
                            .document_id_conn(&conn, &tx.library_id, &change.path)
                            .await?
                            .ok_or_else(|| QuarryError::NotFound(change.path.clone()))?;
                        publish_put_conn(&conn, &doc_id, &version_id).await?;
                        ensure_path_inodes_conn(&conn, &tx.library_id, &change.path).await?;
                        events.push(StoreEvent {
                            kind: StoreEventKind::DocumentPut,
                            library_id: tx.library_id.clone(),
                            path: Some(change.path.clone()),
                            new_path: None,
                            source: Some(tx.source.clone()),
                            tx_id: Some(tx.id.clone()),
                        });
                    }
                    "delete" => {
                        if let Some((doc_id, _)) =
                            self.document_identity_conn(&conn, &tx.library_id, &change.path).await?
                        {
                            conn.execute(
                                "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                                params![now_timestamp(), doc_id],
                            )
                            .await
                            .map_err(map_turso_error)?;
                        }
                        events.push(StoreEvent {
                            kind: StoreEventKind::DocumentDelete,
                            library_id: tx.library_id.clone(),
                            path: Some(change.path.clone()),
                            new_path: None,
                            source: Some(tx.source.clone()),
                            tx_id: Some(tx.id.clone()),
                        });
                    }
                    "move" => {
                        let new_path = change.new_path.ok_or_else(|| {
                            QuarryError::Storage("move change missing new path".to_string())
                        })?;
                        let (doc_id, _) = self
                            .document_identity_conn(&conn, &tx.library_id, &change.path)
                            .await?
                            .ok_or_else(|| QuarryError::NotFound(change.path.clone()))?;
                        if let Some((to_doc_id, old_to_version_id)) = self
                            .document_any_identity_conn(&conn, &tx.library_id, &new_path)
                            .await?
                        {
                            let from_document =
                                self.document_conn(&conn, &tx.library_id, &change.path).await?;
                            let content_type = from_document.version.content_type.clone();
                            let version = self
                                .insert_version_conn(
                                    &conn,
                                    &to_doc_id,
                                    &tx.id,
                                    from_document.content,
                                    from_document.metadata,
                                    &content_type,
                                )
                                .await?;
                            insert_change_conn(
                                &conn,
                                &tx.id,
                                &new_path,
                                ChangeType::Put,
                                old_to_version_id.as_deref(),
                                Some(&version.id),
                                None,
                            )
                            .await?;
                            publish_put_conn(&conn, &to_doc_id, &version.id).await?;
                            conn.execute(
                                "UPDATE documents SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                                params![now_timestamp(), doc_id],
                            )
                            .await
                            .map_err(map_turso_error)?;
                            move_path_inode_conn(&conn, &tx.library_id, &change.path, &new_path)
                                .await?;
                            events.push(StoreEvent {
                                kind: StoreEventKind::DocumentMove,
                                library_id: tx.library_id.clone(),
                                path: Some(change.path.clone()),
                                new_path: Some(new_path),
                                source: Some(tx.source.clone()),
                                tx_id: Some(tx.id.clone()),
                            });
                            continue;
                        }
                        conn.execute(
                            "UPDATE documents SET path = ?1, updated_at = ?2 WHERE id = ?3",
                            params![new_path.clone(), now_timestamp(), doc_id],
                        )
                        .await
                        .map_err(map_turso_error)?;
                        move_path_inode_conn(&conn, &tx.library_id, &change.path, &new_path)
                            .await?;
                        events.push(StoreEvent {
                            kind: StoreEventKind::DocumentMove,
                            library_id: tx.library_id.clone(),
                            path: Some(change.path.clone()),
                            new_path: Some(new_path),
                            source: Some(tx.source.clone()),
                            tx_id: Some(tx.id.clone()),
                        });
                    }
                    other => {
                        return Err(QuarryError::Storage(format!("unknown change type {other}")));
                    }
                }
            }
            commit_transaction_record_conn(&conn, tx_id).await?;
            let tx = self.transaction_conn(&conn, tx_id).await?;
            Ok((tx, events))
        }
        .await;
        let (tx, events) = finish_tx(&conn, result).await?;
        for event in events {
            self.emit_event(event);
        }
        Ok(tx)
    }

    pub async fn rollback_transaction(&self, tx_id: &str) -> Result<TransactionRecord> {
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let tx = self.transaction_conn(&conn, tx_id).await?;
            ensure_open(&tx)?;
            conn.execute(
                "UPDATE transactions SET state = ?1 WHERE id = ?2",
                params![TransactionState::RolledBack.as_str(), tx_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
            self.transaction_conn(&conn, tx_id).await
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn create_git_peer(&self, library: &str, config: JsonValue) -> Result<GitPeer> {
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
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
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn list_git_peers(&self, library: &str) -> Result<Vec<GitPeer>> {
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
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
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            conn.execute(
                "INSERT INTO sync_state
                 (peer_id, path, last_synced_doc_version_id, last_synced_git_oid)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(peer_id, path)
                 DO UPDATE SET
                    last_synced_doc_version_id = excluded.last_synced_doc_version_id,
                    last_synced_git_oid = excluded.last_synced_git_oid",
                vec![
                    Value::Text(peer_id.to_string()),
                    Value::Text(path.clone()),
                    opt_value(last_synced_doc_version_id.clone()),
                    opt_value(last_synced_git_oid.clone()),
                ],
            )
            .await
            .map_err(map_turso_error)?;
            Ok(SyncStateEntry {
                peer_id: peer_id.to_string(),
                path,
                last_synced_doc_version_id,
                last_synced_git_oid,
            })
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn list_conflicts(&self, library: &str) -> Result<Vec<ConflictRecord>> {
        let conn = self.conn()?;
        let library = self.require_library_conn(&conn, library).await?;
        let mut rows = conn
            .query(
                "SELECT id, library_id, path, ours_version_id, theirs_version_id, status, discovered_at, resolved_at
                 FROM conflicts WHERE library_id = ?1 ORDER BY discovered_at DESC",
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
        self.conflict_conn(&conn, conflict_id).await
    }

    pub async fn record_conflict(
        &self,
        library: &str,
        path: &str,
        ours_version_id: Option<String>,
        theirs_version_id: Option<String>,
    ) -> Result<ConflictRecord> {
        let path = normalize_path(path)?;
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let library = self.require_library_conn(&conn, library).await?;
            let conflict = ConflictRecord {
                id: Uuid::new_v4().to_string(),
                library_id: library.id,
                path,
                ours_version_id,
                theirs_version_id,
                status: ConflictStatus::Open,
                discovered_at: now_timestamp(),
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
                    Value::Text(conflict.discovered_at.clone()),
                ],
            )
            .await
            .map_err(map_turso_error)?;
            Ok(conflict)
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn resolve_conflict(&self, conflict_id: &str) -> Result<ConflictRecord> {
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            conn.execute(
                "UPDATE conflicts SET status = ?1, resolved_at = ?2 WHERE id = ?3",
                params![
                    ConflictStatus::Resolved.as_str(),
                    now_timestamp(),
                    conflict_id.to_string()
                ],
            )
            .await
            .map_err(map_turso_error)?;
            self.conflict_conn(&conn, conflict_id).await
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn gc(&self) -> Result<GcReport> {
        self.run_global_operation(async { self.gc_inner().await })
            .await
    }

    async fn gc_inner(&self) -> Result<GcReport> {
        let _guard = self.write_lock.lock().await;
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT dv.content_hash
                 FROM document_versions dv
                 JOIN transactions t ON t.id = dv.tx_id
                 WHERE dv.content_hash IS NOT NULL AND t.state IN ('open', 'committed')",
                (),
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
            reachable_blobs = report.reachable,
            removed_blobs = report.removed,
            "CAS GC completed"
        );
        Ok(report)
    }

    async fn migrate(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(SCHEMA).await.map_err(map_turso_error)?;
        Ok(())
    }

    fn conn(&self) -> Result<Connection> {
        self.db.connect().map_err(map_turso_error)
    }

    async fn library_by_slug_or_id_conn(
        &self,
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

    async fn require_library_conn(&self, conn: &Connection, slug_or_id: &str) -> Result<Library> {
        self.library_by_slug_or_id_conn(conn, slug_or_id)
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
        let mut rows = conn
            .query(
                "SELECT id FROM documents WHERE library_id = ?1 AND path = ?2 LIMIT 1",
                params![library_id.to_string(), path.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        Ok(rows
            .next()
            .await
            .map_err(map_turso_error)?
            .map(|row| text(&row, 0))
            .transpose()?)
    }

    async fn document_identity_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        let mut rows = conn
            .query(
                "SELECT id, head_version_id FROM documents
                 WHERE library_id = ?1 AND path = ?2 AND deleted_at IS NULL AND head_version_id IS NOT NULL
                 LIMIT 1",
                params![library_id.to_string(), path.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            Ok(Some((text(&row, 0)?, opt_text(&row, 1)?)))
        } else {
            Ok(None)
        }
    }

    async fn document_any_identity_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<Option<(String, Option<String>)>> {
        let mut rows = conn
            .query(
                "SELECT id, head_version_id FROM documents
                 WHERE library_id = ?1 AND path = ?2
                 LIMIT 1",
                params![library_id.to_string(), path.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            Ok(Some((text(&row, 0)?, opt_text(&row, 1)?)))
        } else {
            Ok(None)
        }
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
            id,
            document_id: document_id.to_string(),
            tx_id: tx_id.to_string(),
            content_hash,
            inline_content,
            metadata,
            content_type: content_type.to_string(),
            byte_size,
            created_at,
        };
        conn.execute(
            "INSERT INTO document_versions
             (id, document_id, tx_id, content_hash, inline_content, metadata_json, content_type, byte_size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            vec![
                Value::Text(version.id.clone()),
                Value::Text(version.document_id.clone()),
                Value::Text(version.tx_id.clone()),
                opt_value(version.content_hash.clone()),
                match &version.inline_content {
                    Some(bytes) => Value::Blob(bytes.clone()),
                    None => Value::Null,
                },
                Value::Text(version.metadata.to_string()),
                Value::Text(version.content_type.clone()),
                Value::Integer(version.byte_size as i64),
                Value::Text(version.created_at.clone()),
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
        let mut rows = conn
            .query(
                "SELECT d.id, d.path, d.head_version_id, v.content_type, v.byte_size, v.metadata_json, d.updated_at
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.library_id = ?1 AND d.path = ?2 AND d.deleted_at IS NULL AND d.head_version_id IS NOT NULL
                 LIMIT 1",
                params![library_id.to_string(), path.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            document_entry_from_row(&row)
        } else {
            Err(QuarryError::NotFound(path.to_string()))
        }
    }

    async fn document_conn(
        &self,
        conn: &Connection,
        library_id: &str,
        path: &str,
    ) -> Result<Document> {
        let mut rows = conn
            .query(
                "SELECT d.id, d.library_id, d.path, d.created_at, d.updated_at,
                        v.id, v.document_id, v.tx_id, v.content_hash, v.inline_content,
                        v.metadata_json, v.content_type, v.byte_size, v.created_at
                 FROM documents d
                 JOIN document_versions v ON v.id = d.head_version_id
                 WHERE d.library_id = ?1 AND d.path = ?2 AND d.deleted_at IS NULL AND d.head_version_id IS NOT NULL
                 LIMIT 1",
                params![library_id.to_string(), path.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        let row = rows
            .next()
            .await
            .map_err(map_turso_error)?
            .ok_or_else(|| QuarryError::NotFound(path.to_string()))?;
        let version = DocumentVersion {
            id: text(&row, 5)?,
            document_id: text(&row, 6)?,
            tx_id: text(&row, 7)?,
            content_hash: opt_text(&row, 8)?,
            inline_content: opt_blob(&row, 9)?,
            metadata: serde_json::from_str(&text(&row, 10)?)?,
            content_type: text(&row, 11)?,
            byte_size: int(&row, 12)? as u64,
            created_at: text(&row, 13)?,
        };
        let content = match (&version.inline_content, &version.content_hash) {
            (Some(bytes), None) => bytes.clone(),
            (None, Some(hash)) => self.cas.read(hash)?,
            _ => {
                return Err(QuarryError::Storage(format!(
                    "version {} violates inline/CAS invariant",
                    version.id
                )))
            }
        };
        Ok(Document {
            id: text(&row, 0)?,
            library_id: text(&row, 1)?,
            path: text(&row, 2)?,
            metadata: version.metadata.clone(),
            version,
            content,
            created_at: text(&row, 3)?,
            updated_at: text(&row, 4)?,
        })
    }

    async fn transaction_conn(&self, conn: &Connection, tx_id: &str) -> Result<TransactionRecord> {
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

    async fn conflict_conn(&self, conn: &Connection, conflict_id: &str) -> Result<ConflictRecord> {
        let mut rows = conn
            .query(
                "SELECT id, library_id, path, ours_version_id, theirs_version_id, status, discovered_at, resolved_at
                 FROM conflicts WHERE id = ?1 LIMIT 1",
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
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  head_version_id TEXT,
  deleted_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(library_id, path)
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

CREATE INDEX IF NOT EXISTS idx_documents_library_path ON documents(library_id, path);
CREATE INDEX IF NOT EXISTS idx_documents_created_at ON documents(created_at);
CREATE INDEX IF NOT EXISTS idx_documents_updated_at ON documents(updated_at);
CREATE INDEX IF NOT EXISTS idx_versions_document ON document_versions(document_id, created_at);
CREATE INDEX IF NOT EXISTS idx_versions_content_type ON document_versions(content_type);
CREATE INDEX IF NOT EXISTS idx_versions_created_at ON document_versions(created_at);
CREATE INDEX IF NOT EXISTS idx_changes_tx ON transaction_changes(tx_id);
"#;

fn acquire_lock(config: &StoreConfig) -> Result<LockGuard> {
    let path = config
        .lock_path
        .clone()
        .unwrap_or_else(|| config.db_path.with_extension("lock"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                QuarryError::Busy(format!(
                    "another Quarry daemon appears to own {}",
                    config.db_path.display()
                ))
            } else {
                QuarryError::Io(err)
            }
        })?;
    Ok(LockGuard {
        path: Some(path),
        _file: Some(file),
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
        created_at: now_timestamp(),
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
            Value::Text(tx.created_at.clone()),
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

async fn delete_staged_change_conn(conn: &Connection, tx_id: &str, path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM transaction_changes WHERE tx_id = ?1 AND path = ?2",
        params![tx_id.to_string(), path.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

async fn ensure_document_conn(
    conn: &Connection,
    library_id: &str,
    path: &str,
    now: &str,
) -> Result<(String, Option<String>)> {
    let mut rows = conn
        .query(
            "SELECT id, head_version_id FROM documents WHERE library_id = ?1 AND path = ?2 LIMIT 1",
            params![library_id.to_string(), path.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    if let Some(row) = rows.next().await.map_err(map_turso_error)? {
        return Ok((text(&row, 0)?, opt_text(&row, 1)?));
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO documents
         (id, library_id, path, head_version_id, deleted_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?4)",
        params![
            id.clone(),
            library_id.to_string(),
            path.to_string(),
            now.to_string()
        ],
    )
    .await
    .map_err(map_turso_error)?;
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
    let mut max_rows = conn
        .query(
            "SELECT COALESCE(MAX(inode), 0) + 1 FROM inodes WHERE library_id = ?1",
            params![library_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    let inode = max_rows
        .next()
        .await
        .map_err(map_turso_error)?
        .map(|row| int(&row, 0))
        .transpose()?
        .unwrap_or(1);
    conn.execute(
        "INSERT INTO inodes (library_id, inode, path) VALUES (?1, ?2, ?3)",
        params![library_id.to_string(), inode, path.to_string()],
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
        created_at: text(row, 2)?,
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
        id: text(row, 0)?,
        path: text(row, 1)?,
        head_version_id: text(row, 2)?,
        content_type: text(row, 3)?,
        byte_size: int(row, 4)? as u64,
        metadata: serde_json::from_str(&text(row, 5)?)?,
        updated_at: text(row, 6)?,
    })
}

fn version_from_row(row: &Row) -> Result<DocumentVersion> {
    Ok(DocumentVersion {
        id: text(row, 0)?,
        document_id: text(row, 1)?,
        tx_id: text(row, 2)?,
        content_hash: opt_text(row, 3)?,
        inline_content: opt_blob(row, 4)?,
        metadata: serde_json::from_str(&text(row, 5)?)?,
        content_type: text(row, 6)?,
        byte_size: int(row, 7)? as u64,
        created_at: text(row, 8)?,
    })
}

fn transaction_from_row(row: &Row) -> Result<TransactionRecord> {
    Ok(TransactionRecord {
        id: text(row, 0)?,
        library_id: text(row, 1)?,
        state: match text(row, 2)?.as_str() {
            "open" => TransactionState::Open,
            "committed" => TransactionState::Committed,
            "rolled_back" => TransactionState::RolledBack,
            other => return Err(QuarryError::Storage(format!("invalid tx state {other}"))),
        },
        actor: opt_text(row, 3)?,
        source: match text(row, 4)?.as_str() {
            "rest" => DocumentSource::Rest,
            "git" => DocumentSource::Git,
            "fuse" => DocumentSource::Fuse,
            "cli" => DocumentSource::Cli,
            "system" => DocumentSource::System,
            other => return Err(QuarryError::Storage(format!("invalid source {other}"))),
        },
        message: opt_text(row, 5)?,
        provenance: serde_json::from_str(&text(row, 6)?)?,
        created_at: text(row, 7)?,
        committed_at: opt_text(row, 8)?,
    })
}

fn conflict_from_row(row: &Row) -> Result<ConflictRecord> {
    Ok(ConflictRecord {
        id: text(row, 0)?,
        library_id: text(row, 1)?,
        path: text(row, 2)?,
        ours_version_id: opt_text(row, 3)?,
        theirs_version_id: opt_text(row, 4)?,
        status: match text(row, 5)?.as_str() {
            "open" => ConflictStatus::Open,
            "resolved" => ConflictStatus::Resolved,
            other => {
                return Err(QuarryError::Storage(format!(
                    "invalid conflict status {other}"
                )))
            }
        },
        discovered_at: text(row, 6)?,
        resolved_at: opt_text(row, 7)?,
    })
}

fn sync_state_from_row(row: &Row) -> Result<SyncStateEntry> {
    Ok(SyncStateEntry {
        peer_id: text(row, 0)?,
        path: text(row, 1)?,
        last_synced_doc_version_id: opt_text(row, 2)?,
        last_synced_git_oid: opt_text(row, 3)?,
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

fn merge_json(target: &mut JsonValue, patch: JsonValue) {
    match (target, patch) {
        (JsonValue::Object(target), JsonValue::Object(patch)) => {
            for (key, value) in patch {
                merge_json(target.entry(key).or_insert(JsonValue::Null), value);
            }
        }
        (target, value) => *target = value,
    }
}

fn opt_value(value: Option<String>) -> Value {
    value.map(Value::Text).unwrap_or(Value::Null)
}

fn text(row: &Row, index: usize) -> Result<String> {
    row.get::<String>(index).map_err(map_turso_error)
}

fn opt_text(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Text(value) => Ok(Some(value)),
        other => Err(QuarryError::Storage(format!(
            "expected text/null at column {index}, got {other:?}"
        ))),
    }
}

fn opt_blob(row: &Row, index: usize) -> Result<Option<Vec<u8>>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Blob(value) => Ok(Some(value)),
        other => Err(QuarryError::Storage(format!(
            "expected blob/null at column {index}, got {other:?}"
        ))),
    }
}

fn opt_int(row: &Row, index: usize) -> Result<Option<i64>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Integer(value) => Ok(Some(value)),
        other => Err(QuarryError::Storage(format!(
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
        QuarryError::Storage(err.to_string())
    }
}

#[allow(dead_code)]
fn assert_path_exists(path: &Path) -> Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(QuarryError::NotFound(path.display().to_string()))
    }
}
