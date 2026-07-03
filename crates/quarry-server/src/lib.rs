mod assets;
mod collab;
mod discovery;
mod gateway;
mod journal;
mod log_redaction;
mod markdown_write;
mod presence;
mod session;
mod sse;

pub use session::{MSG_QUARRY_CHECKPOINT, MSG_QUARRY_CHECKPOINT_FAILED};

use assets::{browser_asset, browser_ui_bundle_embedded};
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{MatchedPath, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use discovery::{agent_discovery, agent_docs, quarry_skill};
use journal::AgentEventJournal;
use percent_encoding::percent_decode_str;
use presence::{AgentPresenceRegistry, PresenceStreamGuard};
use quarry_collab_codec::{
    ReviewMeta, ReviewMetaEntry, ReviewSuggestionKind as CodecReviewSuggestionKind, review_markers,
    review_meta_with_inline_comment_bodies,
};
use quarry_core::{
    CollabInviteToken, ConflictRecord, DocumentHistoryEntry, DocumentLink, DocumentListEntry,
    DocumentSource, DocumentVersion, DocumentVersionContent, GcReport, GitPeer, GraphEdge,
    GraphNode, GraphResponse, Library, LinkCollection, QuarryError, ReindexReport, SearchResponse,
    SearchResult, SearchSuggestion, TransactionRecord, VersionDiff, WriteOutcome,
    WritePrecondition,
};
use quarry_git::{
    GitExportOptions, GitExportResult, GitImportResult, GitSyncResult, export_worktree,
    import_worktree, pull_peer, push_peer, sync_peer,
};
use quarry_storage::{DocumentScopeRef, PutDocumentRequest, QuarryStore, TransactionMetadata};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sse::{
    StoreEventPayloadMode, events, events_for_library, events_for_tmp_document,
    store_event_payload, store_event_type,
};
use std::collections::{HashMap, HashSet};
use std::future::{Future, IntoFuture};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    store: QuarryStore,
    sessions: session::SessionHub,
    agent_events: AgentEventJournal,
    agent_presence: AgentPresenceRegistry,
    shutdown: CancellationToken,
}

const REQUEST_ID_HEADER: &str = "x-quarry-request-id";
const ALLOW_DOCUMENT_KIND_CHANGE_HEADER: &str = "x-quarry-allow-document-kind-change";
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);
const TMP_DOCUMENT_HTTP_BODY_LIMIT: usize =
    quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 16 * 1024;

/// Implicit presence for the endpoints agents use while working on a
/// document: any such request carrying `X-Agent-Id` refreshes the agent's
/// presence entry, auto-creating a `waiting` one on first contact. A missing
/// document is a no-op — the main operation surfaces the real error — so a
/// PUT that creates a document skips the touch and the agent appears on its
/// next call.
async fn touch_agent_presence(
    state: &AppState,
    headers: &HeaderMap,
    library: Option<&str>,
    path: &str,
) -> Result<(), ApiError> {
    let Some(agent_id) = optional_header(headers, "x-agent-id")? else {
        return Ok(());
    };
    let scope = match library {
        Some(library) => DocumentScopeRef::library(library),
        None => DocumentScopeRef::Tmp,
    };
    if let Ok(document) = state.store.head_document_for_scope(&scope, path).await {
        state
            .agent_presence
            .touch(library, path, &document.id, &agent_id);
    }
    Ok(())
}

/// Builds the server state for `store`. Pair with
/// [`install_markdown_writer`] so same-process Git/FUSE/CLI writes route
/// through the gateway and the session mode switch (one owning process per
/// database; out-of-process writers cannot open the store at all).
pub fn app_state(store: QuarryStore) -> AppState {
    let shutdown = CancellationToken::new();
    let agent_events = AgentEventJournal::default();
    agent_events.spawn_ingest(store.clone(), shutdown.clone());
    let sessions = session::SessionHub::new(store.clone());
    AppState {
        store,
        sessions,
        agent_events,
        agent_presence: AgentPresenceRegistry::default(),
        shutdown,
    }
}

impl AppState {
    fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }
}

/// Creates the Phase 4 whole-file Markdown writer over `state` and installs
/// it into the store. The store keeps only a `Weak` reference (the writer
/// holds store clones — a strong registry ref would leak the store and its
/// lock file past shutdown), so the caller must hold the returned handle for
/// as long as file writes should be served.
pub fn install_markdown_writer(state: &AppState) -> Arc<dyn quarry_storage::BlockMarkdownWriter> {
    let writer: Arc<dyn quarry_storage::BlockMarkdownWriter> =
        Arc::new(markdown_write::GatewayMarkdownWriter::new(state.clone()));
    state.store.set_block_markdown_writer(&writer);
    writer
}

pub fn router(store: QuarryStore) -> Router {
    router_with_state(app_state(store))
}

pub fn router_with_state(state: AppState) -> Router {
    let router = Router::new()
        .route("/quarry.SKILL.md", get(quarry_skill))
        .route("/agent-docs", get(agent_docs))
        .route("/.well-known/agent.json", get(agent_discovery))
        .route("/v1/health", get(health))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/openapi.json", get(openapi_json))
        .route("/v1/admin/gc", post(admin_gc));
    let router = install_collab_routes(router);
    let router = install_tmp_document_routes(router);
    let router = install_library_document_routes(router);
    let router = router.fallback(get(browser_asset));

    let router = router.layer(middleware::from_fn(request_tracing_middleware));

    router.with_state(state)
}

fn install_collab_routes(router: Router<AppState>) -> Router<AppState> {
    if !(cfg!(feature = "tmp-documents") || cfg!(feature = "lib-documents")) {
        return router;
    }

    router.route("/v1/collab/{document_id}", get(collab_websocket))
}

fn install_tmp_document_routes(router: Router<AppState>) -> Router<AppState> {
    if !cfg!(feature = "tmp-documents") {
        return router;
    }

    let tmp_document_route = get(get_tmp_document)
        .head(head_tmp_document)
        .post(post_tmp_document_action)
        .put(put_tmp_document)
        .patch(patch_tmp_document_action)
        .delete(delete_tmp_document)
        .layer(DefaultBodyLimit::max(TMP_DOCUMENT_HTTP_BODY_LIMIT));

    router
        .route(
            "/v1/tmp/documents",
            post(create_tmp_document).layer(DefaultBodyLimit::max(TMP_DOCUMENT_HTTP_BODY_LIMIT)),
        )
        .route("/v1/tmp/collab/{secret}/{room}", get(tmp_collab_websocket))
        .route("/v1/tmp/documents/{*path}", tmp_document_route)
}

fn install_library_document_routes(router: Router<AppState>) -> Router<AppState> {
    if !cfg!(feature = "lib-documents") {
        return router;
    }

    router
        .route("/v1/events", get(events))
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
            "/v1/libraries/{library}/events/pending",
            get(agent_events_pending),
        )
        .route("/v1/libraries/{library}/events/ack", post(agent_events_ack))
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
        )
}

async fn request_tracing_middleware(request: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let request_id_header = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok().filter(|value| !value.trim().is_empty()))
        .and_then(|value| HeaderValue::from_str(value).ok())
        .unwrap_or_else(generated_request_id_header);
    let request_id = request_id_header.to_str().unwrap_or_default().to_string();
    let method = request.method().clone();
    let path = log_redaction::redact_path(request.uri().path()).into_owned();
    let matched_route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_string());

    tracing::debug!(
        event = "http.request.started",
        request_id = %request_id,
        method = %method,
        path = %path,
        matched_route = matched_route.as_deref().unwrap_or("unknown"),
        "http request started"
    );

    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(REQUEST_ID_HEADER, request_id_header);
    let duration_ms = started.elapsed().as_millis() as u64;
    tracing::debug!(
        event = "http.request.completed",
        request_id = %request_id,
        method = %method,
        path = %path,
        matched_route = matched_route.as_deref().unwrap_or("unknown"),
        status = response.status().as_u16(),
        duration_ms,
        "http request completed"
    );
    response
}

fn generated_request_id_header() -> HeaderValue {
    HeaderValue::from_str(&Uuid::new_v4().to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("00000000-0000-0000-0000-000000000000"))
}

pub async fn serve(store: QuarryStore, addr: SocketAddr) -> std::io::Result<()> {
    serve_with_shutdown(store, addr, shutdown_signal()).await
}

pub async fn serve_with_shutdown<F>(
    store: QuarryStore,
    addr: SocketAddr,
    shutdown: F,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let state = app_state(store);
    let _markdown_writer = install_markdown_writer(&state);
    serve_state_with_shutdown(state, addr, shutdown).await
}

/// Serves an already-built [`AppState`] — the same-process embedding hook
/// (`quarry mount --serve-addr` shares one state between the FUSE mount's
/// store-installed writer and the HTTP server, so file writes reach the live
/// sessions). The CALLER owns writer installation
/// ([`install_markdown_writer`]) and must keep the returned handle alive for
/// the serving lifetime.
pub async fn serve_state_with_shutdown<F>(
    state: AppState,
    addr: SocketAddr,
    shutdown: F,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    warn_if_non_loopback(addr);
    if !browser_ui_bundle_embedded() {
        tracing::warn!(
            event = "server.ui_bundle.missing",
            "browser UI bundle not embedded; serving API-only (run `bun run build` in ui/)"
        );
    }
    let (shutdown_requested_tx, shutdown_requested_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_task = tokio::spawn(async move {
        shutdown.await;
        let _ = shutdown_requested_tx.send(());
    });
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            shutdown_task.abort();
            return Err(error);
        }
    };
    tracing::info!(
        event = "server.listening",
        %addr,
        "quarry REST server listening"
    );
    let shutdown_token = state.shutdown_token();
    let shutdown_token_for_signal = shutdown_token.clone();
    let agent_events = state.agent_events.clone();
    let (shutdown_started_tx, shutdown_started_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, router_with_state(state))
        .with_graceful_shutdown(async move {
            let _ = shutdown_requested_rx.await;
            shutdown_token_for_signal.cancel();
            let _ = shutdown_started_tx.send(());
        })
        .into_future();
    tokio::pin!(server);
    tokio::pin!(shutdown_started_rx);

    let result = tokio::select! {
        result = &mut server => result,
        _ = &mut shutdown_started_rx => {
            match tokio::time::timeout(SHUTDOWN_GRACE_PERIOD, &mut server).await {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        event = "shutdown.timeout",
                        timeout_ms = SHUTDOWN_GRACE_PERIOD.as_millis() as u64,
                        "quarry REST server did not finish graceful shutdown within grace period"
                    );
                    Ok(())
                }
            }
        }
    };
    shutdown_token.cancel();
    shutdown_task.abort();
    let _ = shutdown_task.await;
    agent_events.join_ingest().await;
    result
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(
                event = "shutdown.signal.listen_failed",
                signal = "ctrl_c",
                %error,
                "failed to listen for Ctrl-C"
            );
        }
    };

    #[cfg(unix)]
    {
        let sigterm = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => {
                    tracing::warn!(
                        event = "shutdown.signal.listen_failed",
                        signal = "sigterm",
                        %error,
                        "failed to listen for SIGTERM"
                    );
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }

    tracing::info!(
        event = "shutdown.signal.received",
        "shutdown signal received"
    );
}

