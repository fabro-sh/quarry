//! Derived block and review indexes for native Markdown documents.
//!
//! Automerge owns content, structure, and review targets. These rows support
//! queries and the public block API. Every publication updates native state,
//! its indexes, Markdown versions, and command receipts in one SQL transaction.
//! Raw documents retain their byte representation. Markdown import creates
//! native identity once; later whole-file writes reconcile through commands.

use super::*;
use quarry_core::render_markdown_frontmatter;
use quarry_markdown::{BlockRow, LinkRange, MarkRun, block_rows_to_markdown};
use std::collections::HashMap;
use std::sync::Arc;

/// How a document participates in the block model. `BlockDocument`s are
/// canonical in Automerge; `RawDocument`s keep the untouched byte path.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// `context_after` discriminator for a structural suggestion whose
/// `replacement` is a Markdown fragment and whose `block_id` is the
/// top-level block after which the fragment should be inserted (empty means
/// document start). Review items already use kind-specific payload columns;
/// versioning this marker avoids guessing from an inline replacement.
pub const MARKDOWN_INSERT_SUGGESTION_CONTEXT: &str = "quarry:markdown_insert:v1";

impl BlockReviewItem {
    /// Whether this suggestion proposes inserting a parsed Markdown fragment
    /// between top-level blocks rather than replacing inline text.
    pub fn is_markdown_insert_suggestion(&self) -> bool {
        self.kind == BlockReviewKind::Suggestion
            && self.context_after.as_deref() == Some(MARKDOWN_INSERT_SUGGESTION_CONTEXT)
    }

    /// Whether this suggestion proposes deleting its anchored block and
    /// descendants rather than replacing an inline text range.
    pub fn is_block_delete_suggestion(&self) -> bool {
        self.kind == BlockReviewKind::Suggestion
            && self.replacement.is_none()
            && !self.is_markdown_insert_suggestion()
    }
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

/// A whole-file Markdown write translated into native document commands.
/// Git, FUSE and CLI adapters supply their last observed Markdown as the base.
/// The serving process installs the shared command gateway; see
/// [`QuarryStore::set_block_markdown_writer`].
#[derive(Clone, Debug)]
pub struct BlockMarkdownWrite {
    pub scope: DocumentScopeRef,
    pub path: String,
    /// An existing document's identity, when the writer holds a file handle.
    /// Resolve its current path within the write operation. A deleted identity
    /// must fail, even if another document now occupies the original path.
    pub document_id: Option<String>,
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
    /// No original read version is available. Create or byte-identical no-op
    /// only; an existing document must never be overwritten on this basis.
    Unversioned,
    /// The caller has verified a strict head precondition under the write
    /// gate, or is performing an explicit version restore. File adapters
    /// without an original read version must use `Unversioned`.
    CurrentCanonical,
    /// A stored shadow base: Git peer bases, FUSE open-handle bases, or an
    /// explicit REST merge-base version. The full text (frontmatter tolerated);
    /// `version_id` engages the gateway's rebase acks when it names a known
    /// version.
    Markdown {
        markdown: String,
        version_id: Option<String>,
    },
}

/// One open diff3 artifact produced or reused by a whole-file write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockMarkdownConflict {
    pub id: String,
    pub after_block_id: Option<String>,
    pub base_markdown: String,
    pub incoming_markdown: String,
    pub canonical_markdown: String,
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
    /// Stable ids and hunk payloads for the open conflicts relevant to this
    /// write. Repeated identical merges reuse the existing item.
    pub conflict_items: Vec<BlockMarkdownConflict>,
}

/// The whole-file Markdown adapter. Reconciliation failures never fail the
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

/// One consistent read of everything the command gateway needs to apply a
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
    pub review_items: Vec<BlockReviewItem>,
    pub version_ids: HashSet<String>,
    pub replay: Option<BlockTransactionRecord>,
}

