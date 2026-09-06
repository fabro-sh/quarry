//! The durable command entry point shared by HTTP and in-process writers.
use crate::{
    BlockMutationCommit, BlockMutationOutcome, BlockMutationState, BlockTransactionRecord,
    DocumentScopeRef, DurableDocumentCommit, QuarryStore, document_projection,
    document_review_projection,
};
use quarry_core::{DocumentSource, QuarryError};
use quarry_document::{Document, DocumentError};
use serde_json::{Value, json};

pub fn command_error(error: DocumentError) -> QuarryError {
    match error {
        DocumentError::Conflict(_)
        | DocumentError::NotFound { .. }
        | DocumentError::AlreadyExists { .. } => QuarryError::PreconditionFailed(error.to_string()),
        _ => QuarryError::InvalidInput(error.to_string()),
    }
}

impl QuarryStore {
    pub async fn apply_document_commands(
        &self,
        scope: &DocumentScopeRef,
        path: &str,
        request: &quarry_document::CommandBatch,
    ) -> Result<BlockTransactionRecord, QuarryError> {
        if request.request_id.is_empty()
            || request.request_id.len() > 128
            || request.requests.is_empty()
            || request.requests.len() > 1024
        {
            return Err(QuarryError::InvalidInput(
                "Invalid document request size or ID".into(),
            ));
        }
        let payload =
            serde_json::to_value(request).map_err(|e| QuarryError::InvalidInput(e.to_string()))?;
        let hash = request_hash(&payload)?;
        if let Some(record) = self
            .replay_durable_request_for_scope(scope, path, &request.request_id, &hash)
            .await?
        {
            return Ok(record);
        }
        let saved = self
            .durable_document_for_scope(scope, path)
            .await?
            .ok_or_else(|| {
                QuarryError::PreconditionFailed("Document has no native state".into())
            })?;
        let snapshot = self
            .block_mutation_state_for_scope(scope, path, &request.request_id)
            .await?;
        if saved.version_id != snapshot.head_version_id {
            return Err(QuarryError::PreconditionFailed(
                "Document changed while loading".into(),
            ));
        }
        let mut candidate = Document::load(&saved.bytes).map_err(command_error)?;
        for command in &request.requests {
            candidate.apply_request(command).map_err(command_error)?;
        }
        let commit = build_document_commit(
            &snapshot,
            &candidate,
            &request.request_id,
            &request.actor,
            payload,
        )?;
        let outcome = self
            .commit_durable_document_for_scope(
                scope,
                commit,
                DurableDocumentCommit {
                    bytes: candidate.save(),
                    request_hash: hash,
                },
            )
            .await?;
        Ok(match outcome {
            BlockMutationOutcome::Applied { record, .. }
            | BlockMutationOutcome::Replayed(record) => record,
        })
    }
}

pub fn build_document_commit(
    snapshot: &BlockMutationState,
    document: &Document,
    request_id: &str,
    actor: &quarry_document::DocumentActor,
    request: Value,
) -> Result<BlockMutationCommit, QuarryError> {
    let rows = document_projection(document)?;
    let markdown = format!(
        "{}{}",
        quarry_core::render_markdown_frontmatter(&snapshot.metadata)?,
        quarry_markdown::block_rows_to_markdown(&rows)?
    );
    let mut changed: Vec<_> = rows
        .iter()
        .filter(|row| !snapshot.rows.contains(row))
        .map(|row| row.block_id.clone())
        .collect();
    changed.extend(
        snapshot
            .rows
            .iter()
            .filter(|row| !rows.iter().any(|r| r.block_id == row.block_id))
            .map(|row| row.block_id.clone()),
    );
    changed.sort();
    changed.dedup();
    Ok(BlockMutationCommit {
        document_id: snapshot.document_id.clone(),
        expected_head_version_id: snapshot.head_version_id.clone(),
        client_tx_id: request_id.into(),
        actor_kind: actor.kind.clone(),
        actor_id: actor.id.clone(),
        transaction_actor: actor.label.clone().or_else(|| actor.id.clone()),
        transaction_message: None,
        transaction_provenance: Some(
            json!({"mode":"document_commands","schema":quarry_document::SCHEMA_VERSION}),
        ),
        origin_id: actor.id.clone(),
        source: DocumentSource::Rest,
        recorded_ops: json!({"request":request,"actor":actor,"ack":{"status":"applied","heads":document.heads().iter().map(ToString::to_string).collect::<Vec<_>>(),"changed_block_ids":changed}}),
        metadata: snapshot.metadata.clone(),
        content_type: snapshot.content_type.clone(),
        rows,
        review_items: document_review_projection(document)?,
        normalized_markdown: markdown,
    })
}

fn request_hash(request: &Value) -> Result<String, QuarryError> {
    let bytes =
        serde_json::to_vec(request).map_err(|e| QuarryError::InvalidInput(e.to_string()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}
