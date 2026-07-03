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
//! change). Since Phase 4, every Markdown writer (REST PUT, Git, FUSE, CLI,
//! version restores, Git conflict siblings) reconciles through
//! [`BlockMarkdownWriter`] and metadata patches commit through the gateway
//! with a metadata override — the clearing path remains for raw documents,
//! staged-transaction commits, and any direct `put_document`/`patch_metadata`
//! caller that bypasses the server routes. Staged commits stay on the byte
//! path deliberately: the explicit transaction API publishes pre-staged
//! versions for MANY paths atomically in one SQL transaction, which cannot
//! ride the per-document gateway dispatch — a recorded limitation (see the
//! README), not an oversight.
//!
//! `block_shadow_bases` holds the Phase 4 diff3 bases (currently Git peer
//! bases; FUSE bases are per-open-handle in memory, the CLI is two-way);
//! `block_transactions` is the semantic mutation history (Phase 2).

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentScopeRef {
    Library { slug: String },
    Tmp,
}

impl DocumentScopeRef {
    pub fn library(slug: impl Into<String>) -> Self {
        Self::Library { slug: slug.into() }
    }

    pub fn event_library_id(&self) -> &str {
        match self {
            Self::Library { slug } => slug,
            Self::Tmp => TMP_TRANSACTION_LIBRARY_ID,
        }
    }

