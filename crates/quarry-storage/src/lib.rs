mod blocks;
mod directories;
mod events;
mod libraries;
mod links;
mod row;
mod schema;
mod search;
mod store;
mod sync;
mod versions;

pub use blocks::{
    BlockMarkdownWrite, BlockMarkdownWriteOutcome, BlockMarkdownWriter, BlockMutationCommit,
    BlockMutationOutcome, BlockMutationState, BlockReviewItem, BlockReviewKind, BlockReviewState,
    BlockShadowBase, BlockTransactionRecord, BlockWriteBase, DocumentKind, DocumentScopeRef,
    NewBlockReviewItem, SessionSeedState, document_kind,
};
pub use events::{StoreEvent, StoreEventKind};
/// Re-exported because the store's block APIs speak it.
pub use quarry_collab_codec::BlockRow;
use row::{
    collab_invite_token_from_row, conflict_from_row, document_entry_from_row, int, opt_blob,
    opt_text, text, transaction_from_row,
};
use schema::{
    ensure_document_indexes_conn, ensure_links_resolution_status_column,
    migrate_documents_scope_ttl,
};
pub use store::{GlobalOperationGuard, QuarryStore, StorageError, StoreConfig};
pub(crate) use store::{begin_immediate, finish_tx, map_turso_error};
pub use versions::group_version_history;

use chrono::Utc;
use quarry_core::{
    ChangeType, CollabInviteToken, ConflictRecord, ConflictStatus, Document, DocumentHistoryEntry,
    DocumentListEntry, DocumentSource, DocumentVersion, DocumentVersionContent, GcReport,
    INLINE_CONTENT_THRESHOLD, QuarryError, Result, TransactionRecord, TransactionState,
    WriteOutcome, WritePrecondition, normalize_path, now_timestamp, parent_dirs,
};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::time::Instant;
use turso::{Connection, Row, Value, params};
use uuid::Uuid;

const TMP_TRANSACTION_LIBRARY_ID: &str = "__tmp__";
pub const TMP_DOCUMENT_SECRET_LEN: usize = 32;
pub const TMP_DOCUMENT_MARKDOWN_MAX_BYTES: usize = 1024 * 1024;
pub const TMP_DOCUMENT_DEFAULT_CONTENT_TYPE: &str = "text/markdown";

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

struct StagedChange {
    path: String,
    change_type: ChangeType,
    old_version_id: Option<String>,
    new_version_id: Option<String>,
    new_path: Option<String>,
}

fn precondition_name(precondition: &WritePrecondition) -> &'static str {
    match precondition {
        WritePrecondition::None => "none",
        WritePrecondition::IfMatch(_) => "if_match",
        WritePrecondition::IfNoneMatch => "if_none_match",
    }
}

impl QuarryStore {
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

    pub(crate) async fn migrate(&self) -> Result<()> {
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

    pub(crate) fn conn(&self) -> Result<Connection> {
        Ok(self.db.connect().map_err(map_turso_error)?)
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

fn display_name_from_path(path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    file_name
        .strip_suffix(".md")
        .or_else(|| file_name.strip_suffix(".markdown"))
        .unwrap_or(file_name)
        .to_string()
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

fn parse_storage_enum<T>(value: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|err| QuarryError::Invariant(err.to_string()))
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
