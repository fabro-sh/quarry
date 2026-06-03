mod collab;

#[cfg(feature = "bundle_ui")]
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
#[cfg(feature = "bundle_ui")]
use axum::http::Uri;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::{stream, Stream};
use quarry_core::{
    ConflictRecord, DocumentLink, DocumentListEntry, DocumentSource, DocumentVersion,
    DocumentVersionContent, GcReport, GitPeer, GraphEdge, GraphNode, GraphResponse, Library,
    LinkCollection, QuarryError, ReindexReport, SearchResponse, SearchResult, SearchSuggestion,
    TransactionRecord, VersionDiff, WriteOutcome, WritePrecondition,
};
use quarry_git::{
    export_worktree, import_worktree, pull_peer, push_peer, sync_peer, GitExportOptions,
    GitExportResult, GitImportResult, GitSyncResult,
};
use quarry_storage::{QuarryStore, StoreEvent, StoreEventKind};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use utoipa::{OpenApi, ToSchema};

#[derive(Clone)]
pub struct AppState {
    store: QuarryStore,
    collab: collab::CollabHub,
}

pub fn router(store: QuarryStore) -> Router {
    let router = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/openapi.json", get(openapi_json))
        .route("/v1/admin/gc", post(admin_gc))
        .route("/v1/events", get(events))
        .route("/v1/collab/{document_id}", get(collab_websocket))
        .route("/v1/libraries", get(list_libraries).post(create_library))
        .route("/v1/libraries/{library}", get(get_library))
        .route("/v1/libraries/{library}/documents", get(list_documents))
        .route("/v1/libraries/{library}/search", get(search_documents))
        .route(
            "/v1/libraries/{library}/search/suggest",
            get(suggest_documents),
        )
        .route("/v1/libraries/{library}/reindex", post(reindex_library))
        .route("/v1/libraries/{library}/graph", get(graph))
        .route(
            "/v1/libraries/{library}/documents/{*path}",
            get(get_document)
                .head(head_document)
                .put(put_document)
                .post(post_document_action)
                .patch(patch_document_metadata)
                .delete(delete_document),
        )
        .route(
            "/v1/libraries/{library}/transactions",
            post(begin_transaction),
        )
        .route(
            "/v1/libraries/{library}/transactions/{tx}/documents/{*path}",
            put(stage_put_document)
                .post(post_transaction_document_action)
                .patch(patch_transaction_document_metadata)
                .delete(stage_delete_document),
        )
        .route(
            "/v1/libraries/{library}/transactions/{tx}/commit",
            post(commit_transaction),
        )
        .route(
            "/v1/libraries/{library}/transactions/{tx}/rollback",
            post(rollback_transaction),
        )
        .route(
            "/v1/libraries/{library}/git/peers",
            get(list_git_peers).post(create_git_peer),
        )
        .route("/v1/libraries/{library}/git/import", post(git_import))
        .route("/v1/libraries/{library}/git/export", post(git_export))
        .route(
            "/v1/libraries/{library}/git/peers/{peer}/pull",
            post(git_pull),
        )
        .route(
            "/v1/libraries/{library}/git/peers/{peer}/push",
            post(git_push),
        )
        .route(
            "/v1/libraries/{library}/git/peers/{peer}/sync",
            post(git_sync),
        )
        .route("/v1/libraries/{library}/conflicts", get(list_conflicts))
        .route(
            "/v1/libraries/{library}/conflicts/{conflict}",
            get(get_conflict),
        )
        .route(
            "/v1/libraries/{library}/conflicts/{conflict}/resolve",
            post(resolve_conflict),
        );

    #[cfg(feature = "bundle_ui")]
    let router = router.fallback(get(browser_asset));

    router.with_state(AppState {
        store,
        collab: collab::CollabHub::default(),
    })
}

pub async fn serve(store: QuarryStore, addr: SocketAddr) -> std::io::Result<()> {
    warn_if_non_loopback(addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "quarry REST server listening");
    axum::serve(listener, router(store)).await
}

fn warn_if_non_loopback(addr: SocketAddr) {
    if should_warn_non_loopback(addr) {
        eprintln!(
            "warning: Quarry phase one has no auth; binding REST to non-loopback address {addr}"
        );
    }
}

fn should_warn_non_loopback(addr: SocketAddr) -> bool {
    let is_loopback = match addr.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    };
    !is_loopback
}

