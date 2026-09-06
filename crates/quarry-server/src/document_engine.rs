//! Server authority for native document commands. Candidate state is local to
//! a request; only committed bytes are visible to readers or event subscribers.
use crate::{ApiError, AppState, json_with_etag};
use axum::{http::StatusCode, response::Response};
use quarry_core::QuarryError;
use quarry_document::{Document, DocumentError};
use quarry_storage::{
    BlockMutationCommit, BlockMutationState, BlockTransactionRecord, DocumentScopeRef,
};
use serde::Serialize;
use serde_json::Value;

#[derive(serde::Deserialize)]
pub(crate) struct AccessQuery {
    pub(crate) token: Option<String>,
    pub(crate) since: Option<String>,
}

pub(crate) async fn library_path_access(
    state: &AppState,
    library: &str,
    path: &str,
    token: Option<String>,
    writing: bool,
) -> Result<bool, ApiError> {
    if token.is_none() {
        return Ok(true);
    }
    let document = state.store.head_document(library, path).await?;
    let (_, _, writable) = library_document(
        state,
        library,
        &document.id,
        &AccessQuery { token, since: None },
        writing,
    )
    .await?;
    Ok(writable)
}

async fn library_document(
    state: &AppState,
    library: &str,
    id: &str,
    query: &AccessQuery,
    writing: bool,
) -> Result<(DocumentScopeRef, String, bool), ApiError> {
    let scope = DocumentScopeRef::library(library);
    let path = state.store.document_path_for_scope_id(&scope, id).await?;
    let mut writable = true;
    if let Some(token) = &query.token {
        let invite = state
            .store
            .collab_invite_tokens(library, &path)
            .await?
            .into_iter()
            .find(|invite| &invite.id == token && invite.revoked_at.is_none())
            .ok_or_else(|| QuarryError::NotFound("Document invitation is unavailable".into()))?;
        writable = invite.role == "editor";
        if writing && !writable {
            return Err(QuarryError::ReadOnly("Document invitation allows viewing".into()).into());
        }
    }
    Ok((scope, path, writable))
}

#[utoipa::path(get, path = "/v1/libraries/{library}/documents-by-id/{document_id}/document-state",
    params(("library" = String, Path), ("document_id" = String, Path), ("token" = Option<String>, Query), ("since" = Option<String>, Query, description = "Comma-separated native heads already held by the client")),
    responses((status = 200, body = DocumentStateResponse)))]
pub(crate) async fn read_by_id(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path((library, id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<AccessQuery>,
) -> Result<Response, ApiError> {
    let _guard = state.documents.lock(&id).await;
    let (scope, path, writable) = library_document(&state, &library, &id, &query, false).await?;
    read_with_access(&state, &scope, &path, writable, query.since.as_deref()).await
}

#[utoipa::path(post, path = "/v1/libraries/{library}/documents-by-id/{document_id}/document-commands",
    params(("library" = String, Path), ("document_id" = String, Path), ("token" = Option<String>, Query)),
    request_body = quarry_document::CommandBatch, responses((status = 200, body = DocumentCommandAck)))]
pub(crate) async fn commands_by_id(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path((library, id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<AccessQuery>,
    axum::Json(payload): axum::Json<Value>,
) -> Result<Response, ApiError> {
    state
        .store
        .run_global_operation(Box::pin(async {
            let _guard = state.documents.lock(&id).await;
            let (scope, path, _) = library_document(&state, &library, &id, &query, true).await?;
            let request: DocumentBatchRequest = serde_json::from_value(payload)
                .map_err(|e| QuarryError::InvalidInput(e.to_string()))?;
            ack(&state
                .store
                .apply_document_commands(&scope, &path, &request)
                .await?)
        }))
        .await
}

#[utoipa::path(get, path = "/v1/libraries/{library}/documents-by-id/{document_id}/events/stream",
    params(("library" = String, Path), ("document_id" = String, Path), ("token" = Option<String>, Query)),
    responses((status = 200, description = "Native document events")))]
pub(crate) async fn events_by_id(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path((library, id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<AccessQuery>,
) -> Result<Response, ApiError> {
    use axum::response::IntoResponse;
    library_document(&state, &library, &id, &query, false).await?;
    Ok(
        crate::sse::events_for_native_document(&state.store, &library, id, state.shutdown_token())
            .await?
            .into_response(),
    )
}

#[utoipa::path(post, path = "/v1/libraries/{library}/documents-by-id/{document_id}/transactions",
    params(("library" = String, Path), ("document_id" = String, Path), ("token" = Option<String>, Query)),
    request_body = Value, responses((status = 200, description = "Committed document transaction")))]
