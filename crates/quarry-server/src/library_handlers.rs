use crate::{ApiError, ApiErrorResponse, AppState};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use quarry_core::Library;
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateLibraryRequest {
    pub slug: String,
}

#[utoipa::path(
    post,
    path = "/v1/libraries",
    request_body = CreateLibraryRequest,
    responses((status = 201, body = Library), (status = 409, body = ApiErrorResponse))
)]
pub(crate) async fn create_library(
    State(state): State<AppState>,
    Json(request): Json<CreateLibraryRequest>,
) -> Result<(StatusCode, Json<Library>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(state.store.create_library(&request.slug).await?),
    ))
}

#[utoipa::path(get, path = "/v1/libraries", responses((status = 200, body = [Library])))]
pub(crate) async fn list_libraries(
    State(state): State<AppState>,
) -> Result<Json<Vec<Library>>, ApiError> {
    Ok(Json(state.store.list_libraries().await?))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}",
    params(("library" = String, Path)),
    responses((status = 200, body = Library), (status = 404, body = ApiErrorResponse))
)]
pub(crate) async fn get_library(
    State(state): State<AppState>,
    Path(library): Path<String>,
) -> Result<Json<Library>, ApiError> {
    Ok(Json(state.store.get_library(&library).await?))
}