#[cfg(feature = "bundle_ui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../ui/dist"]
struct BrowserAssets;

#[cfg(feature = "bundle_ui")]
async fn browser_asset(uri: Uri) -> Response {
    if uri.path().starts_with("/v1/") || uri.path() == "/v1" {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "not found".to_string(),
            }),
        )
            .into_response();
    }

    let requested_path = uri.path().trim_start_matches('/');
    let asset_path = if requested_path.is_empty() {
        "index.html"
    } else {
        requested_path
    };
    let (asset_path, asset) = BrowserAssets::get(asset_path)
        .map(|asset| (asset_path, asset))
        .or_else(|| BrowserAssets::get("index.html").map(|asset| ("index.html", asset)))
        .expect("embedded browser bundle must contain index.html");

    let mut response = Response::new(Body::from(asset.data.into_owned()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(
            mime_guess::from_path(asset_path)
                .first_or_octet_stream()
                .essence_str(),
        )
        .unwrap(),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(browser_cache_control(asset_path)),
    );
    response
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        openapi_json,
        admin_gc,
        events,
        create_library,
        list_libraries,
        get_library,
        list_documents,
        search_documents,
        suggest_documents,
        reindex_library,
        graph,
        get_document,
        document_backlinks_openapi,
        document_outgoing_links_openapi,
        document_versions_openapi,
        document_version_openapi,
        document_version_diff_openapi,
        document_version_restore_openapi,
        head_document,
        put_document,
        post_document_action,
        patch_document_metadata,
        delete_document,
        begin_transaction,
        stage_put_document,
        post_transaction_document_action,
        patch_transaction_document_metadata,
        stage_delete_document,
        commit_transaction,
        rollback_transaction,
        create_git_peer,
        list_git_peers,
        git_import,
        git_export,
        git_pull,
        git_push,
        git_sync,
        list_conflicts,
        get_conflict,
        resolve_conflict
    ),
    components(schemas(
        CreateLibraryRequest,
        BeginTransactionRequest,
        ErrorResponse,
        MoveRequest,
        Library,
        DocumentListEntry,
        DocumentVersion,
        DocumentVersionContent,
        WriteOutcome,
        TransactionRecord,
        ConflictRecord,
        SearchResponse,
        SearchResult,
        SearchSuggestion,
        ReindexReport,
        DocumentLink,
        LinkCollection,
        GraphNode,
        GraphEdge,
        GraphResponse,
        VersionDiff,
        GitPeerRequest,
        GitPeer,
        GitImportRequest,
        GitExportRequest,
        GitImportResult,
        GitExportResult,
        GitSyncResult,
        GcReport
    ))
)]
struct ApiDoc;

#[utoipa::path(get, path = "/v1/health", responses((status = 200, body = JsonValue)))]
async fn health() -> Json<JsonValue> {
    Json(serde_json::json!({"ok": true, "service": "quarry"}))
}

#[utoipa::path(get, path = "/v1/openapi.json", responses((status = 200, body = JsonValue)))]
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

async fn collab_websocket(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        state.collab.serve_socket(document_id, socket).await;
    })
}