    pub fn library_slug(&self) -> Option<&str> {
        match self {
            Self::Library { slug } => Some(slug),
            Self::Tmp => None,
        }
    }
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
            other => Err(QuarryError::Invariant(format!(
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
            other => Err(QuarryError::Invariant(format!(
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

/// A whole-file Markdown write of a BlockDocument, reconciled against the
/// canonical block rows via diff3 (Phase 4). Adapters (Git, FUSE, CLI) differ
/// only in base bookkeeping; the single implementation lives in quarry-server
/// (it owns the mutation gateway and the session mode switch) and is
/// installed into the store by the serving process — see
/// [`QuarryStore::set_block_markdown_writer`].
#[derive(Clone, Debug)]
pub struct BlockMarkdownWrite {
    pub scope: DocumentScopeRef,
    pub path: String,
    /// The full incoming text (frontmatter + body).
    pub markdown: String,
    /// Caller metadata merged over the incoming frontmatter (at minimum
    /// `{"content_type": …}`), mirroring the import path's metadata rule.
    pub metadata: JsonValue,
    pub base: BlockWriteBase,
    pub source: DocumentSource,
    /// Actor kind recorded on the transaction history ("git", "fuse", "cli",
    /// "rest").
    pub surface: String,
    /// Display label for history attribution (e.g. "Git sync (origin)").
    pub actor_label: Option<String>,
}

/// The diff3 base selector for one whole-file write.
#[derive(Clone, Debug)]
pub enum BlockWriteBase {
    /// Two-way degenerate case (CLI, missing shadow base): the base is the
    /// current canonical state, so nothing can conflict and every incoming
    /// difference applies.
    CurrentCanonical,
    /// A stored shadow base: Git peer bases, FUSE open-handle bases, REST
    /// `If-Match` version content. The full text (frontmatter tolerated);
    /// `version_id` engages the gateway's rebase acks when it names a known
    /// version.
    Markdown {
        markdown: String,
        version_id: Option<String>,
    },
}

/// What a reconciled whole-file write did.
#[derive(Clone, Debug)]
pub struct BlockMarkdownWriteOutcome {
    /// The head after the write: a fresh commit when `changed`, the
    /// untouched current head otherwise.
    pub outcome: WriteOutcome,
    /// `false` when the incoming text was byte-identical to the head
    /// content: nothing was committed (no version churn on re-imports and
    /// no-op saves).
    pub changed: bool,
    /// The canonical Markdown BODY (no frontmatter) after the write — what
    /// shadow bases store.
    pub canonical_body: String,
    /// Conflict review items recorded by this write.
    pub conflicts: usize,
}

/// The Phase 4 whole-file write path. Reconciliation failures never fail the
/// write (conflicts become review items); errors are content errors
/// (CriticMarkup → [`QuarryError::UnsupportedMarkdown`]) or ordinary storage
/// failures.
pub trait BlockMarkdownWriter: Send + Sync {
    fn write_markdown(
        &self,
        write: BlockMarkdownWrite,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BlockMarkdownWriteOutcome>> + Send + '_>,
    >;
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

/// One consistent read of everything the Phase 2 gateway needs to apply a
/// semantic mutation: the document head, the block rows (materialized
/// in-memory from the head Markdown when the stored projection is missing —
/// nothing is persisted by this read), review anchors, the set of known
/// version ids (stale-clock validation), and any already-recorded transaction
/// for `client_tx_id` (idempotent replay).
#[derive(Clone, Debug)]
pub struct BlockMutationState {
    pub document_id: String,
    pub path: String,
    pub head_version_id: String,
    pub content_type: String,
    pub metadata: JsonValue,
    pub rows: Vec<BlockRow>,
    /// True when `rows` were materialized in-memory from the head Markdown
    /// because the stored projection was missing (legacy write cleared it or
    /// the document was never imported).
    pub projection_missing: bool,
    pub review_items: Vec<BlockReviewItem>,
    pub version_ids: HashSet<String>,
    pub replay: Option<BlockTransactionRecord>,
}

/// The computed final state of one semantic mutation, committed atomically by
/// [`QuarryStore::commit_block_mutation`]: the full row set, the full review
/// item set, the normalized Markdown export of those rows (published through
/// the existing `document_versions` path so legacy readers/events keep
/// working), and the history record inputs.
#[derive(Clone, Debug)]
pub struct BlockMutationCommit {
    pub document_id: String,
    /// The head version the mutation was computed against. A different head
    /// at commit time fails with `PreconditionFailed` so the caller can
    /// reload and recompute.
    pub expected_head_version_id: String,
    pub client_tx_id: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    /// Display actor recorded on the legacy `transactions` history row.
    pub transaction_actor: Option<String>,
    /// Optional message/provenance for the legacy `transactions` history row
    /// (session checkpoints mark themselves as coalesced autosave history).
    pub transaction_message: Option<String>,
    pub transaction_provenance: Option<JsonValue>,
    /// Rides on the emitted `doc.changed` event so browsers can classify the
    /// write (session checkpoints use a benign provenance).
    pub origin_id: Option<String>,
    pub source: DocumentSource,
    /// Stored verbatim in `block_transactions.ops`; the gateway includes the
    /// request ops plus the ack it needs to answer idempotent replays.
    pub recorded_ops: JsonValue,
    pub metadata: JsonValue,
    pub content_type: String,
    pub rows: Vec<BlockRow>,
    pub review_items: Vec<BlockReviewItem>,
    pub normalized_markdown: String,
}

/// One consistent read of everything a live editing session needs to seed
/// from canonical state (or a checkpoint needs to refresh its head): the
/// document's identity, head version, and block projection. Rows are
/// materialized in-memory from the head Markdown when the stored projection
/// is missing (nothing is persisted by this read — the first checkpoint
/// persists them).
#[derive(Clone, Debug)]
pub struct SessionSeedState {
    pub document_id: String,
    pub scope: DocumentScopeRef,
    pub path: String,
    pub head_version_id: String,
    pub content_type: String,
    pub metadata: JsonValue,
    pub rows: Vec<BlockRow>,
    pub review_items: Vec<BlockReviewItem>,
}

#[derive(Debug)]
pub enum BlockMutationOutcome {
    Applied {
        outcome: Box<WriteOutcome>,
        record: BlockTransactionRecord,
    },
    /// `client_tx_id` was already recorded for this document; nothing was
    /// re-applied. The caller answers from the stored record.
    Replayed(BlockTransactionRecord),
}

/// The empty writer-registry slot (`Weak::<dyn …>::new()` needs a sized
/// type to coerce from); never instantiated.
pub(crate) struct NoBlockMarkdownWriter;

impl BlockMarkdownWriter for NoBlockMarkdownWriter {
    fn write_markdown(
        &self,
        _write: BlockMarkdownWrite,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BlockMarkdownWriteOutcome>> + Send + '_>,
    > {
        unreachable!("NoBlockMarkdownWriter is never constructed")
    }
}

impl QuarryStore {
    /// Installs the whole-file Markdown write path (Phase 4). The serving
    /// process (quarry-server) calls this once at startup and keeps the
    /// strong `Arc` alive for the serving lifetime (the registry holds a
    /// `Weak` — see the field docs); Git/FUSE/CLI then route every
    /// BlockDocument file write through
    /// [`QuarryStore::write_block_markdown`].
    pub fn set_block_markdown_writer(&self, writer: &Arc<dyn BlockMarkdownWriter>) {
        *self
            .block_markdown_writer
            .write()
            .expect("writer registry lock") = Arc::downgrade(writer);
    }

    /// Routes a whole-file BlockDocument write through the installed
    /// reconciling writer. Errors when no writer is installed — adapters
    /// must never fall back to the legacy byte path for Markdown, or the
    /// write would clear the block projection and bypass live sessions.
    pub async fn write_block_markdown(
        &self,
        write: BlockMarkdownWrite,
    ) -> Result<BlockMarkdownWriteOutcome> {
        let writer = self
            .block_markdown_writer
            .read()
            .expect("writer registry lock")
            .upgrade()
            .ok_or_else(|| {
                QuarryError::Unsupported(
                    "no block markdown writer installed; markdown writes require the owning \
                     quarry process"
                        .to_string(),
                )
            })?;
        writer.write_markdown(write).await
    }

    /// Loads a document's block rows in depth-first document order (parents
    /// before children, siblings by `position`).
    pub async fn load_block_tree(&self, document_id: &str) -> Result<Vec<BlockRow>> {
        let conn = self.conn()?;
        load_block_tree_conn(&conn, document_id).await
    }

    /// Replaces a document's whole block row set in one transaction.
    /// Per-operation mutation arrives with the Phase 2 gateway.
    pub async fn replace_block_tree(&self, document_id: &str, rows: &[BlockRow]) -> Result<()> {
        let document_id = document_id.to_string();
        let rows = rows.to_vec();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
                require_document_conn(conn, &document_id).await?;
                replace_block_rows_conn(conn, &document_id, &rows).await
            })
        })
        .await
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
        self.import_block_document_for_scope(
            &DocumentScopeRef::library(library),
            path,
            markdown,
            metadata,
            content_type,
            source,
            precondition,
            None,
            TransactionMetadata::default(),
        )
        .await
    }

    pub async fn import_tmp_block_document(
        &self,
        path: &str,
        markdown: &str,
        metadata: JsonValue,
        content_type: &str,
        precondition: WritePrecondition,
    ) -> Result<WriteOutcome> {
        self.import_block_document_for_scope(
            &DocumentScopeRef::Tmp,
            path,
            markdown,
            metadata,
            content_type,
            DocumentSource::Rest,
            precondition,
            None,
            TransactionMetadata::default(),
        )
        .await
    }

    /// [`QuarryStore::import_block_document`] for either scope, with an
    /// `origin_id` echoed on the emitted `doc.changed` event (the Phase 4
    /// first-import path) and transaction attribution recorded on the import
    /// transaction. Unset provenance defaults per scope.
    #[allow(clippy::too_many_arguments)]
    pub async fn import_block_document_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
        markdown: &str,
        metadata: JsonValue,
        content_type: &str,
        source: DocumentSource,
        precondition: WritePrecondition,
        origin_id: Option<String>,
        transaction: TransactionMetadata,
    ) -> Result<WriteOutcome> {
        let path = match scope {
            DocumentScopeRef::Library { .. } => normalize_path(path)?,
            DocumentScopeRef::Tmp => TmpDocumentSecret::parse(path)?.as_str().to_string(),
        };
        let content_type = match scope {
            DocumentScopeRef::Library { .. } => content_type.to_string(),
            DocumentScopeRef::Tmp => normalize_tmp_markdown_content_type(content_type)?.to_string(),
        };
        let metadata = match scope {
            DocumentScopeRef::Library { .. } => metadata,
            DocumentScopeRef::Tmp => tmp_metadata_with_content_type(metadata, &content_type),
        };
        if document_kind(&path, &content_type) == DocumentKind::RawDocument {
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
        if matches!(scope, DocumentScopeRef::Tmp) {
            validate_tmp_markdown_text(&normalized)?;
        }
        let scope = scope.clone();

        let outcome = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let resolved_scope = store.resolve_document_scope_conn(conn, &scope).await?;
                    match &resolved_scope {
                        ResolvedDocumentScope::Library { id } => {
                            store
                                .check_precondition_conn(conn, id, &path, &precondition)
                                .await?;
                        }
                        ResolvedDocumentScope::Tmp => {
                            store
                                .check_tmp_precondition_conn(conn, &path, &precondition)
                                .await?;
                        }
                    }
                    let provenance =
                        transaction
                            .provenance
                            .unwrap_or_else(|| match &resolved_scope {
                                ResolvedDocumentScope::Library { .. } => {
                                    serde_json::json!({ "mode": "block_import" })
                                }
                                ResolvedDocumentScope::Tmp => {
                                    serde_json::json!({ "mode": "tmp_block_import" })
                                }
                            });
                    let tx = insert_transaction_conn(
                        conn,
                        resolved_scope.transaction_library_id(),
                        source,
                        transaction.actor,
                        transaction.message,
                        provenance,
                    )
                    .await?;
                    let (doc_id, old_version_id) = match &resolved_scope {
                        ResolvedDocumentScope::Library { id } => {
                            ensure_document_conn(conn, id, &path, &now_timestamp()).await?
                        }
                        ResolvedDocumentScope::Tmp => {
                            // Keep the existing expiry (or the default for a fresh
                            // document); an import never extends a tmp TTL.
                            let expires_at = store
                                .tmp_document_expires_at_conn(conn, &path)
                                .await?
                                .unwrap_or_else(default_tmp_expires_at);
                            ensure_tmp_document_conn(conn, &path, &expires_at, &now_timestamp())
                                .await?
                        }
                    };
                    // insert_version_conn re-parses the frontmatter rendered into
                    // `normalized` and re-merges the caller metadata over it; both
                    // were derived from `merged_metadata`, so the stored metadata
                    // round-trips to exactly `merged_metadata` (modulo content_type,
                    // which the renderer excludes).
                    let version = store
                        .insert_version_conn(
                            conn,
                            &doc_id,
                            &tx.id,
                            normalized.into_bytes(),
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
                    replace_block_rows_conn(conn, &doc_id, &rows).await?;
                    if let ResolvedDocumentScope::Library { id } = &resolved_scope {
                        ensure_path_inodes_conn(conn, id, &path).await?;
                        store.reindex_links_conn(conn, id).await?;
                    }
                    commit_transaction_record_conn(conn, &tx.id).await?;
                    let document = match &resolved_scope {
                        ResolvedDocumentScope::Library { id } => {
                            store.document_entry_conn(conn, id, &path).await?
                        }
                        ResolvedDocumentScope::Tmp => {
                            store.tmp_document_entry_conn(conn, &path).await?
                        }
                    };
                    let tx = store.transaction_conn(conn, &tx.id).await?;
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
            if item.kind != BlockReviewKind::Conflict {
                let block_text = block_text_conn(&conn, &item.document_id, &item.block_id).await?;
                validate_anchor_offsets(&item, &block_text)?;
            }
            // Conflict items (Phase 4) anchor by `after_block_id` in
            // `block_id` ("" = document start) with a collapsed placement
            // range — no text anchor to validate (mirrors
            // validate_review_items_against_rows).
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
            insert_block_transaction_conn(
                &conn,
                document_id,
                client_tx_id,
                actor_kind,
                actor_id,
                ops,
                resulting_version_id,
            )
            .await
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

    /// Loads the consistent pre-state for one semantic mutation (see
    /// [`BlockMutationState`]). When the stored projection is missing for a
    /// BlockDocument, rows are materialized in-memory from the head Markdown
    /// (fresh `block_id`s, not persisted); committing the mutation persists
    /// them. RawDocuments load with empty rows — callers reject those before
    /// using them.
    pub async fn block_mutation_state(
        &self,
        library: &str,
        path: &str,
        client_tx_id: &str,
    ) -> Result<BlockMutationState> {
        self.block_mutation_state_for_scope(&DocumentScopeRef::library(library), path, client_tx_id)
            .await
    }

    pub async fn block_mutation_state_for_scope(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
        client_tx_id: &str,
    ) -> Result<BlockMutationState> {
        let conn = self.conn()?;
        conn.execute("BEGIN", ()).await.map_err(map_turso_error)?;
        let result = async {
            let path = match scope {
                DocumentScopeRef::Library { .. } => normalize_path(path)?,
                DocumentScopeRef::Tmp => TmpDocumentSecret::parse(path)?.as_str().to_string(),
            };
            let document = match scope {
                DocumentScopeRef::Library { slug } => {
                    let library = self.require_library_conn(&conn, slug).await?;
                    self.document_conn(&conn, &library.id, &path).await?
                }
                DocumentScopeRef::Tmp => self.tmp_document_conn(&conn, &path).await?,
            };
            let stored_rows = load_block_tree_conn(&conn, &document.id).await?;
            let projection_missing = stored_rows.is_empty();
            let rows = if projection_missing
                && document_kind(&document.path, &document.version.content_type)
                    == DocumentKind::BlockDocument
            {
                let markdown = String::from_utf8(document.content.clone()).map_err(|_| {
                    QuarryError::InvalidInput(format!(
                        "document {} is not valid UTF-8 Markdown",
                        document.path
                    ))
                })?;
                let (_, body) = split_markdown_frontmatter(&markdown)?;
                canonical_rows(markdown_to_block_rows(body, || Uuid::new_v4().to_string())?)
            } else {
                stored_rows
            };
            let review_items = list_block_review_items_conn(&conn, &document.id).await?;
            let version_ids = document_version_ids_conn(&conn, &document.id).await?;
            let replay = block_transaction_conn(&conn, &document.id, client_tx_id).await?;
            Ok(BlockMutationState {
                document_id: document.id,
                path: document.path,
                head_version_id: document.version.id,
                content_type: document.version.content_type,
                metadata: document.version.metadata,
                rows,
                projection_missing,
                review_items,
                version_ids,
                replay,
            })
        }
        .await;
        finish_tx(&conn, result).await
    }

    /// Loads the consistent state a live session seeds from (or a checkpoint
    /// refreshes against), keyed by document id. Returns `Ok(None)` for
    /// missing/deleted documents and for RawDocuments (which never host
    /// sessions).
    pub async fn session_seed_state(&self, document_id: &str) -> Result<Option<SessionSeedState>> {
        let conn = self.conn()?;
        conn.execute("BEGIN", ()).await.map_err(map_turso_error)?;
        let result = async {
            let mut head_rows = conn
                .query(
                    "SELECT d.id, d.path, d.document_scope, l.slug, v.id, v.content_type, v.metadata_json,
                            v.content_hash, v.inline_content
                     FROM documents d
                     LEFT JOIN libraries l ON l.id = d.library_id
                     JOIN document_versions v ON v.id = d.head_version_id
                     WHERE d.id = ?1
                       AND d.deleted_at IS NULL
                       AND d.head_version_id IS NOT NULL
                       AND (
                         (d.document_scope = 'library' AND (d.expires_at IS NULL OR d.expires_at > ?2))
                         OR (d.document_scope = 'tmp' AND d.library_id IS NULL AND d.expires_at > ?2)
                       )
                     LIMIT 1",
                    params![document_id.to_string(), now_timestamp()],
                )
                .await
                .map_err(map_turso_error)?;
            let Some(row) = head_rows.next().await.map_err(map_turso_error)? else {
                return Ok(None);
            };
            let path = text(&row, 1)?;
            let scope = match text(&row, 2)?.as_str() {
                "library" => DocumentScopeRef::Library {
                    slug: opt_text(&row, 3)?.ok_or_else(|| {
                        QuarryError::Invariant(format!(
                            "library document {document_id} is missing a library slug"
                        ))
                    })?,
                },
                "tmp" => DocumentScopeRef::Tmp,
                other => {
                    return Err(QuarryError::Invariant(format!(
                        "document {document_id} has unsupported scope {other}"
                    )));
                }
            };
            let content_type = text(&row, 5)?;
            if document_kind(&path, &content_type) == DocumentKind::RawDocument {
                return Ok(None);
            }
            let content_hash = opt_text(&row, 7)?;
            let inline_content = opt_blob(&row, 8)?;
            let stored_rows = load_block_tree_conn(&conn, document_id).await?;
            let rows = if stored_rows.is_empty() {
                let content = match (inline_content, content_hash) {
                    (Some(bytes), None) => bytes,
                    (None, Some(hash)) => self.cas.read(&hash)?,
                    _ => {
                        return Err(QuarryError::Invariant(format!(
                            "head version for document {document_id} violates inline/CAS invariant"
                        )))
                    }
                };
                let markdown = String::from_utf8(content).map_err(|_| {
                    QuarryError::InvalidInput(format!(
                        "document {path} is not valid UTF-8 Markdown"
                    ))
                })?;
                let (_, body) = split_markdown_frontmatter(&markdown)?;
                canonical_rows(markdown_to_block_rows(body, || Uuid::new_v4().to_string())?)
            } else {
                stored_rows
            };
            let review_items = list_block_review_items_conn(&conn, document_id).await?;
            Ok(Some(SessionSeedState {
                document_id: text(&row, 0)?,
                scope,
                path,
                head_version_id: text(&row, 4)?,
                content_type,
                metadata: serde_json::from_str(&text(&row, 6)?)?,
                rows,
                review_items,
            }))
        }
        .await;
        finish_tx(&conn, result).await
    }

    /// Commits one semantic mutation atomically: replaces the row set and the
    /// review item set, publishes the normalized Markdown export as ONE new
    /// document version through the existing `document_versions` path (so
    /// legacy readers, history, and links keep working), and records the
    /// mutation history row — all in one SQL transaction. Emits the same
    /// document-changed events as other writes after the commit.
    ///
    /// A duplicate `client_tx_id` returns [`BlockMutationOutcome::Replayed`]
    /// without re-applying; a moved head fails with `PreconditionFailed` so
    /// the caller can reload state and recompute.
    pub async fn commit_block_mutation(
        &self,
        library: &str,
        commit: BlockMutationCommit,
    ) -> Result<BlockMutationOutcome> {
        self.commit_block_mutation_for_scope(&DocumentScopeRef::library(library), commit)
            .await
    }

    pub async fn commit_block_mutation_for_scope(
        &self,
        scope: &DocumentScopeRef,
        commit: BlockMutationCommit,
    ) -> Result<BlockMutationOutcome> {
        let mut commit = commit;
        if matches!(scope, DocumentScopeRef::Tmp) {
            let content_type =
                normalize_tmp_markdown_content_type(&commit.content_type)?.to_string();
            validate_tmp_markdown_text(&commit.normalized_markdown)?;
            commit.metadata = tmp_metadata_with_content_type(commit.metadata, &content_type);
            commit.content_type = content_type;
        }
        validate_review_items_against_rows(&commit.rows, &commit.review_items)?;
        let origin_id = commit.origin_id;
        let scope = scope.clone();
        let outcome = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let resolved_scope = store.resolve_document_scope_conn(conn, &scope).await?;
                    if let Some(replayed) =
                        block_transaction_conn(conn, &commit.document_id, &commit.client_tx_id)
                            .await?
                    {
                        return Ok(BlockMutationOutcome::Replayed(replayed));
                    }
                    let head =
                        document_head_for_scope_conn(conn, &resolved_scope, &commit.document_id)
                            .await?;
                    if head.head_version_id != commit.expected_head_version_id {
                        return Err(QuarryError::PreconditionFailed(format!(
                            "document {} head moved from {} to {}",
                            commit.document_id,
                            commit.expected_head_version_id,
                            head.head_version_id
                        )));
                    }
                    let provenance = commit.transaction_provenance.clone().unwrap_or_else(|| {
                        serde_json::json!({
                            "mode": "block_transaction",
                            "client_tx_id": commit.client_tx_id,
                        })
                    });
                    let tx = insert_transaction_conn(
                        conn,
                        resolved_scope.transaction_library_id(),
                        commit.source,
                        commit.transaction_actor.clone(),
                        commit.transaction_message.clone(),
                        provenance,
                    )
                    .await?;
                    let version = store
                        .insert_version_conn(
                            conn,
                            &commit.document_id,
                            &tx.id,
                            commit.normalized_markdown.clone().into_bytes(),
                            commit.metadata.clone(),
                            &commit.content_type,
                        )
                        .await?;
                    insert_change_conn(
                        conn,
                        &tx.id,
                        &head.path,
                        ChangeType::Put,
                        Some(&head.head_version_id),
                        Some(&version.id),
                        None,
                    )
                    .await?;
                    publish_put_conn(conn, &commit.document_id, &version.id).await?;
                    replace_block_rows_conn(conn, &commit.document_id, &commit.rows).await?;
                    replace_block_review_items_conn(
                        conn,
                        &commit.document_id,
                        &commit.review_items,
                    )
                    .await?;
                    let record = insert_block_transaction_conn(
                        conn,
                        &commit.document_id,
                        &commit.client_tx_id,
                        &commit.actor_kind,
                        commit.actor_id.clone(),
                        commit.recorded_ops.clone(),
                        Some(version.id.clone()),
                    )
                    .await?;
                    if let ResolvedDocumentScope::Library { id, .. } = &resolved_scope {
                        store.reindex_links_conn(conn, id).await?;
                    }
                    commit_transaction_record_conn(conn, &tx.id).await?;
                    let document = match &resolved_scope {
                        ResolvedDocumentScope::Library { id, .. } => {
                            store.document_entry_conn(conn, id, &head.path).await?
                        }
                        ResolvedDocumentScope::Tmp => {
                            store.tmp_document_entry_conn(conn, &head.path).await?
                        }
                    };
                    let tx = store.transaction_conn(conn, &tx.id).await?;
                    Ok(BlockMutationOutcome::Applied {
                        outcome: Box::new(WriteOutcome {
                            document,
                            version,
                            transaction: tx,
                        }),
                        record,
                    })
                })
            })
            .await?;
        if let BlockMutationOutcome::Applied { outcome, .. } = &outcome {
            self.emit_document_put_events(outcome, origin_id);
        }
        Ok(outcome)
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
        return Err(QuarryError::Invariant(format!(
            "document {document_id} has orphaned block rows"
        )));
    }
    Ok(ordered)
}

fn block_row_from_row(row: &Row) -> Result<BlockRow> {
    let position = row.get::<i64>(2).map_err(map_turso_error)?;
    let position = u32::try_from(position)
        .map_err(|_| QuarryError::Invariant(format!("block position {position} out of range")))?;
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
            .map_err(|_| QuarryError::Invariant(format!("anchor start {start_offset} invalid")))?,
        end_offset: u32::try_from(end_offset)
            .map_err(|_| QuarryError::Invariant(format!("anchor end {end_offset} invalid")))?,
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

async fn insert_block_transaction_conn(
    conn: &Connection,
    document_id: &str,
    client_tx_id: &str,
    actor_kind: &str,
    actor_id: Option<String>,
    ops: JsonValue,
    resulting_version_id: Option<String>,
) -> Result<BlockTransactionRecord> {
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

/// The review-item set is replaced wholesale at mutation commit; items carry
/// their ids and timestamps through unchanged so history-stable identifiers
/// survive text adjustments.
async fn replace_block_review_items_conn(
    conn: &Connection,
    document_id: &str,
    items: &[BlockReviewItem],
) -> Result<()> {
    conn.execute(
        "DELETE FROM block_review_items WHERE document_id = ?1",
        params![document_id.to_string()],
    )
    .await
    .map_err(map_turso_error)?;
    for item in items {
        conn.execute(
            "INSERT INTO block_review_items
             (id, document_id, block_id, kind, start_offset, end_offset, body, replacement,
              author, state, quote, context_before, context_after, parent_item_id,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            vec![
                Value::Text(item.id.clone()),
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
                Value::Text(item.created_at.clone()),
                Value::Text(item.updated_at.clone()),
            ],
        )
        .await
        .map_err(map_turso_error)?;
    }
    Ok(())
}

async fn list_block_review_items_conn(
    conn: &Connection,
    document_id: &str,
) -> Result<Vec<BlockReviewItem>> {
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

async fn document_version_ids_conn(
    conn: &Connection,
    document_id: &str,
) -> Result<HashSet<String>> {
    let mut rows = conn
        .query(
            "SELECT id FROM document_versions WHERE document_id = ?1",
            params![document_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    let mut ids = HashSet::new();
    while let Some(row) = rows.next().await.map_err(map_turso_error)? {
        ids.insert(text(&row, 0)?);
    }
    Ok(ids)
}

struct DocumentHeadRef {
    path: String,
    head_version_id: String,
}

enum ResolvedDocumentScope {
    Library { id: String },
    Tmp,
}

impl ResolvedDocumentScope {
    fn transaction_library_id(&self) -> &str {
        match self {
            Self::Library { id } => id,
            Self::Tmp => TMP_TRANSACTION_LIBRARY_ID,
        }
    }
}

impl QuarryStore {
    async fn resolve_document_scope_conn(
        &self,
        conn: &Connection,
        scope: &DocumentScopeRef,
    ) -> Result<ResolvedDocumentScope> {
        match scope {
            DocumentScopeRef::Library { slug } => {
                let library = self.require_library_conn(conn, slug).await?;
                Ok(ResolvedDocumentScope::Library { id: library.id })
            }
            DocumentScopeRef::Tmp => Ok(ResolvedDocumentScope::Tmp),
        }
    }
}

async fn document_head_conn(
    conn: &Connection,
    library_id: &str,
    document_id: &str,
) -> Result<DocumentHeadRef> {
    let mut rows = conn
        .query(
            "SELECT path, head_version_id FROM documents
             WHERE id = ?1 AND library_id = ?2 AND deleted_at IS NULL
               AND head_version_id IS NOT NULL
             LIMIT 1",
            params![document_id.to_string(), library_id.to_string()],
        )
        .await
        .map_err(map_turso_error)?;
    match rows.next().await.map_err(map_turso_error)? {
        Some(row) => Ok(DocumentHeadRef {
            path: text(&row, 0)?,
            head_version_id: text(&row, 1)?,
        }),
        None => Err(QuarryError::NotFound(format!("document {document_id}"))),
    }
}

async fn document_head_for_scope_conn(
    conn: &Connection,
    scope: &ResolvedDocumentScope,
    document_id: &str,
) -> Result<DocumentHeadRef> {
    match scope {
        ResolvedDocumentScope::Library { id } => document_head_conn(conn, id, document_id).await,
        ResolvedDocumentScope::Tmp => {
            let mut rows = conn
                .query(
                    "SELECT path, head_version_id FROM documents
                     WHERE id = ?1
                       AND document_scope = 'tmp'
                       AND library_id IS NULL
                       AND deleted_at IS NULL
                       AND head_version_id IS NOT NULL
                       AND expires_at > ?2
                     LIMIT 1",
                    params![document_id.to_string(), now_timestamp()],
                )
                .await
                .map_err(map_turso_error)?;
            match rows.next().await.map_err(map_turso_error)? {
                Some(row) => Ok(DocumentHeadRef {
                    path: text(&row, 0)?,
                    head_version_id: text(&row, 1)?,
                }),
                None => Err(QuarryError::NotFound(format!("document {document_id}"))),
            }
        }
    }
}

/// Internal invariant check before a mutation commit: every review item must
/// either anchor a live block with in-range boundary-aligned offsets, or be a
/// dead anchor (any non-open state) — open items never reference missing
/// blocks, and a collapsed range is only legal for insertions or their replies.
fn validate_review_items_against_rows(rows: &[BlockRow], items: &[BlockReviewItem]) -> Result<()> {
    let texts: HashMap<&str, &str> = rows
        .iter()
        .map(|row| (row.block_id.as_str(), row.text.as_str()))
        .collect();
    let items_by_id: HashMap<&str, &BlockReviewItem> =
        items.iter().map(|item| (item.id.as_str(), item)).collect();
    for item in items {
        if item.kind == BlockReviewKind::Conflict {
            // Conflict items (Phase 4) anchor by `after_block_id` in
            // `block_id` ("" = document start) with a collapsed placement
            // range; they carry Markdown payloads, not text anchors, so
            // row-anchored offset validation does not apply.
            continue;
        }
        let Some(text) = texts.get(item.block_id.as_str()) else {
            if item.state == BlockReviewState::Open {
                return Err(QuarryError::InvalidInput(format!(
                    "open review item {} anchors missing block {}",
                    item.id, item.block_id
                )));
            }
            continue;
        };
        if item.start_offset > item.end_offset
            || item.end_offset > utf16_len(text)
            || !is_utf16_boundary(text, item.start_offset)
            || !is_utf16_boundary(text, item.end_offset)
        {
            return Err(QuarryError::InvalidInput(format!(
                "review item {} has offsets [{}, {}) outside block {}",
                item.id, item.start_offset, item.end_offset, item.block_id
            )));
        }
        // A collapsed range is meaningful for an open INSERTION suggestion
        // (the live-session "type in suggesting mode" shape): nothing is
        // anchored, but the replacement text is the proposal. Replies to that
        // suggestion inherit the same collapsed anchor.
        let collapsed_insertion_reply = is_reply_to_open_insertion_suggestion(item, &items_by_id);
        if item.start_offset == item.end_offset
            && item.state == BlockReviewState::Open
            && !is_open_insertion_suggestion(item)
            && !collapsed_insertion_reply
        {
            return Err(QuarryError::InvalidInput(format!(
                "open review item {} has a collapsed range",
                item.id
            )));
        }
    }
    Ok(())
}

fn is_open_insertion_suggestion(item: &BlockReviewItem) -> bool {
    item.kind == BlockReviewKind::Suggestion
        && item.state == BlockReviewState::Open
        && item.start_offset == item.end_offset
        && item
            .replacement
            .as_deref()
            .is_some_and(|replacement| !replacement.is_empty())
}

fn is_reply_to_open_insertion_suggestion(
    item: &BlockReviewItem,
    items_by_id: &HashMap<&str, &BlockReviewItem>,
) -> bool {
    if item.kind != BlockReviewKind::Comment || item.parent_item_id.is_none() {
        return false;
    }
    let Some(parent) = item
        .parent_item_id
        .as_deref()
        .and_then(|parent_id| items_by_id.get(parent_id))
    else {
        return false;
    };
    is_open_insertion_suggestion(parent)
        && parent.document_id == item.document_id
        && parent.block_id == item.block_id
        && parent.start_offset == item.start_offset
        && parent.end_offset == item.end_offset
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