/// The computed final state of one semantic mutation, committed atomically by
/// [`QuarryStore::commit_durable_document_for_scope`]: the full row set, the full review
/// item set, the normalized Markdown export of those rows (published through
/// the existing `document_versions` path so version readers/events keep
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
    /// Display actor recorded on the `transactions` history row.
    pub transaction_actor: Option<String>,
    /// Optional message and provenance for the transaction history.
    pub transaction_message: Option<String>,
    pub transaction_provenance: Option<JsonValue>,
    /// Identifies the writer on the emitted `doc.changed` event.
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
    /// Resolve an active document by native id within the caller's scope.
    pub async fn document_path_for_scope_id(
        &self,
        scope: &DocumentScopeRef,
        id: &str,
    ) -> Result<String> {
        let conn = self.conn()?;
        let scope = self.resolve_document_scope_conn(&conn, scope).await?;
        Ok(document_head_for_scope_conn(&conn, &scope, id).await?.path)
    }
    /// Installs the whole-file Markdown write path. The serving
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
    /// Markdown writes require native commands that preserve text identity.
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

    /// Imports a Markdown document as canonical block rows.
    ///
    /// Frontmatter merges into document metadata (the existing mechanism);
    /// the body becomes rows via the codec, falling back to `raw_markdown`
    /// rows for safe unsupported constructs and surfacing the codec's typed
    /// [`quarry_markdown::Unsupported`] error for unsafe ones. The
    /// deterministic normalized export is written through the existing
    /// version path in the same transaction, so rows and version content
    /// always agree and version readers see the same content.
    #[expect(
        clippy::too_many_arguments,
        reason = "commit rows mirror storage columns"
    )]
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
    /// `origin_id` echoed on the emitted `doc.changed` event and transaction attribution recorded on the import
    /// transaction. Unset provenance defaults per scope.
    #[expect(
        clippy::too_many_arguments,
        reason = "review rows mirror storage columns"
    )]
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
        self.import_block_document_with_native(
            scope,
            path,
            markdown,
            metadata,
            content_type,
            source,
            precondition,
            origin_id,
            transaction,
            None,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "import carries the same publication fields as the public storage API"
    )]
    pub(crate) async fn import_block_document_with_native(
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
        native: Option<Box<quarry_document::Document>>,
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
        // Validate through the same importer used by every document creator.
        // Review syntax must reach publication intact so its native targets
        // and discussions are created together with the body.
        let imported = Box::new(quarry_markdown::import_markdown_document(
            "validation",
            body,
            &mut || Uuid::new_v4().to_string(),
        )?);
        let mut merged_metadata = frontmatter;
        merge_json(&mut merged_metadata, metadata.clone());
        let normalized = format!(
            "{}{}",
            render_markdown_frontmatter(&merged_metadata)?,
            if imported
                .comments()
                .map_err(crate::document_state::document_error)?
                .is_empty()
                && imported
                    .proposals()
                    .map_err(crate::document_state::document_error)?
                    .is_empty()
            {
                quarry_markdown::document_to_markdown(&imported)?
            } else {
                body.to_string()
            }
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
                    if let Some(native) = &native {
                        if old_version_id.is_some()
                            || document_state::load_state_conn(conn, &doc_id)
                                .await?
                                .is_some()
                        {
                            return Err(QuarryError::PreconditionFailed(
                                "An archive can only create a new document".into(),
                            ));
                        }
                        let mut copy = native
                            .copy_as(&doc_id)
                            .map_err(document_state::document_error)?;
                        crate::document_write::persist_projection(
                            conn,
                            &doc_id,
                            &version.id,
                            &mut copy,
                        )
                        .await?;
                        crate::publish_native_put_conn(conn, &doc_id, &version.id).await?;
                    } else {
                        store
                            .publish_document_version_conn(conn, &doc_id, &version.id)
                            .await?;
                    }
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
                    let tx = QuarryStore::transaction_conn(conn, &tx.id).await?;
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

    /// Export the native document's Markdown projection with its metadata.
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
            let saved = document_state::load_state_conn(&conn, document_id)
                .await?
                .ok_or_else(|| {
                    QuarryError::Invariant("Markdown document has no native state".into())
                })?;
            let document = quarry_document::Document::load(&saved.bytes)
                .map_err(document_state::document_error)?;
            let rows = document_projection(&document)?;
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
    /// character boundaries (never inside a surrogate pair). A collapsed
    /// range is legal for orphaned anchors and structural block-deletion
    /// suggestions.
    pub async fn put_block_review_item(&self, item: NewBlockReviewItem) -> Result<BlockReviewItem> {
        let conn = self.conn()?;
        let mut rows = conn.query("SELECT d.document_scope,d.path,l.slug FROM documents d LEFT JOIN libraries l ON l.id=d.library_id WHERE d.id=?1", params![item.document_id.clone()]).await.map_err(map_turso_error)?;
        let row = rows
            .next()
            .await
            .map_err(map_turso_error)?
            .ok_or_else(|| QuarryError::NotFound(item.document_id.clone()))?;
        let scope = if text(&row, 0)? == "tmp" {
            DocumentScopeRef::Tmp
        } else {
            DocumentScopeRef::library(&text(&row, 2)?)
        };
        let path = text(&row, 1)?;
        let saved = self
            .durable_document_for_scope(&scope, &path)
            .await?
            .ok_or_else(|| {
                QuarryError::Unsupported("Review requires a Markdown document".into())
            })?;
        let document =
            Box::new(quarry_document::Document::load(&saved.bytes).map_err(crate::command_error)?);
        let id = Uuid::new_v4().to_string();
        let author = item.author.unwrap_or_default();
        let mut commands = Vec::new();
        use quarry_document::Command;
        match item.kind {
            BlockReviewKind::Comment => {
                if let Some(parent) = item.parent_item_id {
                    commands.push(Command::ReplyComment {
                        id: id.clone(),
                        parent,
                        author: author.clone(),
                        body: item.body.unwrap_or_default(),
                    });
                } else {
                    let ranges = document
                        .selection(
                            &item.block_id,
                            item.start_offset as usize,
                            item.end_offset as usize,
                        )
                        .map_err(crate::command_error)?;
                    commands.push(Command::AddComment {
                        id: id.clone(),
                        author: author.clone(),
                        body: item.body.unwrap_or_default(),
                        ranges,
                    });
                }
                if item.state == BlockReviewState::Resolved {
                    commands.push(Command::ResolveComment {
                        id: id.clone(),
                        resolved: true,
                    });
                } else if item.state != BlockReviewState::Open {
                    return Err(QuarryError::InvalidInput("New comments require a current target; missing targets belong to document import".into()));
                }
            }
            BlockReviewKind::Suggestion => {
                commands.push(Command::ProposeReplacement {
                    id: id.clone(),
                    author: author.clone(),
                    block: item.block_id.clone(),
                    at: document
                        .point(&item.block_id, item.start_offset as usize)
                        .map_err(crate::command_error)?,
                    ranges: document
                        .selection(
                            &item.block_id,
                            item.start_offset as usize,
                            item.end_offset as usize,
                        )
                        .map_err(crate::command_error)?,
                    text: item.replacement.unwrap_or_default(),
                });
                if let Some(body) = item.body {
                    commands.push(Command::EditProposal {
                        id: id.clone(),
                        body,
                    });
                }
            }
            BlockReviewKind::Conflict => commands.push(Command::AddConflict {
                conflict: quarry_document::Conflict {
                    id: id.clone(),
                    after: (!item.block_id.is_empty()).then_some(item.block_id),
                    base: item.context_before.unwrap_or_default(),
                    incoming: item.body.unwrap_or_default(),
                    canonical: item.quote.unwrap_or_default(),
                    resolved: item.state == BlockReviewState::Resolved,
                    author: author.clone(),
                    metadata: Default::default(),
                },
            }),
        }
        let request_id = Uuid::new_v4().to_string();
        self.apply_document_commands(
            &scope,
            &path,
            &quarry_document::CommandBatch {
                request_id: request_id.clone(),
                actor: quarry_document::DocumentActor {
                    kind: "api".into(),
                    id: None,
                    label: Some(author),
                },
                requests: vec![quarry_document::CommandRequest {
                    request_id,
                    base: document.heads().iter().map(ToString::to_string).collect(),
                    commands,
                    at: now_timestamp(),
                }],
            },
        )
        .await?;
        self.list_block_review_items(&item.document_id)
            .await?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| QuarryError::Invariant("Committed review item is missing".into()))
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
        let surface = surface.to_string();
        let scope_key = scope_key.to_string();
        let document_id = document_id.to_string();
        let base_markdown = base_markdown.to_string();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
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
                        Value::Text(surface.clone()),
                        Value::Text(scope_key.clone()),
                        Value::Text(document_id.clone()),
                        Value::Text(base_markdown.clone()),
                        opt_value(base_version_id.clone()),
                        Value::Text(updated_at.clone()),
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                Ok(BlockShadowBase {
                    surface,
                    scope_key,
                    document_id,
                    base_markdown,
                    base_version_id,
                    updated_at,
                })
            })
        })
        .await
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

    pub async fn block_transaction(
        &self,
        document_id: &str,
        client_tx_id: &str,
    ) -> Result<Option<BlockTransactionRecord>> {
        let conn = self.conn()?;
        block_transaction_conn(&conn, document_id, client_tx_id).await
    }

    /// Loads native state, its projections and receipts from one SQL snapshot.
    /// Markdown documents require native state. Raw documents have no blocks.
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
                    let library = QuarryStore::require_library_conn(&conn, slug).await?;
                    self.document_conn(&conn, &library.id, &path).await?
                }
                DocumentScopeRef::Tmp => self.tmp_document_conn(&conn, &path).await?,
            };
            let saved = document_state::load_state_conn(&conn, &document.id).await?;
            let (rows, review_items) = if let Some(saved) = saved {
                if saved.version_id != document.version.id.as_str() {
                    return Err(QuarryError::Invariant(
                        "Native state and published version disagree".into(),
                    ));
                }
                let native = quarry_document::Document::load(&saved.bytes)
                    .map_err(document_state::document_error)?;
                (
                    document_projection(&native)?,
                    crate::document_review_projection(&native)?,
                )
            } else if document_kind(&document.path, &document.version.content_type)
                == DocumentKind::RawDocument
            {
                (Vec::new(), Vec::new())
            } else {
                return Err(QuarryError::Invariant(
                    "Markdown document has no native state".into(),
                ));
            };
            let version_ids = document_version_ids_conn(&conn, &document.id).await?;
            let replay = block_transaction_conn(&conn, &document.id, client_tx_id).await?;
            Ok(BlockMutationState {
                document_id: document.id.to_string(),
                path: document.path,
                head_version_id: document.version.id.to_string(),
                content_type: document.version.content_type,
                metadata: document.version.metadata,
                rows,
                review_items,
                version_ids,
                replay,
            })
        }
        .await;
        finish_tx(&conn, result).await
    }

    pub async fn commit_durable_document_for_scope(
        &self,
        scope: &DocumentScopeRef,
        commit: BlockMutationCommit,
        durable: DurableDocumentCommit,
    ) -> Result<BlockMutationOutcome> {
        if matches!(scope, DocumentScopeRef::Tmp) {
            validate_tmp_markdown_text(&commit.normalized_markdown)?;
        }
        let document = quarry_document::Document::load(&durable.bytes)
            .map_err(document_state::document_error)?;
        if document.id().map_err(document_state::document_error)? != commit.document_id {
            return Err(QuarryError::InvalidInput(
                "Document state belongs to another document".into(),
            ));
        }
        if durable.request_hash.is_empty() {
            return Err(QuarryError::InvalidInput(
                "Document command requires a request hash".into(),
            ));
        }
        if document_projection(&document)? != commit.rows {
            return Err(QuarryError::InvalidInput(
                "Block projection does not match the canonical document".into(),
            ));
        }
        if crate::document_review_projection(&document)? != commit.review_items {
            return Err(QuarryError::InvalidInput(
                "Review projection does not match the canonical document".into(),
            ));
        }
        let body = block_rows_to_markdown(&commit.rows).map_err(document_state::document_error)?;
        let markdown = format!("{}{}", render_markdown_frontmatter(&commit.metadata)?, body);
        if markdown != commit.normalized_markdown {
            return Err(QuarryError::InvalidInput(
                "Markdown projection does not match the canonical document".into(),
            ));
        }
        self.commit_document_mutation_inner(scope, commit, durable)
            .await
    }

    async fn commit_document_mutation_inner(
        &self,
        scope: &DocumentScopeRef,
        commit: BlockMutationCommit,
        durable: DurableDocumentCommit,
    ) -> Result<BlockMutationOutcome> {
        let mut commit = commit;
        if matches!(scope, DocumentScopeRef::Tmp) {
            let content_type =
                normalize_tmp_markdown_content_type(&commit.content_type)?.to_string();
            validate_tmp_markdown_text(&commit.normalized_markdown)?;
            commit.metadata = tmp_metadata_with_content_type(commit.metadata, &content_type);
            commit.content_type = content_type;
        }
        let origin_id = commit.origin_id;
        let scope = scope.clone();
        let outcome = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let resolved_scope = store.resolve_document_scope_conn(conn, &scope).await?;
                    let head =
                        document_head_for_scope_conn(conn, &resolved_scope, &commit.document_id)
                            .await?;
                    if let Some(replayed) =
                        block_transaction_conn(conn, &commit.document_id, &commit.client_tx_id)
                            .await?
                    {
                        if document_state::receipt_hash_conn(
                            conn,
                            &commit.document_id,
                            &commit.client_tx_id,
                        )
                        .await?
                        .as_deref()
                            != Some(durable.request_hash.as_str())
                        {
                            return Err(QuarryError::PreconditionFailed(
                                "Idempotency key was used for a different request".into(),
                            ));
                        }
                        return Ok(BlockMutationOutcome::Replayed(replayed));
                    }
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
                    crate::publish_native_put_conn(conn, &commit.document_id, &version.id).await?;
                    replace_block_rows_conn(conn, &commit.document_id, &commit.rows).await?;
                    replace_block_review_items_conn(
                        conn,
                        &commit.document_id,
                        &commit.review_items,
                    )
                    .await?;
                    {
                        document_state::write_state_conn(
                            conn,
                            &commit.document_id,
                            &version.id,
                            &commit.client_tx_id,
                            &durable,
                        )
                        .await?;
                    }
                    let record = insert_block_transaction_conn(
                        conn,
                        &commit.document_id,
                        &commit.client_tx_id,
                        &commit.actor_kind,
                        commit.actor_id.clone(),
                        commit.recorded_ops.clone(),
                        Some(version.id.to_string()),
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
                    let tx = QuarryStore::transaction_conn(conn, &tx.id).await?;
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
pub(crate) async fn replace_block_review_items_conn(
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

pub(crate) async fn list_block_review_items_conn(
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
                let library = QuarryStore::require_library_conn(conn, slug).await?;
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

pub(crate) struct BlockDocumentHead {
    path: String,
    content_type: String,
    pub(crate) metadata: JsonValue,
}

pub(crate) async fn head_version_head_conn(
    conn: &Connection,
    document_id: &str,
) -> Result<BlockDocumentHead> {
    let mut rows = conn
        .query(
            "SELECT d.path, v.content_type, v.metadata_json
             FROM documents d
             JOIN document_versions v ON v.id = d.head_version_id
             WHERE d.id = ?1 AND d.deleted_at IS NULL AND (d.expires_at IS NULL OR d.expires_at > ?2)
             LIMIT 1",
            params![document_id.to_string(), now_timestamp()],
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

pub(crate) async fn block_transaction_conn(
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
