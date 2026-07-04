use crate::{
    PutDocumentRequest, QuarryStore, TransactionMetadata, map_turso_error, row::version_from_row,
    text,
};

use chrono::{DateTime, Utc};
use quarry_core::{
    DocumentHistoryEntry, DocumentSource, DocumentVersion, DocumentVersionContent, QuarryError,
    Result, VersionDiff, WriteOutcome, WritePrecondition, normalize_path,
};
use serde_json::Value as JsonValue;
use turso::{Connection, params};

impl QuarryStore {
    pub async fn version_history(
        &self,
        library: &str,
        path: &str,
    ) -> Result<Vec<DocumentHistoryEntry>> {
        let raw = self.raw_version_history(library, path).await?;
        Ok(group_version_history(raw))
    }

    pub async fn raw_version_history(
        &self,
        library: &str,
        path: &str,
    ) -> Result<Vec<DocumentVersion>> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        self.raw_version_history_for_document_conn(&conn, &document_id)
            .await
    }

    pub(crate) async fn raw_version_history_for_document_conn(
        &self,
        conn: &Connection,
        document_id: &str,
    ) -> Result<Vec<DocumentVersion>> {
        let mut rows = conn
            .query(
                "SELECT v.id, v.document_id, v.tx_id, v.content_hash, v.inline_content, v.metadata_json,
                        v.content_type, v.byte_size, v.created_at, t.source, t.actor, t.message, t.provenance_json
                 FROM document_versions v
                 JOIN transactions t ON t.id = v.tx_id
                 WHERE v.document_id = ?1 ORDER BY v.created_at DESC, v.id DESC",
                params![document_id],
            )
            .await
            .map_err(map_turso_error)?;
        let mut versions = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_turso_error)? {
            versions.push(version_from_row(&row)?);
        }
        Ok(versions)
    }

    pub async fn document_version(
        &self,
        library: &str,
        path: &str,
        version_id: &str,
    ) -> Result<DocumentVersionContent> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library.id, &path)
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

    pub async fn version_diff(
        &self,
        library: &str,
        path: &str,
        version_id: &str,
        against: Option<&str>,
    ) -> Result<VersionDiff> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library_record = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library_record.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        let (_, base_content) = self
            .version_content_conn(&conn, &document_id, version_id)
            .await?;
        let against_id = if let Some(against) = against {
            against.to_string()
        } else {
            self.document_entry_conn(&conn, &library_record.id, &path)
                .await?
                .head_version_id
                .to_string()
        };
        let (_, against_content) = self
            .version_content_conn(&conn, &document_id, &against_id)
            .await?;

        Ok(VersionDiff {
            base_version_id: version_id.into(),
            against_version_id: against_id.into(),
            unified_diff: unified_line_diff(
                &String::from_utf8_lossy(&base_content),
                &String::from_utf8_lossy(&against_content),
            ),
        })
    }

    /// Restores a version through the legacy byte path. RawDocuments only:
    /// the server routes BlockDocument restores through the reconciling
    /// gateway (`markdown_write::restore_block_document_version`).
    pub async fn restore_document_version_with_origin(
        &self,
        library: &str,
        path: &str,
        version_id: &str,
        origin_id: Option<String>,
        actor: Option<String>,
    ) -> Result<WriteOutcome> {
        let path = normalize_path(path)?;
        let conn = self.conn()?;
        let library_record = Self::require_library_conn(&conn, library).await?;
        let document_id = self
            .document_id_conn(&conn, &library_record.id, &path)
            .await?
            .ok_or_else(|| QuarryError::NotFound(path.clone()))?;
        let (version, content) = self
            .version_content_conn(&conn, &document_id, version_id)
            .await?;
        drop(conn);

        self.put_document(PutDocumentRequest {
            library: library.to_string(),
            path,
            content,
            metadata: version.metadata,
            content_type: version.content_type,
            source: DocumentSource::Rest,
            precondition: WritePrecondition::None,
            origin_id,
            transaction: TransactionMetadata {
                actor,
                message: Some(format!("Restore version {version_id}")),
                provenance: Some(serde_json::json!({
                    "mode": "auto_commit",
                    "history": {"kind": "checkpoint", "reason": "restore"}
                })),
            },
        })
        .await
    }

    pub(crate) async fn version_content_conn(
        &self,
        conn: &Connection,
        document_id: &str,
        version_id: &str,
    ) -> Result<(DocumentVersion, Vec<u8>)> {
        let mut rows = conn
            .query(
                "SELECT v.id, v.document_id, v.tx_id, v.content_hash, v.inline_content, v.metadata_json,
                        v.content_type, v.byte_size, v.created_at, t.source, t.actor, t.message, t.provenance_json
                 FROM document_versions v
                 JOIN transactions t ON t.id = v.tx_id
                 WHERE v.document_id = ?1 AND v.id = ?2 LIMIT 1",
                params![document_id.to_string(), version_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        let row = rows
            .next()
            .await
            .map_err(map_turso_error)?
            .ok_or_else(|| QuarryError::NotFound(format!("version {version_id}")))?;
        let version = version_from_row(&row)?;
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
        Ok((version, content))
    }

    pub(crate) async fn document_id_for_version_conn(
        conn: &Connection,
        version_id: &str,
    ) -> Result<String> {
        let mut rows = conn
            .query(
                "SELECT document_id FROM document_versions WHERE id = ?1 LIMIT 1",
                params![version_id.to_string()],
            )
            .await
            .map_err(map_turso_error)?;
        rows.next()
            .await
            .map_err(map_turso_error)?
            .map(|row| text(&row, 0))
            .transpose()?
            .ok_or_else(|| QuarryError::NotFound(format!("version {version_id}")))
    }
}

fn unified_line_diff(base: &str, against: &str) -> String {
    let base_lines: Vec<&str> = base.lines().collect();
    let against_lines: Vec<&str> = against.lines().collect();
    let mut diff = String::from("--- base\n+++ against\n");
    let max = base_lines.len().max(against_lines.len());
    for index in 0..max {
        match (base_lines.get(index), against_lines.get(index)) {
            (Some(base_line), Some(against_line)) if base_line == against_line => {
                diff.push(' ');
                diff.push_str(base_line);
                diff.push('\n');
            }
            (Some(base_line), Some(against_line)) => {
                diff.push('-');
                diff.push_str(base_line);
                diff.push('\n');
                diff.push('+');
                diff.push_str(against_line);
                diff.push('\n');
            }
            (Some(base_line), None) => {
                diff.push('-');
                diff.push_str(base_line);
                diff.push('\n');
            }
            (None, Some(against_line)) => {
                diff.push('+');
                diff.push_str(against_line);
                diff.push('\n');
            }
            (None, None) => {}
        }
    }
    diff
}

const AUTOSAVE_IDLE_SPLIT_SECONDS: i64 = 120;
const AUTOSAVE_MAX_SPAN_SECONDS: i64 = 600;

pub fn group_version_history(mut versions: Vec<DocumentVersion>) -> Vec<DocumentHistoryEntry> {
    versions.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut groups: Vec<Vec<DocumentVersion>> = Vec::new();
    for version in versions {
        if let Some(current) = groups.last_mut()
            && can_group_autosave(current, &version)
        {
            current.push(version);
            continue;
        }
        groups.push(vec![version]);
    }

    groups
        .into_iter()
        .rev()
        .map(history_entry_from_group)
        .collect()
}

fn can_group_autosave(group: &[DocumentVersion], next: &DocumentVersion) -> bool {
    let Some(first) = group.first() else {
        return false;
    };
    let Some(previous) = group.last() else {
        return false;
    };
    if !is_autosave_version(first) || !is_autosave_version(next) {
        return false;
    }
    if first.transaction_source != next.transaction_source
        || first.transaction_actor != next.transaction_actor
        || first.content_type != next.content_type
        || autosave_session_id(first) != autosave_session_id(next)
        || checkpoint_reason(first) != checkpoint_reason(next)
    {
        return false;
    }
    let Some(first_at) = parse_history_time(&first.created_at) else {
        return false;
    };
    let Some(previous_at) = parse_history_time(&previous.created_at) else {
        return false;
    };
    let Some(next_at) = parse_history_time(&next.created_at) else {
        return false;
    };
    next_at.signed_duration_since(previous_at).num_seconds() <= AUTOSAVE_IDLE_SPLIT_SECONDS
        && next_at.signed_duration_since(first_at).num_seconds() <= AUTOSAVE_MAX_SPAN_SECONDS
}

fn history_entry_from_group(group: Vec<DocumentVersion>) -> DocumentHistoryEntry {
    let earliest = group.first().expect("history group must contain a version");
    let latest = group.last().expect("history group must contain a version");
    DocumentHistoryEntry {
        id: latest.id.clone(),
        document_id: latest.document_id.clone(),
        latest_version_id: latest.id.clone(),
        earliest_version_id: earliest.id.clone(),
        raw_version_count: group.len() as u64,
        source: latest.transaction_source.clone(),
        actor: latest.transaction_actor.clone(),
        message: latest.transaction_message.clone(),
        provenance: latest.transaction_provenance.clone(),
        checkpoint_reason: checkpoint_reason(latest),
        content_type: latest.content_type.clone(),
        byte_size: latest.byte_size,
        created_at: earliest.created_at.clone(),
        updated_at: latest.created_at.clone(),
    }
}

fn is_autosave_version(version: &DocumentVersion) -> bool {
    version
        .transaction_provenance
        .as_ref()
        .and_then(|provenance| provenance.get("history"))
        .and_then(|history| history.get("kind"))
        .and_then(JsonValue::as_str)
        == Some("autosave")
}

fn autosave_session_id(version: &DocumentVersion) -> Option<String> {
    version
        .transaction_provenance
        .as_ref()
        .and_then(|provenance| provenance.get("history"))
        .and_then(|history| history.get("session_id"))
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

fn checkpoint_reason(version: &DocumentVersion) -> Option<String> {
    version
        .transaction_provenance
        .as_ref()
        .and_then(|provenance| provenance.get("history"))
        .and_then(|history| history.get("reason"))
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

fn parse_history_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}
