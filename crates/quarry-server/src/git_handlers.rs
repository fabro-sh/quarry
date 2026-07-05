use crate::{ApiError, AppState};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use quarry_core::GitPeer;
use quarry_git::{
    GitExportOptions, GitExportResult, GitImportResult, GitSyncResult, export_worktree,
    import_worktree, pull_peer, push_peer, sync_peer,
};
use quarry_storage::QuarryStore;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use utoipa::{OpenApi, ToSchema};

/// OpenAPI fragment for the `lib-documents`-gated `/v1/libraries/{library}/git/*`
/// namespace. Merged into the served document only when the feature is enabled,
/// so the tmp-only build neither compiles nor documents these routes.
#[derive(OpenApi)]
#[openapi(
    paths(
        create_git_peer,
        list_git_peers,
        git_import,
        git_export,
        git_pull,
        git_push,
        git_sync
    ),
    components(schemas(
        GitPeer,
        GitPeerRequest,
        GitImportRequest,
        GitExportRequest,
        GitImportResult,
        GitExportResult,
        GitSyncResult
    ))
)]
pub(crate) struct GitApiDoc;

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct GitPeerRequest {
    pub repo: String,
    pub remote: Option<String>,
    pub branch: Option<String>,
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/git/peers",
    params(("library" = String, Path)),
    request_body = GitPeerRequest,
    responses((status = 201, body = GitPeer))
)]
pub(crate) async fn create_git_peer(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Json(request): Json<GitPeerRequest>,
) -> Result<(StatusCode, Json<GitPeer>), ApiError> {
    let mut config = serde_json::json!({
        "repo": request.repo,
        "branch": request.branch.unwrap_or_else(|| "main".to_string())
    });
    if let (Some(remote), Some(object)) = (request.remote, config.as_object_mut()) {
        object.insert("remote".to_string(), JsonValue::String(remote));
    }
    Ok((
        StatusCode::CREATED,
        Json(state.store.create_git_peer(&library, config).await?),
    ))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/git/peers",
    params(("library" = String, Path)),
    responses((status = 200, body = [GitPeer]))
)]
pub(crate) async fn list_git_peers(
    State(state): State<AppState>,
    Path(library): Path<String>,
) -> Result<Json<Vec<GitPeer>>, ApiError> {
    Ok(Json(state.store.list_git_peers(&library).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct GitImportRequest {
    pub repo: String,
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/git/import",
    params(("library" = String, Path)),
    request_body = GitImportRequest,
    responses((status = 200, body = GitImportResult))
)]
pub(crate) async fn git_import(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Json(request): Json<GitImportRequest>,
) -> Result<Json<GitImportResult>, ApiError> {
    Ok(Json(
        import_worktree(&state.store, &library, std::path::Path::new(&request.repo)).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct GitExportRequest {
    pub repo: String,
    pub branch: Option<String>,
    pub force_large: Option<bool>,
    pub frontmatter_markdown: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/git/export",
    params(("library" = String, Path)),
    request_body = GitExportRequest,
    responses((status = 200, body = GitExportResult))
)]
pub(crate) async fn git_export(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Json(request): Json<GitExportRequest>,
) -> Result<Json<GitExportResult>, ApiError> {
    Ok(Json(
        export_worktree(
            &state.store,
            &library,
            std::path::Path::new(&request.repo),
            export_options(&request),
        )
        .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/git/peers/{peer}/pull",
    params(("library" = String, Path), ("peer" = String, Path)),
    responses((status = 200, body = GitSyncResult))
)]
pub(crate) async fn git_pull(
    State(state): State<AppState>,
    Path((library, peer)): Path<(String, String)>,
) -> Result<Json<GitSyncResult>, ApiError> {
    let result = pull_peer(&state.store, &library, &peer).await?;
    emit_git_sync_completed(&state.store, &library, &peer, &result).await?;
    Ok(Json(result))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/git/peers/{peer}/push",
    params(("library" = String, Path), ("peer" = String, Path)),
    responses((status = 200, body = GitSyncResult))
)]
pub(crate) async fn git_push(
    State(state): State<AppState>,
    Path((library, peer)): Path<(String, String)>,
) -> Result<Json<GitSyncResult>, ApiError> {
    let result = push_peer(&state.store, &library, &peer).await?;
    emit_git_sync_completed(&state.store, &library, &peer, &result).await?;
    Ok(Json(result))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/git/peers/{peer}/sync",
    params(("library" = String, Path), ("peer" = String, Path)),
    responses((status = 200, body = GitSyncResult))
)]
pub(crate) async fn git_sync(
    State(state): State<AppState>,
    Path((library, peer)): Path<(String, String)>,
) -> Result<Json<GitSyncResult>, ApiError> {
    let result = sync_peer(&state.store, &library, &peer).await?;
    emit_git_sync_completed(&state.store, &library, &peer, &result).await?;
    Ok(Json(result))
}

async fn emit_git_sync_completed(
    store: &QuarryStore,
    library: &str,
    peer: &str,
    result: &GitSyncResult,
) -> Result<(), ApiError> {
    store
        .emit_git_sync_completed(
            library,
            peer,
            git_sync_applied_count(result),
            git_sync_conflict_count(result),
        )
        .await?;
    Ok(())
}

fn git_sync_applied_count(result: &GitSyncResult) -> usize {
    result.imported_paths.len() + result.exported_paths.len()
}

fn git_sync_conflict_count(result: &GitSyncResult) -> usize {
    result.conflict_paths.len().max(result.conflicts.len())
}

fn export_options(request: &GitExportRequest) -> GitExportOptions {
    GitExportOptions {
        branch: request.branch.clone().unwrap_or_else(|| "main".to_string()),
        force_large: request.force_large.unwrap_or(false),
        frontmatter_markdown: request.frontmatter_markdown.unwrap_or(true),
    }
}