#[utoipa::path(
    get,
    path = "/v1/events",
    params(("library" = String, Query)),
    responses((status = 200, description = "Server-sent event stream"))
)]
async fn events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let library = state.store.get_library(&query.library).await?;
    let receiver = state.store.subscribe_events();
    let stream = stream::unfold(
        (receiver, library.id, library.slug),
        |(mut receiver, library_id, library_slug)| async move {
            loop {
                match receiver.recv().await {
                    Ok(event) if event.library_id == library_id => {
                        let event_type = store_event_type(&event);
                        let payload = store_event_payload(&library_slug, &event_type, &event);
                        let event = Event::default().event(event_type).data(payload.to_string());
                        return Some((Ok(event), (receiver, library_id, library_slug)));
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        let event_type = "stream.lagged".to_string();
                        let payload = serde_json::json!({
                            "type": event_type,
                            "library": library_slug,
                            "skipped": skipped
                        });
                        let event = Event::default().event(event_type).data(payload.to_string());
                        return Some((Ok(event), (receiver, library_id, library_slug)));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

#[utoipa::path(post, path = "/v1/admin/gc", responses((status = 200, body = GcReport)))]
async fn admin_gc(State(state): State<AppState>) -> Result<Json<GcReport>, ApiError> {
    let report = state.store.gc().await?;
    tracing::info!(
        reachable_blobs = report.reachable,
        removed_blobs = report.removed,
        "admin GC completed"
    );
    Ok(Json(report))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateLibraryRequest {
    pub slug: String,
}

#[utoipa::path(
    post,
    path = "/v1/libraries",
    request_body = CreateLibraryRequest,
    responses((status = 201, body = Library), (status = 409, body = ErrorResponse))
)]
async fn create_library(
    State(state): State<AppState>,
    Json(request): Json<CreateLibraryRequest>,
) -> Result<(StatusCode, Json<Library>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(state.store.create_library(&request.slug).await?),
    ))
}

#[utoipa::path(get, path = "/v1/libraries", responses((status = 200, body = [Library])))]
async fn list_libraries(State(state): State<AppState>) -> Result<Json<Vec<Library>>, ApiError> {
    Ok(Json(state.store.list_libraries().await?))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}",
    params(("library" = String, Path)),
    responses((status = 200, body = Library), (status = 404, body = ErrorResponse))
)]
async fn get_library(
    State(state): State<AppState>,
    Path(library): Path<String>,
) -> Result<Json<Library>, ApiError> {
    Ok(Json(state.store.get_library(&library).await?))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    prefix: Option<String>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<u64>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    root: Option<String>,
    depth: Option<u64>,
    limit: Option<u64>,
    folder: Option<String>,
    tag: Option<String>,
    link_kind: Option<String>,
    resolved: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    library: String,
}

#[derive(Debug, Deserialize)]
struct DocumentGetQuery {
    against: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents",
    params(("library" = String, Path), ("prefix" = Option<String>, Query), ("limit" = Option<u64>, Query)),
    responses((status = 200, body = [DocumentListEntry]))
)]
async fn list_documents(
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
    get,
    path = "/v1/libraries/{library}/search",
    params(("library" = String, Path), ("q" = Option<String>, Query), ("limit" = Option<u64>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, body = SearchResponse))
)]
async fn search_documents(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    let _cursor = query.cursor.as_deref();
    Ok(Json(
        state
            .store
            .search_documents(
                &library,
                query.q.as_deref().unwrap_or_default(),
                query.limit,
            )
            .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/search/suggest",
    params(("library" = String, Path), ("q" = Option<String>, Query), ("limit" = Option<u64>, Query)),
    responses((status = 200, body = [SearchSuggestion]))
)]
async fn suggest_documents(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<SearchSuggestion>>, ApiError> {
    Ok(Json(
        state
            .store
            .suggest_documents(
                &library,
                query.q.as_deref().unwrap_or_default(),
                query.limit,
            )
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/reindex",
    params(("library" = String, Path)),
    responses((status = 200, body = ReindexReport))
)]
async fn reindex_library(
    State(state): State<AppState>,
    Path(library): Path<String>,
) -> Result<Json<ReindexReport>, ApiError> {
    Ok(Json(state.store.reindex_library(&library).await?))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/graph",
    params(("library" = String, Path), ("root" = Option<String>, Query), ("depth" = Option<u64>, Query), ("limit" = Option<u64>, Query), ("folder" = Option<String>, Query), ("tag" = Option<String>, Query), ("link_kind" = Option<String>, Query), ("resolved" = Option<bool>, Query)),
    responses((status = 200, body = GraphResponse))
)]
async fn graph(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<GraphResponse>, ApiError> {
    Ok(Json(
        state
            .store
            .graph(
                &library,
                query.root.as_deref(),
                query.depth,
                query.limit,
                query.folder.as_deref(),
                query.tag.as_deref(),
                query.link_kind.as_deref(),
                query.resolved,
            )
            .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = String), (status = 404, body = ErrorResponse))
)]
async fn get_document(
    State(state): State<AppState>,
    Query(query): Query<DocumentGetQuery>,
    Path((library, path)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    if let Some(path) = path.strip_suffix("/backlinks") {
        return json_response(
            StatusCode::OK,
            &state.store.backlinks(&library, path).await?,
        );
    }
    if let Some(path) = path.strip_suffix("/outgoing-links") {
        return json_response(
            StatusCode::OK,
            &state.store.outgoing_links(&library, path).await?,
        );
    }
    if let Some(path) = path.strip_suffix("/versions") {
        return json_response(
            StatusCode::OK,
            &state.store.version_history(&library, path).await?,
        );
    }
    if let Some((path, version)) = document_version_diff_path(&path) {
        return json_response(
            StatusCode::OK,
            &state
                .store
                .version_diff(&library, path, version, query.against.as_deref())
                .await?,
        );
    }
    if let Some((path, version)) = document_version_path(&path) {
        return json_response(
            StatusCode::OK,
            &state
                .store
                .document_version(&library, path, version)
                .await?,
        );
    }

    let document = state.store.get_document(&library, &path).await?;
    bytes_response(
        StatusCode::OK,
        document.content,
        &document.version.content_type,
        &document.version.id,
        &document.id,
    )
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/backlinks",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = LinkCollection), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_backlinks_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/outgoing-links",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = LinkCollection), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_outgoing_links_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [DocumentVersion]), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_versions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = DocumentVersionContent), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_version_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}/diff",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path), ("against" = Option<String>, Query)),
    responses((status = 200, body = VersionDiff), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_version_diff_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}/restore",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = WriteOutcome), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_version_restore_openapi() {}

