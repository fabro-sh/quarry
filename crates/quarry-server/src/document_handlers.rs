use crate::{ApiError, AppState};
use axum::Json;
use axum::extract::{Path, Query, State};
use quarry_core::DocumentListEntry;
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
