mod agent_events;
mod agent_prompt;
mod assets;
mod collab;
mod collab_handlers;
mod conflicts;
mod discovery;
mod document_handlers;
mod error;
mod gateway;
#[cfg(feature = "lib-documents")]
mod git_handlers;
mod headers;
mod journal;
mod library_handlers;
mod log_redaction;
mod markdown_write;
mod onboarding;
mod presence;
mod review;
mod search_handlers;
mod session;
mod sse;
mod system_handlers;
mod tmp_document_handlers;
mod transaction_handlers;

pub use session::{MSG_QUARRY_CHECKPOINT, MSG_QUARRY_CHECKPOINT_FAILED};

use agent_events::{
    AgentEventRecord, AgentEventsAckRequest, AgentEventsAckResponse, AgentPendingEventsResponse,
};
use assets::{browser_asset, browser_ui_bundle_embedded};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::DefaultBodyLimit;
use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use discovery::{agent_discovery, agent_docs, quarry_skill};
pub use error::{ApiError, ApiErrorCode, ApiErrorResponse};
pub(crate) use headers::{
    agent_id_from_headers_or_body, bytes_response_with_expiry, content_type,
    insert_document_headers, json_response, json_with_etag, metadata_from_headers,
    normalized_agent_status, optional_header, precondition_from_headers,
    reject_block_document_downgrade_for_library, require_tmp_markdown_content_type,
    tmp_metadata_from_headers, transaction_metadata_from_headers,
};
use journal::AgentEventJournal;
use library_handlers::CreateLibraryRequest;
use presence::AgentPresenceRegistry;
use quarry_core::{
    CollabInviteToken, ConflictRecord, DocumentHistoryEntry, DocumentLink, DocumentListEntry,
    DocumentVersion, DocumentVersionContent, GraphEdge, GraphNode, GraphResponse, Library,
    LinkCollection, QuarryError, ReindexReport, SearchResponse, SearchResult, SearchSuggestion,
    TransactionRecord, VersionDiff, WriteOutcome,
};
use quarry_storage::{DocumentScopeRef, QuarryStore};
use review::{
    AgentBlockRef, AgentDocumentSnapshot, AgentReviewComment, AgentReviewConflict,
    AgentReviewReply, AgentReviewResponse, AgentReviewSuggestion, AgentSnapshotBlock,
    AgentSuggestionKind, AgentSuggestionPreview, DryRunValue,
};
use serde::{Deserialize, Serialize};
use sse::events;
use std::future::{Future, IntoFuture};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use transaction_handlers::BeginTransactionRequest;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    store: QuarryStore,
    client_ip_source: ClientIpSource,
    sessions: session::SessionHub,
    agent_events: AgentEventJournal,
    agent_presence: AgentPresenceRegistry,
    shutdown: CancellationToken,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClientIpSource {
    #[default]
    None,
    CloudFrontViewerAddress,
}

