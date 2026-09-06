use crate::{
    DocumentScopeRef, QuarryStore, TransactionMetadata, blocks, document_state, finish_tx,
    map_turso_error,
};
use quarry_core::{DocumentSource, QuarryError, Result, WriteOutcome, WritePrecondition};
use quarry_document::Document;
use serde::{Deserialize, Serialize};

/// Portable native history, review targets, and document metadata. Markdown
/// alone cannot represent retained characters or every review decision.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentArchive {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub bytes: Vec<u8>,
}

impl QuarryStore {
    pub async fn export_document_archive(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
    ) -> Result<DocumentArchive> {
        let conn = self.conn()?;
        conn.execute("BEGIN", ()).await.map_err(map_turso_error)?;
        let result = async {
            let document = self
                .head_document_for_scope_conn(&conn, scope, path)
                .await?;
            let head = blocks::head_version_head_conn(&conn, &document.id).await?;
            let state = document_state::load_state_conn(&conn, &document.id)
                .await?
                .ok_or_else(|| {
                    QuarryError::Unsupported("Only Markdown documents have a review archive".into())
                })?;
            Ok(DocumentArchive {
                format: "quarry-document".into(),
                version: 1,
                metadata: head.metadata,
                bytes: state.bytes,
            })
        }
        .await;
        finish_tx(&conn, result).await
    }

    /// Archives create a separate authority while preserving source identity.
    /// They cannot overwrite a document or merge unrelated native histories.
    pub async fn import_document_archive(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
        archive: DocumentArchive,
    ) -> Result<WriteOutcome> {
        if archive.format != "quarry-document" || archive.version != 1 {
            return Err(QuarryError::InvalidInput(
                "Unsupported Quarry archive".into(),
            ));
        }
        if !archive.metadata.is_null() && !archive.metadata.is_object() {
            return Err(QuarryError::InvalidInput(
                "Archive metadata must be an object".into(),
            ));
        }
        let metadata = if archive.metadata.is_null() {
            serde_json::json!({})
        } else {
            archive.metadata
        };
        let native = Document::load(&archive.bytes).map_err(document_state::document_error)?;
        let markdown = quarry_markdown::document_to_markdown(&native)?;
        self.import_block_document_with_native(
            scope,
            path,
            &markdown,
            metadata,
            "text/markdown",
            DocumentSource::Rest,
            WritePrecondition::IfNoneMatch,
            None,
            TransactionMetadata::default(),
            Some(Box::new(native)),
        )
        .await
    }
}