#[utoipa::path(
    head,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200), (status = 404, body = ErrorResponse))
)]
async fn head_document(
    State(state): State<AppState>,
    Path((library, path)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let document = state.store.head_document(&library, &path).await?;
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(&document.head_version_id)).unwrap(),
    );
    response.headers_mut().insert(
        "x-quarry-document-id",
        HeaderValue::from_str(&document.id).unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&document.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    Ok(response)
}

#[utoipa::path(
    put,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = String,
    responses((status = 200, body = WriteOutcome), (status = 412, body = ErrorResponse))
)]
async fn put_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let content_type = content_type(&headers);
    let metadata = metadata_from_headers(&headers, &content_type)?;
    let precondition = precondition_from_headers(&headers)?;
    let collab_session_id = optional_header(&headers, "x-quarry-collab-session-id")?;
    let outcome = state
        .store
        .put_document_with_collab_session(
            &library,
            &path,
            body.to_vec(),
            metadata,
            &content_type,
            DocumentSource::Rest,
            precondition,
            collab_session_id,
        )
        .await?;
    json_with_etag(StatusCode::OK, &outcome, &outcome.version.id)
}

#[utoipa::path(
    patch,
    path = "/v1/libraries/{library}/documents/{path}/metadata",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = JsonValue,
    responses((status = 200, body = WriteOutcome))
)]
async fn patch_document_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
    Json(patch): Json<JsonValue>,
) -> Result<Response, ApiError> {
    let Some(path) = path.strip_suffix("/metadata") else {
        return Err(QuarryError::InvalidPath(
            "metadata patch endpoint must end with /metadata".to_string(),
        )
        .into());
    };
    let outcome = state
        .store
        .patch_metadata(
            &library,
            path,
            patch,
            DocumentSource::Rest,
            precondition_from_headers(&headers)?,
        )
        .await?;
    json_with_etag(StatusCode::OK, &outcome, &outcome.version.id)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MoveRequest {
    pub to_path: String,
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/move",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = MoveRequest,
    responses((status = 200, body = TransactionRecord))
)]
async fn post_document_action(
    State(state): State<AppState>,
    Path((library, path)): Path<(String, String)>,
    Json(request): Json<JsonValue>,
) -> Result<Response, ApiError> {
    if let Some((path, version)) = document_version_restore_path(&path) {
        let outcome = state
            .store
            .restore_document_version(&library, path, version)
            .await?;
        return json_with_etag(StatusCode::OK, &outcome, &outcome.version.id);
    }

    if let Some(from_path) = path.strip_suffix("/move") {
        let to_path = request
            .get("to_path")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| QuarryError::InvalidPath("move request missing to_path".to_string()))?;
        let transaction = state
            .store
            .move_document(&library, from_path, to_path, DocumentSource::Rest)
            .await?;
        return json_response(StatusCode::OK, &transaction);
    }

    Err(QuarryError::NotFound(path).into())
}

#[utoipa::path(
    delete,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
async fn delete_document(
    State(state): State<AppState>,
    Path((library, path)): Path<(String, String)>,
) -> Result<Json<TransactionRecord>, ApiError> {
    Ok(Json(
        state
            .store
            .delete_document(&library, &path, DocumentSource::Rest)
            .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BeginTransactionRequest {
    pub actor: Option<String>,
    pub message: Option<String>,
    pub provenance: Option<JsonValue>,
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/transactions",
    params(("library" = String, Path)),
    request_body = BeginTransactionRequest,
    responses((status = 201, body = TransactionRecord))
)]
async fn begin_transaction(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Json(request): Json<BeginTransactionRequest>,
) -> Result<(StatusCode, Json<TransactionRecord>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .store
                .begin_transaction(
                    &library,
                    DocumentSource::Rest,
                    request.actor,
                    request.message,
                    request.provenance.unwrap_or_else(|| serde_json::json!({})),
                )
                .await?,
        ),
    ))
}