pub(crate) async fn transactions_by_id(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path((library, id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<AccessQuery>,
    axum::Json(payload): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let (_, path, _) = library_document(&state, &library, &id, &query, true).await?;
    crate::gateway::document_block_transactions(&state, &library, &path, payload).await
}

pub(crate) type DocumentBatchRequest = quarry_document::CommandBatch;

#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "format", rename_all = "snake_case")]
pub(crate) enum DocumentStateResponse {
    Automerge {
        base: Option<Vec<String>>,
        writable: bool,
        metadata: Value,
        document_clock: String,
        bytes: Vec<u8>,
        document: quarry_document::DocumentView,
    },
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct DocumentCommandAck {
    request_id: String,
    document_clock: String,
    transaction_id: String,
    heads: Vec<String>,
    changed_block_ids: Vec<String>,
}

pub(crate) fn document_error(error: DocumentError) -> QuarryError {
    match error {
        DocumentError::Conflict(_)
        | DocumentError::NotFound { .. }
        | DocumentError::AlreadyExists { .. } => QuarryError::PreconditionFailed(error.to_string()),
        _ => QuarryError::InvalidInput(error.to_string()),
    }
}

pub(crate) async fn read(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    since: Option<&str>,
) -> Result<Response, ApiError> {
    read_with_access(state, scope, path, true, since).await
}

pub(crate) async fn read_with_access(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    writable: bool,
    since: Option<&str>,
) -> Result<Response, ApiError> {
    let Some(saved) = state.store.durable_document_for_scope(scope, path).await? else {
        return Err(
            QuarryError::Unsupported("Only Markdown documents have native state".into()).into(),
        );
    };
    let document = Document::load(&saved.bytes).map_err(document_error)?;
    let (base, bytes) = if let Some(since) = since {
        let hashes = since
            .split(',')
            .map(|hash| {
                hash.parse()
                    .map_err(|_| QuarryError::InvalidInput("Invalid native base hash".into()))
            })
            .collect::<quarry_core::Result<Vec<_>>>()?;
        let bytes = document.save_after(&hashes).map_err(document_error)?;
        (
            Some(hashes.iter().map(ToString::to_string).collect()),
            bytes,
        )
    } else {
        (None, saved.bytes)
    };
    json_with_etag(
        StatusCode::OK,
        &DocumentStateResponse::Automerge {
            base,
            writable,
            metadata: saved.metadata,
            document_clock: saved.version_id.clone(),
            bytes,
            document: document.view().map_err(document_error)?,
        },
        &saved.version_id,
    )
}

pub(crate) async fn commands(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    payload: Value,
) -> Result<Response, ApiError> {
    let request: DocumentBatchRequest =
        serde_json::from_value(payload).map_err(|e| QuarryError::InvalidInput(e.to_string()))?;
    state
        .store
        .run_global_operation(Box::pin(async {
            let entry = state.store.head_document_for_scope(scope, path).await?;
            let _guard = state.documents.lock(&entry.id).await;
            let record = state
                .store
                .apply_document_commands(scope, path, &request)
                .await?;
            ack(&record)
        }))
        .await
}

pub(crate) fn build_commit(
    snapshot: &BlockMutationState,
    document: &Document,
    request_id: &str,
    actor: &crate::gateway::BlockTransactionActor,
    request: Value,
) -> Result<BlockMutationCommit, QuarryError> {
    quarry_storage::build_document_commit(
        snapshot,
        document,
        request_id,
        &quarry_document::DocumentActor {
            kind: actor.kind.clone(),
            id: actor.id.clone(),
            label: actor.label.clone(),
        },
        request,
    )
}

fn ack(record: &BlockTransactionRecord) -> Result<Response, ApiError> {
    let version = record
        .resulting_version_id
        .as_ref()
        .ok_or_else(|| QuarryError::Invariant("Native receipt has no version".into()))?;
    json_with_etag(
        StatusCode::OK,
        &DocumentCommandAck {
            request_id: record.client_tx_id.clone(),
            document_clock: version.clone(),
            transaction_id: record.id.clone(),
            heads: serde_json::from_value(record.ops["ack"]["heads"].clone())
                .map_err(QuarryError::Json)?,
            changed_block_ids: serde_json::from_value(
                record.ops["ack"]["changed_block_ids"].clone(),
            )
            .map_err(QuarryError::Json)?,
        },
        version,
    )
}

pub(crate) async fn archive(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    payload: Option<Value>,
) -> Result<Response, ApiError> {
    if let Some(payload) = payload {
        let archive = serde_json::from_value(payload).map_err(|error| {
            QuarryError::InvalidInput(format!("Invalid Quarry archive: {error}"))
        })?;
        let outcome = state
            .store
            .import_document_archive(scope, path, archive)
            .await?;
        json_with_etag(StatusCode::CREATED, &outcome, &outcome.version.id)
    } else {
        crate::json_response(
            StatusCode::OK,
            &state.store.export_document_archive(scope, path).await?,
        )
    }
}

#[utoipa::path(get, path = "/v1/libraries/{library}/documents/{path}/archive",
    params(("library" = String, Path), ("path" = String, Path)), responses((status = 200, body = quarry_storage::DocumentArchive)))]
#[expect(dead_code, reason = "OpenAPI documentation stub")]
pub(crate) async fn library_archive_get_openapi() {}
#[utoipa::path(post, path = "/v1/libraries/{library}/documents/{path}/archive",
    params(("library" = String, Path), ("path" = String, Path)), request_body = quarry_storage::DocumentArchive,
    responses((status = 201, body = quarry_core::WriteOutcome), (status = 412, description = "Destination exists")))]
