use crate::{ApiError, AppState};
use axum::Json;
use axum::extract::{Path, State};
use quarry_core::{ConflictRecord, QuarryError};
use quarry_storage::QuarryStore;

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/conflicts",
    params(("library" = String, Path)),
    responses((status = 200, body = [ConflictRecord]))
)]
pub(crate) async fn list_conflicts(
    State(state): State<AppState>,
    Path(library): Path<String>,
) -> Result<Json<Vec<ConflictRecord>>, ApiError> {
    Ok(Json(state.store.list_conflicts(&library).await?))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/conflicts/{conflict}",
    params(("library" = String, Path), ("conflict" = String, Path)),
    responses((status = 200, body = ConflictRecord))
)]
pub(crate) async fn get_conflict(
    State(state): State<AppState>,
    Path((library, conflict)): Path<(String, String)>,
) -> Result<Json<ConflictRecord>, ApiError> {
    Ok(Json(
        scoped_conflict(&state.store, &library, &conflict).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/conflicts/{conflict}/resolve",
    params(("library" = String, Path), ("conflict" = String, Path)),
    responses((status = 200, body = ConflictRecord))
)]
pub(crate) async fn resolve_conflict(
    State(state): State<AppState>,
    Path((library, conflict)): Path<(String, String)>,
) -> Result<Json<ConflictRecord>, ApiError> {
    scoped_conflict(&state.store, &library, &conflict).await?;
    Ok(Json(state.store.resolve_conflict(&conflict).await?))
}

async fn scoped_conflict(
    store: &QuarryStore,
    library: &str,
    conflict: &str,
) -> Result<ConflictRecord, ApiError> {
    let library = store.get_library(library).await?;
    let conflict_record = store.get_conflict(conflict).await?;
    if conflict_record.library_id != library.id {
        return Err(QuarryError::NotFound(format!("conflict {conflict}")).into());
    }
    Ok(conflict_record)
}
