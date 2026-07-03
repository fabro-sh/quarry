use crate::{ApiError, AppState, optional_header, transaction_metadata_from_headers};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use quarry_core::{DocumentListEntry, DocumentSource, TransactionRecord};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    prefix: Option<String>,
    limit: Option<u64>,
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents",
    params(("library" = String, Path), ("prefix" = Option<String>, Query), ("limit" = Option<u64>, Query)),
    responses((status = 200, body = [DocumentListEntry]))
)]
pub(crate) async fn list_documents(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<DocumentListEntry>>, ApiError> {
    Ok(Json(
        state
            .store
            .list_documents(&library, query.prefix.as_deref(), query.limit)
            .await?,
    ))
}

#[utoipa::path(
    delete,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
pub(crate) async fn delete_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
) -> Result<Json<TransactionRecord>, ApiError> {
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    let actor = transaction_metadata_from_headers(&headers)?.actor;
    Ok(Json(
        state
            .store
            .delete_document_with_origin(&library, &path, DocumentSource::Rest, origin_id, actor)
            .await?,
    ))
}
