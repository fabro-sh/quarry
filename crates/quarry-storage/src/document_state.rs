//! Canonical Automerge bytes and command receipts share the document's SQL
//! transaction. Projections may be rebuilt; the native text identity may not.
use crate::{
    BlockRow, DocumentScopeRef, QuarryStore, map_turso_error,
    row::{opt_blob, text},
};
use quarry_core::{QuarryError, Result};
use quarry_document::Document;
use turso::{Connection, params};

pub(crate) const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS document_states(
  document_id TEXT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
  version_id TEXT NOT NULL REFERENCES document_versions(id),
  schema_version INTEGER NOT NULL,
  state BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS document_command_receipts(
  document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  client_tx_id TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  PRIMARY KEY(document_id, client_tx_id)
);
CREATE TABLE IF NOT EXISTS document_state_versions(
  document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  version_id TEXT NOT NULL REFERENCES document_versions(id) ON DELETE CASCADE,
  heads TEXT NOT NULL,
  PRIMARY KEY(document_id,version_id)
);
";

#[derive(Clone, Debug)]
pub struct DurableDocumentState {
    pub document_id: String,
    pub version_id: String,
    pub bytes: Vec<u8>,
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct DurableDocumentCommit {
    pub bytes: Vec<u8>,
    /// Hash of the complete caller request, including actor and base version.
    pub request_hash: String,
}

pub(crate) fn document_error(error: impl std::fmt::Display) -> QuarryError {
    QuarryError::InvalidInput(format!("Invalid Automerge document: {error}"))
}

/// Committed Markdown and review rows can be imported without the retired
/// engine. Unpublished binary drafts cannot. Refuse before changing schema
/// instead of deleting edits that the user has not saved yet.
pub(crate) async fn check_upgrade_drafts(conn: &Connection) -> Result<()> {
    let mut tables = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='collab_recovery_states'",
            (),
        )
        .await
        .map_err(map_turso_error)?;
    if tables.next().await.map_err(map_turso_error)?.is_none() {
        return Ok(());
    }
    let mut drafts = conn
        .query(
            "SELECT 1 FROM collab_recovery_states WHERE dirty != 0 LIMIT 1",
            (),
        )
        .await
        .map_err(map_turso_error)?;
    if drafts.next().await.map_err(map_turso_error)?.is_some() {
        return Err(QuarryError::Conflict("This database has unpublished drafts from an older Quarry version. Open that version and save the drafts before upgrading. The database has not been migrated.".into()));
    }
    Ok(())
}

pub fn document_projection(document: &Document) -> Result<Vec<BlockRow>> {
    Ok(quarry_markdown::document_to_block_rows(document)?)
}

pub(crate) fn project_block_views(views: Vec<quarry_document::BlockView>) -> Result<Vec<BlockRow>> {
    Ok(quarry_markdown::block_views_to_rows(views)?)
}

impl QuarryStore {
    pub async fn replay_durable_request_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
        request: &str,
        hash: &str,
    ) -> Result<Option<crate::BlockTransactionRecord>> {
        let document = self.head_document_for_scope(scope, path).await?;
        let conn = self.conn()?;
        let record = crate::blocks::block_transaction_conn(&conn, &document.id, request).await?;
        if record.is_some()
            && receipt_hash_conn(&conn, &document.id, request)
                .await?
                .as_deref()
                != Some(hash)
        {
            return Err(QuarryError::PreconditionFailed(
                "Request ID was already used with different content".into(),
            ));
        }
        Ok(record)
    }

    pub async fn durable_heads_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
        version: &str,
    ) -> Result<Option<Vec<String>>> {
        let document = self.head_document_for_scope(scope, path).await?;
        let mut rows = self
            .conn()?
            .query(
                "SELECT heads FROM document_state_versions WHERE document_id=?1 AND version_id=?2",
                params![document.id.to_string(), version],
            )
            .await
            .map_err(map_turso_error)?;
        match rows.next().await.map_err(map_turso_error)? {
            None => Ok(None),
            Some(row) => Ok(Some(
                serde_json::from_str(&text(&row, 0)?).map_err(document_error)?,
            )),
        }
    }

    pub async fn durable_document_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
    ) -> Result<Option<DurableDocumentState>> {
        let conn = self.conn()?;
        conn.execute("BEGIN", ()).await.map_err(map_turso_error)?;
        let result = async {
            // Scope, version, metadata and native bytes belong to one snapshot.
            let document = self
                .head_document_for_scope_conn(&conn, scope, path)
                .await?;
            let state = load_state_conn(&conn, &document.id).await?;
            if state
                .as_ref()
                .is_some_and(|s| s.version_id != document.head_version_id.as_str())
            {
                return Err(QuarryError::Invariant(
                    "Native state and published version disagree".into(),
                ));
            }
            Ok(state)
        }
        .await;
        crate::finish_tx(&conn, result).await
    }
}

