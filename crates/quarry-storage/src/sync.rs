use crate::{QuarryStore, StoreEvent, map_turso_error, row::sync_state_from_row, row::text};

use quarry_core::{GitPeer, Result, SyncStateEntry, normalize_path};
use serde_json::Value as JsonValue;
use turso::{Value, params};
use uuid::Uuid;

impl QuarryStore {
    pub async fn emit_git_sync_completed(
        &self,
        library: &str,
        peer_id: &str,
        applied: usize,
        conflicts: usize,
    ) -> Result<()> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        self.emit_event(StoreEvent::git_sync_completed(
            library.id,
            peer_id.to_string(),
            applied,
            conflicts,
        ));
        Ok(())
    }

    pub async fn create_git_peer(&self, library: &str, config: JsonValue) -> Result<GitPeer> {
        let library = library.to_string();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
                let library = Self::require_library_conn(conn, &library).await?;
                let peer = GitPeer {
                    id: Uuid::new_v4().to_string(),
                    library_id: library.id,
                    kind: "git".to_string(),
                    config,
                };
                conn.execute(
                    "INSERT INTO sync_peers (id, library_id, kind, config_json) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        peer.id.clone(),
                        peer.library_id.clone(),
                        peer.kind.clone(),
                        peer.config.to_string()
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                Ok(peer)
            })
        })
        .await
    }

    pub async fn list_git_peers(&self, library: &str) -> Result<Vec<GitPeer>> {
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let mut rows = conn
            .query(
                "SELECT id, library_id, kind, config_json FROM sync_peers
                 WHERE library_id = ?1 AND kind = 'git' ORDER BY id",
                params![library.id],
            )
            .await
            .map_err(map_turso_error)?;
        let mut peers = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            peers.push(GitPeer {
                id: text(&row, 0)?,
                library_id: text(&row, 1)?,
                kind: text(&row, 2)?,
                config: serde_json::from_str(&text(&row, 3)?)?,
            });
        }
        Ok(peers)
    }

    pub async fn sync_state(&self, peer_id: &str, path: &str) -> Result<Option<SyncStateEntry>> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT peer_id, path, last_synced_doc_version_id, last_synced_git_oid
                 FROM sync_state WHERE peer_id = ?1 AND path = ?2 LIMIT 1",
                params![peer_id.to_string(), path],
            )
            .await
            .map_err(map_turso_error)?;
        if let Some(row) = rows.next().await.map_err(map_turso_error)? {
            Ok(Some(sync_state_from_row(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_sync_state(&self, peer_id: &str) -> Result<Vec<SyncStateEntry>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT peer_id, path, last_synced_doc_version_id, last_synced_git_oid
                 FROM sync_state WHERE peer_id = ?1 ORDER BY path",
                params![peer_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            entries.push(sync_state_from_row(&row)?);
        }
        Ok(entries)
    }

    pub async fn upsert_sync_state(
        &self,
        peer_id: &str,
        path: &str,
        last_synced_doc_version_id: Option<String>,
        last_synced_git_oid: Option<String>,
    ) -> Result<SyncStateEntry> {
        let path = normalize_path(path)?;
        let peer_id = peer_id.to_string();
        self.write_transaction(move |_store, conn| {
            Box::pin(async move {
                conn.execute(
                    "INSERT INTO sync_state
                     (peer_id, path, last_synced_doc_version_id, last_synced_git_oid)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(peer_id, path)
                     DO UPDATE SET
                        last_synced_doc_version_id = excluded.last_synced_doc_version_id,
                        last_synced_git_oid = excluded.last_synced_git_oid",
                    vec![
                        Value::Text(peer_id.clone()),
                        Value::Text(path.clone()),
                        optional_text(last_synced_doc_version_id.clone()),
                        optional_text(last_synced_git_oid.clone()),
                    ],
                )
                .await
                .map_err(map_turso_error)?;
                Ok(SyncStateEntry {
                    peer_id,
                    path,
                    last_synced_doc_version_id: last_synced_doc_version_id.map(Into::into),
                    last_synced_git_oid,
                })
            })
        })
        .await
    }
}

fn optional_text(value: Option<String>) -> Value {
    value.map(Value::Text).unwrap_or(Value::Null)
}
