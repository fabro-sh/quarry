use crate::{
    DirectoryMetadata, QuarryStore, ensure_inode_conn, map_turso_error,
    move_path_prefix_inodes_conn, replace_path_prefix,
    row::{directory_metadata_from_row, int, opt_int, text},
};
use quarry_core::{
    DocumentSource, QuarryError, Result, normalize_path, now_timestamp, parent_dirs,
};
use turso::{Connection, Value, params};

impl QuarryStore {
    pub async fn ensure_directory(
        &self,
        library: &str,
        path: &str,
        mode: Option<i64>,
    ) -> Result<DirectoryMetadata> {
        let path = normalize_directory_path(path)?;
        let library = library.to_string();
        let event_path = path.clone();
        let (metadata, library_id) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    ensure_inode_conn(conn, &library.id, "").await?;
                    if !path.is_empty() {
                        for dir in directory_path_and_parents(&path) {
                            ensure_inode_conn(conn, &library.id, &dir).await?;
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
                    let metadata = store
                        .directory_metadata_conn(conn, &library.id, &path)
                        .await?;
                    Ok((metadata, library_id))
                })
            })
            .await?;
        self.emit_event(crate::StoreEvent::directory_put(
            library_id,
            event_path,
            DocumentSource::Fuse,
        ));
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
        let library = library.to_string();
        let event_path = path.clone();
        let mtime = mtime.map(str::to_string);
        let source_for_event = source.clone();
        let (metadata, library_id) = self
            .write_transaction(move |store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    let updated = conn
                        .execute(
                            "UPDATE dir_metadata
                     SET mode = COALESCE(?1, mode),
                         mtime = COALESCE(?2, mtime)
                     WHERE library_id = ?3 AND path = ?4",
                            vec![
                                mode.map(Value::Integer).unwrap_or(Value::Null),
                                mtime.map(Value::Text).unwrap_or(Value::Null),
                                Value::Text(library.id.clone()),
                                Value::Text(path.clone()),
                            ],
                        )
                        .await
                        .map_err(map_turso_error)?;
                    if updated == 0 {
                        return Err(QuarryError::NotFound(path.clone()));
                    }
                    let metadata = store
                        .directory_metadata_conn(conn, &library.id, &path)
                        .await?;
                    Ok((metadata, library_id))
                })
            })
            .await?;
        self.emit_event(crate::StoreEvent::directory_put(
            library_id,
            event_path,
            source_for_event,
        ));
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
        let library = library.to_string();
        let event_from_path = from_path.clone();
        let event_to_path = to_path.clone();
        let source_for_event = source.clone();
        let library_id = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
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
                     WHERE document_scope = 'library'
                       AND library_id = ?1
                       AND deleted_at IS NULL
                       AND head_version_id IS NOT NULL
                       AND (expires_at IS NULL OR expires_at > ?2)
                       AND path LIKE ?3
                     LIMIT 1",
                            params![
                                library.id.clone(),
                                now_timestamp(),
                                format!("{from_prefix}%")
                            ],
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
                    move_path_prefix_inodes_conn(conn, &library.id, &from_path, &to_path).await?;
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
                })
            })
            .await?;
        self.emit_event(crate::StoreEvent::directory_move(
            library_id,
            event_from_path,
            event_to_path,
            source_for_event,
        ));
        Ok(())
    }

    pub async fn remove_directory(&self, library: &str, path: &str) -> Result<()> {
        let path = normalize_directory_path(path)?;
        if path.is_empty() {
            return Err(QuarryError::Conflict(
                "cannot remove root directory".to_string(),
            ));
        }
        let library = library.to_string();
        let event_path = path.clone();
        let library_id = self
            .write_transaction(move |_store, conn| {
                Box::pin(async move {
                    let library = Self::require_library_conn(conn, &library).await?;
                    let library_id = library.id.clone();
                    conn.execute(
                        "DELETE FROM dir_metadata WHERE library_id = ?1 AND path = ?2",
                        params![library.id, path.clone()],
                    )
                    .await
                    .map_err(map_turso_error)?;
                    Ok(library_id)
                })
            })
            .await?;
        self.emit_event(crate::StoreEvent::directory_delete(
            library_id,
            event_path,
            DocumentSource::Fuse,
        ));
        Ok(())
    }

    pub async fn list_directories(
        &self,
        library: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<DirectoryMetadata>> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
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
        let library = Self::require_library_conn(&conn, library).await?;
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
            .ok_or(QuarryError::NotFound(path))
    }

    pub async fn path_for_inode(&self, library: &str, inode: i64) -> Result<String> {
        if inode <= 0 {
            return Err(QuarryError::InvalidPath(format!("invalid inode {inode}")));
        }
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
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
