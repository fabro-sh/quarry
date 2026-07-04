use super::*;

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
}
