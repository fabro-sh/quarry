//! Canonical block rows: the relational source of truth for BlockDocuments.
//!
//! Phase 1 of the session-scoped collaboration rewrite (see
//! `docs/superpowers/plans/2026-06-09-session-scoped-collab-rewrite.md`).
//! Markdown documents import into `blocks` rows through the production codec
//! (`quarry-collab-codec`), with frontmatter merged into document metadata —
//! the place frontmatter already lives — and the deterministic normalized
//! export written through the existing `document_versions` path so legacy
//! read paths keep working. Review anchors are `{block_id, start_offset,
//! end_offset}` in UTF-16 code units (matching Yjs); a collapsed range
//! (`start == end`) means orphaned at the row layer.
//!
//! Legacy writes and the projection: a version published outside the import
//! path (`put_document`, staged-transaction commits) or a document delete
//! drops the block projection — rows and review anchors — fail-closed via
//! [`clear_block_state_conn`], because the rows would otherwise serve stale
//! content. `export_block_document` returns `NotFound` until the document is
//! re-imported. An imported empty body is canonicalized to one empty
//! paragraph row so "zero rows" always means "no projection". Document moves
//! keep the projection (rows are keyed by document id and content does not
//! change). Phase 4's diff3 reconciliation replaces clearing with
//! identity-preserving merges.
//!
//! `block_shadow_bases` (diff3 bases, Phase 4) and `block_transactions`
//! (semantic mutation history, Phase 2) get schema plus minimal read/write
//! helpers only; their real consumers arrive in later phases.

use super::*;
use quarry_collab_codec::{
    block_rows_to_markdown, is_utf16_boundary, markdown_to_block_rows, utf16_len, BlockRow,
    LinkRange, MarkRun,
};
use quarry_core::render_markdown_frontmatter;

/// How a document participates in the block model. `BlockDocument`s are
/// canonical in block rows; `RawDocument`s keep the untouched byte path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    BlockDocument,
    RawDocument,
}

