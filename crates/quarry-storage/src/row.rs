use crate::{DirectoryMetadata, map_turso_error, parse_storage_enum};
use quarry_core::{
    CollabInviteToken, ConflictRecord, DocumentLink, DocumentListEntry, DocumentVersion, Library,
    Result, SyncStateEntry, TransactionRecord,
};
use turso::{Row, Value};

pub(crate) fn library_from_row(row: &Row) -> Result<Library> {
    Ok(Library {
        id: text(row, 0)?,
        slug: text(row, 1)?,
        created_at: text(row, 2)?.into(),
        settings: serde_json::from_str(&text(row, 3)?)?,
    })
}

pub(crate) fn directory_metadata_from_row(row: &Row) -> Result<DirectoryMetadata> {
    Ok(DirectoryMetadata {
        path: text(row, 0)?,
        mode: opt_int(row, 1)?,
        mtime: text(row, 2)?,
        inode: int(row, 3)?,
    })
}

pub(crate) fn document_entry_from_row(row: &Row) -> Result<DocumentListEntry> {
    Ok(DocumentListEntry {
        id: text(row, 0)?.into(),
        library_id: opt_text(row, 1)?,
        path: text(row, 2)?,
        head_version_id: text(row, 3)?.into(),
        content_type: text(row, 4)?,
        byte_size: int(row, 5)? as u64,
        content_hash: opt_text(row, 6)?,
        metadata: serde_json::from_str(&text(row, 7)?)?,
        expires_at: opt_text(row, 8)?.map(Into::into),
        updated_at: text(row, 9)?.into(),
    })
}

pub(crate) fn link_from_row(row: &Row) -> Result<DocumentLink> {
    let target_doc_id = opt_text(row, 5)?;
    let target_path = opt_text(row, 6)?;
    let resolved = target_doc_id.is_some() && target_path.is_some();
    let stored_resolution_status = text(row, 11)?;
    let resolution_status = if resolved {
        "resolved".to_string()
    } else if stored_resolution_status == "ambiguous" {
        "ambiguous".to_string()
    } else if stored_resolution_status == "external" {
        "external".to_string()
    } else {
        "unresolved".to_string()
    };
    Ok(DocumentLink {
        src_doc_id: text(row, 0)?.into(),
        src_version_id: text(row, 1)?.into(),
        src_path: text(row, 2)?,
        target_kind: text(row, 3)?,
        target_text: text(row, 4)?,
        target_doc_id: if resolved {
            target_doc_id.map(Into::into)
        } else {
            None
        },
        target_path,
        target_anchor: opt_text(row, 7)?,
        alias: opt_text(row, 8)?,
        start_offset: int(row, 9)? as usize,
        end_offset: int(row, 10)? as usize,
        resolved,
        resolution_status,
    })
}

pub(crate) fn version_from_row(row: &Row) -> Result<DocumentVersion> {
    let mut version = DocumentVersion {
        id: text(row, 0)?.into(),
        document_id: text(row, 1)?.into(),
        tx_id: text(row, 2)?,
        transaction_source: None,
        transaction_actor: None,
        transaction_message: None,
        transaction_provenance: None,
        content_hash: opt_text(row, 3)?,
        inline_content: opt_blob(row, 4)?,
        metadata: serde_json::from_str(&text(row, 5)?)?,
        content_type: text(row, 6)?,
        byte_size: int(row, 7)? as u64,
        created_at: text(row, 8)?.into(),
    };
    if let Some(source) = opt_text(row, 9)? {
        version.transaction_source = Some(parse_storage_enum(&source)?);
        version.transaction_actor = opt_text(row, 10)?;
        version.transaction_message = opt_text(row, 11)?;
        version.transaction_provenance = Some(serde_json::from_str(&text(row, 12)?)?);
    }
    Ok(version)
}

pub(crate) fn transaction_from_row(row: &Row) -> Result<TransactionRecord> {
    Ok(TransactionRecord {
        id: text(row, 0)?,
        library_id: text(row, 1)?,
        state: parse_storage_enum(&text(row, 2)?)?,
        actor: opt_text(row, 3)?,
        source: parse_storage_enum(&text(row, 4)?)?,
        message: opt_text(row, 5)?,
        provenance: serde_json::from_str(&text(row, 6)?)?,
        created_at: text(row, 7)?.into(),
        committed_at: opt_text(row, 8)?.map(Into::into),
    })
}

pub(crate) fn conflict_from_row(row: &Row) -> Result<ConflictRecord> {
    Ok(ConflictRecord {
        id: text(row, 0)?,
        library_id: text(row, 1)?,
        path: text(row, 2)?,
        conflict_path: opt_text(row, 8)?,
        ours_version_id: opt_text(row, 3)?.map(Into::into),
        theirs_version_id: opt_text(row, 4)?.map(Into::into),
        status: parse_storage_enum(&text(row, 5)?)?,
        discovered_at: text(row, 6)?.into(),
        resolved_at: opt_text(row, 7)?.map(Into::into),
    })
}

pub(crate) fn sync_state_from_row(row: &Row) -> Result<SyncStateEntry> {
    Ok(SyncStateEntry {
        peer_id: text(row, 0)?,
        path: text(row, 1)?,
        last_synced_doc_version_id: opt_text(row, 2)?.map(Into::into),
        last_synced_git_oid: opt_text(row, 3)?,
    })
}

pub(crate) fn collab_invite_token_from_row(row: &Row) -> Result<CollabInviteToken> {
    Ok(CollabInviteToken {
        id: text(row, 0)?,
        document_id: text(row, 1)?.into(),
        role: text(row, 2)?,
        by_hint: opt_text(row, 3)?,
        created_at: text(row, 4)?.into(),
        revoked_at: opt_text(row, 5)?.map(Into::into),
    })
}

pub(crate) fn text(row: &Row, index: usize) -> Result<String> {
    Ok(row.get::<String>(index).map_err(map_turso_error)?)
}

pub(crate) fn opt_text(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Text(value) => Ok(Some(value)),
        other => Err(quarry_core::QuarryError::Invariant(format!(
            "expected text/null at column {index}, got {other:?}"
        ))),
    }
}

pub(crate) fn opt_blob(row: &Row, index: usize) -> Result<Option<Vec<u8>>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Blob(value) => Ok(Some(value)),
        other => Err(quarry_core::QuarryError::Invariant(format!(
            "expected blob/null at column {index}, got {other:?}"
        ))),
    }
}

pub(crate) fn opt_int(row: &Row, index: usize) -> Result<Option<i64>> {
    match row.get_value(index).map_err(map_turso_error)? {
        Value::Null => Ok(None),
        Value::Integer(value) => Ok(Some(value)),
        other => Err(quarry_core::QuarryError::Invariant(format!(
            "expected integer/null at column {index}, got {other:?}"
        ))),
    }
}

pub(crate) fn int(row: &Row, index: usize) -> Result<i64> {
    Ok(row.get::<i64>(index).map_err(map_turso_error)?)
}
