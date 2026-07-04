use super::*;

pub const TMP_DOCUMENT_SECRET_LEN: usize = 32;
pub const TMP_DOCUMENT_MARKDOWN_MAX_BYTES: usize = 1024 * 1024;
pub const TMP_DOCUMENT_DEFAULT_CONTENT_TYPE: &str = "text/markdown";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TmpTtl {
    Default,
    Unchanged,
    ExpiresAt(String),
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
pub(crate) struct TmpDocumentSecret(String);

impl TmpDocumentSecret {
    pub(crate) fn generate() -> Self {
        Self(Uuid::new_v4().simple().to_string())
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if !is_tmp_document_secret(value) {
            return Err(QuarryError::InvalidPath(
                "invalid tmp document secret".to_string(),
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl QuarryStore {
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

pub(crate) fn validate_tmp_markdown_text(markdown: &str) -> Result<()> {
    validate_tmp_markdown_bytes(markdown.as_bytes())
}

pub(crate) fn tmp_metadata_with_content_type(
    mut metadata: JsonValue,
    content_type: &str,
) -> JsonValue {
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