impl FromStr for ClientIpSource {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "cloudfront-viewer-address" => Ok(Self::CloudFrontViewerAddress),
            _ => Err(format!(
                "unsupported client IP source {value:?}; expected none or cloudfront-viewer-address"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServerConfig {
    pub client_ip_source: ClientIpSource,
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
    app_state_with_config(store, ServerConfig::default())
}

pub fn app_state_with_config(store: QuarryStore, config: ServerConfig) -> AppState {
    let shutdown = CancellationToken::new();
    let agent_events = AgentEventJournal::default();
    agent_events.spawn_ingest(store.clone(), shutdown.clone());
    let sessions = session::SessionHub::new(store.clone());
    AppState {
        store,
        client_ip_source: config.client_ip_source,
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

    fn client_ip_source(&self) -> ClientIpSource {
        self.client_ip_source
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
        .route("/", get(onboarding::home_page))
        .route("/home.js", get(onboarding::home_js))
        .route("/setup.md", get(onboarding::setup_md))
        .route("/prompt.md", get(onboarding::prompt_md))
        .route("/example.md", get(onboarding::example_md))
        .route("/quarry.SKILL.md", get(quarry_skill))
        .route("/agent-docs", get(agent_docs))
        .route("/.well-known/agent.json", get(agent_discovery))
        .route("/v1/health", get(system_handlers::health))
        .route("/v1/capabilities", get(system_handlers::capabilities))
        .route("/v1/openapi.json", get(system_handlers::openapi_json));
    let router = install_admin_routes(router);
    let router = install_collab_routes(router);
    let router = install_tmp_document_routes(router);
    let router = install_library_document_routes(router);
    let router = router.fallback(get(browser_asset));

    let router = router.layer(middleware::from_fn(api_error_envelope_middleware));
    let router = router.layer(middleware::from_fn(request_tracing_middleware));
    let router = router.layer(middleware::from_fn(security_headers_middleware));

    router.with_state(state)
}

/// Axum extractor and routing rejections bypass [`ApiError`]. Normalize those
/// final non-JSON responses so every `/v1` HTTP failure keeps one wire shape.
async fn api_error_envelope_middleware(request: Request, next: Next) -> Response {
    let is_api = request.uri().path() == "/v1" || request.uri().path().starts_with("/v1/");
    let response = next.run(request).await;
    if !is_api || !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if is_json {
        let (mut parts, body) = response.into_parts();
        let status = parts.status;
        let original_headers = parts.headers.clone();
        const ERROR_BODY_LIMIT: usize = 64 * 1024;
        if let Ok(bytes) = to_bytes(body, ERROR_BODY_LIMIT).await {
            if let Ok(payload) = serde_json::from_slice::<ApiErrorResponse>(&bytes)
                && payload.code.status() == status
            {
                if status == axum::http::StatusCode::SERVICE_UNAVAILABLE {
                    parts
                        .headers
                        .entry(header::RETRY_AFTER)
                        .or_insert(HeaderValue::from_static("1"));
                }
                return Response::from_parts(parts, Body::from(bytes));
            }
        }
        return normalized_error_response(status, original_headers);
    }

    let status = response.status();
    let original_headers = response.headers().clone();
    normalized_error_response(status, original_headers)
}

fn normalized_error_response(
    status: axum::http::StatusCode,
    original_headers: HeaderMap,
) -> Response {
    let mut normalized = error::fallback_error_for_status(status).into_response();
    for (name, value) in original_headers {
        if let Some(name) = name
            && name != header::CONTENT_TYPE
            && name != header::CONTENT_LENGTH
        {
            normalized.headers_mut().insert(name, value);
        }
    }
    normalized
}

/// Strict Content-Security-Policy for the tmp-only public build. `'unsafe-inline'`
/// is required for the editor's runtime styles; `connect-src 'self'` covers the
/// same-origin collab WebSocket and SSE; the strict `frame-src`/`default-src`
/// deliberately block external media embeds.
const CONTENT_SECURITY_POLICY: HeaderValue = HeaderValue::from_static(
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; connect-src 'self'; frame-src 'none'; \
     frame-ancestors 'none'; base-uri 'self'",
);

/// Sets response-hardening headers on every response (handler, fallback, and
/// error alike) and marks secret-bearing tmp responses uncacheable. Runs as an
/// outer layer so it also covers the asset fallback and error bodies.
async fn security_headers_middleware(request: Request, next: Next) -> Response {
    // A tmp path carries the document secret in the URL, so its responses must
    // never be cached by shared proxies. `/tmp/` is the SPA shell; `/v1/tmp/`
    // is the API surface (including inline and error responses).
    let path = request.uri().path();
    let is_tmp_path = path.starts_with("/v1/tmp/") || path.starts_with("/tmp/");

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY);
    if is_tmp_path {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// Installs the `/v1/admin/*` namespace, gated behind the compile-time
/// `admin-api` feature. The default (public tmp-documents) build omits every
/// admin route entirely; the whole namespace is unreachable and undocumented.
/// New admin routes belong here so a single feature gates the group.
fn install_admin_routes(router: Router<AppState>) -> Router<AppState> {
    #[cfg(feature = "admin-api")]
    let router = router.route("/v1/admin/gc", post(system_handlers::admin_gc));
    router
}

fn install_collab_routes(router: Router<AppState>) -> Router<AppState> {
    // The raw `/v1/collab/{document_id}` route takes an internal id and no
    // secret, so it serves only library documents. Tmp documents use the
    // secret-authenticated `/v1/tmp/collab/{secret}/{room}` route instead, and
    // this route is omitted entirely from the tmp-only build.
    if !cfg!(feature = "lib-documents") {
        return router;
    }

    router.route(
        "/v1/collab/{document_id}",
        get(collab_handlers::collab_websocket),
    )
}

fn install_tmp_document_routes(router: Router<AppState>) -> Router<AppState> {
    if !cfg!(feature = "tmp-documents") {
        return router;
    }

    let tmp_document_route = get(tmp_document_handlers::get_tmp_document)
        .head(tmp_document_handlers::head_tmp_document)
        .post(tmp_document_handlers::post_tmp_document_action)
        .put(tmp_document_handlers::put_tmp_document)
        .patch(tmp_document_handlers::patch_tmp_document_action)
        .delete(tmp_document_handlers::delete_tmp_document)
        .layer(DefaultBodyLimit::max(TMP_DOCUMENT_HTTP_BODY_LIMIT));

    router
        .route(
            "/v1/tmp/documents",
            post(tmp_document_handlers::create_tmp_document)
                .layer(DefaultBodyLimit::max(TMP_DOCUMENT_HTTP_BODY_LIMIT)),
        )
        .route(
            "/v1/tmp/collab/{secret}/{room}",
            get(collab_handlers::tmp_collab_websocket),
        )
        .route("/v1/tmp/documents/{*path}", tmp_document_route)
}

fn install_library_document_routes(router: Router<AppState>) -> Router<AppState> {
    if !cfg!(feature = "lib-documents") {
        return router;
    }

    let router = router
        .route("/v1/events", get(events))
        .route(
            "/v1/libraries",
            get(library_handlers::list_libraries).post(library_handlers::create_library),
        )
        .route(
            "/v1/libraries/{library}",
            get(library_handlers::get_library),
        )
        .route(
            "/v1/libraries/{library}/documents",
            get(document_handlers::list_documents),
        )
        .route(
            "/v1/libraries/{library}/search",
            get(search_handlers::search_documents),
        )
        .route(
            "/v1/libraries/{library}/search/suggest",
            get(search_handlers::suggest_documents),
        )
        .route(
            "/v1/libraries/{library}/reindex",
            post(search_handlers::reindex_library),
        )
        .route("/v1/libraries/{library}/graph", get(search_handlers::graph))
        .route(
            "/v1/libraries/{library}/events/pending",
            get(agent_events::agent_events_pending),
        )
        .route(
            "/v1/libraries/{library}/events/ack",
            post(agent_events::agent_events_ack),
        )
        .route(
            "/v1/libraries/{library}/documents/{*path}",
            get(document_handlers::get_document)
                .head(document_handlers::head_document)
                .put(document_handlers::put_document)
                .post(document_handlers::post_document_action)
                .patch(document_handlers::patch_document_metadata)
                .delete(document_handlers::delete_document),
        )
        .route(
            "/v1/libraries/{library}/transactions",
            post(transaction_handlers::begin_transaction),
        )
        .route(
            "/v1/libraries/{library}/transactions/{tx}/documents/{*path}",
            put(transaction_handlers::stage_put_document)
                .post(transaction_handlers::post_transaction_document_action)
                .patch(transaction_handlers::patch_transaction_document_metadata)
                .delete(transaction_handlers::stage_delete_document),
        )
        .route(
            "/v1/libraries/{library}/transactions/{tx}/commit",
            post(transaction_handlers::commit_transaction),
        )
        .route(
            "/v1/libraries/{library}/transactions/{tx}/rollback",
            post(transaction_handlers::rollback_transaction),
        );
    let router = install_git_routes(router);
    router
        .route(
            "/v1/libraries/{library}/conflicts",
            get(conflicts::list_conflicts),
        )
        .route(
            "/v1/libraries/{library}/conflicts/{conflict}",
            get(conflicts::get_conflict),
        )
        .route(
            "/v1/libraries/{library}/conflicts/{conflict}/resolve",
            post(conflicts::resolve_conflict),
        )
}

/// Installs the `/v1/libraries/{library}/git/*` routes. Git peering pulls in
/// `quarry-git` (and libgit2), so the whole group compiles only under the
/// `lib-documents` feature and is absent from the tmp-only build's binary.
fn install_git_routes(router: Router<AppState>) -> Router<AppState> {
    #[cfg(feature = "lib-documents")]
    let router = router
        .route(
            "/v1/libraries/{library}/git/peers",
            get(git_handlers::list_git_peers).post(git_handlers::create_git_peer),
        )
        .route(
            "/v1/libraries/{library}/git/import",
            post(git_handlers::git_import),
        )
        .route(
            "/v1/libraries/{library}/git/export",
            post(git_handlers::git_export),
        )
        .route(
            "/v1/libraries/{library}/git/peers/{peer}/pull",
            post(git_handlers::git_pull),
        )
        .route(
            "/v1/libraries/{library}/git/peers/{peer}/push",
            post(git_handlers::git_push),
        )
        .route(
            "/v1/libraries/{library}/git/peers/{peer}/sync",
            post(git_handlers::git_sync),
        );
    router
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
    serve_with_config(store, addr, ServerConfig::default()).await
}

pub async fn serve_with_config(
    store: QuarryStore,
    addr: SocketAddr,
    config: ServerConfig,
) -> std::io::Result<()> {
    serve_with_config_and_shutdown(store, addr, config, shutdown_signal()).await
}

pub async fn serve_with_shutdown<F>(
    store: QuarryStore,
    addr: SocketAddr,
    shutdown: F,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_config_and_shutdown(store, addr, ServerConfig::default(), shutdown).await
}

pub async fn serve_with_config_and_shutdown<F>(
    store: QuarryStore,
    addr: SocketAddr,
    config: ServerConfig,
    shutdown: F,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let state = app_state_with_config(store, config);
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
    let mut shutdown = Box::pin(shutdown);
    // Poll shutdown once before the listener can become reachable. In
    // particular, this installs the process signal handlers before another
    // process can observe readiness and send SIGTERM.
    let listener = tokio::select! {
        biased;
        () = shutdown.as_mut() => return Ok(()),
        result = tokio::net::TcpListener::bind(addr) => result?,
    };
    let (shutdown_requested_tx, shutdown_requested_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_task = tokio::spawn(async move {
        shutdown.await;
        let _ = shutdown_requested_tx.send(());
    });
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
                ?error,
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
                        ?error,
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
        system_handlers::health,
        system_handlers::capabilities,
        system_handlers::openapi_json,
        collab_handlers::collab_websocket_openapi,
        sse::events,
        library_handlers::create_library,
        library_handlers::list_libraries,
        library_handlers::get_library,
        document_handlers::list_documents,
        tmp_document_handlers::create_tmp_document,
        collab_handlers::tmp_collab_websocket_openapi,
        tmp_document_handlers::get_tmp_document,
        tmp_document_handlers::head_tmp_document,
        tmp_document_handlers::put_tmp_document,
        tmp_document_handlers::delete_tmp_document,
        tmp_document_handlers::tmp_document_versions_openapi,
        tmp_document_handlers::tmp_document_versions_raw_openapi,
        tmp_document_handlers::tmp_document_version_openapi,
        tmp_document_handlers::tmp_document_version_diff_openapi,
        tmp_document_handlers::tmp_document_version_restore_openapi,
        tmp_document_handlers::tmp_document_ttl_openapi,
        tmp_document_handlers::tmp_document_promote_openapi,
        tmp_document_handlers::tmp_document_fork_openapi,
        review::tmp_document_review_openapi,
        tmp_document_handlers::tmp_document_blocks_openapi,
        tmp_document_handlers::tmp_document_block_transactions_openapi,
        tmp_document_handlers::tmp_document_events_stream_openapi,
        tmp_document_handlers::tmp_agent_presence_list_openapi,
        tmp_document_handlers::tmp_agent_presence_openapi,
        tmp_document_handlers::tmp_document_agent_prompt_openapi,
        search_handlers::search_documents,
        search_handlers::suggest_documents,
        search_handlers::reindex_library,
        search_handlers::graph,
        document_handlers::get_document,
        document_handlers::document_backlinks_openapi,
        document_handlers::document_outgoing_links_openapi,
        document_handlers::document_snapshot_openapi,
        review::document_review_openapi,
        document_handlers::document_blocks_openapi,
        document_handlers::document_block_transactions_openapi,
        document_handlers::document_events_stream_openapi,
        document_handlers::document_share_openapi,
        document_handlers::document_share_create_openapi,
        document_handlers::document_share_revoke_openapi,
        document_handlers::agent_presence_list_openapi,
        document_handlers::agent_presence_openapi,
        document_handlers::document_agent_prompt_openapi,
        agent_events::agent_events_pending,
        agent_events::agent_events_ack,
        document_handlers::document_versions_openapi,
        document_handlers::document_versions_raw_openapi,
        document_handlers::document_version_openapi,
        document_handlers::document_version_diff_openapi,
        document_handlers::document_version_restore_openapi,
        document_handlers::document_ttl_openapi,
        document_handlers::head_document,
        document_handlers::put_document,
        document_handlers::post_document_action,
        document_handlers::patch_document_metadata,
        document_handlers::delete_document,
        transaction_handlers::begin_transaction,
        transaction_handlers::stage_put_document,
        transaction_handlers::post_transaction_document_action,
        transaction_handlers::patch_transaction_document_metadata,
        transaction_handlers::stage_delete_document,
        transaction_handlers::commit_transaction,
        transaction_handlers::rollback_transaction,
        conflicts::list_conflicts,
        conflicts::get_conflict,
        conflicts::resolve_conflict
    ),
    components(schemas(
        CreateLibraryRequest,
        system_handlers::Capabilities,
        BeginTransactionRequest,
        ApiErrorResponse,
        MoveRequest,
        DryRunValue,
        Library,
        DocumentListEntry,
        DocumentHistoryEntry,
        DocumentVersion,
        DocumentVersionContent,
        WriteOutcome,
        markdown_write::PutDocumentOutcome,
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
        VersionDiff
    ))
)]
struct ApiDoc;

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

#[derive(Debug, Deserialize, ToSchema)]
pub struct MoveRequest {
    pub to_path: String,
}

pub(crate) async fn agent_presence_tmp_document(
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
    use crate::sse::{StoreEventPayloadMode, store_event_payload, store_event_type};
    use axum::body::{Body, to_bytes};
    #[cfg(any(feature = "lib-documents", feature = "tmp-documents"))]
    use axum::http::Method;
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    use quarry_core::DocumentSource;
    #[cfg(feature = "lib-documents")]
    use quarry_core::WritePrecondition;
    use quarry_storage::StoreConfig;
    use quarry_storage::StoreEvent;
    #[cfg(feature = "lib-documents")]
    use quarry_storage::TransactionMetadata;
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

    #[tokio::test]
    async fn busy_errors_map_to_service_unavailable_with_retry_after() {
        let response =
            ApiError::from(QuarryError::Busy("database locked".to_string())).into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["code"], "SERVICE_BUSY");
        assert_eq!(body["retryable"], true);
        assert_eq!(body["message"], "service temporarily unavailable");
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
            parse_document_subresource("notes/daily.md/agent-prompt"),
            ("notes/daily.md", DocumentSubResource::AgentPrompt)
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
            parse_tmp_document_subresource("tmp-secret/agent-prompt"),
            ("tmp-secret", TmpDocumentSubResource::AgentPrompt)
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
            parse_tmp_document_subresource("tmp-secret/fork"),
            ("tmp-secret", TmpDocumentSubResource::Fork)
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
            .put_document(quarry_storage::PutDocumentRequest {
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
            output.contains("scope=tmp") && output.contains(document_id.as_str()),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentSubResource<'path> {
    Document,
    Backlinks,
    OutgoingLinks,
    Blocks,
    Snapshot,
    Review,
    Presence,
    EventsStream,
    AgentPrompt,
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
pub(crate) enum TmpDocumentSubResource<'path> {
    Document,
    AgentPrompt,
    Blocks,
    Review,
    Presence,
    EventsStream,
    RawVersions,
    Versions,
    Version(&'path str),
    VersionDiff(&'path str),
    VersionRestore(&'path str),
    Ttl,
    Transactions,
    Promote,
    Fork,
}

pub(crate) fn parse_document_subresource(path: &str) -> (&str, DocumentSubResource<'_>) {
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
        ("/agent-prompt", DocumentSubResource::AgentPrompt),
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

pub(crate) fn parse_tmp_document_subresource(path: &str) -> (&str, TmpDocumentSubResource<'_>) {
    if let Some((path, version)) = document_version_path_with_suffix(path, "/diff") {
        return (path, TmpDocumentSubResource::VersionDiff(version));
    }
    if let Some((path, version)) = document_version_path_with_suffix(path, "/restore") {
        return (path, TmpDocumentSubResource::VersionRestore(version));
    }
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
        ("/agent-prompt", TmpDocumentSubResource::AgentPrompt),
        ("/transactions", TmpDocumentSubResource::Transactions),
        ("/presence", TmpDocumentSubResource::Presence),
        ("/promote", TmpDocumentSubResource::Promote),
        ("/fork", TmpDocumentSubResource::Fork),
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