fn warn_if_non_loopback(addr: SocketAddr) {
    if should_warn_non_loopback(addr) {
        eprintln!(
            "warning: Quarry phase one has no auth; binding REST to non-loopback address {addr}"
        );
        tracing::warn!(
            event = "api.non_loopback_auth_warning",
            %addr,
            outcome = "degraded",
            reason_code = "rest_auth_not_enabled",
            reason = "REST server is bound to a non-loopback address while phase-one auth is not enabled",
            "REST server bound to non-loopback address without auth"
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

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        capabilities,
        openapi_json,
        admin_gc,
        collab_websocket_openapi,
        sse::events,
        create_library,
        list_libraries,
        get_library,
        list_documents,
        create_tmp_document,
        tmp_collab_websocket_openapi,
        get_tmp_document,
        head_tmp_document,
        put_tmp_document,
        delete_tmp_document,
        tmp_document_versions_openapi,
        tmp_document_versions_raw_openapi,
        tmp_document_version_openapi,
        tmp_document_ttl_openapi,
        tmp_document_promote_openapi,
        tmp_document_review_openapi,
        tmp_document_blocks_openapi,
        tmp_document_block_transactions_openapi,
        tmp_document_events_stream_openapi,
        tmp_agent_presence_list_openapi,
        tmp_agent_presence_openapi,
        search_documents,
        suggest_documents,
        reindex_library,
        graph,
        get_document,
        document_backlinks_openapi,
        document_outgoing_links_openapi,
        document_snapshot_openapi,
        document_review_openapi,
        document_blocks_openapi,
        document_block_transactions_openapi,
        document_events_stream_openapi,
        document_share_openapi,
        document_share_create_openapi,
        document_share_revoke_openapi,
        agent_presence_list_openapi,
        agent_presence_openapi,
        agent_events_pending,
        agent_events_ack,
        document_versions_openapi,
        document_versions_raw_openapi,
        document_version_openapi,
        document_version_diff_openapi,
        document_version_restore_openapi,
        document_ttl_openapi,
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
        Capabilities,
        BeginTransactionRequest,
        ErrorResponse,
        MoveRequest,
        DryRunValue,
        Library,
        DocumentListEntry,
        DocumentHistoryEntry,
        DocumentVersion,
        DocumentVersionContent,
        WriteOutcome,
        AgentDocumentSnapshot,
        AgentSnapshotBlock,
        AgentBlockRef,
        AgentReviewResponse,
        AgentReviewComment,
        AgentReviewReply,
        AgentReviewSuggestion,
        AgentReviewConflict,
        AgentSuggestionPreview,
        AgentSuggestionKind,
        AgentPresenceStatus,
        AgentPresenceRequest,
        AgentPresenceResponse,
        AgentPresenceListResponse,
        AgentPresenceEntry,
        TmpAgentPresenceResponse,
        TmpAgentPresenceListResponse,
        TmpAgentPresenceEntry,
        AgentPendingEventsResponse,
        AgentEventRecord,
        AgentEventsAckRequest,
        AgentEventsAckResponse,
        gateway::BlockTreeResponse,
        gateway::BlockNodePayload,
        gateway::BlockMarkRunPayload,
        gateway::BlockLinkRangePayload,
        gateway::BlockReviewAnchor,
        gateway::BlockTransactionRequest,
        gateway::BlockTransactionActor,
        gateway::BlockTransactionAck,
        gateway::BlockTransactionError,
        CollabInviteToken,
        CreateCollabInviteRequest,
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

#[derive(Debug, Serialize, ToSchema)]
struct Capabilities {
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
async fn health() -> Json<JsonValue> {
    Json(serde_json::json!({"ok": true, "service": "quarry"}))
}

#[utoipa::path(get, path = "/v1/capabilities", responses((status = 200, body = Capabilities)))]
async fn capabilities() -> Json<Capabilities> {
    Json(Capabilities::current())
}

#[utoipa::path(get, path = "/v1/openapi.json", responses((status = 200, body = JsonValue)))]
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(active_openapi())
}

fn active_openapi() -> utoipa::openapi::OpenApi {
    let mut openapi = ApiDoc::openapi();
    openapi
        .paths
        .paths
        .retain(|path, _| openapi_path_enabled(path));
    openapi
}

fn openapi_path_enabled(path: &str) -> bool {
    if path.starts_with("/v1/tmp/documents") {
        return cfg!(feature = "tmp-documents")
            && (path != "/v1/tmp/documents/{secret}/promote" || cfg!(feature = "lib-documents"));
    }
    if path.starts_with("/v1/tmp/collab") {
        return cfg!(feature = "tmp-documents");
    }
    if path.starts_with("/v1/collab") {
        return cfg!(feature = "tmp-documents") || cfg!(feature = "lib-documents");
    }
    if path == "/v1/events" || path.starts_with("/v1/libraries") {
        return cfg!(feature = "lib-documents");
    }
    true
}

#[utoipa::path(
    get,
    path = "/v1/collab/{document_id}",
    params(("document_id" = String, Path)),
    responses((status = 101, description = "Yjs collaboration websocket"))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn collab_websocket_openapi() {}

async fn collab_websocket(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let shutdown = state.shutdown_token();
    ws.on_upgrade(move |socket| async move {
        state
            .sessions
            .serve_socket(document_id, socket, shutdown)
            .await;
    })
}

#[utoipa::path(
    get,
    path = "/v1/tmp/collab/{secret}/{room}",
    params(("secret" = String, Path), ("room" = String, Path)),
    responses((status = 101, description = "Yjs collaboration websocket for tmp capability documents"))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_collab_websocket_openapi() {}

async fn tmp_collab_websocket(
    State(state): State<AppState>,
    Path((secret, _room)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let document = state.store.head_tmp_document(&secret).await?;
    let shutdown = state.shutdown_token();
    Ok(ws
        .on_upgrade(move |socket| async move {
            state
                .sessions
                .serve_socket(document.id, socket, shutdown)
                .await;
        })
        .into_response())
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/events/pending",
    params(("library" = String, Path), ("after" = Option<u64>, Query), ("limit" = Option<usize>, Query)),
    responses((status = 200, body = AgentPendingEventsResponse), (status = 404, body = ErrorResponse))
)]
async fn agent_events_pending(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(query): Query<AgentPendingEventsQuery>,
) -> Result<Json<AgentPendingEventsResponse>, ApiError> {
    let library = state.store.get_library(&library).await?;
    let after = query.after.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let pending = state
        .agent_events
        .pending_since(&library.id, after, limit)
        .await;
    let next_after = pending.last().map(|event| event.id).unwrap_or(after);
    let events = pending
        .into_iter()
        .map(|logged| {
            let event_type = store_event_type(&logged.event);
            let mut data = store_event_payload(
                &library.slug,
                &event_type,
                &logged.event,
                StoreEventPayloadMode::IncludePaths,
            );
            if let Some(object) = data.as_object_mut() {
                object.insert("event_id".to_string(), JsonValue::from(logged.id));
            }
            AgentEventRecord {
                id: logged.id,
                event: event_type,
                data,
            }
        })
        .collect();

    Ok(Json(AgentPendingEventsResponse { events, next_after }))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/events/ack",
    params(("library" = String, Path)),
    request_body = AgentEventsAckRequest,
    responses((status = 200, body = AgentEventsAckResponse), (status = 404, body = ErrorResponse))
)]
async fn agent_events_ack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(library): Path<String>,
    Json(request): Json<AgentEventsAckRequest>,
) -> Result<Json<AgentEventsAckResponse>, ApiError> {
    state.store.get_library(&library).await?;
    let agent_id = agent_id_from_headers_or_body(&headers, request.agent_id.as_deref())?;
    state
        .agent_events
        .ack(agent_id.clone(), request.event_id)
        .await;
    Ok(Json(AgentEventsAckResponse {
        ok: true,
        agent_id,
        acked_through: request.event_id,
    }))
}

#[utoipa::path(post, path = "/v1/admin/gc", responses((status = 200, body = GcReport)))]
async fn admin_gc(State(state): State<AppState>) -> Result<Json<GcReport>, ApiError> {
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
struct AgentPendingEventsQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentEventRecord {
    pub id: u64,
    pub event: String,
    pub data: JsonValue,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentPendingEventsResponse {
    pub events: Vec<AgentEventRecord>,
    #[serde(rename = "nextAfter")]
    pub next_after: u64,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct AgentEventsAckRequest {
    #[serde(default, rename = "agentId")]
    pub agent_id: Option<String>,
    #[serde(rename = "eventId")]
    pub event_id: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentEventsAckResponse {
    pub ok: bool,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "ackedThrough")]
    pub acked_through: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentPresenceStatus {
    Reading,
    Thinking,
    Acting,
    Waiting,
    Completed,
    Error,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct AgentPresenceRequest {
    #[serde(default, rename = "agentId")]
    pub agent_id: Option<String>,
    #[schema(value_type = AgentPresenceStatus)]
    pub status: String,
    #[serde(default)]
    pub by: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentPresenceEntry {
    pub library: Option<String>,
    pub path: String,
    #[serde(rename = "documentId")]
    pub document_id: String,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentPresenceResponse {
    pub current: AgentPresenceEntry,
    pub presence: Vec<AgentPresenceEntry>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentPresenceListResponse {
    pub presence: Vec<AgentPresenceEntry>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TmpAgentPresenceEntry {
    #[serde(rename = "documentId")]
    pub document_id: String,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

impl From<AgentPresenceEntry> for TmpAgentPresenceEntry {
    fn from(entry: AgentPresenceEntry) -> Self {
        Self {
            document_id: entry.document_id,
            agent_id: entry.agent_id,
            status: entry.status,
            by: entry.by,
            updated_at: entry.updated_at,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TmpAgentPresenceResponse {
    pub current: TmpAgentPresenceEntry,
    pub presence: Vec<TmpAgentPresenceEntry>,
}

impl From<AgentPresenceResponse> for TmpAgentPresenceResponse {
    fn from(response: AgentPresenceResponse) -> Self {
        Self {
            current: response.current.into(),
            presence: response.presence.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TmpAgentPresenceListResponse {
    pub presence: Vec<TmpAgentPresenceEntry>,
}

impl From<AgentPresenceListResponse> for TmpAgentPresenceListResponse {
    fn from(response: AgentPresenceListResponse) -> Self {
        Self {
            presence: response.presence.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct CreateCollabInviteRequest {
    pub role: String,
    #[serde(default, rename = "byHint")]
    pub by_hint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DocumentGetQuery {
    against: Option<String>,
    #[serde(default, flatten)]
    review: DocumentReviewQuery,
}

#[derive(Debug, Default, Deserialize)]
struct DocumentReviewQuery {
    #[serde(default, rename = "includeResolved", alias = "include_resolved")]
    include_resolved: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub enum DryRunValue {
    #[serde(rename = "1")]
    One,
    #[serde(rename = "true")]
    True,
    #[serde(rename = "yes")]
    Yes,
    #[serde(rename = "0")]
    Zero,
    #[serde(rename = "false")]
    False,
    #[serde(rename = "no")]
    No,
}

impl DocumentReviewQuery {
    fn include_resolved(&self) -> Result<bool, ApiError> {
        parse_agent_bool_query(self.include_resolved.as_deref(), "includeResolved")
    }
}

fn parse_agent_bool_query(value: Option<&str>, name: &str) -> Result<bool, ApiError> {
    let Some(value) = value else {
        return Ok(false);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(QuarryError::InvalidPath(format!("invalid {name} value")).into()),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentBlockRef {
    pub ordinal: usize,
    #[serde(
        rename = "contentHash",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub content_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentSnapshotBlock {
    #[serde(rename = "ref")]
    pub block_ref: AgentBlockRef,
    pub markdown: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentDocumentSnapshot {
    #[serde(rename = "documentId")]
    pub document_id: String,
    #[serde(rename = "baseToken")]
    pub base_token: String,
    pub blocks: Vec<AgentSnapshotBlock>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentReviewResponse {
    #[serde(rename = "documentId")]
    pub document_id: String,
    #[serde(rename = "baseToken")]
    pub base_token: String,
    pub comments: Vec<AgentReviewComment>,
    pub suggestions: Vec<AgentReviewSuggestion>,
    /// diff3 conflict review items (Phase 4): unresolved whole-file merge
    /// conflicts, present only for documents with canonical block rows.
    pub conflicts: Vec<AgentReviewConflict>,
}

/// A `kind = conflict` review item: a diff3 merge kept the canonical side and
/// recorded the losing incoming hunk here. Resolves and deletes through
/// `POST .../transactions` with `comment.resolve` / `comment.delete`;
/// resolution never mutates the document.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentReviewConflict {
    pub id: String,
    pub status: String,
    pub by: String,
    pub at: String,
    /// The surviving block the conflict region attaches after; `null` means
    /// the document start.
    #[serde(rename = "afterBlockId")]
    pub after_block_id: Option<String>,
    /// The base (shadow) context the merge diffed against.
    #[serde(rename = "baseMarkdown")]
    pub base_markdown: String,
    /// The losing incoming hunk (empty = the write deleted this region).
    #[serde(rename = "incomingMarkdown")]
    pub incoming_markdown: String,
    /// The canonical side that was retained (empty = canonical had deleted
    /// the region).
    #[serde(rename = "canonicalMarkdown")]
    pub canonical_markdown: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentReviewComment {
    pub id: String,
    pub status: String,
    pub by: String,
    pub at: String,
    #[serde(rename = "editedAt")]
    pub edited_at: Option<String>,
    #[serde(rename = "ref")]
    pub block_ref: AgentBlockRef,
    pub quote: String,
    pub body: String,
    pub replies: Vec<AgentReviewReply>,
    /// Row-anchored position; present only when the document has canonical
    /// block rows (the Phase 2 review projection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<gateway::BlockReviewAnchor>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentReviewReply {
    pub id: String,
    pub status: String,
    pub by: String,
    pub at: String,
    #[serde(rename = "editedAt")]
    pub edited_at: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentReviewSuggestion {
    pub id: String,
    pub status: String,
    pub kind: AgentSuggestionKind,
    pub by: String,
    pub at: String,
    #[serde(rename = "ref")]
    pub block_ref: AgentBlockRef,
    pub quote: String,
    pub content: String,
    pub preview: AgentSuggestionPreview,
    pub replies: Vec<AgentReviewReply>,
    /// Row-anchored position; present only when the document has canonical
    /// block rows (the Phase 2 review projection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<gateway::BlockReviewAnchor>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentSuggestionPreview {
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentSuggestionKind {
    Insert,
    Delete,
    Remove,
    Replace,
    Substitution,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTmpDocumentRequest {
    pub content: Option<String>,
    pub metadata: Option<JsonValue>,
    pub content_type: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TtlRequest {
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TtlResponse {
    pub expires_at: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PromoteTmpDocumentRequest {
    pub library: String,
    pub path: String,
    pub if_match: Option<String>,
}

#[utoipa::path(
    post,
    path = "/v1/tmp/documents",
    request_body(
        content = CreateTmpDocumentRequest,
        description = "Create a Markdown-only tmp scratch document. content_type defaults to text/markdown; any supplied value must be a Markdown media type. Canonical UTF-8 Markdown is limited to 1 MiB."
    ),
    responses(
        (status = 201, body = WriteOutcome),
        (status = 413, description = "Tmp Markdown content exceeds 1 MiB", body = ErrorResponse),
        (status = 415, description = "Tmp documents are Markdown-only", body = ErrorResponse)
    )
)]
async fn create_tmp_document(
    State(state): State<AppState>,
    Json(request): Json<CreateTmpDocumentRequest>,
) -> Result<Response, ApiError> {
    let requested_content_type = request
        .content_type
        .as_deref()
        .unwrap_or(quarry_storage::TMP_DOCUMENT_DEFAULT_CONTENT_TYPE);
    let content_type =
        quarry_storage::normalize_tmp_markdown_content_type(requested_content_type)?.to_string();
    let mut metadata = request.metadata.unwrap_or_else(|| serde_json::json!({}));
    if let JsonValue::Object(object) = &mut metadata {
        object.insert(
            "content_type".to_string(),
            JsonValue::String(content_type.clone()),
        );
    }
    let ttl = request
        .expires_at
        .map(quarry_storage::TmpTtl::ExpiresAt)
        .unwrap_or(quarry_storage::TmpTtl::Default);
    let outcome = state
        .store
        .create_tmp_document(
            request.content.unwrap_or_default().into_bytes(),
            metadata,
            &content_type,
            ttl,
        )
        .await?;
    json_with_etag(StatusCode::CREATED, &outcome, &outcome.version.id)
}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}",
    params(("secret" = String, Path)),
    responses((status = 200, body = String), (status = 410, body = ErrorResponse))
)]
async fn get_tmp_document(
    State(state): State<AppState>,
    Query(query): Query<DocumentReviewQuery>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (document_path, subresource) = parse_tmp_document_subresource(&path);
    match subresource {
        TmpDocumentSubResource::Blocks => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            return gateway::tmp_document_blocks(&state, document_path).await;
        }
        TmpDocumentSubResource::Review => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            let include_resolved = query.include_resolved()?;
            return json_response(
                StatusCode::OK,
                &agent_tmp_document_review(&state.store, document_path, include_resolved).await?,
            );
        }
        TmpDocumentSubResource::Presence => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            state.store.head_tmp_document(document_path).await?;
            return json_response(
                StatusCode::OK,
                &TmpAgentPresenceListResponse::from(state.agent_presence.list(None, document_path)),
            );
        }
        TmpDocumentSubResource::EventsStream => {
            let document = state.store.head_tmp_document(document_path).await?;
            let document_id = document.id.clone();
            let presence_guard = optional_header(&headers, "x-agent-id")?.map(|agent_id| {
                PresenceStreamGuard::open(
                    state.agent_presence.clone(),
                    None,
                    document_path.to_string(),
                    document_id.clone(),
                    agent_id,
                )
            });
            return Ok(events_for_tmp_document(
                &state.store,
                document_path.to_string(),
                document_id,
                presence_guard,
                state.shutdown_token(),
            )
            .await?
            .into_response());
        }
        TmpDocumentSubResource::RawVersions => {
            return json_response(
                StatusCode::OK,
                &state.store.raw_tmp_version_history(document_path).await?,
            );
        }
        TmpDocumentSubResource::Versions => {
            return json_response(
                StatusCode::OK,
                &state.store.tmp_version_history(document_path).await?,
            );
        }
        TmpDocumentSubResource::Version(version) => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .tmp_document_version(document_path, version)
                    .await?,
            );
        }
        TmpDocumentSubResource::Document
        | TmpDocumentSubResource::Ttl
        | TmpDocumentSubResource::Transactions
        | TmpDocumentSubResource::Promote => {}
    }
    touch_agent_presence(&state, &headers, None, &path).await?;
    let document = state.store.get_tmp_document(&path).await?;
    bytes_response_with_expiry(
        StatusCode::OK,
        document.content,
        &document.version.content_type,
        &document.version.id,
        &document.id,
        document.expires_at.as_deref(),
    )
}

#[utoipa::path(
    head,
    path = "/v1/tmp/documents/{secret}",
    params(("secret" = String, Path)),
    responses((status = 200), (status = 410, body = ErrorResponse))
)]
async fn head_tmp_document(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Response, ApiError> {
    let document = state.store.head_tmp_document(&path).await?;
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::OK;
    insert_document_headers(
        response.headers_mut(),
        &document.content_type,
        &document.head_version_id,
        &document.id,
        document.expires_at.as_deref(),
    )?;
    Ok(response)
}

#[utoipa::path(
    put,
    path = "/v1/tmp/documents/{secret}",
    params(
        ("secret" = String, Path),
        (
            "If-Match" = Option<String>,
            Header,
            description = "Optional ETag/document clock used as the merge base for Markdown writes"
        ),
        (
            "If-None-Match" = Option<String>,
            Header,
            description = "Use * to create a new tmp document at this capability path"
        )
    ),
    request_body(
        description = "Tmp documents are Markdown-only scratch documents. Whole-document writes require Content-Type: text/markdown (or another accepted Markdown media type) and canonical UTF-8 Markdown no larger than 1 MiB.",
        content(
            (String = "text/markdown")
        )
    ),
    responses(
        (status = 200, body = WriteOutcome),
        (status = 412, body = ErrorResponse),
        (status = 413, description = "Tmp Markdown body exceeds 1 MiB", body = ErrorResponse),
        (status = 415, description = "Tmp writes require a Markdown Content-Type", body = ErrorResponse)
    )
)]
async fn put_tmp_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(path): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    touch_agent_presence(&state, &headers, None, &path).await?;
    let content_type = require_tmp_markdown_content_type(&headers)?;
    let metadata = tmp_metadata_from_headers(&headers, &content_type)?;
    let precondition = precondition_from_headers(&headers)?;
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    let transaction = transaction_metadata_from_headers(&headers)?;

    gateway::gateway_reply(
        markdown_write::put_tmp_block_document(
            &state,
            &path,
            markdown_write::PutBlockDocumentRequest {
                body: body.to_vec(),
                metadata,
                precondition,
                origin_id,
                transaction,
            },
        )
        .await,
    )
}

#[utoipa::path(
    delete,
    path = "/v1/tmp/documents/{secret}",
    params(("secret" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
async fn delete_tmp_document(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Json<TransactionRecord>, ApiError> {
    Ok(Json(state.store.delete_tmp_document(&path).await?))
}

async fn patch_tmp_document_action(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Json(request): Json<TtlRequest>,
) -> Result<Response, ApiError> {
    let (document_path, subresource) = parse_tmp_document_subresource(&path);
    if subresource != TmpDocumentSubResource::Ttl {
        return Err(QuarryError::NotFound(path).into());
    }
    let entry = state
        .store
        .set_tmp_document_ttl(document_path, request.expires_at)
        .await?;
    json_response(
        StatusCode::OK,
        &TtlResponse {
            expires_at: entry.expires_at,
        },
    )
}

async fn post_tmp_document_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(path): Path<String>,
    Json(request): Json<JsonValue>,
) -> Result<Response, ApiError> {
    let (document_path, subresource) = parse_tmp_document_subresource(&path);
    match subresource {
        TmpDocumentSubResource::Transactions => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            gateway::tmp_document_block_transactions(&state, document_path, request).await
        }
        TmpDocumentSubResource::Presence => {
            let request: AgentPresenceRequest =
                serde_json::from_value(request).map_err(|error| {
                    QuarryError::InvalidPath(format!("invalid presence request: {error}"))
                })?;
            let response =
                agent_presence_tmp_document(&state, &headers, document_path, request).await?;
            json_response(StatusCode::OK, &response)
        }
        TmpDocumentSubResource::Promote => {
            if !cfg!(feature = "lib-documents") {
                return Err(QuarryError::NotFound(document_path.to_string()).into());
            }
            let request: PromoteTmpDocumentRequest =
                serde_json::from_value(request).map_err(|error| {
                    QuarryError::InvalidPath(format!("invalid promote request: {error}"))
                })?;
            let precondition = request
                .if_match
                .map(WritePrecondition::IfMatch)
                .unwrap_or(WritePrecondition::None);
            let entry = state
                .store
                .promote_tmp_document(document_path, &request.library, &request.path, precondition)
                .await?;
            json_response(StatusCode::OK, &entry)
        }
        TmpDocumentSubResource::Document
        | TmpDocumentSubResource::Blocks
        | TmpDocumentSubResource::Review
        | TmpDocumentSubResource::EventsStream
        | TmpDocumentSubResource::RawVersions
        | TmpDocumentSubResource::Versions
        | TmpDocumentSubResource::Version(_)
        | TmpDocumentSubResource::Ttl => Err(QuarryError::NotFound(path).into()),
    }
}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/versions",
    params(("secret" = String, Path)),
    responses((status = 200, body = [DocumentHistoryEntry]))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_versions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/versions/raw",
    params(("secret" = String, Path)),
    responses((status = 200, body = [DocumentVersion]))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_versions_raw_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/versions/{version}",
    params(("secret" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = DocumentVersionContent))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_version_openapi() {}

#[utoipa::path(
    patch,
    path = "/v1/tmp/documents/{secret}/ttl",
    params(("secret" = String, Path)),
    request_body = TtlRequest,
    responses((status = 200, body = TtlResponse), (status = 400, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_ttl_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/tmp/documents/{secret}/promote",
    params(("secret" = String, Path)),
    request_body = PromoteTmpDocumentRequest,
    responses((status = 200, body = DocumentListEntry), (status = 409, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_promote_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/review",
    params(("secret" = String, Path), ("includeResolved" = Option<DryRunValue>, Query)),
    responses((status = 200, body = AgentReviewResponse), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_review_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/blocks",
    params(("secret" = String, Path)),
    responses(
        (status = 200, body = gateway::BlockTreeResponse),
        (status = 404, body = ErrorResponse),
        (status = 422, body = gateway::BlockTransactionError)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_blocks_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/tmp/documents/{secret}/transactions",
    params(("secret" = String, Path)),
    request_body = gateway::BlockTransactionRequest,
    responses(
        (status = 200, body = gateway::BlockTransactionAck),
        (status = 400, body = gateway::BlockTransactionError),
        (status = 404, body = gateway::BlockTransactionError),
        (status = 412, body = gateway::BlockTransactionError),
        (status = 413, body = gateway::BlockTransactionError),
        (status = 422, body = gateway::BlockTransactionError)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_block_transactions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/events/stream",
    params(("secret" = String, Path)),
    responses((status = 200, description = "Tmp document-scoped server-sent event stream"), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_document_events_stream_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/presence",
    params(("secret" = String, Path)),
    responses((status = 200, body = TmpAgentPresenceListResponse), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_agent_presence_list_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/tmp/documents/{secret}/presence",
    params(("secret" = String, Path)),
    request_body = AgentPresenceRequest,
    responses((status = 200, body = TmpAgentPresenceResponse), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn tmp_agent_presence_openapi() {}

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
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (document_path, subresource) = parse_document_subresource(&path);
    match subresource {
        DocumentSubResource::Backlinks => {
            return json_response(
                StatusCode::OK,
                &state.store.backlinks(&library, document_path).await?,
            );
        }
        DocumentSubResource::OutgoingLinks => {
            return json_response(
                StatusCode::OK,
                &state.store.outgoing_links(&library, document_path).await?,
            );
        }
        DocumentSubResource::Blocks => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            return gateway::document_blocks(&state, &library, document_path).await;
        }
        DocumentSubResource::Snapshot => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            return json_response(
                StatusCode::OK,
                &agent_document_snapshot(&state.store, &library, document_path).await?,
            );
        }
        DocumentSubResource::Review => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            let include_resolved = query.review.include_resolved()?;
            return json_response(
                StatusCode::OK,
                &agent_document_review(&state.store, &library, document_path, include_resolved)
                    .await?,
            );
        }
        DocumentSubResource::Presence => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            state.store.head_document(&library, document_path).await?;
            return json_response(
                StatusCode::OK,
                &state.agent_presence.list(Some(&library), document_path),
            );
        }
        DocumentSubResource::EventsStream => {
            let document = state.store.head_document(&library, document_path).await?;
            let presence_guard = optional_header(&headers, "x-agent-id")?.map(|agent_id| {
                PresenceStreamGuard::open(
                    state.agent_presence.clone(),
                    Some(library.clone()),
                    document_path.to_string(),
                    document.id,
                    agent_id,
                )
            });
            return Ok(events_for_library(
                &state.store,
                &library,
                Some(document_path.to_string()),
                presence_guard,
                state.shutdown_token(),
            )
            .await?
            .into_response());
        }
        DocumentSubResource::Share => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .collab_invite_tokens(&library, document_path)
                    .await?,
            );
        }
        DocumentSubResource::RawVersions => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .raw_version_history(&library, document_path)
                    .await?,
            );
        }
        DocumentSubResource::Versions => {
            return json_response(
                StatusCode::OK,
                &state.store.version_history(&library, document_path).await?,
            );
        }
        DocumentSubResource::Version(version) => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .document_version(&library, document_path, version)
                    .await?,
            );
        }
        DocumentSubResource::VersionDiff(version) => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .version_diff(&library, document_path, version, query.against.as_deref())
                    .await?,
            );
        }
        DocumentSubResource::Document
        | DocumentSubResource::Metadata
        | DocumentSubResource::Move
        | DocumentSubResource::ShareRevoke(_)
        | DocumentSubResource::Transactions
        | DocumentSubResource::Ttl
        | DocumentSubResource::VersionRestore(_) => {}
    }

    touch_agent_presence(&state, &headers, Some(&library), &path).await?;
    let document = state.store.get_document(&library, &path).await?;
    bytes_response_with_expiry(
        StatusCode::OK,
        document.content,
        &document.version.content_type,
        &document.version.id,
        &document.id,
        document.expires_at.as_deref(),
    )
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/backlinks",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = LinkCollection), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_backlinks_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/outgoing-links",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = LinkCollection), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_outgoing_links_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/snapshot",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = AgentDocumentSnapshot), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_snapshot_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/review",
    params(("library" = String, Path), ("path" = String, Path), ("includeResolved" = Option<DryRunValue>, Query)),
    responses((status = 200, body = AgentReviewResponse), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_review_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/blocks",
    params(("library" = String, Path), ("path" = String, Path)),
    responses(
        (status = 200, body = gateway::BlockTreeResponse),
        (status = 404, body = ErrorResponse),
        (status = 422, body = gateway::BlockTransactionError)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_blocks_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/transactions",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = gateway::BlockTransactionRequest,
    responses(
        (status = 200, body = gateway::BlockTransactionAck),
        (status = 400, body = gateway::BlockTransactionError),
        (status = 404, body = gateway::BlockTransactionError),
        (status = 412, body = gateway::BlockTransactionError),
        (status = 422, body = gateway::BlockTransactionError)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_block_transactions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/events/stream",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, description = "Document-scoped server-sent event stream"), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_events_stream_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/share",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [CollabInviteToken]), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_share_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/share",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = CreateCollabInviteRequest,
    responses((status = 201, body = CollabInviteToken), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_share_create_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/share/{token}/revoke",
    params(("library" = String, Path), ("path" = String, Path), ("token" = String, Path)),
    responses((status = 200, body = CollabInviteToken), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_share_revoke_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [DocumentHistoryEntry]), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_versions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/raw",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [DocumentVersion]), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_versions_raw_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = DocumentVersionContent), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_version_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}/diff",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path), ("against" = Option<String>, Query)),
    responses((status = 200, body = VersionDiff), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_version_diff_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}/restore",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = WriteOutcome), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
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
    insert_document_headers(
        response.headers_mut(),
        &document.content_type,
        &document.head_version_id,
        &document.id,
        document.expires_at.as_deref(),
    )?;
    Ok(response)
}

#[utoipa::path(
    put,
    path = "/v1/libraries/{library}/documents/{path}",
    params(
        ("library" = String, Path),
        ("path" = String, Path),
        (
            "If-Match" = Option<String>,
            Header,
            description = "Optional ETag/document clock used as the merge base for Markdown writes"
        ),
        (
            "If-None-Match" = Option<String>,
            Header,
            description = "Use * to create a new document"
        ),
        (
            "X-Quarry-Allow-Document-Kind-Change" = Option<String>,
            Header,
            description = "Set to true to intentionally change an existing Markdown block document into a raw document"
        )
    ),
    request_body(
        description = "Whole-document Markdown writes require Content-Type: text/markdown. Raw writes must use an explicit raw media type; existing Markdown documents reject raw kind changes unless X-Quarry-Allow-Document-Kind-Change: true is sent.",
        content(
            (String = "text/markdown"),
            (String = "text/plain"),
            (String = "application/octet-stream")
        )
    ),
    responses(
        (status = 200, body = WriteOutcome),
        (status = 409, description = "Existing Markdown document would be changed into a raw document without X-Quarry-Allow-Document-Kind-Change: true", body = ErrorResponse),
        (status = 412, body = ErrorResponse)
    )
)]
async fn put_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    touch_agent_presence(&state, &headers, Some(&library), &path).await?;
    let content_type = content_type(&headers);
    let metadata = metadata_from_headers(&headers, &content_type)?;
    let precondition = precondition_from_headers(&headers)?;
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    let transaction = transaction_metadata_from_headers(&headers)?;
    let incoming_kind = quarry_storage::document_kind(&path, &content_type);

    // Phase 4: a BlockDocument PUT is a whole-file write reconciled via
    // diff3 against the canonical block rows — block ids and review anchors
    // survive, true conflicts become review items, and a live session
    // receives the merge as a collaborator edit. RawDocuments keep the
    // untouched legacy byte path below.
    reject_block_document_downgrade_for_library(
        &state.store,
        &headers,
        &library,
        &path,
        incoming_kind,
    )
    .await?;
    if incoming_kind == quarry_storage::DocumentKind::BlockDocument {
        return gateway::gateway_reply(
            markdown_write::put_block_document(
                &state,
                &library,
                &path,
                markdown_write::PutBlockDocumentRequest {
                    body: body.to_vec(),
                    metadata,
                    precondition,
                    origin_id,
                    transaction,
                },
            )
            .await,
        );
    }

    let outcome = state
        .store
        .put_document(PutDocumentRequest {
            library,
            path,
            content: body.to_vec(),
            metadata,
            content_type,
            source: DocumentSource::Rest,
            precondition,
            origin_id,
            transaction,
        })
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
    let (document_path, subresource) = parse_document_subresource(&path);
    if subresource == DocumentSubResource::Ttl {
        let request: TtlRequest = serde_json::from_value(patch)
            .map_err(|error| QuarryError::InvalidPath(format!("invalid ttl request: {error}")))?;
        let entry = state
            .store
            .set_document_ttl(&library, document_path, request.expires_at)
            .await?;
        return json_response(
            StatusCode::OK,
            &TtlResponse {
                expires_at: entry.expires_at,
            },
        );
    }

    if subresource != DocumentSubResource::Metadata {
        return Err(QuarryError::InvalidPath(
            "metadata patch endpoint must end with /metadata".to_string(),
        )
        .into());
    }
    // Phase 4: a metadata patch on a BlockDocument must NOT destroy the
    // block projection (the legacy path re-puts the content, which clears
    // rows and review items fail-closed, and bypasses the session mutex).
    // It routes through the gateway as a zero-op transaction with a
    // metadata override instead — see `markdown_write::patch_block_document_metadata`.
    if let Ok(head) = state.store.head_document(&library, document_path).await
        && quarry_storage::document_kind(document_path, &head.content_type)
            == quarry_storage::DocumentKind::BlockDocument
    {
        return gateway::gateway_reply(
            markdown_write::patch_block_document_metadata(
                &state,
                &library,
                document_path,
                patch,
                precondition_from_headers(&headers)?,
            )
            .await,
        );
    }
    let outcome = state
        .store
        .patch_metadata(
            &library,
            document_path,
            patch,
            DocumentSource::Rest,
            precondition_from_headers(&headers)?,
        )
        .await?;
    json_with_etag(StatusCode::OK, &outcome, &outcome.version.id)
}

#[utoipa::path(
    patch,
    path = "/v1/libraries/{library}/documents/{path}/ttl",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = TtlRequest,
    responses((status = 200, body = TtlResponse), (status = 410, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn document_ttl_openapi() {}

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
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
    Json(request): Json<JsonValue>,
) -> Result<Response, ApiError> {
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    let actor = transaction_metadata_from_headers(&headers)?.actor;
    let (document_path, subresource) = parse_document_subresource(&path);
    if let DocumentSubResource::VersionRestore(version) = subresource {
        touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
        let target = state
            .store
            .document_version(&library, document_path, version)
            .await?;
        // BlockDocument restores are whole-file writes through the reconciler
        // (gateway-dispatched: projection preserved, session-mode aware);
        // RawDocuments keep the byte path.
        if quarry_storage::document_kind(document_path, &target.version.content_type)
            == quarry_storage::DocumentKind::BlockDocument
        {
            return gateway::gateway_reply(
                markdown_write::restore_block_document_version(
                    &state,
                    &library,
                    document_path,
                    &target,
                    origin_id.clone(),
                    actor.clone(),
                )
                .await,
            );
        }
        let outcome = state
            .store
            .restore_document_version_with_origin(
                &library,
                document_path,
                version,
                origin_id.clone(),
                actor.clone(),
            )
            .await?;
        return json_with_etag(StatusCode::OK, &outcome, &outcome.version.id);
    }

    if subresource == DocumentSubResource::Move {
        let to_path = request
            .get("to_path")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| QuarryError::InvalidPath("move request missing to_path".to_string()))?;
        let transaction = state
            .store
            .move_document_with_origin(
                &library,
                document_path,
                to_path,
                DocumentSource::Rest,
                origin_id.clone(),
                actor.clone(),
            )
            .await?;
        return json_response(StatusCode::OK, &transaction);
    }

    if subresource == DocumentSubResource::Share {
        let request: CreateCollabInviteRequest = serde_json::from_value(request)
            .map_err(|error| QuarryError::InvalidPath(format!("invalid share request: {error}")))?;
        let token = state
            .store
            .create_collab_invite_token(&library, document_path, &request.role, request.by_hint)
            .await?;
        return json_response(StatusCode::CREATED, &token);
    }

    if let DocumentSubResource::ShareRevoke(token_id) = subresource {
        let token = state.store.revoke_collab_invite_token(token_id).await?;
        return json_response(StatusCode::OK, &token);
    }

    // The legacy `/edit`, `/ops`, and `POST /review` mutation facades are
    // deleted (Phase 7): they fall through to the 404 below like any unknown
    // route. `POST .../transactions` is the single mutation contract;
    // GET `/review` (the read projection) is unaffected.

    if subresource == DocumentSubResource::Presence {
        let request: AgentPresenceRequest = serde_json::from_value(request).map_err(|error| {
            QuarryError::InvalidPath(format!("invalid presence request: {error}"))
        })?;
        let response =
            agent_presence_document(&state, &headers, &library, document_path, request).await?;
        return json_response(StatusCode::OK, &response);
    }

    if subresource == DocumentSubResource::Transactions {
        touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
        return gateway::document_block_transactions(&state, &library, document_path, request)
            .await;
    }

    Err(QuarryError::NotFound(path).into())
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/presence",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = AgentPresenceListResponse), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn agent_presence_list_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/presence",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = AgentPresenceRequest,
    responses((status = 200, body = AgentPresenceResponse), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
async fn agent_presence_openapi() {}

// `ReviewMeta` / `ReviewMetaEntry` and the endmatter readers are imported from
// the `quarry_collab_codec` facade, single-sourced with the slate conversion
// that needs them.

async fn agent_presence_document(
    state: &AppState,
    headers: &HeaderMap,
    library: &str,
    path: &str,
    request: AgentPresenceRequest,
) -> Result<AgentPresenceResponse, ApiError> {
    let document = state.store.head_document(library, path).await?;
    let agent_id = agent_id_from_headers_or_body(headers, request.agent_id.as_deref())?;
    let status = normalized_agent_status(&request.status)?;
    Ok(state.agent_presence.update(
        Some(library),
        path,
        &document.id,
        agent_id,
        status,
        request.by.filter(|by| !by.trim().is_empty()),
    ))
}

async fn agent_presence_tmp_document(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
    request: AgentPresenceRequest,
) -> Result<TmpAgentPresenceResponse, ApiError> {
    let document = state.store.head_tmp_document(path).await?;
    let agent_id = agent_id_from_headers_or_body(headers, request.agent_id.as_deref())?;
    let status = normalized_agent_status(&request.status)?;
    Ok(TmpAgentPresenceResponse::from(state.agent_presence.update(
        None,
        path,
        &document.id,
        agent_id,
        status,
        request.by.filter(|by| !by.trim().is_empty()),
    )))
}

async fn agent_document_snapshot(
    store: &QuarryStore,
    library: &str,
    path: &str,
) -> Result<AgentDocumentSnapshot, ApiError> {
    let document = store.get_document(library, path).await?;
    let markdown = document_markdown(&document)?;
    let base_token = document.version.id;
    let blocks = snapshot_blocks(&markdown);
    Ok(AgentDocumentSnapshot {
        document_id: document.id,
        base_token,
        blocks,
    })
}

async fn agent_document_review(
    store: &QuarryStore,
    library: &str,
    path: &str,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    let document = store.get_document(library, path).await?;
    agent_document_review_from_document(store, document, include_resolved).await
}

async fn agent_tmp_document_review(
    store: &QuarryStore,
    path: &str,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    let document = store.get_tmp_document(path).await?;
    agent_document_review_from_document(store, document, include_resolved).await
}

async fn agent_document_review_from_document(
    store: &QuarryStore,
    document: quarry_core::Document,
    include_resolved: bool,
) -> Result<AgentReviewResponse, ApiError> {
    // Documents with canonical block rows project review items from
    // `block_review_items` (the Phase 2 rows-backed projection); documents
    // without rows keep the legacy CriticMarkup/endmatter projection.
    let rows = store.load_block_tree(&document.id).await?;
    if !rows.is_empty() {
        let items = store.list_block_review_items(&document.id).await?;
        return Ok(gateway::review_response_from_rows(
            document.id,
            document.version.id,
            &rows,
            &items,
            include_resolved,
        ));
    }
    let markdown = document_markdown(&document)?;
    Ok(agent_review_response_from_markdown(
        document.id,
        document.version.id,
        &markdown,
        include_resolved,
    ))
}

fn agent_review_response_from_markdown(
    document_id: String,
    base_token: String,
    markdown: &str,
    include_resolved: bool,
) -> AgentReviewResponse {
    let blocks = snapshot_blocks(markdown);
    let (_, meta) = review_meta_with_inline_comment_bodies(markdown);
    let markers = agent_review_markers(&blocks);
    let comments = agent_review_comments(&markers.comments, &meta, include_resolved);
    let suggestions = agent_review_suggestions(&markers.suggestions, &meta, include_resolved);
    AgentReviewResponse {
        document_id,
        base_token,
        comments,
        suggestions,
        // Conflict items exist only for documents with block rows (the
        // Phase 4 reconciler); the legacy projection has none.
        conflicts: Vec::new(),
    }
}

fn document_markdown(document: &quarry_core::Document) -> Result<String, ApiError> {
    if !is_markdown_content_type(&document.version.content_type) {
        return Err(QuarryError::InvalidPath(
            "agent document APIs require markdown content".to_string(),
        )
        .into());
    }
    std::str::from_utf8(&document.content)
        .map(str::to_string)
        .map_err(|_| {
            QuarryError::InvalidPath("agent document APIs require UTF-8 markdown".to_string())
                .into()
        })
}

fn is_markdown_content_type(content_type: &str) -> bool {
    matches!(
        content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "text/markdown" | "text/x-markdown" | "application/markdown" | "application/x-markdown"
    )
}

fn snapshot_blocks(markdown: &str) -> Vec<AgentSnapshotBlock> {
    split_markdown_blocks(markdown)
        .into_iter()
        .enumerate()
        .map(|(ordinal, markdown)| AgentSnapshotBlock {
            block_ref: AgentBlockRef {
                ordinal,
                content_hash: Some(block_hash(&markdown)),
            },
            markdown,
        })
        .collect()
}

#[derive(Clone, Debug)]
struct ReviewCommentMarker {
    id: String,
    block_ref: AgentBlockRef,
    quote: String,
    body: String,
}

#[derive(Clone, Debug)]
struct ReviewSuggestionMarker {
    id: String,
    block_ref: AgentBlockRef,
    kind: AgentSuggestionKind,
    quote: String,
    content: String,
    preview: AgentSuggestionPreview,
}

struct AgentReviewMarkers {
    comments: Vec<ReviewCommentMarker>,
    suggestions: Vec<ReviewSuggestionMarker>,
}

fn agent_review_comments(
    markers: &[ReviewCommentMarker],
    meta: &ReviewMeta,
    include_resolved: bool,
) -> Vec<AgentReviewComment> {
    let mut replies = agent_review_replies_by_parent(meta, include_resolved);
    markers
        .iter()
        .filter_map(|marker| {
            let entry = meta.comments.get(&marker.id)?;
            if entry.re.is_some() || !include_review_entry(entry, include_resolved) {
                return None;
            }
            Some(AgentReviewComment {
                id: marker.id.clone(),
                status: review_entry_status(entry),
                by: entry.by.clone(),
                at: entry.at.clone(),
                edited_at: entry.edited_at.clone(),
                block_ref: marker.block_ref.clone(),
                quote: marker.quote.clone(),
                body: entry.body.clone().unwrap_or_else(|| marker.body.clone()),
                replies: replies.remove(&marker.id).unwrap_or_default(),
                anchor: None,
            })
        })
        .collect()
}

fn agent_review_replies_by_parent(
    meta: &ReviewMeta,
    include_resolved: bool,
) -> HashMap<String, Vec<AgentReviewReply>> {
    let mut replies = HashMap::new();
    for (id, entry) in &meta.comments {
        let Some(parent_id) = entry.re.as_deref() else {
            continue;
        };
        if !include_review_entry(entry, include_resolved) {
            continue;
        }
        replies
            .entry(parent_id.to_string())
            .or_insert_with(Vec::new)
            .push(AgentReviewReply {
                id: id.clone(),
                status: review_entry_status(entry),
                by: entry.by.clone(),
                at: entry.at.clone(),
                edited_at: entry.edited_at.clone(),
                body: entry.body.clone().unwrap_or_default(),
            });
    }
    replies
}

fn agent_review_suggestions(
    markers: &[ReviewSuggestionMarker],
    meta: &ReviewMeta,
    include_resolved: bool,
) -> Vec<AgentReviewSuggestion> {
    let mut replies = agent_review_replies_by_parent(meta, include_resolved);
    markers
        .iter()
        .filter_map(|marker| {
            let entry = meta.suggestions.get(&marker.id)?;
            if review_entry_is_resolved(entry) {
                return None;
            }
            Some(AgentReviewSuggestion {
                id: marker.id.clone(),
                status: review_entry_status(entry),
                kind: marker.kind.clone(),
                by: entry.by.clone(),
                at: entry.at.clone(),
                block_ref: marker.block_ref.clone(),
                quote: marker.quote.clone(),
                content: marker.content.clone(),
                preview: marker.preview.clone(),
                replies: replies.remove(&marker.id).unwrap_or_default(),
                anchor: None,
            })
        })
        .collect()
}

fn agent_review_markers(blocks: &[AgentSnapshotBlock]) -> AgentReviewMarkers {
    let mut seen_comments = HashSet::new();
    let mut seen_suggestions = HashSet::new();
    let mut comments = Vec::new();
    let mut suggestions = Vec::new();
    for block in blocks {
        let markers = review_markers(&block.markdown);
        for marker in markers.comments {
            if seen_comments.insert(marker.id.clone()) {
                comments.push(ReviewCommentMarker {
                    id: marker.id,
                    block_ref: block.block_ref.clone(),
                    quote: marker.quote,
                    body: marker.body,
                });
            }
        }
        for marker in markers.suggestions {
            if seen_suggestions.insert(marker.id.clone()) {
                suggestions.push(ReviewSuggestionMarker {
                    id: marker.id,
                    block_ref: block.block_ref.clone(),
                    kind: agent_suggestion_kind(marker.kind),
                    quote: marker.quote,
                    content: marker.content,
                    preview: AgentSuggestionPreview {
                        before: marker.before,
                        after: marker.after,
                    },
                });
            }
        }
    }
    AgentReviewMarkers {
        comments,
        suggestions,
    }
}

fn agent_suggestion_kind(kind: CodecReviewSuggestionKind) -> AgentSuggestionKind {
    match kind {
        CodecReviewSuggestionKind::Insert => AgentSuggestionKind::Insert,
        CodecReviewSuggestionKind::Delete => AgentSuggestionKind::Delete,
        CodecReviewSuggestionKind::Substitution => AgentSuggestionKind::Substitution,
    }
}

fn include_review_entry(entry: &ReviewMetaEntry, include_resolved: bool) -> bool {
    include_resolved || !review_entry_is_resolved(entry)
}

fn review_entry_is_resolved(entry: &ReviewMetaEntry) -> bool {
    entry
        .status
        .as_deref()
        .map(str::trim)
        .is_some_and(|status| status.eq_ignore_ascii_case("resolved"))
}

fn review_entry_status(entry: &ReviewMetaEntry) -> String {
    match entry.status.as_deref().map(str::trim) {
        Some(status) if status.eq_ignore_ascii_case("resolved") => "resolved".to_string(),
        Some(status) if !status.is_empty() => status.to_string(),
        _ => "open".to_string(),
    }
}

fn block_hash(markdown: &str) -> String {
    blake3::hash(markdown.as_bytes()).to_hex().to_string()
}

fn split_markdown_blocks(markdown: &str) -> Vec<String> {
    if markdown.is_empty() {
        return Vec::new();
    }

    let mut blocks = Vec::new();
    let mut block_start = 0usize;
    let mut offset = 0usize;
    let mut pending_boundary = None;
    let mut fence = None;

    for line in markdown.split_inclusive('\n') {
        let line_start = offset;
        let line_end = line_start + line.len();
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let outside_fence = fence.is_none();
        let blank_outside_fence = outside_fence && trimmed.trim().is_empty();

        if outside_fence
            && !blank_outside_fence
            && let Some(boundary) = pending_boundary.take()
            && block_start < boundary
        {
            blocks.push(markdown[block_start..boundary].to_string());
            block_start = boundary;
        }

        update_markdown_fence(trimmed, &mut fence);

        if blank_outside_fence {
            pending_boundary = Some(line_end);
        } else if fence.is_none() {
            pending_boundary = None;
        }

        offset = line_end;
    }

    if offset < markdown.len() {
        let line = &markdown[offset..];
        let outside_fence = fence.is_none();
        if outside_fence
            && !line.trim().is_empty()
            && let Some(boundary) = pending_boundary.take()
            && block_start < boundary
        {
            blocks.push(markdown[block_start..boundary].to_string());
            block_start = boundary;
        }
        if outside_fence && line.trim().is_empty() {
            pending_boundary = Some(markdown.len());
        }
    }

    if let Some(boundary) = pending_boundary
        && boundary > block_start
        && boundary == markdown.len()
    {
        blocks.push(markdown[block_start..boundary].to_string());
        return blocks;
    }
    if block_start < markdown.len() {
        blocks.push(markdown[block_start..].to_string());
    }
    blocks
}

fn update_markdown_fence(trimmed_line: &str, fence: &mut Option<(char, usize)>) {
    let Some((marker, count)) = markdown_fence_marker(trimmed_line) else {
        return;
    };
    match fence {
        Some((open_marker, open_count)) if *open_marker == marker && count >= *open_count => {
            *fence = None;
        }
        None => {
            *fence = Some((marker, count));
        }
        _ => {}
    }
}

fn markdown_fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|char| *char == marker).count();
    (count >= 3).then_some((marker, count))
}

#[utoipa::path(
    delete,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
async fn delete_document(
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
    let (from_path, subresource) = parse_document_subresource(&path);
    if subresource != DocumentSubResource::Move {
        return Err(QuarryError::NotFound(path).into());
    }
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
    let (document_path, subresource) = parse_document_subresource(&path);
    if subresource != DocumentSubResource::Metadata {
        return Err(QuarryError::InvalidPath(
            "metadata patch endpoint must end with /metadata".to_string(),
        )
        .into());
    }
    scoped_transaction(&state.store, &library, &tx).await?;
    let version = state
        .store
        .stage_metadata(&tx, document_path, patch)
        .await?;
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

impl ApiError {
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl From<QuarryError> for ApiError {
    fn from(value: QuarryError) -> Self {
        let status = match &value {
            QuarryError::NotFound(_) => StatusCode::NOT_FOUND,
            QuarryError::Gone(_) => StatusCode::GONE,
            QuarryError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            QuarryError::Conflict(_) => StatusCode::CONFLICT,
            QuarryError::Busy(_) => StatusCode::SERVICE_UNAVAILABLE,
            QuarryError::InvalidPath(_) => StatusCode::BAD_REQUEST,
            QuarryError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            QuarryError::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            QuarryError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
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
        let status = self.status;
        let message = self.message;
        let reason_code = api_error_reason_code(status);
        tracing::debug!(
            event = "api.error.returned",
            status = status.as_u16(),
            outcome = "error",
            reason_code,
            reason = %message,
            "API error returned"
        );
        if status == StatusCode::PRECONDITION_FAILED {
            tracing::debug!(
                event = "api.precondition.failed",
                status = status.as_u16(),
                outcome = "rejected",
                reason_code,
                reason = %message,
                "API precondition failed"
            );
        }
        if status == StatusCode::SERVICE_UNAVAILABLE {
            tracing::warn!(
                event = "api.busy.returned",
                status = status.as_u16(),
                outcome = "busy",
                reason_code,
                reason = %message,
                "API busy response returned"
            );
        }
        let mut response = (status, Json(ErrorResponse { error: message })).into_response();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

fn api_error_reason_code(status: StatusCode) -> &'static str {
    match status {
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::GONE => "gone",
        StatusCode::PRECONDITION_FAILED => "precondition_failed",
        StatusCode::CONFLICT => "conflict",
        StatusCode::SERVICE_UNAVAILABLE => "busy",
        StatusCode::BAD_REQUEST => "bad_request",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        _ => "internal_error",
    }
}

fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn require_tmp_markdown_content_type(headers: &HeaderMap) -> Result<String, ApiError> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(ApiError {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            message: "tmp writes require Content-Type: text/markdown".to_string(),
        });
    };
    let content_type = value.to_str().map_err(|_| ApiError {
        status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
        message: "tmp writes require Content-Type: text/markdown".to_string(),
    })?;
    Ok(quarry_storage::normalize_tmp_markdown_content_type(content_type)?.to_string())
}

fn tmp_metadata_from_headers(
    headers: &HeaderMap,
    content_type: &str,
) -> Result<JsonValue, ApiError> {
    let mut metadata = metadata_from_headers(headers, content_type)?;
    match &mut metadata {
        JsonValue::Object(object) => {
            object.insert(
                "content_type".to_string(),
                JsonValue::String(content_type.to_string()),
            );
            Ok(metadata)
        }
        _ => Ok(serde_json::json!({ "content_type": content_type })),
    }
}

async fn reject_block_document_downgrade_for_library(
    store: &QuarryStore,
    headers: &HeaderMap,
    library: &str,
    path: &str,
    incoming_kind: quarry_storage::DocumentKind,
) -> Result<(), ApiError> {
    if incoming_kind != quarry_storage::DocumentKind::RawDocument
        || document_kind_change_allowed(headers)
    {
        return Ok(());
    }
    match store.head_document(library, path).await {
        Ok(document) => reject_block_document_downgrade(
            path,
            &document.path,
            &document.content_type,
            incoming_kind,
        ),
        Err(QuarryError::NotFound(_)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn reject_block_document_downgrade(
    request_path: &str,
    stored_path: &str,
    stored_content_type: &str,
    incoming_kind: quarry_storage::DocumentKind,
) -> Result<(), ApiError> {
    let current_kind = quarry_storage::document_kind(stored_path, stored_content_type);
    if current_kind == quarry_storage::DocumentKind::BlockDocument
        && incoming_kind == quarry_storage::DocumentKind::RawDocument
    {
        return Err(QuarryError::Conflict(format!(
            "refusing to change {request_path} from a Markdown block document to a raw document; send {ALLOW_DOCUMENT_KIND_CHANGE_HEADER}: true to opt in"
        ))
        .into());
    }
    Ok(())
}

fn document_kind_change_allowed(headers: &HeaderMap) -> bool {
    headers
        .get(ALLOW_DOCUMENT_KIND_CHANGE_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
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
#[expect(
    clippy::items_after_test_module,
    reason = "integration-style test helpers live beside server tests"
)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "tests use unwrap for server fixture setup"
    )]

    use super::*;
    #[cfg(any(feature = "lib-documents", feature = "tmp-documents"))]
    use axum::body::{Body, to_bytes};
    #[cfg(any(feature = "lib-documents", feature = "tmp-documents"))]
    use axum::http::Method;
    use axum::response::IntoResponse;
    use quarry_core::DocumentSource;
    #[cfg(feature = "lib-documents")]
    use quarry_core::WritePrecondition;
    use quarry_storage::StoreConfig;
    use quarry_storage::StoreEvent;
    #[cfg(feature = "tmp-documents")]
    use std::io::Write;
    #[cfg(feature = "tmp-documents")]
    use std::sync::{Arc, Mutex};
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;
    #[cfg(any(feature = "lib-documents", feature = "tmp-documents"))]
    use tower::ServiceExt;
    #[cfg(feature = "tmp-documents")]
    use tracing_subscriber::fmt::MakeWriter;

    async fn test_store() -> (tempfile::TempDir, QuarryStore) {
        let root = tempfile::tempdir().unwrap();
        let store = QuarryStore::open(StoreConfig {
            db_path: root.path().join("quarry.db"),
            cas_path: root.path().join("cas"),
            lock_path: None,
        })
        .await
        .unwrap();
        (root, store)
    }

    #[cfg(feature = "tmp-documents")]
    #[derive(Clone, Default)]
    struct CapturedLogs {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    #[cfg(feature = "tmp-documents")]
    impl CapturedLogs {
        fn clear(&self) {
            self.buffer.lock().unwrap().clear();
        }

        fn output(&self) -> String {
            String::from_utf8(self.buffer.lock().unwrap().clone()).unwrap()
        }
    }

    #[cfg(feature = "tmp-documents")]
    struct CapturedLogWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    #[cfg(feature = "tmp-documents")]
    impl Write for CapturedLogWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.buffer.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[cfg(feature = "tmp-documents")]
    impl<'writer> MakeWriter<'writer> for CapturedLogs {
        type Writer = CapturedLogWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            CapturedLogWriter {
                buffer: self.buffer.clone(),
            }
        }
    }

    #[cfg(feature = "tmp-documents")]
    fn capture_debug_logs() -> (CapturedLogs, tracing::dispatcher::DefaultGuard) {
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("quarry_server=debug"))
            .with_writer(logs.clone())
            .with_ansi(false)
            .with_target(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (logs, guard)
    }

    #[test]
    fn busy_errors_map_to_service_unavailable_with_retry_after() {
        let response =
            ApiError::from(QuarryError::Busy("database locked".to_string())).into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    }

    #[test]
    fn document_subresource_parser_matches_suffix_routes_without_eating_document_paths() {
        assert_eq!(
            parse_document_subresource("notes/daily.md"),
            ("notes/daily.md", DocumentSubResource::Document)
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/blocks"),
            ("notes/daily.md", DocumentSubResource::Blocks)
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/move"),
            ("notes/daily.md", DocumentSubResource::Move)
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/metadata"),
            ("notes/daily.md", DocumentSubResource::Metadata)
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/versions/raw"),
            ("notes/daily.md", DocumentSubResource::RawVersions)
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/versions/v1/diff"),
            ("notes/daily.md", DocumentSubResource::VersionDiff("v1"))
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/versions/v1/restore"),
            ("notes/daily.md", DocumentSubResource::VersionRestore("v1"))
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/share/token-1/revoke"),
            (
                "notes/daily.md",
                DocumentSubResource::ShareRevoke("token-1")
            )
        );
        assert_eq!(
            parse_document_subresource("notes/versions/raw"),
            ("notes", DocumentSubResource::RawVersions)
        );
        assert_eq!(
            parse_document_subresource("notes/daily.md/versions/v1/extra"),
            (
                "notes/daily.md/versions/v1/extra",
                DocumentSubResource::Document
            )
        );
    }

    #[test]
    fn tmp_document_subresource_parser_matches_suffix_routes_without_eating_secrets() {
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret"),
            ("tmp-secret", TmpDocumentSubResource::Document)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/blocks"),
            ("tmp-secret", TmpDocumentSubResource::Blocks)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/review"),
            ("tmp-secret", TmpDocumentSubResource::Review)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/presence"),
            ("tmp-secret", TmpDocumentSubResource::Presence)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/events/stream"),
            ("tmp-secret", TmpDocumentSubResource::EventsStream)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/transactions"),
            ("tmp-secret", TmpDocumentSubResource::Transactions)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/promote"),
            ("tmp-secret", TmpDocumentSubResource::Promote)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/ttl"),
            ("tmp-secret", TmpDocumentSubResource::Ttl)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/versions/raw"),
            ("tmp-secret", TmpDocumentSubResource::RawVersions)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/versions"),
            ("tmp-secret", TmpDocumentSubResource::Versions)
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/versions/v1"),
            ("tmp-secret", TmpDocumentSubResource::Version("v1"))
        );
        assert_eq!(
            parse_tmp_document_subresource("tmp-secret/versions/v1/extra"),
            (
                "tmp-secret/versions/v1/extra",
                TmpDocumentSubResource::Document
            )
        );
    }

    #[test]
    fn non_loopback_warning_policy_only_warns_for_external_binds() {
        assert!(!should_warn_non_loopback("127.0.0.1:7831".parse().unwrap()));
        assert!(!should_warn_non_loopback("[::1]:7831".parse().unwrap()));
        assert!(should_warn_non_loopback("0.0.0.0:7831".parse().unwrap()));
        assert!(should_warn_non_loopback("[::]:7831".parse().unwrap()));
    }

    #[cfg(feature = "lib-documents")]
    #[tokio::test]
    async fn sse_events_stream_completes_after_shutdown_cancellation() {
        let (_root, store) = test_store().await;
        store.create_library("events-shutdown").await.unwrap();
        let state = app_state(store);
        let app = router_with_state(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/events?library=events-shutdown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        state.shutdown_token().cancel();
        timeout(
            Duration::from_secs(1),
            to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("SSE body should complete after shutdown")
        .unwrap();
    }

    #[cfg(feature = "lib-documents")]
    #[tokio::test]
    async fn document_sse_shutdown_keeps_presence_until_ttl() {
        let (_root, store) = test_store().await;
        let library = store.create_library("doc-sse-shutdown").await.unwrap();
        store
            .put_document(PutDocumentRequest {
                library: library.slug.clone(),
                path: "live.md".to_string(),
                content: b"hello".to_vec(),
                metadata: serde_json::json!({"content_type":"text/markdown"}),
                content_type: "text/markdown".to_string(),
                source: DocumentSource::Rest,
                precondition: WritePrecondition::None,
                origin_id: None,
                transaction: TransactionMetadata::default(),
            })
            .await
            .unwrap();
        let state = app_state(store);
        let app = router_with_state(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/libraries/doc-sse-shutdown/documents/live.md/events/stream")
                    .header("X-Agent-Id", "agent-s")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            state
                .agent_presence
                .list(Some("doc-sse-shutdown"), "live.md")
                .presence
                .len(),
            1
        );

        state.shutdown_token().cancel();
        timeout(
            Duration::from_secs(1),
            to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("document SSE body should complete after shutdown")
        .unwrap();
        assert_eq!(
            state
                .agent_presence
                .list(Some("doc-sse-shutdown"), "live.md")
                .presence
                .len(),
            1,
            "closing the stream must not remove presence; only TTL expiry does"
        );
    }

    #[cfg(feature = "tmp-documents")]
    #[tokio::test(flavor = "current_thread")]
    async fn tmp_sse_shutdown_log_omits_capability_path() {
        let (logs, _guard) = capture_debug_logs();
        let (_root, store) = test_store().await;
        let state = app_state(store.clone());
        let app = router_with_state(state.clone());
        let outcome = store
            .create_tmp_document(
                b"hello".to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                quarry_storage::TmpTtl::Default,
            )
            .await
            .unwrap();
        let secret = outcome.document.path;
        let document_id = outcome.document.id;

        logs.clear();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/v1/tmp/documents/{secret}/events/stream"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        state.shutdown_token().cancel();
        timeout(
            Duration::from_secs(1),
            to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("tmp SSE body should complete after shutdown")
        .unwrap();

        let output = logs.output();
        assert!(
            !output.contains(&secret),
            "tmp SSE shutdown logs must not contain tmp secret:\n{output}"
        );
        assert!(
            output.contains("sse.stream.closed"),
            "tmp SSE close event should still be logged:\n{output}"
        );
        assert!(
            output.contains("scope=tmp") && output.contains(&document_id),
            "tmp SSE close logs should keep scope and document id diagnostics:\n{output}"
        );
    }

    #[tokio::test]
    async fn agent_event_journal_ingest_exits_after_shutdown_cancellation() {
        let (_root, store) = test_store().await;
        let journal = AgentEventJournal::default();
        let shutdown = CancellationToken::new();
        journal.spawn_ingest(store, shutdown.clone());

        shutdown.cancel();
        timeout(Duration::from_secs(1), journal.join_ingest())
            .await
            .expect("journal ingest should exit after shutdown");
    }

    #[test]
    fn document_put_store_events_map_to_sse_payloads_with_document_metadata() {
        let event = StoreEvent::document_put(
            "library-id".to_string(),
            "notes/daily.md".to_string(),
            DocumentSource::Rest,
            "tx-1".to_string(),
            "doc-1".to_string(),
            "version-1".to_string(),
            Some("browser:session-1".to_string()),
        );

        let event_type = store_event_type(&event);
        let payload = store_event_payload(
            "notes",
            &event_type,
            &event,
            StoreEventPayloadMode::IncludePaths,
        );

        assert_eq!(event_type, "doc.changed");
        assert_eq!(payload["type"], "doc.changed");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["path"], "notes/daily.md");
        assert_eq!(payload["doc_id"], "doc-1");
        assert_eq!(payload["version_id"], "version-1");
        assert_eq!(payload["etag"], "\"version-1\"");
        assert_eq!(payload["origin_id"], "browser:session-1");

        let tmp_payload =
            store_event_payload("tmp", &event_type, &event, StoreEventPayloadMode::OmitPaths);
        assert_eq!(tmp_payload["type"], "doc.changed");
        assert_eq!(tmp_payload["library"], "tmp");
        assert_eq!(tmp_payload["doc_id"], "doc-1");
        assert_eq!(tmp_payload["version_id"], "version-1");
        assert!(tmp_payload.get("path").is_none());
    }

    #[test]
    fn document_delete_and_move_store_events_map_to_sse_payloads_with_origin() {
        let delete = StoreEvent::document_delete(
            "library-id".to_string(),
            "notes/daily.md".to_string(),
            DocumentSource::Rest,
            "tx-1".to_string(),
            Some("doc-1".to_string()),
            Some("browser:session-1".to_string()),
        );
        let event_type = store_event_type(&delete);
        let payload = store_event_payload(
            "notes",
            &event_type,
            &delete,
            StoreEventPayloadMode::IncludePaths,
        );
        assert_eq!(event_type, "doc.deleted");
        assert_eq!(payload["doc_id"], "doc-1");
        assert_eq!(payload["origin_id"], "browser:session-1");

        let move_event = StoreEvent::document_move(
            "library-id".to_string(),
            "notes/daily.md".to_string(),
            "notes/archive.md".to_string(),
            DocumentSource::Rest,
            "tx-2".to_string(),
            Some("doc-1".to_string()),
            Some("browser:session-1".to_string()),
        );
        let event_type = store_event_type(&move_event);
        let payload = store_event_payload(
            "notes",
            &event_type,
            &move_event,
            StoreEventPayloadMode::IncludePaths,
        );
        assert_eq!(event_type, "doc.moved");
        assert_eq!(payload["from"], "notes/daily.md");
        assert_eq!(payload["to"], "notes/archive.md");
        assert_eq!(payload["doc_id"], "doc-1");
        assert_eq!(payload["origin_id"], "browser:session-1");

        let tmp_payload = store_event_payload(
            "tmp",
            &event_type,
            &move_event,
            StoreEventPayloadMode::OmitPaths,
        );
        assert!(tmp_payload.get("path").is_none());
        assert!(tmp_payload.get("from").is_none());
        assert!(tmp_payload.get("to").is_none());
    }

    #[test]
    fn conflict_store_events_map_to_sse_payloads() {
        let event = StoreEvent::conflict_created(
            "library-id".to_string(),
            "notes/conflicted.md".to_string(),
            "conflict-1".to_string(),
        );

        let event_type = store_event_type(&event);
        let payload = store_event_payload(
            "notes",
            &event_type,
            &event,
            StoreEventPayloadMode::IncludePaths,
        );

        assert_eq!(event_type, "conflict.created");
        assert_eq!(payload["type"], "conflict.created");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["path"], "notes/conflicted.md");
        assert_eq!(payload["conflict_id"], "conflict-1");
    }

    #[test]
    fn reindex_store_events_map_to_sse_payloads() {
        let event = StoreEvent::library_reindexed("library-id".to_string());

        let event_type = store_event_type(&event);
        let payload = store_event_payload(
            "notes",
            &event_type,
            &event,
            StoreEventPayloadMode::IncludePaths,
        );

        assert_eq!(event_type, "library.reindexed");
        assert_eq!(payload["type"], "library.reindexed");
        assert_eq!(payload["library"], "notes");
    }

    #[test]
    fn links_indexed_store_events_map_to_sse_payloads() {
        let event =
            StoreEvent::links_indexed("library-id".to_string(), "notes/daily.md".to_string());

        let event_type = store_event_type(&event);
        let payload = store_event_payload(
            "notes",
            &event_type,
            &event,
            StoreEventPayloadMode::IncludePaths,
        );

        assert_eq!(event_type, "links.indexed");
        assert_eq!(payload["type"], "links.indexed");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["path"], "notes/daily.md");
    }

    #[test]
    fn git_sync_store_events_map_to_sse_payloads() {
        let event =
            StoreEvent::git_sync_completed("library-id".to_string(), "peer-1".to_string(), 2, 1);

        let event_type = store_event_type(&event);
        let payload = store_event_payload(
            "notes",
            &event_type,
            &event,
            StoreEventPayloadMode::IncludePaths,
        );

        assert_eq!(event_type, "git.sync.completed");
        assert_eq!(payload["type"], "git.sync.completed");
        assert_eq!(payload["library"], "notes");
        assert_eq!(payload["peer_id"], "peer-1");
        assert_eq!(payload["applied"], 2);
        assert_eq!(payload["conflicts"], 1);
    }
}

fn precondition_from_headers(headers: &HeaderMap) -> Result<WritePrecondition, ApiError> {
    if let Some(value) = headers.get(header::IF_NONE_MATCH)
        && value.to_str().unwrap_or_default().trim() == "*"
    {
        return Ok(WritePrecondition::IfNoneMatch);
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
                .map_err(|_| QuarryError::InvalidInput(format!("invalid {name} header")).into())
        })
        .transpose()
}

fn transaction_metadata_from_headers(headers: &HeaderMap) -> Result<TransactionMetadata, ApiError> {
    let mut metadata = TransactionMetadata {
        // The browser cannot send non-Latin-1 header values, so the UI
        // percent-encodes the actor's display name. Lossy decoding so a
        // malformed encoding never fails the write.
        actor: optional_header(headers, "x-quarry-transaction-actor")?
            .map(|value| percent_decode_str(&value).decode_utf8_lossy().into_owned()),
        message: optional_header(headers, "x-quarry-transaction-message")?,
        ..TransactionMetadata::default()
    };
    if let Some(value) = headers.get("x-quarry-transaction-provenance") {
        metadata.provenance = Some(
            serde_json::from_str(value.to_str().map_err(|_| {
                QuarryError::InvalidPath("invalid x-quarry-transaction-provenance".to_string())
            })?)
            .map_err(|_| {
                QuarryError::InvalidPath("invalid x-quarry-transaction-provenance".to_string())
            })?,
        );
    }
    Ok(metadata)
}

fn agent_id_from_headers_or_body(
    headers: &HeaderMap,
    body_agent_id: Option<&str>,
) -> Result<String, ApiError> {
    optional_header(headers, "x-agent-id")?
        .or_else(|| body_agent_id.map(str::to_string))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            QuarryError::InvalidPath("agent request missing X-Agent-Id or agentId".to_string())
                .into()
        })
}

fn normalized_agent_status(status: &str) -> Result<String, ApiError> {
    let status = status.trim().to_ascii_lowercase();
    match status.as_str() {
        "reading" | "thinking" | "acting" | "waiting" | "completed" | "error" => Ok(status),
        _ => Err(
            QuarryError::InvalidPath(format!("unsupported agent presence status {status}")).into(),
        ),
    }
}

fn export_options(request: &GitExportRequest) -> GitExportOptions {
    GitExportOptions {
        branch: request.branch.clone().unwrap_or_else(|| "main".to_string()),
        force_large: request.force_large.unwrap_or(false),
        frontmatter_markdown: request.frontmatter_markdown.unwrap_or(true),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocumentSubResource<'path> {
    Document,
    Backlinks,
    OutgoingLinks,
    Blocks,
    Snapshot,
    Review,
    Presence,
    EventsStream,
    Share,
    ShareRevoke(&'path str),
    RawVersions,
    Versions,
    Version(&'path str),
    VersionDiff(&'path str),
    VersionRestore(&'path str),
    Metadata,
    Ttl,
    Move,
    Transactions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmpDocumentSubResource<'path> {
    Document,
    Blocks,
    Review,
    Presence,
    EventsStream,
    RawVersions,
    Versions,
    Version(&'path str),
    Ttl,
    Transactions,
    Promote,
}

fn parse_document_subresource(path: &str) -> (&str, DocumentSubResource<'_>) {
    if let Some((path, token_id)) = document_share_revoke_path(path) {
        return (path, DocumentSubResource::ShareRevoke(token_id));
    }
    if let Some((path, version)) = document_version_path_with_suffix(path, "/diff") {
        return (path, DocumentSubResource::VersionDiff(version));
    }
    if let Some((path, version)) = document_version_path_with_suffix(path, "/restore") {
        return (path, DocumentSubResource::VersionRestore(version));
    }
    if let Some(path) = path.strip_suffix("/versions/raw") {
        return (path, DocumentSubResource::RawVersions);
    }
    if let Some(path) = path.strip_suffix("/versions") {
        return (path, DocumentSubResource::Versions);
    }
    if let Some((path, version)) = document_version_path(path) {
        return (path, DocumentSubResource::Version(version));
    }

    for (suffix, subresource) in [
        ("/events/stream", DocumentSubResource::EventsStream),
        ("/outgoing-links", DocumentSubResource::OutgoingLinks),
        ("/transactions", DocumentSubResource::Transactions),
        ("/backlinks", DocumentSubResource::Backlinks),
        ("/metadata", DocumentSubResource::Metadata),
        ("/presence", DocumentSubResource::Presence),
        ("/snapshot", DocumentSubResource::Snapshot),
        ("/blocks", DocumentSubResource::Blocks),
        ("/review", DocumentSubResource::Review),
        ("/share", DocumentSubResource::Share),
        ("/move", DocumentSubResource::Move),
        ("/ttl", DocumentSubResource::Ttl),
    ] {
        if let Some(path) = path.strip_suffix(suffix) {
            return (path, subresource);
        }
    }

    (path, DocumentSubResource::Document)
}

fn parse_tmp_document_subresource(path: &str) -> (&str, TmpDocumentSubResource<'_>) {
    if let Some(path) = path.strip_suffix("/versions/raw") {
        return (path, TmpDocumentSubResource::RawVersions);
    }
    if let Some(path) = path.strip_suffix("/versions") {
        return (path, TmpDocumentSubResource::Versions);
    }
    if let Some((path, version)) = document_version_path(path) {
        return (path, TmpDocumentSubResource::Version(version));
    }

    for (suffix, subresource) in [
        ("/events/stream", TmpDocumentSubResource::EventsStream),
        ("/transactions", TmpDocumentSubResource::Transactions),
        ("/presence", TmpDocumentSubResource::Presence),
        ("/promote", TmpDocumentSubResource::Promote),
        ("/blocks", TmpDocumentSubResource::Blocks),
        ("/review", TmpDocumentSubResource::Review),
        ("/ttl", TmpDocumentSubResource::Ttl),
    ] {
        if let Some(path) = path.strip_suffix(suffix) {
            return (path, subresource);
        }
    }

    (path, TmpDocumentSubResource::Document)
}

fn document_version_path(path: &str) -> Option<(&str, &str)> {
    let (path, version) = path.rsplit_once("/versions/")?;
    if path.is_empty() || version.is_empty() || version.contains('/') {
        return None;
    }
    Some((path, version))
}

fn document_version_path_with_suffix<'path>(
    path: &'path str,
    suffix: &str,
) -> Option<(&'path str, &'path str)> {
    document_version_path(path.strip_suffix(suffix)?)
}

fn document_share_revoke_path(path: &str) -> Option<(&str, &str)> {
    let path = path.strip_suffix("/revoke")?;
    let (document_path, token_id) = path.rsplit_once("/share/")?;
    if document_path.is_empty() || token_id.is_empty() || token_id.contains('/') {
        return None;
    }
    Some((document_path, token_id))
}

fn etag(version_id: &str) -> String {
    format!("\"{version_id}\"")
}

fn checked_header_value(name: &str, value: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value).map_err(|error| {
        QuarryError::Invariant(format!("invalid {name} response header value: {error}")).into()
    })
}

fn insert_document_headers(
    headers: &mut HeaderMap,
    content_type: &str,
    version_id: &str,
    document_id: &str,
    expires_at: Option<&str>,
) -> Result<(), ApiError> {
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::ETAG,
        checked_header_value(header::ETAG.as_str(), &etag(version_id))?,
    );
    headers.insert(
        "x-quarry-document-id",
        checked_header_value("x-quarry-document-id", document_id)?,
    );
    if let Some(expires_at) = expires_at {
        headers.insert(
            "x-quarry-expires-at",
            checked_header_value("x-quarry-expires-at", expires_at)?,
        );
    }
    Ok(())
}

fn bytes_response_with_expiry(
    status: StatusCode,
    content: Vec<u8>,
    content_type: &str,
    version_id: &str,
    document_id: &str,
    expires_at: Option<&str>,
) -> Result<Response, ApiError> {
    let mut response = Response::new(axum::body::Body::from(content));
    *response.status_mut() = status;
    insert_document_headers(
        response.headers_mut(),
        content_type,
        version_id,
        document_id,
        expires_at,
    )?;
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
        checked_header_value(header::ETAG.as_str(), &etag(version_id))?,
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
