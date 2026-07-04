use super::*;

struct StagedChange {
    path: String,
    change_type: ChangeType,
    old_version_id: Option<String>,
    new_version_id: Option<String>,
    new_path: Option<String>,
}

impl QuarryStore {
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
}