pub(crate) async fn load_state_conn(
    conn: &Connection,
    document_id: &str,
) -> Result<Option<DurableDocumentState>> {
    let mut rows = conn
        .query(
            "SELECT s.document_id, s.version_id, s.state, v.metadata_json FROM document_states s JOIN document_versions v ON v.id=s.version_id WHERE s.document_id = ?1",
            params![document_id],
        )
        .await
        .map_err(map_turso_error)?;
    match rows.next().await.map_err(map_turso_error)? {
        None => Ok(None),
        Some(row) => Ok(Some(DurableDocumentState {
            document_id: text(&row, 0)?,
            version_id: text(&row, 1)?,
            bytes: opt_blob(&row, 2)?
                .ok_or_else(|| QuarryError::Invariant("Missing canonical document bytes".into()))?,
            metadata: serde_json::from_str(&text(&row, 3)?).map_err(document_error)?,
        })),
    }
}

pub(crate) async fn clone_state_conn(
    conn: &Connection,
    source: &str,
    target: &str,
    version: &str,
) -> Result<bool> {
    let Some(state) = load_state_conn(conn, source).await? else {
        return Ok(false);
    };
    let copy = Document::load(&state.bytes)
        .map_err(document_error)?
        .copy_as(target)
        .map_err(document_error)?;
    crate::blocks::replace_block_rows_conn(conn, target, &document_projection(&copy)?).await?;
    crate::blocks::replace_block_review_items_conn(
        conn,
        target,
        &crate::document_review_projection(&copy)?,
    )
    .await?;
    conn.execute("INSERT INTO document_states(document_id, version_id, schema_version, state) VALUES(?1,?2,?3,?4)",
        params![target, version, 1_i64, copy.save()]).await.map_err(map_turso_error)?;
    let heads: Vec<String> = copy.heads().iter().map(ToString::to_string).collect();
    conn.execute(
        "INSERT INTO document_state_versions(document_id, version_id, heads) VALUES(?1,?2,?3)",
        params![
            target,
            version,
            serde_json::to_string(&heads).map_err(document_error)?
        ],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(true)
}

pub(crate) async fn receipt_hash_conn(
    conn: &Connection,
    document_id: &str,
    request: &str,
) -> Result<Option<String>> {
    let mut rows=conn.query("SELECT request_hash FROM document_command_receipts WHERE document_id=?1 AND client_tx_id=?2",params![document_id,request]).await.map_err(map_turso_error)?;
    rows.next()
        .await
        .map_err(map_turso_error)?
        .map(|row| text(&row, 0))
        .transpose()
}

/// Native IDs are local to a document. These compatibility tables must not
/// couple unrelated documents through a global block or review primary key.
pub(crate) async fn migrate_projection_keys(conn: &Connection) -> Result<()> {
    let mut migrate = Vec::new();
    for table in ["blocks", "block_review_items"] {
        let mut rows = conn
            .query(&format!("PRAGMA table_info({table})"), ())
            .await
            .map_err(map_turso_error)?;
        let mut scoped = false;
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            if text(&row, 1)? == "document_id" && row.get::<i64>(5).map_err(map_turso_error)? > 0 {
                scoped = true;
            }
        }
        if !scoped {
            migrate.push(table);
        }
    }
    if migrate.is_empty() {
        return Ok(());
    }
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(map_turso_error)?;
    let result=async {
        for table in migrate {
            let (columns,definition)=if table=="blocks" {
                ("block_id,document_id,parent_block_id,position,block_type,attrs,text,marks",
                 "block_id TEXT NOT NULL,document_id TEXT NOT NULL,parent_block_id TEXT,position INTEGER NOT NULL,block_type TEXT NOT NULL,attrs TEXT NOT NULL,text TEXT NOT NULL,marks TEXT NOT NULL,PRIMARY KEY(document_id,block_id)")
            } else {
                ("id,document_id,block_id,kind,start_offset,end_offset,body,replacement,author,state,quote,context_before,context_after,parent_item_id,created_at,updated_at",
                 "id TEXT NOT NULL,document_id TEXT NOT NULL,block_id TEXT NOT NULL,kind TEXT NOT NULL,start_offset INTEGER NOT NULL,end_offset INTEGER NOT NULL,body TEXT,replacement TEXT,author TEXT,state TEXT NOT NULL,quote TEXT,context_before TEXT,context_after TEXT,parent_item_id TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(document_id,id)")
            };
            // Table and column names above are fixed program constants.
            conn.execute(&format!("CREATE TABLE {table}_scoped({definition})"),()).await.map_err(map_turso_error)?;
            conn.execute(&format!("INSERT INTO {table}_scoped({columns}) SELECT {columns} FROM {table}"),()).await.map_err(map_turso_error)?;
            conn.execute(&format!("DROP TABLE {table}"),()).await.map_err(map_turso_error)?;
            conn.execute(&format!("ALTER TABLE {table}_scoped RENAME TO {table}"),()).await.map_err(map_turso_error)?;
        }
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_blocks_document ON blocks(document_id,parent_block_id,position); CREATE INDEX IF NOT EXISTS idx_block_review_items_document ON block_review_items(document_id); CREATE INDEX IF NOT EXISTS idx_block_review_items_block ON block_review_items(block_id);").await.map_err(map_turso_error)?;
        Ok(())
    }.await;
    crate::finish_tx(conn, result).await
}