#[utoipa::path(
    put,
    path = "/v1/libraries/{library}/transactions/{tx}/documents/{path}",
    params(("library" = String, Path), ("tx" = String, Path), ("path" = String, Path)),
    request_body = String,
    responses((status = 200, body = JsonValue))
)]
async fn stage_put_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((library, tx, path)): Path<(String, String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    scoped_transaction(&state.store, &library, &tx).await?;
    let content_type = content_type(&headers);
    let metadata = metadata_from_headers(&headers, &content_type)?;
    let version = state
        .store
        .stage_put(&tx, &path, body.to_vec(), metadata, &content_type)
        .await?;
    json_with_etag(StatusCode::OK, &version, &version.id)
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/transactions/{tx}/documents/{path}/move",
    params(("library" = String, Path), ("tx" = String, Path), ("path" = String, Path)),
    request_body = MoveRequest,
    responses((status = 200, body = JsonValue))
)]
async fn post_transaction_document_action(
    State(state): State<AppState>,
    Path((library, tx, path)): Path<(String, String, String)>,
    Json(request): Json<MoveRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let Some(from_path) = path.strip_suffix("/move") else {
        return Err(QuarryError::NotFound(path).into());
    };
    scoped_transaction(&state.store, &library, &tx).await?;
    state
        .store
        .stage_move(&tx, from_path, &request.to_path)
        .await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[utoipa::path(
    patch,
    path = "/v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata",
    params(("library" = String, Path), ("tx" = String, Path), ("path" = String, Path)),
    request_body = JsonValue,
    responses((status = 200, body = JsonValue))
)]
async fn patch_transaction_document_metadata(
    State(state): State<AppState>,
    Path((library, tx, path)): Path<(String, String, String)>,
    Json(patch): Json<JsonValue>,
) -> Result<Response, ApiError> {
    let Some(path) = path.strip_suffix("/metadata") else {
        return Err(QuarryError::InvalidPath(
            "metadata patch endpoint must end with /metadata".to_string(),
        )
        .into());
    };
    scoped_transaction(&state.store, &library, &tx).await?;
    let version = state.store.stage_metadata(&tx, path, patch).await?;
    json_with_etag(StatusCode::OK, &version, &version.id)
}

#[utoipa::path(
    delete,
    path = "/v1/libraries/{library}/transactions/{tx}/documents/{path}",
    params(("library" = String, Path), ("tx" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = JsonValue))
)]
async fn stage_delete_document(
    State(state): State<AppState>,
    Path((library, tx, path)): Path<(String, String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    scoped_transaction(&state.store, &library, &tx).await?;
    state.store.stage_delete(&tx, &path).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/transactions/{tx}/commit",
    params(("library" = String, Path), ("tx" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
async fn commit_transaction(
    State(state): State<AppState>,
    Path((library, tx)): Path<(String, String)>,
) -> Result<Json<TransactionRecord>, ApiError> {
    scoped_transaction(&state.store, &library, &tx).await?;
    Ok(Json(state.store.commit_transaction(&tx).await?))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/transactions/{tx}/rollback",
    params(("library" = String, Path), ("tx" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
async fn rollback_transaction(
    State(state): State<AppState>,
    Path((library, tx)): Path<(String, String)>,
) -> Result<Json<TransactionRecord>, ApiError> {
    scoped_transaction(&state.store, &library, &tx).await?;
    Ok(Json(state.store.rollback_transaction(&tx).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GitPeerRequest {
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
async fn create_git_peer(
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
async fn list_git_peers(
    State(state): State<AppState>,
    Path(library): Path<String>,
) -> Result<Json<Vec<GitPeer>>, ApiError> {
    Ok(Json(state.store.list_git_peers(&library).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GitImportRequest {
    pub repo: String,
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/git/import",
    params(("library" = String, Path)),
    request_body = GitImportRequest,
    responses((status = 200, body = GitImportResult))
)]
async fn git_import(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Json(request): Json<GitImportRequest>,
) -> Result<Json<GitImportResult>, ApiError> {
    Ok(Json(
        import_worktree(&state.store, &library, std::path::Path::new(&request.repo)).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GitExportRequest {
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
async fn git_export(
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
async fn git_pull(
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
async fn git_push(
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
async fn git_sync(
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

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/conflicts",
    params(("library" = String, Path)),
    responses((status = 200, body = [ConflictRecord]))
)]
async fn list_conflicts(
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
async fn get_conflict(
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
async fn resolve_conflict(
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

async fn scoped_transaction(store: &QuarryStore, library: &str, tx: &str) -> Result<(), ApiError> {
    let library = store.get_library(library).await?;
    let transaction = store.get_transaction(tx).await?;
    if transaction.library_id != library.id {
        return Err(QuarryError::NotFound(format!("transaction {tx}")).into());
    }
    Ok(())
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    error: String,
}

impl From<QuarryError> for ApiError {
    fn from(value: QuarryError) -> Self {
        let status = match &value {
            QuarryError::NotFound(_) => StatusCode::NOT_FOUND,
            QuarryError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            QuarryError::Conflict(_) => StatusCode::CONFLICT,
            QuarryError::Busy(_) => StatusCode::SERVICE_UNAVAILABLE,
            QuarryError::InvalidPath(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            message: value.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response();
        if self.status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn metadata_from_headers(headers: &HeaderMap, content_type: &str) -> Result<JsonValue, ApiError> {
    let mut metadata = if let Some(value) = headers.get("x-quarry-metadata") {
        serde_json::from_str(
            value
                .to_str()
                .map_err(|_| QuarryError::InvalidPath("invalid x-quarry-metadata".to_string()))?,
        )
        .map_err(QuarryError::from)?
    } else {
        serde_json::json!({})
    };
    if let JsonValue::Object(object) = &mut metadata {
        object
            .entry("content_type")
            .or_insert_with(|| JsonValue::String(content_type.to_string()));
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn busy_errors_map_to_service_unavailable_with_retry_after() {
        let response =
            ApiError::from(QuarryError::Busy("database locked".to_string())).into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    }

    #[test]
    fn non_loopback_warning_policy_only_warns_for_external_binds() {
        assert!(!should_warn_non_loopback("127.0.0.1:7831".parse().unwrap()));
        assert!(!should_warn_non_loopback("[::1]:7831".parse().unwrap()));
        assert!(should_warn_non_loopback("0.0.0.0:7831".parse().unwrap()));
        assert!(should_warn_non_loopback("[::]:7831".parse().unwrap()));
    }

    #[test]
    fn browser_asset_cache_policy_distinguishes_index_and_hashed_assets() {
        assert_eq!(browser_cache_control("index.html"), "no-cache");
        assert_eq!(
            browser_cache_control("assets/index-abc123.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(browser_cache_control("favicon.ico"), "public, max-age=300");
    }

    #[test]
    fn document_put_store_events_map_to_sse_payloads_with_document_metadata() {
        let event = StoreEvent {
            kind: StoreEventKind::DocumentPut,
            library_id: "library-id".to_string(),
            path: Some("notes/daily.md".to_string()),
            new_path: None,
            source: Some(DocumentSource::Rest),
            tx_id: Some("tx-1".to_string()),
            doc_id: Some("doc-1".to_string()),
            version_id: Some("version-1".to_string()),
            conflict_id: None,
            peer_id: None,
            applied: None,
            conflicts: None,
            collab_session_id: Some("browser:session-1".to_string()),
        };

        let event_type = store_event_type(&event);
        let payload = store_event_payload("notes", &event_type, &event);

        assert_eq!(event_type, "doc.changed");
        assert_eq!(payload["type"], "doc.changed");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["path"], "notes/daily.md");
        assert_eq!(payload["doc_id"], "doc-1");
        assert_eq!(payload["version_id"], "version-1");
        assert_eq!(payload["etag"], "\"version-1\"");
        assert_eq!(payload["collab_session_id"], "browser:session-1");
    }

    #[test]
    fn conflict_store_events_map_to_sse_payloads() {
        let event = StoreEvent {
            kind: StoreEventKind::ConflictCreated,
            library_id: "library-id".to_string(),
            path: Some("notes/conflicted.md".to_string()),
            new_path: None,
            source: None,
            tx_id: None,
            doc_id: None,
            version_id: None,
            conflict_id: Some("conflict-1".to_string()),
            peer_id: None,
            applied: None,
            conflicts: None,
            collab_session_id: None,
        };

        let event_type = store_event_type(&event);
        let payload = store_event_payload("notes", &event_type, &event);

        assert_eq!(event_type, "conflict.created");
        assert_eq!(payload["type"], "conflict.created");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["path"], "notes/conflicted.md");
        assert_eq!(payload["conflict_id"], "conflict-1");
    }

    #[test]
    fn reindex_store_events_map_to_sse_payloads() {
        let event = StoreEvent {
            kind: StoreEventKind::LibraryReindexed,
            library_id: "library-id".to_string(),
            path: None,
            new_path: None,
            source: None,
            tx_id: None,
            doc_id: None,
            version_id: None,
            conflict_id: None,
            peer_id: None,
            applied: None,
            conflicts: None,
            collab_session_id: None,
        };

        let event_type = store_event_type(&event);
        let payload = store_event_payload("notes", &event_type, &event);

        assert_eq!(event_type, "library.reindexed");
        assert_eq!(payload["type"], "library.reindexed");
        assert_eq!(payload["library"], "notes");
    }

    #[test]
    fn links_indexed_store_events_map_to_sse_payloads() {
        let event = StoreEvent {
            kind: StoreEventKind::LinksIndexed,
            library_id: "library-id".to_string(),
            path: Some("notes/daily.md".to_string()),
            new_path: None,
            source: None,
            tx_id: None,
            doc_id: None,
            version_id: None,
            conflict_id: None,
            peer_id: None,
            applied: None,
            conflicts: None,
            collab_session_id: None,
        };

        let event_type = store_event_type(&event);
        let payload = store_event_payload("notes", &event_type, &event);

        assert_eq!(event_type, "links.indexed");
        assert_eq!(payload["type"], "links.indexed");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["path"], "notes/daily.md");
    }

    #[test]
    fn git_sync_store_events_map_to_sse_payloads() {
        let event = StoreEvent {
            kind: StoreEventKind::GitSyncCompleted,
            library_id: "library-id".to_string(),
            path: None,
            new_path: None,
            source: None,
            tx_id: None,
            doc_id: None,
            version_id: None,
            conflict_id: None,
            peer_id: Some("peer-1".to_string()),
            applied: Some(2),
            conflicts: Some(1),
            collab_session_id: None,
        };

        let event_type = store_event_type(&event);
        let payload = store_event_payload("notes", &event_type, &event);

        assert_eq!(event_type, "git.sync.completed");
        assert_eq!(payload["type"], "git.sync.completed");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["peer_id"], "peer-1");
        assert_eq!(payload["applied"], 2);
        assert_eq!(payload["conflicts"], 1);
    }
}

fn precondition_from_headers(headers: &HeaderMap) -> Result<WritePrecondition, ApiError> {
    if let Some(value) = headers.get(header::IF_NONE_MATCH) {
        if value.to_str().unwrap_or_default().trim() == "*" {
            return Ok(WritePrecondition::IfNoneMatch);
        }
    }
    if let Some(value) = headers.get(header::IF_MATCH) {
        let value = value
            .to_str()
            .map_err(|_| QuarryError::PreconditionFailed("invalid If-Match".to_string()))?
            .trim()
            .trim_matches('"')
            .to_string();
        return Ok(WritePrecondition::IfMatch(value));
    }
    Ok(WritePrecondition::None)
}

fn optional_header(headers: &HeaderMap, name: &'static str) -> Result<Option<String>, ApiError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|_| QuarryError::Storage(format!("invalid {name} header")).into())
        })
        .transpose()
}

fn export_options(request: &GitExportRequest) -> GitExportOptions {
    GitExportOptions {
        branch: request.branch.clone().unwrap_or_else(|| "main".to_string()),
        force_large: request.force_large.unwrap_or(false),
        frontmatter_markdown: request.frontmatter_markdown.unwrap_or(true),
    }
}

fn document_version_path(path: &str) -> Option<(&str, &str)> {
    let (path, version) = path.rsplit_once("/versions/")?;
    if path.is_empty() || version.is_empty() || version.contains('/') {
        return None;
    }
    Some((path, version))
}

fn document_version_diff_path(path: &str) -> Option<(&str, &str)> {
    document_version_path(path.strip_suffix("/diff")?)
}

fn document_version_restore_path(path: &str) -> Option<(&str, &str)> {
    document_version_path(path.strip_suffix("/restore")?)
}

fn store_event_type(event: &StoreEvent) -> String {
    match &event.kind {
        StoreEventKind::DocumentPut => "doc.changed",
        StoreEventKind::DocumentDelete => "doc.deleted",
        StoreEventKind::DocumentMove => "doc.moved",
        StoreEventKind::LinksIndexed => "links.indexed",
        StoreEventKind::ConflictCreated => "conflict.created",
        StoreEventKind::ConflictResolved => "conflict.resolved",
        StoreEventKind::LibraryReindexed => "library.reindexed",
        StoreEventKind::GitSyncCompleted => "git.sync.completed",
        StoreEventKind::DirectoryPut
        | StoreEventKind::DirectoryDelete
        | StoreEventKind::DirectoryMove => "directory.changed",
    }
    .to_string()
}

fn store_event_payload(library: &str, event_type: &str, event: &StoreEvent) -> JsonValue {
    let mut payload = serde_json::json!({
        "type": event_type,
        "library": library,
        "path": event.path.clone(),
        "source": event.source.clone(),
        "tx_id": event.tx_id.clone()
    });
    if let Some(object) = payload.as_object_mut() {
        if matches!(
            &event.kind,
            StoreEventKind::DocumentMove | StoreEventKind::DirectoryMove
        ) {
            object.insert(
                "from".to_string(),
                event
                    .path
                    .clone()
                    .map(JsonValue::String)
                    .unwrap_or(JsonValue::Null),
            );
            object.insert(
                "to".to_string(),
                event
                    .new_path
                    .clone()
                    .map(JsonValue::String)
                    .unwrap_or(JsonValue::Null),
            );
        }
        if let Some(conflict_id) = &event.conflict_id {
            object.insert(
                "conflict_id".to_string(),
                JsonValue::String(conflict_id.clone()),
            );
        }
        if let Some(doc_id) = &event.doc_id {
            object.insert("doc_id".to_string(), JsonValue::String(doc_id.clone()));
        }
        if let Some(version_id) = &event.version_id {
            object.insert(
                "version_id".to_string(),
                JsonValue::String(version_id.clone()),
            );
            object.insert("etag".to_string(), JsonValue::String(etag(version_id)));
        }
        if let Some(peer_id) = &event.peer_id {
            object.insert("peer_id".to_string(), JsonValue::String(peer_id.clone()));
        }
        if let Some(applied) = event.applied {
            object.insert("applied".to_string(), JsonValue::from(applied));
        }
        if let Some(conflicts) = event.conflicts {
            object.insert("conflicts".to_string(), JsonValue::from(conflicts));
        }
        if let Some(collab_session_id) = &event.collab_session_id {
            object.insert(
                "collab_session_id".to_string(),
                JsonValue::String(collab_session_id.clone()),
            );
        }
    }
    payload
}

#[cfg(any(feature = "bundle_ui", test))]
fn browser_cache_control(path: &str) -> &'static str {
    if path == "index.html" {
        "no-cache"
    } else if is_hashed_browser_asset(path) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    }
}

#[cfg(any(feature = "bundle_ui", test))]
fn is_hashed_browser_asset(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    path.starts_with("assets/")
        && file_name.contains('-')
        && file_name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| !ext.is_empty())
}

fn etag(version_id: &str) -> String {
    format!("\"{version_id}\"")
}

fn bytes_response(
    status: StatusCode,
    content: Vec<u8>,
    content_type: &str,
    version_id: &str,
    document_id: &str,
) -> Result<Response, ApiError> {
    let mut response = Response::new(axum::body::Body::from(content));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(version_id)).unwrap(),
    );
    response.headers_mut().insert(
        "x-quarry-document-id",
        HeaderValue::from_str(document_id).unwrap(),
    );
    Ok(response)
}

fn json_with_etag<T: Serialize>(
    status: StatusCode,
    value: &T,
    version_id: &str,
) -> Result<Response, ApiError> {
    let bytes = serde_json::to_vec(value).map_err(QuarryError::from)?;
    let mut response = Response::new(axum::body::Body::from(bytes));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(version_id)).unwrap(),
    );
    Ok(response)
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Result<Response, ApiError> {
    let bytes = serde_json::to_vec(value).map_err(QuarryError::from)?;
    let mut response = Response::new(axum::body::Body::from(bytes));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}