#[expect(dead_code, reason = "OpenAPI documentation stub")]
pub(crate) async fn library_archive_post_openapi() {}
#[utoipa::path(get, path = "/v1/tmp/documents/{secret}/archive",
    params(("secret" = String, Path)), responses((status = 200, body = quarry_storage::DocumentArchive)))]
#[expect(dead_code, reason = "OpenAPI documentation stub")]
pub(crate) async fn tmp_archive_get_openapi() {}
#[utoipa::path(post, path = "/v1/tmp/documents/{secret}/archive",
    params(("secret" = String, Path)), request_body = quarry_storage::DocumentArchive,
    responses((status = 201, body = quarry_core::WriteOutcome), (status = 412, description = "Destination exists")))]
#[expect(dead_code, reason = "OpenAPI documentation stub")]
pub(crate) async fn tmp_archive_post_openapi() {}

#[utoipa::path(post, path = "/v1/libraries/{library}/documents-by-id/{document_id}/selection",
    params(("library" = String, Path), ("document_id" = String, Path), ("token" = Option<String>, Query)),
    request_body = crate::document_authority::DocumentSelection,
    responses((status = 200, body = Vec<crate::document_authority::DocumentSelection>)))]
pub(crate) async fn selection_by_id(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path((library, id)): axum::extract::Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<AccessQuery>,
    axum::Json(selection): axum::Json<crate::document_authority::DocumentSelection>,
) -> Result<Response, ApiError> {
    library_document(&state, &library, &id, &query, false).await?;
    publish_selection(&state, &id, selection)
}

pub(crate) async fn selection_for_scope(
    state: &AppState,
    scope: &DocumentScopeRef,
    path: &str,
    payload: Value,
) -> Result<Response, ApiError> {
    let document = state.store.head_document_for_scope(scope, path).await?;
    let selection = serde_json::from_value(payload)
        .map_err(|error| QuarryError::InvalidInput(format!("Invalid selection: {error}")))?;
    publish_selection(state, &document.id, selection)
}

fn publish_selection(
    state: &AppState,
    id: &str,
    selection: crate::document_authority::DocumentSelection,
) -> Result<Response, ApiError> {
    if selection.client_id.is_empty()
        || selection.client_id.len() > 256
        || selection.author.len() > 256
        || !matches!(selection.points.len(), 0 | 2)
        || selection
            .points
            .iter()
            .any(|point| point.source.len() > 1024 || point.cursor.len() > 1024)
    {
        return Err(QuarryError::InvalidInput("Invalid selection size or client ID".into()).into());
    }
    crate::json_response(
        StatusCode::OK,
        &state.documents.selections(id, Some(selection)),
    )
}

#[utoipa::path(post, path = "/v1/tmp/documents/{secret}/selection",
    params(("secret" = String, Path)), request_body = crate::document_authority::DocumentSelection,
    responses((status = 200, body = Vec<crate::document_authority::DocumentSelection>)))]
#[expect(dead_code, reason = "OpenAPI documentation stub")]
pub(crate) async fn tmp_selection_openapi() {}