pub(crate) async fn write_state_conn(
    conn: &Connection,
    document_id: &str,
    version: &str,
    request: &str,
    state: &DurableDocumentCommit,
) -> Result<()> {
    let document = Document::load(&state.bytes).map_err(document_error)?;
    if document.id().map_err(document_error)? != document_id {
        return Err(document_error(
            "Native state belongs to a different document",
        ));
    }
    if let Some(previous) = load_state_conn(conn, document_id).await? {
        let previous = Document::load(&previous.bytes).map_err(document_error)?;
        if !document.contains_history(&previous.heads()) {
            return Err(document_error(
                "Native publication must retain the document's complete history",
            ));
        }
    }
    conn.execute("INSERT INTO document_states(document_id,version_id,schema_version,state) VALUES(?1,?2,?3,?4) ON CONFLICT(document_id) DO UPDATE SET version_id=excluded.version_id,schema_version=excluded.schema_version,state=excluded.state",
        params![document_id,version,quarry_document::SCHEMA_VERSION as i64,state.bytes.clone()]).await.map_err(map_turso_error)?;
    conn.execute("INSERT INTO document_command_receipts(document_id,client_tx_id,request_hash) VALUES(?1,?2,?3)",params![document_id,request,state.request_hash.clone()]).await.map_err(map_turso_error)?;
    let heads = serde_json::to_string(
        &document
            .heads()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    )
    .map_err(document_error)?;
    conn.execute(
        "INSERT INTO document_state_versions(document_id,version_id,heads) VALUES(?1,?2,?3)",
        params![document_id, version, heads],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}