pub fn document_kind(path: &str, content_type: &str) -> DocumentKind {
    let path = path.to_ascii_lowercase();
    if path.ends_with(".md")
        || path.ends_with(".markdown")
        || is_markdown_content_type(content_type)
    {
        DocumentKind::BlockDocument
    } else {
        DocumentKind::RawDocument
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReviewKind {
    Comment,
    Suggestion,
    Conflict,
}

impl BlockReviewKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Suggestion => "suggestion",
            Self::Conflict => "conflict",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "comment" => Ok(Self::Comment),
            "suggestion" => Ok(Self::Suggestion),
            "conflict" => Ok(Self::Conflict),
            other => Err(QuarryError::Storage(format!(
                "unknown block review kind {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReviewState {
    Open,
    Resolved,
    Orphaned,
    Invalidated,
}

impl BlockReviewState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Orphaned => "orphaned",
            Self::Invalidated => "invalidated",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            "orphaned" => Ok(Self::Orphaned),
            "invalidated" => Ok(Self::Invalidated),
            other => Err(QuarryError::Storage(format!(
                "unknown block review state {other}"
            ))),
        }
    }
}

/// A review anchor row. Offsets are UTF-16 code units into the block's flat
/// text; `end_offset` is exclusive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockReviewItem {
    pub id: String,
    pub document_id: String,
    pub block_id: String,
    pub kind: BlockReviewKind,
    pub start_offset: u32,
    pub end_offset: u32,
    pub body: Option<String>,
    pub replacement: Option<String>,
    pub author: Option<String>,
    pub state: BlockReviewState,
    pub quote: Option<String>,
    pub context_before: Option<String>,
    pub context_after: Option<String>,
    pub parent_item_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Insert payload for [`QuarryStore::put_block_review_item`]; the store mints
/// the id and timestamps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewBlockReviewItem {
    pub document_id: String,
    pub block_id: String,
    pub kind: BlockReviewKind,
    pub start_offset: u32,
    pub end_offset: u32,
    pub body: Option<String>,
    pub replacement: Option<String>,
    pub author: Option<String>,
    pub state: BlockReviewState,
    pub quote: Option<String>,
    pub context_before: Option<String>,
    pub context_after: Option<String>,
    pub parent_item_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockShadowBase {
    pub surface: String,
    pub scope_key: String,
    pub document_id: String,
    pub base_markdown: String,
    pub base_version_id: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockTransactionRecord {
    pub id: String,
    pub document_id: String,
    pub client_tx_id: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub ops: JsonValue,
    pub resulting_version_id: Option<String>,
    pub created_at: String,
}

/// JSON shape of the `blocks.marks` column: inline marks and links together.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct InlineRanges {
    marks: Vec<MarkRun>,
    links: Vec<LinkRange>,
}

impl QuarryStore {
    /// Loads a document's block rows in depth-first document order (parents
    /// before children, siblings by `position`).
    pub async fn load_block_tree(&self, document_id: &str) -> Result<Vec<BlockRow>> {
        let conn = self.conn()?;
        load_block_tree_conn(&conn, document_id).await
    }

    /// Replaces a document's whole block row set in one transaction.
    /// Per-operation mutation arrives with the Phase 2 gateway.
    pub async fn replace_block_tree(&self, document_id: &str, rows: &[BlockRow]) -> Result<()> {
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.acquire_write_lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            require_document_conn(&conn, document_id).await?;
            replace_block_rows_conn(&conn, document_id, rows).await
        }
        .await;
        finish_tx(&conn, result).await
    }

    /// Imports a Markdown document as canonical block rows.
    ///
    /// Frontmatter merges into document metadata (the existing mechanism);
    /// the body becomes rows via the codec, falling back to `raw_markdown`
    /// rows for safe unsupported constructs and surfacing the codec's typed
    /// [`quarry_collab_codec::Unsupported`] error for unsafe ones. The
    /// deterministic normalized export is written through the existing
    /// version path in the same transaction, so rows and version content
    /// always agree and legacy readers keep working.
    #[allow(clippy::too_many_arguments)]
    pub async fn import_block_document(
        &self,
        library: &str,
        path: &str,
        markdown: &str,
        metadata: JsonValue,
        content_type: &str,
        source: DocumentSource,
        precondition: WritePrecondition,
    ) -> Result<WriteOutcome> {
        let path = normalize_path(path)?;
        if document_kind(&path, content_type) == DocumentKind::RawDocument {
            return Err(QuarryError::Unsupported(format!(
                "cannot import {path} ({content_type}) as a block document"
            )));
        }
        let (frontmatter, body) = split_markdown_frontmatter(markdown)?;
        let rows = canonical_rows(markdown_to_block_rows(body, || Uuid::new_v4().to_string())?);
        let mut merged_metadata = frontmatter;
        merge_json(&mut merged_metadata, metadata.clone());
        let normalized = format!(
            "{}{}",
            render_markdown_frontmatter(&merged_metadata)?,
            block_rows_to_markdown(&rows)?
        );

        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.acquire_write_lock().await;
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
                serde_json::json!({ "mode": "block_import" }),
            )
            .await?;
            let (doc_id, old_version_id) =
                ensure_document_conn(&conn, &library.id, &path, &now_timestamp()).await?;
            // insert_version_conn re-parses the frontmatter rendered into
            // `normalized` and re-merges the caller metadata over it; both
            // were derived from `merged_metadata`, so the stored metadata
            // round-trips to exactly `merged_metadata` (modulo content_type,
            // which the renderer excludes).
            let version = self
                .insert_version_conn(
                    &conn,
                    &doc_id,
                    &tx.id,
                    normalized.into_bytes(),
                    metadata,
                    content_type,
                )
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
            replace_block_rows_conn(&conn, &doc_id, &rows).await?;
            ensure_path_inodes_conn(&conn, &library.id, &path).await?;
            self.reindex_links_conn(&conn, &library.id).await?;
            commit_transaction_record_conn(&conn, &tx.id).await?;
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
        self.emit_document_put_events(&outcome, None);
        Ok(outcome)
    }

    /// Exports a BlockDocument from its canonical rows: frontmatter rendered
    /// from document metadata plus the deterministic Markdown body.
    ///
    /// A document without rows has no block projection (it was never
    /// imported, or a legacy write cleared it) and returns `NotFound` rather
    /// than serving stale or fabricated content.
    pub async fn export_block_document(&self, document_id: &str) -> Result<String> {
        let conn = self.conn()?;
        // One read transaction so the head metadata and the rows cannot tear
        // against a concurrent import.
        conn.execute("BEGIN", ()).await.map_err(map_turso_error)?;
        let result = async {
            let head = head_version_head_conn(&conn, document_id).await?;
            if document_kind(&head.path, &head.content_type) == DocumentKind::RawDocument {
                return Err(QuarryError::Unsupported(format!(
                    "cannot export {} ({}) as a block document",
                    head.path, head.content_type
                )));
            }
            let rows = load_block_tree_conn(&conn, document_id).await?;
            if rows.is_empty() {
                return Err(QuarryError::NotFound(format!(
                    "block rows for document {document_id} (re-import required)"
                )));
            }
            Ok(format!(
                "{}{}",
                render_markdown_frontmatter(&head.metadata)?,
                block_rows_to_markdown(&rows)?
            ))
        }
        .await;
        finish_tx(&conn, result).await
    }

    /// Stores a review anchor after validating its offsets against the
    /// anchored block's text: offsets are UTF-16 code units, must lie on
    /// character boundaries (never inside a surrogate pair), and a collapsed
    /// range (`start == end`) is only legal for orphaned anchors.
    pub async fn put_block_review_item(&self, item: NewBlockReviewItem) -> Result<BlockReviewItem> {
        let _guard = self.acquire_write_lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            let block_text = block_text_conn(&conn, &item.document_id, &item.block_id).await?;
            validate_anchor_offsets(&item, &block_text)?;
            let id = Uuid::new_v4().to_string();
            let now = now_timestamp();
            conn.execute(
                "INSERT INTO block_review_items
                 (id, document_id, block_id, kind, start_offset, end_offset, body, replacement,
                  author, state, quote, context_before, context_after, parent_item_id,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                vec![
                    Value::Text(id.clone()),
                    Value::Text(item.document_id.clone()),
                    Value::Text(item.block_id.clone()),
                    Value::Text(item.kind.as_str().to_string()),
                    Value::Integer(i64::from(item.start_offset)),
                    Value::Integer(i64::from(item.end_offset)),
                    opt_value(item.body.clone()),
                    opt_value(item.replacement.clone()),
                    opt_value(item.author.clone()),
                    Value::Text(item.state.as_str().to_string()),
                    opt_value(item.quote.clone()),
                    opt_value(item.context_before.clone()),
                    opt_value(item.context_after.clone()),
                    opt_value(item.parent_item_id.clone()),
                    Value::Text(now.clone()),
                    Value::Text(now.clone()),
                ],
            )
            .await
            .map_err(map_turso_error)?;
            Ok(BlockReviewItem {
                id,
                document_id: item.document_id,
                block_id: item.block_id,
                kind: item.kind,
                start_offset: item.start_offset,
                end_offset: item.end_offset,
                body: item.body,
                replacement: item.replacement,
                author: item.author,
                state: item.state,
                quote: item.quote,
                context_before: item.context_before,
                context_after: item.context_after,
                parent_item_id: item.parent_item_id,
                created_at: now.clone(),
                updated_at: now,
            })
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn list_block_review_items(&self, document_id: &str) -> Result<Vec<BlockReviewItem>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, document_id, block_id, kind, start_offset, end_offset, body,
                        replacement, author, state, quote, context_before, context_after,
                        parent_item_id, created_at, updated_at
                 FROM block_review_items WHERE document_id = ?1
                 ORDER BY created_at, id",
                params![document_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            items.push(block_review_item_from_row(&row)?);
        }
        Ok(items)
    }

    /// Upserts the diff3 shadow base for one surface/scope/document triple.
    pub async fn put_block_shadow_base(
        &self,
        surface: &str,
        scope_key: &str,
        document_id: &str,
        base_markdown: &str,
        base_version_id: Option<String>,
    ) -> Result<BlockShadowBase> {
        let _guard = self.acquire_write_lock().await;
        let conn = self.conn()?;
        let updated_at = now_timestamp();
        conn.execute(
            "INSERT INTO block_shadow_bases
             (surface, scope_key, document_id, base_markdown, base_version_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(surface, scope_key, document_id) DO UPDATE SET
               base_markdown = excluded.base_markdown,
               base_version_id = excluded.base_version_id,
               updated_at = excluded.updated_at",
            vec![
                Value::Text(surface.to_string()),
                Value::Text(scope_key.to_string()),
                Value::Text(document_id.to_string()),
                Value::Text(base_markdown.to_string()),
                opt_value(base_version_id.clone()),
                Value::Text(updated_at.clone()),
            ],
        )
        .await
        .map_err(map_turso_error)?;
        Ok(BlockShadowBase {
            surface: surface.to_string(),
            scope_key: scope_key.to_string(),
            document_id: document_id.to_string(),
            base_markdown: base_markdown.to_string(),
            base_version_id,
            updated_at,
        })
    }

    pub async fn block_shadow_base(
        &self,
        surface: &str,
        scope_key: &str,
        document_id: &str,
    ) -> Result<Option<BlockShadowBase>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT surface, scope_key, document_id, base_markdown, base_version_id, updated_at
                 FROM block_shadow_bases
                 WHERE surface = ?1 AND scope_key = ?2 AND document_id = ?3
                 LIMIT 1",
                params![
                    surface.to_string(),
                    scope_key.to_string(),
                    document_id.to_string()
                ],
            )
            .await
            .map_err(map_turso_error)?;
        match rows.next().await.map_err(map_turso_error)? {
            Some(row) => Ok(Some(BlockShadowBase {
                surface: text(&row, 0)?,
                scope_key: text(&row, 1)?,
                document_id: text(&row, 2)?,
                base_markdown: text(&row, 3)?,
                base_version_id: opt_text(&row, 4)?,
                updated_at: text(&row, 5)?,
            })),
            None => Ok(None),
        }
    }

    /// Records one semantic mutation transaction. `client_tx_id` is unique
    /// per document: a duplicate returns `QuarryError::Conflict` so Phase 2
    /// can answer idempotently from the stored record.
    pub async fn record_block_transaction(
        &self,
        document_id: &str,
        client_tx_id: &str,
        actor_kind: &str,
        actor_id: Option<String>,
        ops: JsonValue,
        resulting_version_id: Option<String>,
    ) -> Result<BlockTransactionRecord> {
        let _guard = self.acquire_write_lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = async {
            if block_transaction_conn(&conn, document_id, client_tx_id)
                .await?
                .is_some()
            {
                return Err(QuarryError::Conflict(format!(
                    "block transaction {client_tx_id} already recorded for document {document_id}"
                )));
            }
            let record = BlockTransactionRecord {
                id: Uuid::new_v4().to_string(),
                document_id: document_id.to_string(),
                client_tx_id: client_tx_id.to_string(),
                actor_kind: actor_kind.to_string(),
                actor_id,
                ops,
                resulting_version_id,
                created_at: now_timestamp(),
            };
            conn.execute(
                "INSERT INTO block_transactions
                 (id, document_id, client_tx_id, actor_kind, actor_id, ops,
                  resulting_version_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                vec![
                    Value::Text(record.id.clone()),
                    Value::Text(record.document_id.clone()),
                    Value::Text(record.client_tx_id.clone()),
                    Value::Text(record.actor_kind.clone()),
                    opt_value(record.actor_id.clone()),
                    Value::Text(record.ops.to_string()),
                    opt_value(record.resulting_version_id.clone()),
                    Value::Text(record.created_at.clone()),
                ],
            )
            .await
            .map_err(map_turso_error)?;
            Ok(record)
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn block_transaction(
        &self,
        document_id: &str,
        client_tx_id: &str,
    ) -> Result<Option<BlockTransactionRecord>> {
        let conn = self.conn()?;
        block_transaction_conn(&conn, document_id, client_tx_id).await
    }
}

fn validate_anchor_offsets(item: &NewBlockReviewItem, block_text: &str) -> Result<()> {
    let length = utf16_len(block_text);
    if item.start_offset > item.end_offset {
        return Err(QuarryError::InvalidInput(format!(
            "anchor start {} is after end {}",
            item.start_offset, item.end_offset
        )));
    }
    if item.end_offset > length {
        return Err(QuarryError::InvalidInput(format!(
            "anchor end {} is past the block text (UTF-16 length {length})",
            item.end_offset
        )));
    }
    if !is_utf16_boundary(block_text, item.start_offset)
        || !is_utf16_boundary(block_text, item.end_offset)
    {
        return Err(QuarryError::InvalidInput(format!(
            "anchor offsets [{}, {}) split a surrogate pair",
            item.start_offset, item.end_offset
        )));
    }
    if item.start_offset == item.end_offset && item.state != BlockReviewState::Orphaned {
        return Err(QuarryError::InvalidInput(
            "a collapsed anchor range means orphaned at the row layer".to_string(),
        ));
    }
    Ok(())
}

async fn block_text_conn(conn: &Connection, document_id: &str, block_id: &str) -> Result<String> {
    let mut rows = conn
        .query(
            "SELECT text FROM blocks WHERE block_id = ?1 AND document_id = ?2 LIMIT 1",
            params![block_id.to_string(), document_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    match rows.next().await.map_err(map_turso_error)? {
        Some(row) => text(&row, 0),
        None => Err(QuarryError::NotFound(format!(
            "block {block_id} in document {document_id}"
        ))),
    }
}

async fn require_document_conn(conn: &Connection, document_id: &str) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT id FROM documents WHERE id = ?1 LIMIT 1",
            params![document_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    if rows.next().await.map_err(map_turso_error)?.is_none() {
        return Err(QuarryError::NotFound(format!("document {document_id}")));
    }
    Ok(())
}

struct BlockDocumentHead {
    path: String,
    content_type: String,
    metadata: JsonValue,
}

async fn head_version_head_conn(conn: &Connection, document_id: &str) -> Result<BlockDocumentHead> {
    let mut rows = conn
        .query(
            "SELECT d.path, v.content_type, v.metadata_json
             FROM documents d
             JOIN document_versions v ON v.id = d.head_version_id
             WHERE d.id = ?1
             LIMIT 1",
            params![document_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    match rows.next().await.map_err(map_turso_error)? {
        Some(row) => Ok(BlockDocumentHead {
            path: text(&row, 0)?,
            content_type: text(&row, 1)?,
            metadata: serde_json::from_str(&text(&row, 2)?)?,
        }),
        None => Err(QuarryError::NotFound(format!("document {document_id}"))),
    }
}

/// Canonical row shape for an imported body: an empty body becomes one empty
/// paragraph row (matching the editor's empty-document shape), so "zero rows"
/// always means "no block projection" rather than "empty document".
fn canonical_rows(rows: Vec<BlockRow>) -> Vec<BlockRow> {
    if !rows.is_empty() {
        return rows;
    }
    vec![BlockRow {
        block_id: Uuid::new_v4().to_string(),
        parent_block_id: None,
        position: 0,
        block_type: "p".to_string(),
        attrs: quarry_collab_codec::Attrs::new(),
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    }]
}

/// Drops a document's block projection (rows and review anchors).
///
/// Called by the legacy write paths (`put_document`, staged-transaction
/// commits, `delete_document`): a version published outside the import path
/// would leave the rows serving stale content, and a delete would orphan
/// them. The projection is removed fail-closed; `export_block_document`
/// returns `NotFound` until the document is re-imported. Phase 4's diff3
/// reconciliation replaces this with identity-preserving merges.
pub(crate) async fn clear_block_state_conn(conn: &Connection, document_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM blocks WHERE document_id = ?1",
        params![document_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    conn.execute(
        "DELETE FROM block_review_items WHERE document_id = ?1",
        params![document_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    Ok(())
}

pub(crate) async fn replace_block_rows_conn(
    conn: &Connection,
    document_id: &str,
    rows: &[BlockRow],
) -> Result<()> {
    conn.execute(
        "DELETE FROM blocks WHERE document_id = ?1",
        params![document_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    for row in rows {
        let ranges = InlineRanges {
            marks: row.marks.clone(),
            links: row.links.clone(),
        };
        conn.execute(
            "INSERT INTO blocks
             (block_id, document_id, parent_block_id, position, block_type, attrs, text, marks)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            vec![
                Value::Text(row.block_id.clone()),
                Value::Text(document_id.to_string()),
                opt_value(row.parent_block_id.clone()),
                Value::Integer(i64::from(row.position)),
                Value::Text(row.block_type.clone()),
                Value::Text(serde_json::to_string(&row.attrs)?),
                Value::Text(row.text.clone()),
                Value::Text(serde_json::to_string(&ranges)?),
            ],
        )
        .await
        .map_err(map_turso_error)?;
    }
    Ok(())
}

pub(crate) async fn load_block_tree_conn(
    conn: &Connection,
    document_id: &str,
) -> Result<Vec<BlockRow>> {
    let mut rows = conn
        .query(
            "SELECT block_id, parent_block_id, position, block_type, attrs, text, marks
             FROM blocks WHERE document_id = ?1",
            params![document_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    let mut loaded = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        loaded.push(block_row_from_row(&row)?);
    }
    order_depth_first(document_id, loaded)
}

/// Orders flat rows into depth-first document order: parents before children,
/// siblings by `position`.
fn order_depth_first(document_id: &str, rows: Vec<BlockRow>) -> Result<Vec<BlockRow>> {
    let total = rows.len();
    let mut by_parent: HashMap<Option<String>, Vec<BlockRow>> = HashMap::new();
    for row in rows {
        by_parent
            .entry(row.parent_block_id.clone())
            .or_default()
            .push(row);
    }
    for children in by_parent.values_mut() {
        children.sort_by_key(|row| row.position);
        children.reverse();
    }
    let mut ordered = Vec::with_capacity(total);
    let mut stack = by_parent.remove(&None).unwrap_or_default();
    while let Some(row) = stack.pop() {
        if let Some(children) = by_parent.remove(&Some(row.block_id.clone())) {
            stack.extend(children);
        }
        ordered.push(row);
    }
    if ordered.len() != total {
        return Err(QuarryError::Storage(format!(
            "document {document_id} has orphaned block rows"
        )));
    }
    Ok(ordered)
}

fn block_row_from_row(row: &Row) -> Result<BlockRow> {
    let position = row.get::<i64>(2).map_err(map_turso_error)?;
    let position = u32::try_from(position)
        .map_err(|_| QuarryError::Storage(format!("block position {position} out of range")))?;
    let ranges: InlineRanges = serde_json::from_str(&text(row, 6)?)?;
    Ok(BlockRow {
        block_id: text(row, 0)?,
        parent_block_id: opt_text(row, 1)?,
        position,
        block_type: text(row, 3)?,
        attrs: serde_json::from_str(&text(row, 4)?)?,
        text: text(row, 5)?,
        marks: ranges.marks,
        links: ranges.links,
    })
}

fn block_review_item_from_row(row: &Row) -> Result<BlockReviewItem> {
    let start_offset = row.get::<i64>(4).map_err(map_turso_error)?;
    let end_offset = row.get::<i64>(5).map_err(map_turso_error)?;
    Ok(BlockReviewItem {
        id: text(row, 0)?,
        document_id: text(row, 1)?,
        block_id: text(row, 2)?,
        kind: BlockReviewKind::parse(&text(row, 3)?)?,
        start_offset: u32::try_from(start_offset)
            .map_err(|_| QuarryError::Storage(format!("anchor start {start_offset} invalid")))?,
        end_offset: u32::try_from(end_offset)
            .map_err(|_| QuarryError::Storage(format!("anchor end {end_offset} invalid")))?,
        body: opt_text(row, 6)?,
        replacement: opt_text(row, 7)?,
        author: opt_text(row, 8)?,
        state: BlockReviewState::parse(&text(row, 9)?)?,
        quote: opt_text(row, 10)?,
        context_before: opt_text(row, 11)?,
        context_after: opt_text(row, 12)?,
        parent_item_id: opt_text(row, 13)?,
        created_at: text(row, 14)?,
        updated_at: text(row, 15)?,
    })
}

async fn block_transaction_conn(
    conn: &Connection,
    document_id: &str,
    client_tx_id: &str,
) -> Result<Option<BlockTransactionRecord>> {
    let mut rows = conn
        .query(
            "SELECT id, document_id, client_tx_id, actor_kind, actor_id, ops,
                    resulting_version_id, created_at
             FROM block_transactions
             WHERE document_id = ?1 AND client_tx_id = ?2
             LIMIT 1",
            params![document_id.to_string(), client_tx_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    match rows.next().await.map_err(map_turso_error)? {
        Some(row) => Ok(Some(BlockTransactionRecord {
            id: text(&row, 0)?,
            document_id: text(&row, 1)?,
            client_tx_id: text(&row, 2)?,
            actor_kind: text(&row, 3)?,
            actor_id: opt_text(&row, 4)?,
            ops: serde_json::from_str(&text(&row, 5)?)?,
            resulting_version_id: opt_text(&row, 6)?,
            created_at: text(&row, 7)?,
        })),
        None => Ok(None),
    }
}
