use crate::ApiDoc;
#[cfg(feature = "admin-api")]
use crate::{ApiError, AppState};
use axum::Json;
#[cfg(feature = "admin-api")]
use axum::extract::State;
#[cfg(feature = "admin-api")]
use quarry_core::GcReport;
use serde::Serialize;
use serde_json::Value as JsonValue;
use utoipa::{OpenApi, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Capabilities {
    tmp_documents: bool,
    lib_documents: bool,
}

impl Capabilities {
    fn current() -> Self {
        Self {
            tmp_documents: cfg!(feature = "tmp-documents"),
            lib_documents: cfg!(feature = "lib-documents"),
        }
    }
}

#[utoipa::path(get, path = "/v1/health", responses((status = 200, body = JsonValue)))]
pub(crate) async fn health() -> Json<JsonValue> {
    Json(serde_json::json!({"ok": true, "service": "quarry"}))
}

#[utoipa::path(get, path = "/v1/capabilities", responses((status = 200, body = Capabilities)))]
pub(crate) async fn capabilities() -> Json<Capabilities> {
    Json(Capabilities::current())
}

#[utoipa::path(get, path = "/v1/openapi.json", responses((status = 200, body = JsonValue)))]
pub(crate) async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(active_openapi())
}

fn active_openapi() -> utoipa::openapi::OpenApi {
    let mut openapi = ApiDoc::openapi();
    #[cfg(feature = "admin-api")]
    openapi.merge(AdminApiDoc::openapi());
    #[cfg(feature = "lib-documents")]
    openapi.merge(crate::git_handlers::GitApiDoc::openapi());
    openapi
        .paths
        .paths
        .retain(|path, _| openapi_path_enabled(path));
    openapi
}

fn openapi_path_enabled(path: &str) -> bool {
    if path.starts_with("/v1/admin") {
        return cfg!(feature = "admin-api");
    }
    if path.starts_with("/v1/tmp/documents") {
        return cfg!(feature = "tmp-documents")
            && (path != "/v1/tmp/documents/{secret}/promote" || cfg!(feature = "lib-documents"));
    }
    if path.starts_with("/v1/tmp/collab") {
        return cfg!(feature = "tmp-documents");
    }
    if path.starts_with("/v1/collab") {
        // The raw-id collab route serves library documents only; the tmp-only
        // build reaches tmp documents through /v1/tmp/collab instead.
        return cfg!(feature = "lib-documents");
    }
    if path == "/v1/events" || path.starts_with("/v1/libraries") {
        return cfg!(feature = "lib-documents");
    }
    true
}

/// OpenAPI fragment for the `admin-api`-gated `/v1/admin/*` namespace. Merged
/// into the served document only when the feature is enabled, so the default
/// build neither compiles nor documents these routes.
#[cfg(feature = "admin-api")]
#[derive(OpenApi)]
#[openapi(paths(admin_gc), components(schemas(GcReport)))]
struct AdminApiDoc;

#[cfg(feature = "admin-api")]
#[utoipa::path(post, path = "/v1/admin/gc", responses((status = 200, body = GcReport)))]
pub(crate) async fn admin_gc(State(state): State<AppState>) -> Result<Json<GcReport>, ApiError> {
    let report = state.store.gc().await?;
    tracing::info!(
        event = "storage.gc.completed",
        source = "admin_api",
        reachable_blobs = report.reachable,
        removed_blobs = report.removed,
        "admin GC completed"
    );
    Ok(Json(report))
}
