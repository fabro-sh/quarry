use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, StoreError>;
pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("utf-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid logical path: {0}")]
    InvalidPath(String),
    #[error("object at {path} is {kind}, not a readable blob")]
    UnsupportedObject { path: String, kind: ObjectKind },
    #[error("policy denied: {0}")]
    PolicyDenied(String),
    #[error("command failed: {program} {args:?}: {stderr}")]
    CommandFailed {
        program: String,
        args: Vec<String>,
        stderr: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Agent,
    GitImport,
    System,
    Integration,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Actor {
    pub id: String,
    pub display_name: String,
    pub kind: ActorKind,
    pub avatar_url: Option<String>,
}

impl Actor {
    pub fn local_human(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            display_name: id.clone(),
            id,
            kind: ActorKind::Human,
            avatar_url: None,
        }
    }

    pub fn system() -> Self {
        Self {
            id: "system".to_string(),
            display_name: "Quarry".to_string(),
            kind: ActorKind::System,
            avatar_url: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobRecord {
    pub hash: String,
    pub size: u64,
    pub media_type: Option<String>,
    pub path: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryObject {
    pub id: String,
    pub hash: String,
    pub size: u64,
    pub media_type: Option<String>,
    pub path: Option<String>,
    pub external_url: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Blob,
    StructuredDoc,
    BinaryObject,
}

impl std::fmt::Display for ObjectKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blob => formatter.write_str("blob"),
            Self::StructuredDoc => formatter.write_str("structured_doc"),
            Self::BinaryObject => formatter.write_str("binary_object"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    pub object_kind: ObjectKind,
    pub object_id: String,
    pub size: Option<u64>,
    pub media_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefRecord {
    pub name: String,
    pub entries: Vec<TreeEntry>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionRecord {
    pub id: String,
    pub actor: Actor,
    pub message: Option<String>,
    pub affected_paths: Vec<String>,
    pub committed_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnnotationRecord {
    pub id: String,
    pub target: String,
    pub body: String,
    pub actor: Actor,
    pub resolved: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub workspace_id: String,
    pub data_dir: String,
    pub refs: Vec<String>,
    pub blob_count: u64,
    pub transaction_count: u64,
    pub annotation_count: u64,
    pub event_count: u64,
    pub snapshot_count: u64,
    pub document_count: u64,
    pub schema_version: i64,
    pub git_sync: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteResult {
    pub blob: BlobRecord,
    pub ref_record: RefRecord,
    pub transaction: TransactionRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftResult {
    pub draft_ref: RefRecord,
    pub transaction: TransactionRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishResult {
    pub source_ref: String,
    pub target_ref: RefRecord,
    pub transaction: TransactionRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteResult {
    pub ref_record: RefRecord,
    pub transaction: TransactionRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryPointerResult {
    pub binary: BinaryObject,
    pub ref_record: RefRecord,
    pub transaction: TransactionRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitMaterializeResult {
    pub ref_name: String,
    pub repo_dir: String,
    pub branch: String,
    pub commit: Option<String>,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitIngestResult {
    pub target_ref: String,
    pub imported_paths: Vec<String>,
    pub conflict_ref: Option<String>,
    pub conflicts: Vec<String>,
    pub transaction: TransactionRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    pub id: String,
    pub kind: String,
    pub actor: Option<Actor>,
    pub target: Option<String>,
    pub transaction_id: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefSnapshotRecord {
    pub id: String,
    pub ref_name: String,
    pub entries: Vec<TreeEntry>,
    pub transaction_id: Option<String>,
    pub actor: Option<Actor>,
    pub message: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StructuredDocumentRecord {
    pub id: String,
    pub ref_name: String,
    pub path: String,
    pub title: String,
    pub snapshot_json: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DocumentSnapshotRecord {
    pub id: String,
    pub document_id: String,
    pub snapshot_json: serde_json::Value,
    pub transaction_id: Option<String>,
    pub actor: Option<Actor>,
    pub message: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DocumentOpRecord {
    pub id: String,
    pub document_id: String,
    pub actor: Actor,
    pub op_json: serde_json::Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PresenceRecord {
    pub document_id: String,
    pub actor: Actor,
    pub cursor_json: serde_json::Value,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DocumentState {
    pub document: StructuredDocumentRecord,
    pub snapshots: Vec<DocumentSnapshotRecord>,
    pub ops: Vec<DocumentOpRecord>,
    pub presence: Vec<PresenceRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DocumentWriteResult {
    pub document: StructuredDocumentRecord,
    pub ref_record: RefRecord,
    pub transaction: TransactionRecord,
    pub snapshot: DocumentSnapshotRecord,
}

#[derive(Clone, Debug)]
pub struct LocalStore {
    data_dir: PathBuf,
    db_path: PathBuf,
    blob_dir: PathBuf,
}

impl LocalStore {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let blob_dir = data_dir.join("blobs");
        fs::create_dir_all(&blob_dir)?;

        let store = Self {
            db_path: data_dir.join("quarry.sqlite3"),
            data_dir,
            blob_dir,
        };

        let conn = store.connect()?;
        init_schema(&conn)?;
        ensure_workspace_id(&conn)?;
        drop(conn);
        store.ensure_ref("published/main")?;

        Ok(store)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        Ok(conn)
    }

    pub fn workspace_id(&self) -> Result<String> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT value FROM metadata WHERE key = 'workspace_id'",
            [],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
    }

    pub fn status(&self) -> Result<WorkspaceStatus> {
        let conn = self.connect()?;
        let workspace_id = self.workspace_id()?;
        let refs = self
            .list_refs()?
            .into_iter()
            .map(|record| record.name)
            .collect();

        Ok(WorkspaceStatus {
            workspace_id,
            data_dir: self.data_dir.display().to_string(),
            refs,
            blob_count: count_rows(&conn, "blobs")?,
            transaction_count: count_rows(&conn, "transactions")?,
            annotation_count: count_rows(&conn, "annotations")?,
            event_count: count_rows(&conn, "events")?,
            snapshot_count: count_rows(&conn, "ref_snapshots")?,
            document_count: count_rows(&conn, "structured_documents")?,
            schema_version: SCHEMA_VERSION,
            git_sync: "not_configured".to_string(),
        })
    }

    pub fn compact(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        Ok(())
    }

    pub fn put_blob(
        &self,
        bytes: &[u8],
        media_type: Option<&str>,
        path: Option<&str>,
    ) -> Result<BlobRecord> {
        let hash = hash_bytes(bytes);
        let shard = self.blob_dir.join(&hash[0..2]);
        fs::create_dir_all(&shard)?;

        let blob_path = shard.join(&hash);
        if !blob_path.exists() {
            fs::write(&blob_path, bytes)?;
        }

        let created_at = now_timestamp();
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO blobs (hash, size, media_type, path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(hash) DO UPDATE SET
                media_type = COALESCE(excluded.media_type, blobs.media_type),
                path = COALESCE(excluded.path, blobs.path)",
            params![
                hash,
                bytes.len() as u64,
                media_type,
                path.map(str::to_string),
                created_at
            ],
        )?;

        self.blob_record(&hash)
    }

    pub fn blob_record(&self, hash: &str) -> Result<BlobRecord> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT hash, size, media_type, path, created_at FROM blobs WHERE hash = ?1",
            params![hash],
            |row| {
                Ok(BlobRecord {
                    hash: row.get(0)?,
                    size: row.get(1)?,
                    media_type: row.get(2)?,
                    path: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("blob {hash}")))
    }

    pub fn read_blob(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.blob_dir.join(&hash[0..2]).join(hash);
        if !path.exists() {
            return Err(StoreError::NotFound(format!("blob bytes {hash}")));
        }
        fs::read(path).map_err(StoreError::from)
    }

    pub fn ensure_ref(&self, name: &str) -> Result<RefRecord> {
        if let Some(record) = self.find_ref(name)? {
            return Ok(record);
        }

        let record = RefRecord {
            name: name.to_string(),
            entries: Vec::new(),
            updated_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO refs (name, tree_json, updated_at) VALUES (?1, ?2, ?3)",
            params![
                record.name,
                serde_json::to_string(&record.entries)?,
                record.updated_at
            ],
        )?;

        Ok(record)
    }

    pub fn list_refs(&self) -> Result<Vec<RefRecord>> {
        let conn = self.connect()?;
        let mut stmt =
            conn.prepare("SELECT name, tree_json, updated_at FROM refs ORDER BY name ASC")?;
        let rows = stmt.query_map([], ref_from_row)?;
        collect_rows(rows)
    }

    pub fn find_ref(&self, name: &str) -> Result<Option<RefRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT name, tree_json, updated_at FROM refs WHERE name = ?1",
            params![name],
            ref_from_row,
        )
        .optional()
        .map_err(StoreError::from)
    }

    pub fn get_ref(&self, name: &str) -> Result<RefRecord> {
        self.find_ref(name)?
            .ok_or_else(|| StoreError::NotFound(format!("ref {name}")))
    }

    pub fn update_ref_entry(&self, ref_name: &str, entry: TreeEntry) -> Result<RefRecord> {
        let mut record = self.ensure_ref(ref_name)?;
        record
            .entries
            .retain(|existing| existing.path != entry.path);
        record.entries.push(entry);
        record
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        record.updated_at = now_timestamp();

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO refs (name, tree_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET
                tree_json = excluded.tree_json,
                updated_at = excluded.updated_at",
            params![
                record.name,
                serde_json::to_string(&record.entries)?,
                record.updated_at
            ],
        )?;

        Ok(record)
    }

    pub fn replace_ref_entries(
        &self,
        ref_name: &str,
        mut entries: Vec<TreeEntry>,
    ) -> Result<RefRecord> {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let record = RefRecord {
            name: ref_name.to_string(),
            entries,
            updated_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO refs (name, tree_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET
                tree_json = excluded.tree_json,
                updated_at = excluded.updated_at",
            params![
                record.name,
                serde_json::to_string(&record.entries)?,
                record.updated_at
            ],
        )?;

        Ok(record)
    }

    pub fn create_draft(
        &self,
        base_ref: &str,
        draft_name: Option<&str>,
        actor: Actor,
    ) -> Result<DraftResult> {
        let base = self.get_ref(base_ref)?;
        let draft_ref = draft_name
            .map(str::to_string)
            .unwrap_or_else(|| format!("draft/{}", Uuid::new_v4()));
        let draft = self.replace_ref_entries(&draft_ref, base.entries)?;
        let transaction = self.record_transaction(
            actor.clone(),
            Some(format!("start draft {draft_ref} from {base_ref}")),
            Vec::new(),
        )?;
        self.record_ref_snapshot(
            &draft_ref,
            &draft,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("start draft from {base_ref}")),
        )?;
        self.record_event(
            "draft_created",
            Some(actor),
            Some(draft_ref.clone()),
            Some(transaction.id.clone()),
            serde_json::json!({ "base_ref": base_ref, "draft_ref": draft_ref }),
        )?;

        Ok(DraftResult {
            draft_ref: draft,
            transaction,
        })
    }

    pub fn publish_ref(
        &self,
        source_ref: &str,
        target_ref: &str,
        actor: Actor,
    ) -> Result<PublishResult> {
        let source = self.get_ref(source_ref)?;
        let affected_paths = source
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let target = self.replace_ref_entries(target_ref, source.entries)?;
        let transaction = self.record_transaction(
            actor.clone(),
            Some(format!("publish {source_ref} to {target_ref}")),
            affected_paths.clone(),
        )?;
        self.record_ref_snapshot(
            target_ref,
            &target,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("publish {source_ref}")),
        )?;
        self.record_event(
            "ref_published",
            Some(actor),
            Some(target_ref.to_string()),
            Some(transaction.id.clone()),
            serde_json::json!({
                "source_ref": source_ref,
                "target_ref": target_ref,
                "affected_paths": affected_paths
            }),
        )?;

        Ok(PublishResult {
            source_ref: source_ref.to_string(),
            target_ref: target,
            transaction,
        })
    }

    pub fn write_text(
        &self,
        ref_name: &str,
        path: &str,
        content: &str,
        actor: Actor,
        message: Option<String>,
    ) -> Result<WriteResult> {
        let logical_path = normalize_logical_path(path)?;
        let blob = self.put_blob(
            content.as_bytes(),
            Some("text/plain; charset=utf-8"),
            Some(&logical_path),
        )?;
        let ref_record = self.update_ref_entry(
            ref_name,
            TreeEntry {
                path: logical_path.clone(),
                object_kind: ObjectKind::Blob,
                object_id: blob.hash.clone(),
                size: Some(blob.size),
                media_type: blob.media_type.clone(),
            },
        )?;
        let transaction = self.record_transaction(
            actor.clone(),
            message.or_else(|| Some(format!("write {logical_path}"))),
            vec![logical_path.clone()],
        )?;
        self.record_ref_snapshot(
            ref_name,
            &ref_record,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("write {logical_path}")),
        )?;
        self.record_event(
            "file_written",
            Some(actor),
            Some(format!("ref:{ref_name}:path:{logical_path}")),
            Some(transaction.id.clone()),
            serde_json::json!({
                "ref": ref_name,
                "path": logical_path,
                "blob": blob.hash
            }),
        )?;

        Ok(WriteResult {
            blob,
            ref_record,
            transaction,
        })
    }

    pub fn delete_path(&self, ref_name: &str, path: &str, actor: Actor) -> Result<DeleteResult> {
        if ref_name.starts_with("published/") && actor.kind == ActorKind::Agent {
            return Err(StoreError::PolicyDenied(format!(
                "agents cannot delete from published refs; create a draft for {path}"
            )));
        }

        let logical_path = normalize_logical_path(path)?;
        let mut record = self.get_ref(ref_name)?;
        let original_len = record.entries.len();
        record.entries.retain(|entry| entry.path != logical_path);
        if record.entries.len() == original_len {
            return Err(StoreError::NotFound(format!("{ref_name}:{logical_path}")));
        }

        let ref_record = self.replace_ref_entries(ref_name, record.entries)?;
        let transaction = self.record_transaction(
            actor.clone(),
            Some(format!("delete {logical_path}")),
            vec![logical_path.clone()],
        )?;
        self.record_ref_snapshot(
            ref_name,
            &ref_record,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("delete {logical_path}")),
        )?;
        self.record_event(
            "path_deleted",
            Some(actor),
            Some(format!("ref:{ref_name}:path:{logical_path}")),
            Some(transaction.id.clone()),
            serde_json::json!({ "ref": ref_name, "path": logical_path }),
        )?;

        Ok(DeleteResult {
            ref_record,
            transaction,
        })
    }

    pub fn read_text(&self, ref_name: &str, path: &str) -> Result<String> {
        let entry = self.tree_entry(ref_name, path)?;
        match entry.object_kind {
            ObjectKind::Blob => {
                let bytes = self.read_blob(&entry.object_id)?;
                String::from_utf8(bytes).map_err(StoreError::from)
            }
            ObjectKind::StructuredDoc => {
                let document = self.get_document(&entry.object_id)?;
                Ok(document_export_text(&document))
            }
            ObjectKind::BinaryObject => Err(StoreError::UnsupportedObject {
                path: entry.path,
                kind: entry.object_kind,
            }),
        }
    }

    pub fn tree_entry(&self, ref_name: &str, path: &str) -> Result<TreeEntry> {
        let logical_path = normalize_logical_path(path)?;
        let record = self.get_ref(ref_name)?;
        record
            .entries
            .into_iter()
            .find(|entry| entry.path == logical_path)
            .ok_or_else(|| StoreError::NotFound(format!("{ref_name}:{logical_path}")))
    }

    pub fn record_transaction(
        &self,
        actor: Actor,
        message: Option<String>,
        affected_paths: Vec<String>,
    ) -> Result<TransactionRecord> {
        let record = TransactionRecord {
            id: Uuid::new_v4().to_string(),
            actor,
            message,
            affected_paths,
            committed_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO transactions (id, actor_json, message, affected_paths_json, committed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                serde_json::to_string(&record.actor)?,
                record.message,
                serde_json::to_string(&record.affected_paths)?,
                record.committed_at
            ],
        )?;
        self.record_event(
            "transaction",
            Some(record.actor.clone()),
            None,
            Some(record.id.clone()),
            serde_json::json!({
                "message": record.message.clone(),
                "affected_paths": record.affected_paths.clone()
            }),
        )?;

        Ok(record)
    }

    pub fn list_transactions(&self, limit: u64) -> Result<Vec<TransactionRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, actor_json, message, affected_paths_json, committed_at
             FROM transactions
             ORDER BY committed_at DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], transaction_from_row)?;
        collect_rows(rows)
    }

    pub fn get_transaction(&self, id: &str) -> Result<TransactionRecord> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, actor_json, message, affected_paths_json, committed_at
             FROM transactions WHERE id = ?1",
            params![id],
            transaction_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("transaction {id}")))
    }

    pub fn record_event(
        &self,
        kind: &str,
        actor: Option<Actor>,
        target: Option<String>,
        transaction_id: Option<String>,
        payload: serde_json::Value,
    ) -> Result<EventRecord> {
        let record = EventRecord {
            id: Uuid::new_v4().to_string(),
            kind: kind.to_string(),
            actor,
            target,
            transaction_id,
            payload,
            created_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO events (id, kind, actor_json, target, transaction_id, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.kind,
                optional_json(&record.actor)?,
                record.target,
                record.transaction_id,
                serde_json::to_string(&record.payload)?,
                record.created_at
            ],
        )?;

        Ok(record)
    }

    pub fn list_events(&self, limit: u64, target: Option<&str>) -> Result<Vec<EventRecord>> {
        let conn = self.connect()?;
        if let Some(target) = target {
            let mut stmt = conn.prepare(
                "SELECT id, kind, actor_json, target, transaction_id, payload_json, created_at
                 FROM events
                 WHERE target = ?1
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![target, limit], event_from_row)?;
            collect_rows(rows)
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, kind, actor_json, target, transaction_id, payload_json, created_at
                 FROM events
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit], event_from_row)?;
            collect_rows(rows)
        }
    }

    pub fn record_ref_snapshot(
        &self,
        ref_name: &str,
        ref_record: &RefRecord,
        transaction: Option<&TransactionRecord>,
        actor: Option<Actor>,
        message: Option<String>,
    ) -> Result<RefSnapshotRecord> {
        let snapshot = RefSnapshotRecord {
            id: Uuid::new_v4().to_string(),
            ref_name: ref_name.to_string(),
            entries: ref_record.entries.clone(),
            transaction_id: transaction.map(|record| record.id.clone()),
            actor,
            message,
            created_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO ref_snapshots (id, ref_name, tree_json, transaction_id, actor_json, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                snapshot.id,
                snapshot.ref_name,
                serde_json::to_string(&snapshot.entries)?,
                snapshot.transaction_id,
                optional_json(&snapshot.actor)?,
                snapshot.message,
                snapshot.created_at
            ],
        )?;

        Ok(snapshot)
    }

    pub fn list_ref_snapshots(&self, ref_name: &str, limit: u64) -> Result<Vec<RefSnapshotRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, ref_name, tree_json, transaction_id, actor_json, message, created_at
             FROM ref_snapshots
             WHERE ref_name = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![ref_name, limit], ref_snapshot_from_row)?;
        collect_rows(rows)
    }

    pub fn restore_ref_snapshot(
        &self,
        ref_name: &str,
        snapshot_id: &str,
        actor: Actor,
    ) -> Result<RefRecord> {
        let snapshot = self.get_ref_snapshot(snapshot_id)?;
        if snapshot.ref_name != ref_name {
            return Err(StoreError::NotFound(format!(
                "snapshot {snapshot_id} for ref {ref_name}"
            )));
        }

        let restored = self.replace_ref_entries(ref_name, snapshot.entries.clone())?;
        let affected_paths = restored
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let transaction = self.record_transaction(
            actor.clone(),
            Some(format!("restore {ref_name} to snapshot {snapshot_id}")),
            affected_paths,
        )?;
        self.record_ref_snapshot(
            ref_name,
            &restored,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("restore snapshot {snapshot_id}")),
        )?;
        self.record_event(
            "ref_restored",
            Some(actor),
            Some(ref_name.to_string()),
            Some(transaction.id.clone()),
            serde_json::json!({ "ref": ref_name, "snapshot_id": snapshot_id }),
        )?;

        Ok(restored)
    }

    pub fn get_ref_snapshot(&self, id: &str) -> Result<RefSnapshotRecord> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, ref_name, tree_json, transaction_id, actor_json, message, created_at
             FROM ref_snapshots WHERE id = ?1",
            params![id],
            ref_snapshot_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("ref snapshot {id}")))
    }

    pub fn create_annotation(
        &self,
        target: &str,
        body: &str,
        actor: Actor,
    ) -> Result<AnnotationRecord> {
        let record = AnnotationRecord {
            id: Uuid::new_v4().to_string(),
            target: target.to_string(),
            body: body.to_string(),
            actor,
            resolved: false,
            created_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO annotations (id, target, body, actor_json, resolved, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.id,
                record.target,
                record.body,
                serde_json::to_string(&record.actor)?,
                record.resolved as i32,
                record.created_at
            ],
        )?;
        self.record_event(
            "annotation_created",
            Some(record.actor.clone()),
            Some(record.target.clone()),
            None,
            serde_json::json!({
                "annotation_id": record.id.clone(),
                "target": record.target.clone(),
                "resolved": record.resolved
            }),
        )?;

        Ok(record)
    }

    pub fn list_annotations(&self, target: Option<&str>) -> Result<Vec<AnnotationRecord>> {
        let conn = self.connect()?;
        if let Some(target) = target {
            let mut stmt = conn.prepare(
                "SELECT id, target, body, actor_json, resolved, created_at
                 FROM annotations WHERE target = ?1
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![target], annotation_from_row)?;
            collect_rows(rows)
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, target, body, actor_json, resolved, created_at
                 FROM annotations
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map([], annotation_from_row)?;
            collect_rows(rows)
        }
    }

    pub fn create_document(
        &self,
        ref_name: &str,
        path: &str,
        title: Option<&str>,
        snapshot_json: serde_json::Value,
        actor: Actor,
        message: Option<String>,
    ) -> Result<DocumentWriteResult> {
        let logical_path = normalize_logical_path(path)?;
        let title = title
            .map(str::to_string)
            .unwrap_or_else(|| title_from_path(&logical_path));
        let document = StructuredDocumentRecord {
            id: Uuid::new_v4().to_string(),
            ref_name: ref_name.to_string(),
            path: logical_path.clone(),
            title,
            snapshot_json,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO structured_documents (id, ref_name, path, title, snapshot_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                document.id,
                document.ref_name,
                document.path,
                document.title,
                serde_json::to_string(&document.snapshot_json)?,
                document.created_at,
                document.updated_at
            ],
        )?;

        let ref_record = self.update_ref_entry(
            ref_name,
            TreeEntry {
                path: logical_path.clone(),
                object_kind: ObjectKind::StructuredDoc,
                object_id: document.id.clone(),
                size: Some(serde_json::to_vec(&document.snapshot_json)?.len() as u64),
                media_type: Some("application/vnd.quarry.document+json".to_string()),
            },
        )?;
        let transaction = self.record_transaction(
            actor.clone(),
            message.or_else(|| Some(format!("create document {logical_path}"))),
            vec![logical_path.clone()],
        )?;
        self.record_ref_snapshot(
            ref_name,
            &ref_record,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("create document {logical_path}")),
        )?;
        let snapshot = self.record_document_snapshot(
            &document.id,
            &document.snapshot_json,
            Some(&transaction),
            Some(actor.clone()),
            Some("initial snapshot".to_string()),
        )?;
        self.record_event(
            "document_created",
            Some(actor),
            Some(format!("document:{}", document.id)),
            Some(transaction.id.clone()),
            serde_json::json!({
                "document_id": document.id.clone(),
                "ref": ref_name,
                "path": logical_path
            }),
        )?;

        Ok(DocumentWriteResult {
            document,
            ref_record,
            transaction,
            snapshot,
        })
    }

    pub fn get_document(&self, id: &str) -> Result<StructuredDocumentRecord> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, ref_name, path, title, snapshot_json, created_at, updated_at
             FROM structured_documents WHERE id = ?1",
            params![id],
            document_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("document {id}")))
    }

    pub fn document_state(&self, id: &str) -> Result<DocumentState> {
        Ok(DocumentState {
            document: self.get_document(id)?,
            snapshots: self.list_document_snapshots(id, 50)?,
            ops: self.list_document_ops(id, 100)?,
            presence: self.list_presence(id)?,
        })
    }

    pub fn append_document_op(
        &self,
        document_id: &str,
        op_json: serde_json::Value,
        actor: Actor,
    ) -> Result<DocumentWriteResult> {
        let mut document = self.get_document(document_id)?;
        let operation = DocumentOpRecord {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.to_string(),
            actor: actor.clone(),
            op_json: op_json.clone(),
            created_at: now_timestamp(),
        };

        let next_snapshot = apply_document_op(document.snapshot_json.clone(), &op_json)?;
        document.snapshot_json = next_snapshot;
        document.updated_at = now_timestamp();

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO document_ops (id, document_id, actor_json, op_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                operation.id,
                operation.document_id,
                serde_json::to_string(&operation.actor)?,
                serde_json::to_string(&operation.op_json)?,
                operation.created_at
            ],
        )?;
        conn.execute(
            "UPDATE structured_documents
             SET snapshot_json = ?2, updated_at = ?3
             WHERE id = ?1",
            params![
                document.id,
                serde_json::to_string(&document.snapshot_json)?,
                document.updated_at
            ],
        )?;

        let ref_record = self.update_ref_entry(
            &document.ref_name,
            TreeEntry {
                path: document.path.clone(),
                object_kind: ObjectKind::StructuredDoc,
                object_id: document.id.clone(),
                size: Some(serde_json::to_vec(&document.snapshot_json)?.len() as u64),
                media_type: Some("application/vnd.quarry.document+json".to_string()),
            },
        )?;
        let transaction = self.record_transaction(
            actor.clone(),
            Some(format!("document op {}", document.path)),
            vec![document.path.clone()],
        )?;
        self.record_ref_snapshot(
            &document.ref_name,
            &ref_record,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("document op {}", document.path)),
        )?;
        let snapshot = self.record_document_snapshot(
            &document.id,
            &document.snapshot_json,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("op {}", operation.id)),
        )?;
        self.record_event(
            "document_op",
            Some(actor),
            Some(format!("document:{}", document.id)),
            Some(transaction.id.clone()),
            serde_json::json!({
                "document_id": document.id.clone(),
                "op_id": operation.id.clone(),
                "path": document.path.clone()
            }),
        )?;

        Ok(DocumentWriteResult {
            document,
            ref_record,
            transaction,
            snapshot,
        })
    }

    pub fn record_document_snapshot(
        &self,
        document_id: &str,
        snapshot_json: &serde_json::Value,
        transaction: Option<&TransactionRecord>,
        actor: Option<Actor>,
        message: Option<String>,
    ) -> Result<DocumentSnapshotRecord> {
        let snapshot = DocumentSnapshotRecord {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.to_string(),
            snapshot_json: snapshot_json.clone(),
            transaction_id: transaction.map(|record| record.id.clone()),
            actor,
            message,
            created_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO document_snapshots (id, document_id, snapshot_json, transaction_id, actor_json, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                snapshot.id,
                snapshot.document_id,
                serde_json::to_string(&snapshot.snapshot_json)?,
                snapshot.transaction_id,
                optional_json(&snapshot.actor)?,
                snapshot.message,
                snapshot.created_at
            ],
        )?;

        Ok(snapshot)
    }

    pub fn list_document_snapshots(
        &self,
        document_id: &str,
        limit: u64,
    ) -> Result<Vec<DocumentSnapshotRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, document_id, snapshot_json, transaction_id, actor_json, message, created_at
             FROM document_snapshots
             WHERE document_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![document_id, limit], document_snapshot_from_row)?;
        collect_rows(rows)
    }

    pub fn list_document_ops(
        &self,
        document_id: &str,
        limit: u64,
    ) -> Result<Vec<DocumentOpRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, document_id, actor_json, op_json, created_at
             FROM document_ops
             WHERE document_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![document_id, limit], document_op_from_row)?;
        collect_rows(rows)
    }

    pub fn restore_document_snapshot(
        &self,
        document_id: &str,
        snapshot_id: &str,
        actor: Actor,
    ) -> Result<DocumentWriteResult> {
        let snapshot = self
            .list_document_snapshots(document_id, 1000)?
            .into_iter()
            .find(|snapshot| snapshot.id == snapshot_id)
            .ok_or_else(|| StoreError::NotFound(format!("document snapshot {snapshot_id}")))?;
        self.append_document_op(
            document_id,
            serde_json::json!({
                "kind": "replace_snapshot",
                "snapshot": snapshot.snapshot_json
            }),
            actor,
        )
    }

    pub fn upsert_presence(
        &self,
        document_id: &str,
        actor: Actor,
        cursor_json: serde_json::Value,
    ) -> Result<PresenceRecord> {
        self.get_document(document_id)?;
        let record = PresenceRecord {
            document_id: document_id.to_string(),
            actor,
            cursor_json,
            updated_at: now_timestamp(),
        };
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO document_presence (document_id, actor_id, actor_json, cursor_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(document_id, actor_id) DO UPDATE SET
                actor_json = excluded.actor_json,
                cursor_json = excluded.cursor_json,
                updated_at = excluded.updated_at",
            params![
                record.document_id,
                record.actor.id,
                serde_json::to_string(&record.actor)?,
                serde_json::to_string(&record.cursor_json)?,
                record.updated_at
            ],
        )?;
        self.record_event(
            "presence",
            Some(record.actor.clone()),
            Some(format!("document:{}", document_id)),
            None,
            serde_json::json!({
                "document_id": document_id,
                "cursor": record.cursor_json
            }),
        )?;
        Ok(record)
    }

    pub fn list_presence(&self, document_id: &str) -> Result<Vec<PresenceRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT document_id, actor_json, cursor_json, updated_at
             FROM document_presence
             WHERE document_id = ?1
             ORDER BY updated_at DESC, actor_id ASC",
        )?;
        let rows = stmt.query_map(params![document_id], presence_from_row)?;
        collect_rows(rows)
    }

    pub fn create_binary_object(
        &self,
        hash: &str,
        size: u64,
        media_type: Option<&str>,
        path: Option<&str>,
        external_url: Option<&str>,
    ) -> Result<BinaryObject> {
        let record = BinaryObject {
            id: Uuid::new_v4().to_string(),
            hash: hash.to_string(),
            size,
            media_type: media_type.map(str::to_string),
            path: path.map(str::to_string),
            external_url: external_url.map(str::to_string),
            created_at: now_timestamp(),
        };

        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO binary_objects (id, hash, size, media_type, path, external_url, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.hash,
                record.size,
                record.media_type,
                record.path,
                record.external_url,
                record.created_at
            ],
        )?;

        Ok(record)
    }

    pub fn add_binary_pointer(
        &self,
        ref_name: &str,
        logical_path: &str,
        hash: &str,
        size: u64,
        media_type: Option<&str>,
        source_path: Option<&str>,
        external_url: Option<&str>,
        actor: Actor,
    ) -> Result<BinaryPointerResult> {
        let logical_path = normalize_logical_path(logical_path)?;
        let binary =
            self.create_binary_object(hash, size, media_type, source_path, external_url)?;
        let ref_record = self.update_ref_entry(
            ref_name,
            TreeEntry {
                path: logical_path.clone(),
                object_kind: ObjectKind::BinaryObject,
                object_id: binary.id.clone(),
                size: Some(binary.size),
                media_type: binary.media_type.clone(),
            },
        )?;
        let transaction = self.record_transaction(
            actor.clone(),
            Some(format!("add binary pointer {logical_path}")),
            vec![logical_path.clone()],
        )?;
        self.record_ref_snapshot(
            ref_name,
            &ref_record,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("add binary pointer {logical_path}")),
        )?;
        self.record_event(
            "binary_pointer_added",
            Some(actor),
            Some(format!("ref:{ref_name}:path:{logical_path}")),
            Some(transaction.id.clone()),
            serde_json::json!({
                "ref": ref_name,
                "path": logical_path,
                "binary_id": binary.id.clone(),
                "hash": binary.hash.clone()
            }),
        )?;

        Ok(BinaryPointerResult {
            binary,
            ref_record,
            transaction,
        })
    }

    pub fn add_binary_file(
        &self,
        ref_name: &str,
        logical_path: &str,
        file_path: impl AsRef<Path>,
        media_type: Option<&str>,
        actor: Actor,
    ) -> Result<BinaryPointerResult> {
        let file_path = file_path.as_ref();
        let (hash, size) = hash_file(file_path)?;
        self.add_binary_pointer(
            ref_name,
            logical_path,
            &hash,
            size,
            media_type,
            Some(&file_path.display().to_string()),
            None,
            actor,
        )
    }

    pub fn get_binary_object(&self, id: &str) -> Result<BinaryObject> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, hash, size, media_type, path, external_url, created_at
             FROM binary_objects WHERE id = ?1",
            params![id],
            |row| {
                Ok(BinaryObject {
                    id: row.get(0)?,
                    hash: row.get(1)?,
                    size: row.get(2)?,
                    media_type: row.get(3)?,
                    path: row.get(4)?,
                    external_url: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("binary object {id}")))
    }

    pub fn read_binary_content(&self, id: &str) -> Result<Vec<u8>> {
        let binary = self.get_binary_object(id)?;
        let path = binary
            .path
            .ok_or_else(|| StoreError::NotFound(format!("local binary path for {id}")))?;
        let path = Path::new(&path);
        if !path.is_file() {
            return Err(StoreError::NotFound(format!(
                "local binary file {}",
                path.display()
            )));
        }
        fs::read(path).map_err(StoreError::from)
    }

    pub fn list_binary_objects(&self) -> Result<Vec<BinaryObject>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, hash, size, media_type, path, external_url, created_at
             FROM binary_objects
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], binary_from_row)?;
        collect_rows(rows)
    }

    pub fn export_ref(&self, ref_name: &str, out_dir: impl AsRef<Path>) -> Result<()> {
        let record = self.get_ref(ref_name)?;
        let out_dir = out_dir.as_ref();
        fs::create_dir_all(out_dir)?;

        for entry in record.entries {
            let output_path = safe_join(out_dir, &entry.path)?;
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)?;
            }
            match entry.object_kind {
                ObjectKind::Blob => fs::write(output_path, self.read_blob(&entry.object_id)?)?,
                ObjectKind::BinaryObject => {
                    let binary = self.get_binary_object(&entry.object_id)?;
                    fs::write(output_path, binary_pointer_text(&binary))?;
                }
                ObjectKind::StructuredDoc => {
                    let document = self.get_document(&entry.object_id)?;
                    fs::write(output_path, document_export_text(&document))?;
                }
            }
        }

        Ok(())
    }

    pub fn materialize_git(
        &self,
        ref_name: &str,
        repo_dir: impl AsRef<Path>,
        branch: &str,
        message: Option<&str>,
    ) -> Result<GitMaterializeResult> {
        let repo_dir = repo_dir.as_ref();
        fs::create_dir_all(repo_dir)?;

        if !repo_dir.join(".git").exists() {
            run_git(repo_dir, &["init"])?;
        }

        run_git(repo_dir, &["checkout", "-B", branch])?;
        clean_dir_except_git(repo_dir)?;
        self.export_ref(ref_name, repo_dir)?;
        self.write_git_metadata(ref_name, repo_dir)?;
        run_git(repo_dir, &["add", "-A"])?;

        let changed = git_has_staged_changes(repo_dir)?;
        let commit = if changed {
            let git_actor = self
                .latest_actor_for_ref(ref_name)?
                .unwrap_or_else(Actor::system);
            let git_user_name = format!("user.name={}", git_actor.display_name);
            let git_user_email = format!("user.email={}", actor_git_email(&git_actor));
            let message = message
                .map(str::to_string)
                .unwrap_or_else(|| format!("Quarry materialize {ref_name}"));
            run_git(
                repo_dir,
                &[
                    "-c",
                    &git_user_name,
                    "-c",
                    &git_user_email,
                    "commit",
                    "-m",
                    &message,
                ],
            )?;
            let commit = run_git(repo_dir, &["rev-parse", "HEAD"])?
                .trim()
                .to_string();
            self.record_event(
                "git_materialized",
                Some(git_actor),
                Some(ref_name.to_string()),
                None,
                serde_json::json!({
                    "ref": ref_name,
                    "repo_dir": repo_dir.display().to_string(),
                    "branch": branch,
                    "commit": commit.clone()
                }),
            )?;
            Some(commit)
        } else {
            None
        };

        Ok(GitMaterializeResult {
            ref_name: ref_name.to_string(),
            repo_dir: repo_dir.display().to_string(),
            branch: branch.to_string(),
            commit,
            changed,
        })
    }

    pub fn ingest_git(
        &self,
        repo_dir: impl AsRef<Path>,
        target_ref: &str,
        actor: Actor,
    ) -> Result<GitIngestResult> {
        let repo_dir = repo_dir.as_ref();
        let incoming_entries = self.entries_from_dir(repo_dir)?;
        let current = self.ensure_ref(target_ref)?;
        let mut conflicts = Vec::new();

        for incoming in &incoming_entries {
            if let Some(existing) = current
                .entries
                .iter()
                .find(|entry| entry.path == incoming.path)
            {
                if existing.object_id != incoming.object_id
                    || existing.object_kind != incoming.object_kind
                {
                    conflicts.push(incoming.path.clone());
                }
            }
        }

        let imported_paths = incoming_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();

        if conflicts.is_empty() {
            let mut merged_entries = current.entries.clone();
            for incoming in incoming_entries {
                merged_entries.retain(|entry| entry.path != incoming.path);
                merged_entries.push(incoming);
            }
            let ref_record = self.replace_ref_entries(target_ref, merged_entries)?;
            let transaction = self.record_transaction(
                actor.clone(),
                Some(format!("git ingest into {target_ref}")),
                imported_paths.clone(),
            )?;
            self.record_ref_snapshot(
                target_ref,
                &ref_record,
                Some(&transaction),
                Some(actor.clone()),
                Some("git ingest".to_string()),
            )?;
            self.record_event(
                "git_ingested",
                Some(actor),
                Some(target_ref.to_string()),
                Some(transaction.id.clone()),
                serde_json::json!({
                    "target_ref": target_ref,
                    "imported_paths": imported_paths.clone()
                }),
            )?;
            return Ok(GitIngestResult {
                target_ref: target_ref.to_string(),
                imported_paths,
                conflict_ref: None,
                conflicts,
                transaction,
            });
        }

        let conflict_ref = format!("draft/conflict-{}", Uuid::new_v4());
        let mut conflict_entries = current.entries.clone();
        let conflict_id = conflict_ref.replace('/', "-");

        for incoming in incoming_entries {
            if conflicts.iter().any(|path| path == &incoming.path) {
                if let Some(existing) = current
                    .entries
                    .iter()
                    .find(|entry| entry.path == incoming.path)
                {
                    let local_path = format!("conflicts/{conflict_id}/local/{}", existing.path);
                    conflict_entries.push(TreeEntry {
                        path: local_path,
                        object_kind: existing.object_kind.clone(),
                        object_id: existing.object_id.clone(),
                        size: existing.size,
                        media_type: existing.media_type.clone(),
                    });
                }

                let incoming_path = format!("conflicts/{conflict_id}/incoming/{}", incoming.path);
                conflict_entries.push(TreeEntry {
                    path: incoming_path,
                    object_kind: incoming.object_kind,
                    object_id: incoming.object_id,
                    size: incoming.size,
                    media_type: incoming.media_type,
                });
            } else {
                conflict_entries.retain(|entry| entry.path != incoming.path);
                conflict_entries.push(incoming);
            }
        }

        let ref_record = self.replace_ref_entries(&conflict_ref, conflict_entries)?;
        let transaction = self.record_transaction(
            actor.clone(),
            Some(format!("git ingest conflict draft {conflict_ref}")),
            imported_paths.clone(),
        )?;
        self.record_ref_snapshot(
            &conflict_ref,
            &ref_record,
            Some(&transaction),
            Some(actor.clone()),
            Some(format!("git conflict from {target_ref}")),
        )?;
        self.record_event(
            "git_conflict",
            Some(actor),
            Some(conflict_ref.clone()),
            Some(transaction.id.clone()),
            serde_json::json!({
                "target_ref": target_ref,
                "conflict_ref": conflict_ref.clone(),
                "conflicts": conflicts.clone(),
                "imported_paths": imported_paths.clone()
            }),
        )?;

        Ok(GitIngestResult {
            target_ref: target_ref.to_string(),
            imported_paths,
            conflict_ref: Some(conflict_ref),
            conflicts,
            transaction,
        })
    }

    fn entries_from_dir(&self, dir: &Path) -> Result<Vec<TreeEntry>> {
        let mut entries = Vec::new();
        collect_file_paths(dir, dir, &mut entries, self)?;
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn latest_actor_for_ref(&self, ref_name: &str) -> Result<Option<Actor>> {
        let ref_paths = self
            .get_ref(ref_name)?
            .entries
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>();

        for transaction in self.list_transactions(200)? {
            if transaction.affected_paths.is_empty()
                || transaction
                    .affected_paths
                    .iter()
                    .any(|path| ref_paths.iter().any(|ref_path| ref_path == path))
            {
                return Ok(Some(transaction.actor));
            }
        }

        Ok(None)
    }

    fn write_git_metadata(&self, ref_name: &str, repo_dir: &Path) -> Result<()> {
        let metadata_dir = repo_dir.join(".quarry");
        fs::create_dir_all(&metadata_dir)?;
        fs::write(
            metadata_dir.join("ref.json"),
            serde_json::to_vec_pretty(&self.get_ref(ref_name)?)?,
        )?;
        fs::write(
            metadata_dir.join("transactions.json"),
            serde_json::to_vec_pretty(&self.list_transactions(500)?)?,
        )?;
        fs::write(
            metadata_dir.join("annotations.json"),
            serde_json::to_vec_pretty(&self.list_annotations(None)?)?,
        )?;
        fs::write(
            metadata_dir.join("events.json"),
            serde_json::to_vec_pretty(&self.list_events(500, None)?)?,
        )?;
        fs::write(
            metadata_dir.join("snapshots.json"),
            serde_json::to_vec_pretty(&self.list_ref_snapshots(ref_name, 500)?)?,
        )?;
        Ok(())
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS blobs (
            hash TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            media_type TEXT,
            path TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS binary_objects (
            id TEXT PRIMARY KEY,
            hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            media_type TEXT,
            path TEXT,
            external_url TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS refs (
            name TEXT PRIMARY KEY,
            tree_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS transactions (
            id TEXT PRIMARY KEY,
            actor_json TEXT NOT NULL,
            message TEXT,
            affected_paths_json TEXT NOT NULL,
            committed_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS annotations (
            id TEXT PRIMARY KEY,
            target TEXT NOT NULL,
            body TEXT NOT NULL,
            actor_json TEXT NOT NULL,
            resolved INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            actor_json TEXT,
            target TEXT,
            transaction_id TEXT,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ref_snapshots (
            id TEXT PRIMARY KEY,
            ref_name TEXT NOT NULL,
            tree_json TEXT NOT NULL,
            transaction_id TEXT,
            actor_json TEXT,
            message TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS structured_documents (
            id TEXT PRIMARY KEY,
            ref_name TEXT NOT NULL,
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            snapshot_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS document_snapshots (
            id TEXT PRIMARY KEY,
            document_id TEXT NOT NULL,
            snapshot_json TEXT NOT NULL,
            transaction_id TEXT,
            actor_json TEXT,
            message TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS document_ops (
            id TEXT PRIMARY KEY,
            document_id TEXT NOT NULL,
            actor_json TEXT NOT NULL,
            op_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS document_presence (
            document_id TEXT NOT NULL,
            actor_id TEXT NOT NULL,
            actor_json TEXT NOT NULL,
            cursor_json TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (document_id, actor_id)
        );

        CREATE INDEX IF NOT EXISTS idx_events_created_at ON events(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_events_target ON events(target);
        CREATE INDEX IF NOT EXISTS idx_ref_snapshots_ref ON ref_snapshots(ref_name, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_document_snapshots_doc ON document_snapshots(document_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_document_ops_doc ON document_ops(document_id, created_at DESC);
        ",
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
        params![SCHEMA_VERSION, now_timestamp()],
    )?;
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

fn ensure_workspace_id(conn: &Connection) -> Result<()> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'workspace_id'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if existing.is_none() {
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('workspace_id', ?1)",
            params![Uuid::new_v4().to_string()],
        )?;
    }

    Ok(())
}

fn ref_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RefRecord> {
    let tree_json: String = row.get(1)?;
    let entries = serde_json::from_str(&tree_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;

    Ok(RefRecord {
        name: row.get(0)?,
        entries,
        updated_at: row.get(2)?,
    })
}

fn transaction_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TransactionRecord> {
    let actor_json: String = row.get(1)?;
    let paths_json: String = row.get(3)?;
    let actor = serde_json::from_str(&actor_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let affected_paths = serde_json::from_str(&paths_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
    })?;

    Ok(TransactionRecord {
        id: row.get(0)?,
        actor,
        message: row.get(2)?,
        affected_paths,
        committed_at: row.get(4)?,
    })
}

fn annotation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AnnotationRecord> {
    let actor_json: String = row.get(3)?;
    let actor = serde_json::from_str(&actor_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let resolved: i32 = row.get(4)?;

    Ok(AnnotationRecord {
        id: row.get(0)?,
        target: row.get(1)?,
        body: row.get(2)?,
        actor,
        resolved: resolved != 0,
        created_at: row.get(5)?,
    })
}

fn binary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BinaryObject> {
    Ok(BinaryObject {
        id: row.get(0)?,
        hash: row.get(1)?,
        size: row.get(2)?,
        media_type: row.get(3)?,
        path: row.get(4)?,
        external_url: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    let actor_json: Option<String> = row.get(2)?;
    let payload_json: String = row.get(5)?;
    Ok(EventRecord {
        id: row.get(0)?,
        kind: row.get(1)?,
        actor: optional_from_json(actor_json, 2)?,
        target: row.get(3)?,
        transaction_id: row.get(4)?,
        payload: json_from_col(&payload_json, 5)?,
        created_at: row.get(6)?,
    })
}

fn ref_snapshot_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RefSnapshotRecord> {
    let tree_json: String = row.get(2)?;
    let actor_json: Option<String> = row.get(4)?;
    Ok(RefSnapshotRecord {
        id: row.get(0)?,
        ref_name: row.get(1)?,
        entries: json_from_col(&tree_json, 2)?,
        transaction_id: row.get(3)?,
        actor: optional_from_json(actor_json, 4)?,
        message: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn document_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StructuredDocumentRecord> {
    let snapshot_json: String = row.get(4)?;
    Ok(StructuredDocumentRecord {
        id: row.get(0)?,
        ref_name: row.get(1)?,
        path: row.get(2)?,
        title: row.get(3)?,
        snapshot_json: json_from_col(&snapshot_json, 4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn document_snapshot_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentSnapshotRecord> {
    let snapshot_json: String = row.get(2)?;
    let actor_json: Option<String> = row.get(4)?;
    Ok(DocumentSnapshotRecord {
        id: row.get(0)?,
        document_id: row.get(1)?,
        snapshot_json: json_from_col(&snapshot_json, 2)?,
        transaction_id: row.get(3)?,
        actor: optional_from_json(actor_json, 4)?,
        message: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn document_op_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentOpRecord> {
    let actor_json: String = row.get(2)?;
    let op_json: String = row.get(3)?;
    Ok(DocumentOpRecord {
        id: row.get(0)?,
        document_id: row.get(1)?,
        actor: json_from_col(&actor_json, 2)?,
        op_json: json_from_col(&op_json, 3)?,
        created_at: row.get(4)?,
    })
}

fn presence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PresenceRecord> {
    let actor_json: String = row.get(1)?;
    let cursor_json: String = row.get(2)?;
    Ok(PresenceRecord {
        document_id: row.get(0)?,
        actor: json_from_col(&actor_json, 1)?,
        cursor_json: json_from_col(&cursor_json, 2)?,
        updated_at: row.get(3)?,
    })
}

fn optional_json<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(StoreError::from)
}

fn json_from_col<T: for<'de> Deserialize<'de>>(value: &str, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })
}

fn optional_from_json<T: for<'de> Deserialize<'de>>(
    value: Option<String>,
    column: usize,
) -> rusqlite::Result<Option<T>> {
    value.map(|json| json_from_col(&json, column)).transpose()
}

fn collect_rows<T, I>(rows: I) -> Result<Vec<T>>
where
    I: IntoIterator<Item = rusqlite::Result<T>>,
{
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

fn count_rows(conn: &Connection, table: &str) -> Result<u64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get::<_, u64>(0))
        .map_err(StoreError::from)
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 8192];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }

    Ok((hex::encode(hasher.finalize()), size))
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn apply_document_op(
    current: serde_json::Value,
    op: &serde_json::Value,
) -> Result<serde_json::Value> {
    match op.get("kind").and_then(|value| value.as_str()) {
        Some("replace_snapshot") | Some("set_snapshot") => op
            .get("snapshot")
            .cloned()
            .ok_or_else(|| StoreError::InvalidPath("document op missing snapshot".to_string())),
        Some("replace_text") => {
            let text = op
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            Ok(snapshot_from_text(text))
        }
        Some("set_title") => {
            let mut next = current;
            if let Some(title) = op.get("title").and_then(|value| value.as_str()) {
                if let Some(object) = next.as_object_mut() {
                    object.insert(
                        "title".to_string(),
                        serde_json::Value::String(title.to_string()),
                    );
                }
            }
            Ok(next)
        }
        _ => {
            if let Some(snapshot) = op.get("snapshot") {
                Ok(snapshot.clone())
            } else {
                let mut next = current;
                if let Some(object) = next.as_object_mut() {
                    object.insert("_last_op".to_string(), op.clone());
                }
                Ok(next)
            }
        }
    }
}

fn snapshot_from_text(text: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": "quarry.structured_doc.v1",
        "format": "plain_text",
        "text": text,
        "blocks": [
            {
                "type": "p",
                "children": [{ "text": text }]
            }
        ]
    })
}

fn document_export_text(document: &StructuredDocumentRecord) -> String {
    snapshot_text(&document.snapshot_json).unwrap_or_else(|| {
        serde_json::to_string_pretty(&document.snapshot_json).unwrap_or_default()
    })
}

fn snapshot_text(snapshot: &serde_json::Value) -> Option<String> {
    if let Some(text) = snapshot.get("text").and_then(|value| value.as_str()) {
        return Some(text.to_string());
    }

    let blocks = snapshot.get("blocks")?.as_array()?;
    let mut lines = Vec::new();
    for block in blocks {
        let mut line = String::new();
        if let Some(children) = block.get("children").and_then(|value| value.as_array()) {
            for child in children {
                if let Some(text) = child.get("text").and_then(|value| value.as_str()) {
                    line.push_str(text);
                }
            }
        }
        lines.push(line);
    }
    Some(lines.join("\n"))
}

fn title_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|name| name.to_string_lossy().replace('-', " ").replace('_', " "))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Untitled".to_string())
}

fn actor_git_email(actor: &Actor) -> String {
    if actor.id.contains('@') {
        actor.id.clone()
    } else {
        let safe_id = actor
            .id
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '-'
                }
            })
            .collect::<String>();
        format!("{safe_id}@quarry.local")
    }
}

fn normalize_logical_path(path: &str) -> Result<String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() {
        return Err(StoreError::InvalidPath("empty path".to_string()));
    }

    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(StoreError::InvalidPath(path.display().to_string()));
            }
        }
    }

    if parts.is_empty() {
        return Err(StoreError::InvalidPath(path.display().to_string()));
    }

    Ok(parts.join("/"))
}

fn safe_join(base: &Path, logical_path: &str) -> Result<PathBuf> {
    let normalized = normalize_logical_path(logical_path)?;
    Ok(base.join(normalized))
}

fn collect_file_paths(
    base: &Path,
    current: &Path,
    entries: &mut Vec<TreeEntry>,
    store: &LocalStore,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        if file_name == ".git" || file_name == ".quarry" {
            continue;
        }

        if path.is_dir() {
            collect_file_paths(base, &path, entries, store)?;
            continue;
        }

        if !path.is_file() {
            continue;
        }

        let relative = path
            .strip_prefix(base)
            .map_err(|_| StoreError::InvalidPath(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let logical_path = normalize_logical_path(&relative)?;
        let bytes = fs::read(&path)?;

        if std::str::from_utf8(&bytes).is_ok() {
            let blob = store.put_blob(
                &bytes,
                Some("text/plain; charset=utf-8"),
                Some(&logical_path),
            )?;
            entries.push(TreeEntry {
                path: logical_path,
                object_kind: ObjectKind::Blob,
                object_id: blob.hash,
                size: Some(blob.size),
                media_type: blob.media_type,
            });
        } else {
            let hash = hash_bytes(&bytes);
            let binary = store.create_binary_object(
                &hash,
                bytes.len() as u64,
                Some("application/octet-stream"),
                Some(&path.display().to_string()),
                None,
            )?;
            entries.push(TreeEntry {
                path: logical_path,
                object_kind: ObjectKind::BinaryObject,
                object_id: binary.id,
                size: Some(binary.size),
                media_type: binary.media_type,
            });
        }
    }

    Ok(())
}

fn binary_pointer_text(binary: &BinaryObject) -> String {
    let mut pointer = format!(
        "version https://ai-quarry.local/binary-pointer/v1\nhash sha256:{}\nsize {}\n",
        binary.hash, binary.size
    );

    if let Some(media_type) = &binary.media_type {
        pointer.push_str(&format!("media-type {media_type}\n"));
    }
    if let Some(path) = &binary.path {
        pointer.push_str(&format!("path {path}\n"));
    }
    if let Some(url) = &binary.external_url {
        pointer.push_str(&format!("url {url}\n"));
    }

    pointer
}

fn clean_dir_except_git(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
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

fn run_git(repo_dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()?;

    if !output.status.success() {
        return Err(StoreError::CommandFailed {
            program: "git".to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    String::from_utf8(output.stdout).map_err(StoreError::from)
}

fn git_has_staged_changes(repo_dir: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(repo_dir)
        .output()?;

    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(StoreError::CommandFailed {
            program: "git".to_string(),
            args: vec![
                "diff".to_string(),
                "--cached".to_string(),
                "--quiet".to_string(),
            ],
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_and_reopen_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        let actor = Actor::local_human("navan");

        let written = store
            .write_text(
                "published/main",
                "docs/hello.md",
                "# Hello\n",
                actor.clone(),
                None,
            )
            .unwrap();

        assert_eq!(written.blob.size, 8);
        assert_eq!(
            store.read_text("published/main", "docs/hello.md").unwrap(),
            "# Hello\n"
        );
        assert_eq!(store.status().unwrap().transaction_count, 1);

        let reopened = LocalStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened
                .read_text("published/main", "docs/hello.md")
                .unwrap(),
            "# Hello\n"
        );
        assert_eq!(reopened.list_refs().unwrap()[0].name, "published/main");
    }

    #[test]
    fn rejects_paths_that_escape_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        let err = store
            .write_text(
                "published/main",
                "../escape.txt",
                "bad",
                Actor::system(),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::InvalidPath(_)));
    }

    #[test]
    fn annotations_are_targeted_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        let actor = Actor::local_human("reviewer");

        let annotation = store
            .create_annotation(
                "ref:published/main:path:docs/hello.md",
                "tighten this",
                actor,
            )
            .unwrap();

        let annotations = LocalStore::open(dir.path())
            .unwrap()
            .list_annotations(Some(&annotation.target))
            .unwrap();
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].body, "tighten this");
    }

    #[test]
    fn binary_objects_are_pointer_records_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        let binary = store
            .create_binary_object(
                "abc123",
                42,
                Some("application/pdf"),
                Some("designs/spec.pdf"),
                None,
            )
            .unwrap();

        let loaded = store.get_binary_object(&binary.id).unwrap();
        assert_eq!(loaded.hash, "abc123");
        assert_eq!(loaded.size, 42);
    }

    #[test]
    fn draft_publish_updates_target_ref() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        store
            .write_text(
                "published/main",
                "docs/spec.md",
                "before",
                Actor::local_human("navan"),
                None,
            )
            .unwrap();

        let draft = store
            .create_draft(
                "published/main",
                Some("draft/review"),
                Actor::local_human("navan"),
            )
            .unwrap();
        assert_eq!(draft.draft_ref.name, "draft/review");

        store
            .write_text(
                "draft/review",
                "docs/spec.md",
                "after",
                Actor::local_human("navan"),
                None,
            )
            .unwrap();
        store
            .publish_ref(
                "draft/review",
                "published/main",
                Actor::local_human("navan"),
            )
            .unwrap();

        assert_eq!(
            store.read_text("published/main", "docs/spec.md").unwrap(),
            "after"
        );
    }

    #[test]
    fn events_snapshots_and_restore_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        let actor = Actor::local_human("navan");

        store
            .write_text("published/main", "docs/spec.md", "one", actor.clone(), None)
            .unwrap();
        store
            .write_text("published/main", "docs/spec.md", "two", actor.clone(), None)
            .unwrap();

        let snapshots = store.list_ref_snapshots("published/main", 10).unwrap();
        assert!(snapshots.len() >= 2);
        let first_snapshot = snapshots
            .iter()
            .find(|snapshot| {
                snapshot.entries.iter().any(|entry| {
                    entry.path == "docs/spec.md"
                        && entry.object_kind == ObjectKind::Blob
                        && store
                            .read_blob(&entry.object_id)
                            .map(|bytes| String::from_utf8(bytes).unwrap_or_default() == "one")
                            .unwrap_or(false)
                })
            })
            .unwrap()
            .id
            .clone();
        store
            .restore_ref_snapshot("published/main", &first_snapshot, actor)
            .unwrap();

        assert_eq!(
            store.read_text("published/main", "docs/spec.md").unwrap(),
            "one"
        );
        assert!(store.list_events(20, None).unwrap().len() >= 5);
    }

    #[test]
    fn structured_documents_have_ops_presence_snapshots_and_export() {
        let dir = tempfile::tempdir().unwrap();
        let export = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        let actor = Actor::local_human("navan");

        let created = store
            .create_document(
                "published/main",
                "docs/rich.md",
                Some("Rich"),
                snapshot_from_text("hello"),
                actor.clone(),
                None,
            )
            .unwrap();
        let document_id = created.document.id.clone();

        store
            .append_document_op(
                &document_id,
                serde_json::json!({ "kind": "replace_text", "text": "updated" }),
                actor.clone(),
            )
            .unwrap();
        store
            .upsert_presence(
                &document_id,
                actor,
                serde_json::json!({ "path": [0, 0], "offset": 3 }),
            )
            .unwrap();

        let state = store.document_state(&document_id).unwrap();
        assert_eq!(
            snapshot_text(&state.document.snapshot_json).unwrap(),
            "updated"
        );
        assert!(!state.snapshots.is_empty());
        assert_eq!(state.presence.len(), 1);

        store.export_ref("published/main", export.path()).unwrap();
        assert_eq!(
            fs::read_to_string(export.path().join("docs/rich.md")).unwrap(),
            "updated"
        );
    }

    #[test]
    fn binary_file_content_passthrough_reads_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let binary_file = dir.path().join("mock.bin");
        fs::write(&binary_file, [1, 2, 3, 4]).unwrap();
        let store = LocalStore::open(dir.path().join("data")).unwrap();

        let result = store
            .add_binary_file(
                "published/main",
                "assets/mock.bin",
                &binary_file,
                Some("application/octet-stream"),
                Actor::local_human("navan"),
            )
            .unwrap();

        assert_eq!(
            store.read_binary_content(&result.binary.id).unwrap(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn agent_cannot_delete_from_published_ref() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        store
            .write_text(
                "published/main",
                "src/lib.rs",
                "fn main() {}",
                Actor::local_human("navan"),
                None,
            )
            .unwrap();

        let err = store
            .delete_path(
                "published/main",
                "src/lib.rs",
                Actor {
                    id: "codex".to_string(),
                    display_name: "codex".to_string(),
                    kind: ActorKind::Agent,
                    avatar_url: None,
                },
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::PolicyDenied(_)));
    }

    #[test]
    fn git_materialize_creates_repo_commit_and_pointer_file() {
        if !git_available() {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let binary_file = dir.path().join("mock.pdf");
        fs::write(&binary_file, [0, 159, 146, 150]).unwrap();
        let store = LocalStore::open(dir.path().join("data")).unwrap();
        store
            .write_text(
                "published/main",
                "README.md",
                "# Project\n",
                Actor::local_human("navan"),
                None,
            )
            .unwrap();
        store
            .add_binary_file(
                "published/main",
                "assets/mock.pdf",
                &binary_file,
                Some("application/pdf"),
                Actor::local_human("navan"),
            )
            .unwrap();

        let result = store
            .materialize_git("published/main", repo.path(), "main", None)
            .unwrap();

        assert!(result.changed);
        assert!(result.commit.is_some());
        assert_eq!(
            fs::read_to_string(repo.path().join("README.md")).unwrap(),
            "# Project\n"
        );
        let pointer = fs::read_to_string(repo.path().join("assets/mock.pdf")).unwrap();
        assert!(pointer.contains("binary-pointer/v1"));
        assert!(repo.path().join(".quarry/ref.json").exists());
        let author = run_git(repo.path(), &["log", "-1", "--format=%an <%ae>"]).unwrap();
        assert_eq!(author.trim(), "navan <navan@quarry.local>");
    }

    #[test]
    fn git_ingest_preserves_both_sides_in_conflict_draft() {
        let dir = tempfile::tempdir().unwrap();
        let incoming = tempfile::tempdir().unwrap();
        let store = LocalStore::open(dir.path()).unwrap();
        store
            .write_text(
                "published/main",
                "src/app.rs",
                "local",
                Actor::local_human("navan"),
                None,
            )
            .unwrap();
        fs::create_dir_all(incoming.path().join("src")).unwrap();
        fs::write(incoming.path().join("src/app.rs"), "incoming").unwrap();

        let result = store
            .ingest_git(
                incoming.path(),
                "published/main",
                Actor {
                    id: "git".to_string(),
                    display_name: "Git".to_string(),
                    kind: ActorKind::GitImport,
                    avatar_url: None,
                },
            )
            .unwrap();

        let conflict_ref = result.conflict_ref.unwrap();
        assert_eq!(result.conflicts, vec!["src/app.rs"]);
        assert_eq!(
            store.read_text("published/main", "src/app.rs").unwrap(),
            "local"
        );
        let conflict = store.get_ref(&conflict_ref).unwrap();
        assert!(conflict
            .entries
            .iter()
            .any(|entry| entry.path.ends_with("incoming/src/app.rs")));
        assert!(conflict
            .entries
            .iter()
            .any(|entry| entry.path.ends_with("local/src/app.rs")));
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}
