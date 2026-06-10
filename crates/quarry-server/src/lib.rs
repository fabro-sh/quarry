mod collab;
mod gateway;
mod markdown_write;
mod session;

#[cfg(feature = "bundle_ui")]
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{MatchedPath, Path, Query, Request, State};
#[cfg(feature = "bundle_ui")]
use axum::http::Uri;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::{stream, Stream};
use quarry_collab_codec::{
    review_markers, review_meta_with_inline_comment_bodies, ReviewMeta, ReviewMetaEntry,
    ReviewSuggestionKind as CodecReviewSuggestionKind,
};
use quarry_core::{
    now_timestamp, CollabInviteToken, ConflictRecord, DocumentHistoryEntry, DocumentLink,
    DocumentListEntry, DocumentSource, DocumentVersion, DocumentVersionContent, GcReport, GitPeer,
    GraphEdge, GraphNode, GraphResponse, Library, LinkCollection, QuarryError, ReindexReport,
    SearchResponse, SearchResult, SearchSuggestion, TransactionRecord, VersionDiff, WriteOutcome,
    WritePrecondition,
};
use quarry_git::{
    export_worktree, import_worktree, pull_peer, push_peer, sync_peer, GitExportOptions,
    GitExportResult, GitImportResult, GitSyncResult,
};
use quarry_storage::{QuarryStore, StoreEvent, StoreEventKind, TransactionMetadata};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::future::{Future, IntoFuture};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    store: QuarryStore,
    sessions: session::SessionHub,
    agent_events: AgentEventJournal,
    agent_presence: AgentPresenceRegistry,
}

const AGENT_EVENT_JOURNAL_CAPACITY: usize = 4096;
const REQUEST_ID_HEADER: &str = "x-quarry-request-id";

#[derive(Clone, Default)]
struct AgentEventJournal {
    inner: Arc<Mutex<AgentEventJournalInner>>,
    acks: Arc<Mutex<HashMap<String, u64>>>,
}

#[derive(Default)]
struct AgentEventJournalInner {
    next_id: u64,
    events: VecDeque<LoggedStoreEvent>,
}

#[derive(Clone)]
struct LoggedStoreEvent {
    id: u64,
    event: StoreEvent,
}

impl AgentEventJournal {
    fn spawn_ingest(&self, store: QuarryStore) {
        let journal = self.clone();
        let mut receiver = store.subscribe_events();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => journal.push(event).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            event = "sse.stream.lagged",
                            stream = "agent_event_journal",
                            skipped,
                            "agent event journal lagged"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    async fn push(&self, event: StoreEvent) {
        let mut inner = self.inner.lock().await;
        inner.next_id = inner.next_id.saturating_add(1);
        let id = inner.next_id;
        inner.events.push_back(LoggedStoreEvent { id, event });
        while inner.events.len() > AGENT_EVENT_JOURNAL_CAPACITY {
            inner.events.pop_front();
        }
    }

    async fn pending_since(
        &self,
        library_id: &str,
        after: u64,
        limit: usize,
    ) -> Vec<LoggedStoreEvent> {
        let inner = self.inner.lock().await;
        inner
            .events
            .iter()
            .filter(|event| event.id > after && event.event.library_id == library_id)
            .take(limit)
            .cloned()
            .collect()
    }

    async fn ack(&self, agent_id: String, event_id: u64) {
        let mut acks = self.acks.lock().await;
        let ack = acks.entry(agent_id).or_insert(0);
        *ack = (*ack).max(event_id);
    }
}

#[derive(Clone, Default)]
struct AgentPresenceRegistry {
    entries: Arc<Mutex<HashMap<String, AgentPresenceEntry>>>,
}

impl AgentPresenceRegistry {
    async fn update(
        &self,
        library: &str,
        path: &str,
        document_id: &str,
        agent_id: String,
        status: String,
        by: Option<String>,
    ) -> AgentPresenceResponse {
        let entry = AgentPresenceEntry {
            library: library.to_string(),
            path: path.to_string(),
            document_id: document_id.to_string(),
            agent_id,
            status,
            by,
            updated_at: now_timestamp(),
        };
        let key = format!("{}\0{}\0{}", entry.library, entry.path, entry.agent_id);
        let mut entries = self.entries.lock().await;
        entries.insert(key, entry.clone());
        let presence = entries
            .values()
            .filter(|other| other.library == library && other.path == path)
            .cloned()
            .collect();
        AgentPresenceResponse {
            current: entry,
            presence,
        }
    }

    async fn list(&self, library: &str, path: &str) -> AgentPresenceListResponse {
        let entries = self.entries.lock().await;
        let presence = entries
            .values()
            .filter(|entry| entry.library == library && entry.path == path)
            .cloned()
            .collect();
        AgentPresenceListResponse { presence }
    }
}

/// Builds the server state for `store`. Pair with
/// [`install_markdown_writer`] so same-process Git/FUSE/CLI writes route
/// through the gateway and the session mode switch (one owning process per
/// database; out-of-process writers cannot open the store at all).
pub fn app_state(store: QuarryStore) -> AppState {
    let agent_events = AgentEventJournal::default();
    agent_events.spawn_ingest(store.clone());
    let sessions = session::SessionHub::new(store.clone());
    AppState {
        store,
        sessions,
        agent_events,
        agent_presence: AgentPresenceRegistry::default(),
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
        );

    #[cfg(feature = "bundle_ui")]
    let router = router.fallback(get(browser_asset));

    let router = router.layer(middleware::from_fn(request_tracing_middleware));

    router.with_state(state)
}

async fn request_tracing_middleware(request: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let request_id_header = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok().filter(|value| !value.trim().is_empty()))
        .and_then(|value| HeaderValue::from_str(value).ok())
        .unwrap_or_else(|| HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap());
    let request_id = request_id_header.to_str().unwrap_or_default().to_string();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
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
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        event = "server.listening",
        %addr,
        "quarry REST server listening"
    );
    let (shutdown_started_tx, shutdown_started_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, router_with_state(state))
        .with_graceful_shutdown(async move {
            shutdown.await;
            let _ = shutdown_started_tx.send(());
        })
        .into_future();
    tokio::pin!(server);
    tokio::pin!(shutdown_started_rx);

    tokio::select! {
        result = &mut server => result,
        _ = &mut shutdown_started_rx => {
            match tokio::time::timeout(Duration::from_secs(10), &mut server).await {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        event = "shutdown.timeout",
                        timeout_ms = 10_000_u64,
                        "quarry REST server did not finish graceful shutdown within 10 seconds"
                    );
                    Ok(())
                }
            }
        }
    }
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
        document_snapshot_openapi,
        document_review_openapi,
        document_blocks_openapi,
        document_block_transactions_openapi,
        document_events_stream_openapi,
        document_share_openapi,
        document_share_create_openapi,
        document_share_revoke_openapi,
        document_review_process_openapi,
        agent_presence_list_openapi,
        agent_presence_openapi,
        agent_events_pending,
        agent_events_ack,
        document_ops_openapi,
        document_versions_openapi,
        document_versions_raw_openapi,
        document_version_openapi,
        document_version_diff_openapi,
        document_version_restore_openapi,
        head_document,
        put_document,
        post_document_action,
        document_edit_openapi,
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
        AgentReviewProcessRequest,
        AgentReviewProcessOperation,
        AgentReviewProcessResponse,
        AgentReviewProcessResultItem,
        AgentEditRequest,
        AgentEditResponse,
        AgentBlockOperation,
        AgentEditBlock,
        AgentEditOperation,
        AgentOpsOperation,
        AgentSuggestionKind,
        AgentPresenceStatus,
        AgentPresenceRequest,
        AgentPresenceResponse,
        AgentPresenceListResponse,
        AgentPresenceEntry,
        AgentPendingEventsResponse,
        AgentEventRecord,
        AgentEventsAckRequest,
        AgentEventsAckResponse,
        AgentOpsRequest,
        AgentOpsOperationRequest,
        AgentOpsResponse,
        AgentOpsResultItem,
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

const QUARRY_SKILL_MD: &str = include_str!("../resources/quarry.SKILL.md");
const AGENT_DOCS_MD: &str = include_str!("../resources/agent-docs.md");

#[derive(Debug, Serialize)]
struct AgentDiscovery {
    name: &'static str,
    api_base: String,
    docs_url: String,
    skill_url: String,
    openapi_url: String,
    capabilities: Vec<&'static str>,
    auth_note: &'static str,
    auth: AgentDiscoveryAuth,
    presence_statuses: Vec<&'static str>,
    edit_operations: Vec<&'static str>,
    ops_operations: Vec<&'static str>,
    limitations: Vec<&'static str>,
    route_hints: AgentDiscoveryRouteHints,
    endpoints: BTreeMap<&'static str, AgentDiscoveryEndpoint>,
}

#[derive(Debug, Serialize)]
struct AgentDiscoveryAuth {
    mode: &'static str,
    token_role: &'static str,
    required_headers: Vec<&'static str>,
    note: &'static str,
}

#[derive(Debug, Serialize)]
struct AgentDiscoveryRouteHints {
    presence: String,
    snapshot: String,
    review: String,
    review_process: String,
    events_stream: String,
    events_pending: String,
    edit: String,
    ops: String,
}

#[derive(Debug, Serialize)]
struct AgentDiscoveryEndpoint {
    method: &'static str,
    path: &'static str,
    url: String,
}

async fn quarry_skill() -> Response {
    static_text_response("text/markdown; charset=utf-8", QUARRY_SKILL_MD)
}

async fn agent_docs() -> Response {
    static_text_response("text/markdown; charset=utf-8", AGENT_DOCS_MD)
}

async fn agent_discovery(headers: HeaderMap) -> Result<Response, ApiError> {
    let origin = request_origin(&headers);
    let api_base = format!("{origin}/v1");
    let document_path = "/v1/libraries/{library}/documents/{path}";
    let mut endpoints = BTreeMap::new();
    endpoints.insert(
        "presence",
        discovery_endpoint(
            "POST",
            "/v1/libraries/{library}/documents/{path}/presence",
            &api_base,
        ),
    );
    endpoints.insert(
        "presence_list",
        discovery_endpoint(
            "GET",
            "/v1/libraries/{library}/documents/{path}/presence",
            &api_base,
        ),
    );
    endpoints.insert(
        "snapshot",
        discovery_endpoint(
            "GET",
            "/v1/libraries/{library}/documents/{path}/snapshot",
            &api_base,
        ),
    );
    endpoints.insert(
        "review",
        discovery_endpoint(
            "GET",
            "/v1/libraries/{library}/documents/{path}/review",
            &api_base,
        ),
    );
    endpoints.insert(
        "review_process",
        discovery_endpoint(
            "POST",
            "/v1/libraries/{library}/documents/{path}/review",
            &api_base,
        ),
    );
    endpoints.insert(
        "document",
        discovery_endpoint("GET", document_path, &api_base),
    );
    endpoints.insert(
        "events_stream",
        discovery_endpoint(
            "GET",
            "/v1/libraries/{library}/documents/{path}/events/stream",
            &api_base,
        ),
    );
    endpoints.insert(
        "events_pending",
        discovery_endpoint(
            "GET",
            "/v1/libraries/{library}/events/pending?after={last-seen-id}",
            &api_base,
        ),
    );
    endpoints.insert(
        "events_ack",
        discovery_endpoint("POST", "/v1/libraries/{library}/events/ack", &api_base),
    );
    endpoints.insert(
        "edit",
        discovery_endpoint(
            "POST",
            "/v1/libraries/{library}/documents/{path}/edit",
            &api_base,
        ),
    );
    endpoints.insert(
        "ops",
        discovery_endpoint(
            "POST",
            "/v1/libraries/{library}/documents/{path}/ops",
            &api_base,
        ),
    );
    endpoints.insert(
        "openapi",
        discovery_endpoint("GET", "/v1/openapi.json", &api_base),
    );
    endpoints.insert(
        "docs",
        AgentDiscoveryEndpoint {
            method: "GET",
            path: "/agent-docs",
            url: format!("{origin}/agent-docs"),
        },
    );
    endpoints.insert(
        "skill",
        AgentDiscoveryEndpoint {
            method: "GET",
            path: "/quarry.SKILL.md",
            url: format!("{origin}/quarry.SKILL.md"),
        },
    );
    json_response(
        StatusCode::OK,
        &AgentDiscovery {
            name: "quarry",
            api_base: api_base.clone(),
            docs_url: format!("{origin}/agent-docs"),
            skill_url: format!("{origin}/quarry.SKILL.md"),
            openapi_url: format!("{api_base}/openapi.json"),
            capabilities: vec![
                "presence",
                "snapshot",
                "review",
                "review_process",
                "events",
                "block_edit",
                "bulk_block_insert",
                "comments",
                "suggestions",
            ],
            auth_note:
                "Quarry REST agent APIs are trusted-localhost for now; URL tokens identify browser/collab joins and are not enforced as REST bearer auth.",
            auth: AgentDiscoveryAuth {
                mode: "trusted_localhost",
                token_role: "locator_only",
                required_headers: vec!["Content-Type", "X-Agent-Id"],
                note: "Invite URL tokens identify shared document joins; REST agent endpoints trust localhost for now.",
            },
            presence_statuses: vec![
                "reading",
                "thinking",
                "acting",
                "waiting",
                "completed",
                "error",
            ],
            edit_operations: vec![
                "replace_block",
                "insert_before",
                "insert_after",
                "delete_block",
                "replace_document",
            ],
            ops_operations: vec![
                "comment.add",
                "comment.reply",
                "comment.delete",
                "suggestion.add",
                "suggestion.accept",
                "suggestion.reject",
                "comment.resolve",
            ],
            limitations: vec![
                "REST agent endpoints trust localhost and do not currently enforce bearer-token auth.",
                "Invite URL tokens identify browser/collab joins and are not REST bearer tokens.",
                "Direct block edits operate on whole Markdown blocks.",
                "Quarry does not currently support rewrite.apply.",
            ],
            route_hints: AgentDiscoveryRouteHints {
                presence: format!("{api_base}/libraries/{{library}}/documents/{{path}}/presence"),
                snapshot: format!("{api_base}/libraries/{{library}}/documents/{{path}}/snapshot"),
                review: format!("{api_base}/libraries/{{library}}/documents/{{path}}/review"),
                review_process: format!(
                    "{api_base}/libraries/{{library}}/documents/{{path}}/review"
                ),
                events_stream: format!(
                    "{api_base}/libraries/{{library}}/documents/{{path}}/events/stream"
                ),
                events_pending: format!("{api_base}/libraries/{{library}}/events/pending?after={{last-seen-id}}"),
                edit: format!("{api_base}/libraries/{{library}}/documents/{{path}}/edit"),
                ops: format!("{api_base}/libraries/{{library}}/documents/{{path}}/ops"),
            },
            endpoints,
        },
    )
}

fn discovery_endpoint(
    method: &'static str,
    path: &'static str,
    api_base: &str,
) -> AgentDiscoveryEndpoint {
    AgentDiscoveryEndpoint {
        method,
        path,
        url: format!("{}{}", api_base.trim_end_matches("/v1"), path),
    }
}

fn request_origin(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1:7831");
    format!("{scheme}://{host}")
}

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
        state.sessions.serve_socket(document_id, socket).await;
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
    events_for_library(&state.store, &query.library, None).await
}

async fn events_for_library(
    store: &QuarryStore,
    library: &str,
    document_path: Option<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let library = store.get_library(library).await?;
    tracing::debug!(
        event = "sse.stream.opened",
        library = %library.slug,
        library_id = %library.id,
        path = document_path.as_deref().unwrap_or(""),
        "SSE stream opened"
    );
    let receiver = store.subscribe_events();
    let stream = stream::unfold(
        (receiver, library.id, library.slug, document_path),
        |(mut receiver, library_id, library_slug, document_path)| async move {
            loop {
                match receiver.recv().await {
                    Ok(store_event)
                        if store_event.library_id == library_id
                            && event_matches_document_filter(
                                &store_event,
                                document_path.as_deref(),
                            ) =>
                    {
                        let event_type = store_event_type(&store_event);
                        let payload = store_event_payload(&library_slug, &event_type, &store_event);
                        tracing::debug!(
                            event = "sse.event.sent",
                            library = %library_slug,
                            library_id = %library_id,
                            sse_event = %event_type,
                            path = store_event.path.as_deref().unwrap_or(""),
                            new_path = store_event.new_path.as_deref().unwrap_or(""),
                            tx_id = store_event.tx_id.as_deref().unwrap_or(""),
                            doc_id = store_event.doc_id.as_deref().unwrap_or(""),
                            version_id = store_event.version_id.as_deref().unwrap_or(""),
                            conflict_id = store_event.conflict_id.as_deref().unwrap_or(""),
                            origin_id = store_event.origin_id.as_deref().unwrap_or(""),
                            "SSE event sent"
                        );
                        let event = Event::default().event(event_type).data(payload.to_string());
                        return Some((
                            Ok(event),
                            (receiver, library_id, library_slug, document_path),
                        ));
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            event = "sse.stream.lagged",
                            library = %library_slug,
                            library_id = %library_id,
                            skipped,
                            "SSE stream lagged"
                        );
                        let event_type = "stream.lagged".to_string();
                        let payload = serde_json::json!({
                            "type": event_type,
                            "library": library_slug,
                            "skipped": skipped
                        });
                        let event = Event::default().event(event_type).data(payload.to_string());
                        return Some((
                            Ok(event),
                            (receiver, library_id, library_slug, document_path),
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::debug!(
                            event = "sse.stream.closed",
                            library = %library_slug,
                            library_id = %library_id,
                            "SSE stream closed"
                        );
                        return None;
                    }
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

fn event_matches_document_filter(event: &StoreEvent, document_path: Option<&str>) -> bool {
    let Some(document_path) = document_path else {
        return true;
    };
    event.path.as_deref() == Some(document_path) || event.new_path.as_deref() == Some(document_path)
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
            let mut data = store_event_payload(&library.slug, &event_type, &logged.event);
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
struct EventsQuery {
    library: String,
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
    pub library: String,
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
#[serde(deny_unknown_fields)]
pub struct AgentReviewProcessOperation {
    pub op: String,
    #[serde(default, rename = "ref")]
    pub block_ref: Option<AgentBlockRef>,
    #[serde(default)]
    pub block: Option<AgentEditBlock>,
    #[serde(default)]
    pub blocks: Option<Vec<AgentEditBlock>>,
    #[serde(default)]
    pub markdown: Option<String>,
    #[serde(default)]
    pub quote: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default, rename = "parentId")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, alias = "suggestionType")]
    #[schema(value_type = Option<AgentSuggestionKind>)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentReviewProcessRequest {
    #[serde(rename = "baseToken")]
    pub base_token: String,
    #[serde(default)]
    pub by: Option<String>,
    pub operations: Vec<AgentReviewProcessOperation>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentReviewProcessResultItem {
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentReviewProcessResponse {
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(rename = "nextBaseToken", skip_serializing_if = "Option::is_none")]
    pub next_base_token: Option<String>,
    pub results: Vec<AgentReviewProcessResultItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<WriteOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    pub review: AgentReviewResponse,
    /// Legacy field of the deleted injection gate; never emitted (the
    /// endpoint is quarantined and live sessions are the write path).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub injection: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentEditBlock {
    pub markdown: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentEditOperation {
    ReplaceBlock,
    InsertBefore,
    InsertAfter,
    DeleteBlock,
    ReplaceDocument,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentBlockOperation {
    #[schema(value_type = AgentEditOperation)]
    pub op: String,
    #[serde(default, rename = "ref")]
    pub block_ref: Option<AgentBlockRef>,
    #[serde(default)]
    pub block: Option<AgentEditBlock>,
    #[serde(default)]
    pub blocks: Option<Vec<AgentEditBlock>>,
    #[serde(default)]
    pub markdown: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentEditRequest {
    #[serde(rename = "baseToken")]
    pub base_token: String,
    pub operations: Vec<AgentBlockOperation>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentEditResponse {
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(rename = "nextBaseToken", skip_serializing_if = "Option::is_none")]
    pub next_base_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<WriteOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Legacy field of the deleted injection gate; never emitted (the
    /// endpoint is quarantined and live sessions are the write path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub injection: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub enum AgentOpsOperation {
    #[serde(rename = "comment.add")]
    CommentAdd,
    #[serde(rename = "comment.reply")]
    CommentReply,
    #[serde(rename = "comment.delete")]
    CommentDelete,
    #[serde(rename = "suggestion.add")]
    SuggestionAdd,
    #[serde(rename = "suggestion.accept")]
    SuggestionAccept,
    #[serde(rename = "suggestion.reject")]
    SuggestionReject,
    #[serde(rename = "comment.resolve")]
    CommentResolve,
    #[serde(rename = "accept")]
    Accept,
    #[serde(rename = "reject")]
    Reject,
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

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentOpsOperationRequest {
    #[schema(value_type = AgentOpsOperation)]
    pub op: String,
    #[serde(default, rename = "ref")]
    pub block_ref: Option<AgentBlockRef>,
    #[serde(default)]
    pub quote: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default, rename = "parentId")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, alias = "suggestionType")]
    #[schema(value_type = Option<AgentSuggestionKind>)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentOpsRequest {
    #[serde(rename = "baseToken")]
    pub base_token: String,
    #[serde(default)]
    pub by: Option<String>,
    pub operations: Vec<AgentOpsOperationRequest>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentOpsResultItem {
    #[schema(value_type = AgentOpsOperation)]
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentOpsResponse {
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(rename = "nextBaseToken", skip_serializing_if = "Option::is_none")]
    pub next_base_token: Option<String>,
    pub results: Vec<AgentOpsResultItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<WriteOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Legacy field of the deleted injection gate; never emitted (the
    /// endpoint is quarantined and live sessions are the write path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub injection: Option<String>,
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
    if let Some(path) = path.strip_suffix("/blocks") {
        return gateway::document_blocks(&state, &library, path).await;
    }
    if let Some(path) = path.strip_suffix("/snapshot") {
        return json_response(
            StatusCode::OK,
            &agent_document_snapshot(&state.store, &library, path).await?,
        );
    }
    if let Some(path) = path.strip_suffix("/review") {
        let include_resolved = query.review.include_resolved()?;
        return json_response(
            StatusCode::OK,
            &agent_document_review(&state.store, &library, path, include_resolved).await?,
        );
    }
    if let Some(path) = path.strip_suffix("/presence") {
        state.store.head_document(&library, path).await?;
        return json_response(
            StatusCode::OK,
            &state.agent_presence.list(&library, path).await,
        );
    }
    if let Some(path) = path.strip_suffix("/events/stream") {
        state.store.head_document(&library, path).await?;
        return Ok(
            events_for_library(&state.store, &library, Some(path.to_string()))
                .await?
                .into_response(),
        );
    }
    if let Some(path) = path.strip_suffix("/share") {
        return json_response(
            StatusCode::OK,
            &state.store.collab_invite_tokens(&library, path).await?,
        );
    }
    if let Some(path) = path.strip_suffix("/versions/raw") {
        return json_response(
            StatusCode::OK,
            &state.store.raw_version_history(&library, path).await?,
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
    path = "/v1/libraries/{library}/documents/{path}/snapshot",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = AgentDocumentSnapshot), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_snapshot_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/review",
    params(("library" = String, Path), ("path" = String, Path), ("includeResolved" = Option<DryRunValue>, Query)),
    responses((status = 200, body = AgentReviewResponse), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
async fn document_block_transactions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/events/stream",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, description = "Document-scoped server-sent event stream"), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_events_stream_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/share",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [CollabInviteToken]), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_share_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/share",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = CreateCollabInviteRequest,
    responses((status = 201, body = CollabInviteToken), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_share_create_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/share/{token}/revoke",
    params(("library" = String, Path), ("path" = String, Path), ("token" = String, Path)),
    responses((status = 200, body = CollabInviteToken), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_share_revoke_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [DocumentHistoryEntry]), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_versions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/raw",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [DocumentVersion]), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_versions_raw_openapi() {}

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
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    let transaction = transaction_metadata_from_headers(&headers)?;
    let browser_origin = origin_id
        .as_deref()
        .is_some_and(|origin| origin.starts_with("browser:"));

    // Transitional rule (Phase 3, dissolves with the Phase 5 browser): a
    // Markdown PUT from a live-session participant is a checkpoint trigger,
    // not an independent write. The session doc is authoritative and already
    // contains those edits (the autosave body is the flusher's serialization
    // of the same doc), so checkpoint and ack with the new head ETag. The
    // PUT body and its If-Match are deliberately ignored — checkpoints move
    // the head, so the flusher's clock is routinely stale.
    if browser_origin
        && quarry_storage::document_kind(&path, &content_type)
            == quarry_storage::DocumentKind::BlockDocument
    {
        if let Ok(document) = state.store.head_document(&library, &path).await {
            if let Some(outcome) = state.sessions.checkpoint_for_autosave(&document.id).await? {
                tracing::debug!(
                    event = "collab.session.autosave_checkpoint",
                    document_id = %document.id,
                    path = %path,
                    version_id = %outcome.version.id,
                    "browser autosave acknowledged via session checkpoint"
                );
                return json_with_etag(StatusCode::OK, &outcome, &outcome.version.id);
            }
        }
    }

    // Phase 4: a BlockDocument PUT is a whole-file write reconciled via
    // diff3 against the canonical block rows — block ids and review anchors
    // survive, true conflicts become review items, and a live session
    // receives the merge as a collaborator edit. RawDocuments keep the
    // untouched legacy byte path below.
    if quarry_storage::document_kind(&path, &content_type)
        == quarry_storage::DocumentKind::BlockDocument
    {
        return gateway::gateway_reply(
            markdown_write::put_block_document(
                &state,
                &library,
                &path,
                body.to_vec(),
                metadata,
                precondition,
                origin_id,
                transaction,
            )
            .await,
        );
    }

    let outcome = state
        .store
        .put_document_with_transaction(
            &library,
            &path,
            body.to_vec(),
            metadata,
            &content_type,
            DocumentSource::Rest,
            precondition,
            origin_id,
            transaction,
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
    // Phase 4: a metadata patch on a BlockDocument must NOT destroy the
    // block projection (the legacy path re-puts the content, which clears
    // rows and review items fail-closed, and bypasses the session mutex).
    // It routes through the gateway as a zero-op transaction with a
    // metadata override instead — see `markdown_write::patch_block_document_metadata`.
    if let Ok(head) = state.store.head_document(&library, path).await {
        if quarry_storage::document_kind(path, &head.content_type)
            == quarry_storage::DocumentKind::BlockDocument
        {
            return gateway::gateway_reply(
                markdown_write::patch_block_document_metadata(
                    &state,
                    &library,
                    path,
                    patch,
                    precondition_from_headers(&headers)?,
                )
                .await,
            );
        }
    }
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
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
    Json(request): Json<JsonValue>,
) -> Result<Response, ApiError> {
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    if let Some((path, version)) = document_version_restore_path(&path) {
        let outcome = state
            .store
            .restore_document_version_with_origin(&library, path, version, origin_id.clone())
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
            .move_document_with_origin(
                &library,
                from_path,
                to_path,
                DocumentSource::Rest,
                origin_id.clone(),
            )
            .await?;
        return json_response(StatusCode::OK, &transaction);
    }

    if let Some(path) = path.strip_suffix("/share") {
        let request: CreateCollabInviteRequest = serde_json::from_value(request)
            .map_err(|error| QuarryError::InvalidPath(format!("invalid share request: {error}")))?;
        let token = state
            .store
            .create_collab_invite_token(&library, path, &request.role, request.by_hint)
            .await?;
        return json_response(StatusCode::CREATED, &token);
    }

    // Phase 3 quarantine (with `/edit` and `/ops` below): the review-process
    // facade forwarded the whole legacy mutation vocabulary; edits and
    // review ops both live on `POST .../transactions` now. GET `/review`
    // (the read projection) is unaffected.
    if let Some(path) = path.strip_suffix("/review") {
        return Ok(gateway::legacy_endpoint_quarantined(
            &library, path, "/review",
        ));
    }

    if let Some((_, token_id)) = collab_invite_revoke_path(&path) {
        let token = state.store.revoke_collab_invite_token(token_id).await?;
        return json_response(StatusCode::OK, &token);
    }

    // Phase 3 quarantine: the legacy `/edit` and `/ops` mutation facades are
    // gone for block documents (every Markdown document). They rode the
    // deleted injection gate; `POST .../transactions` is the single mutation
    // contract. Full route deletion is Phase 7.
    if let Some(path) = path.strip_suffix("/edit") {
        return Ok(gateway::legacy_endpoint_quarantined(
            &library, path, "/edit",
        ));
    }

    if let Some(path) = path.strip_suffix("/ops") {
        return Ok(gateway::legacy_endpoint_quarantined(&library, path, "/ops"));
    }

    if let Some(path) = path.strip_suffix("/presence") {
        let request: AgentPresenceRequest = serde_json::from_value(request).map_err(|error| {
            QuarryError::InvalidPath(format!("invalid presence request: {error}"))
        })?;
        let response = agent_presence_document(&state, &headers, &library, path, request).await?;
        return json_response(StatusCode::OK, &response);
    }

    if let Some(path) = path.strip_suffix("/transactions") {
        return gateway::document_block_transactions(&state, &library, path, request).await;
    }

    Err(QuarryError::NotFound(path).into())
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/edit",
    description = "Quarantined legacy endpoint: always responds 410 with the \
        typed code `UNSUPPORTED_LEGACY_ENDPOINT`. Use \
        `POST /v1/libraries/{library}/documents/{path}/transactions` for \
        block edits. The route is deleted entirely in a later phase.",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = AgentEditRequest,
    responses((status = 410, body = gateway::BlockTransactionError))
)]
#[allow(dead_code)]
async fn document_edit_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/presence",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = AgentPresenceListResponse), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn agent_presence_list_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/presence",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = AgentPresenceRequest,
    responses((status = 200, body = AgentPresenceResponse), (status = 404, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn agent_presence_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/ops",
    description = "Quarantined legacy endpoint: always responds 410 with the \
        typed code `UNSUPPORTED_LEGACY_ENDPOINT`. Use \
        `POST /v1/libraries/{library}/documents/{path}/transactions` for \
        review and block operations. The route is deleted entirely in a \
        later phase.",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = AgentOpsRequest,
    responses((status = 410, body = gateway::BlockTransactionError))
)]
#[allow(dead_code)]
async fn document_ops_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/review",
    description = "Quarantined legacy endpoint: always responds 410 with the \
        typed code `UNSUPPORTED_LEGACY_ENDPOINT`. Use \
        `POST /v1/libraries/{library}/documents/{path}/transactions` for \
        edits and review operations; `GET .../review` (the read projection) \
        is unaffected. The route is deleted entirely in a later phase.",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = AgentReviewProcessRequest,
    responses((status = 410, body = gateway::BlockTransactionError))
)]
#[allow(dead_code)]
async fn document_review_process_openapi() {}

/// Legacy edit planning kept only for the review-process facade; the ops
/// bookkeeping fed the deleted injection gate. Phase 7 deletes it wholesale.
#[allow(dead_code)]
#[derive(Clone, Debug)]
struct AgentEditPlan {
    markdown: String,
    blocks: Vec<String>,
    ops: Vec<PlannedAgentEditOp>,
    original_blocks: Vec<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
enum PlannedAgentEditOp {
    ReplaceDocument,
    ReplaceBlock {
        ordinal: usize,
        markdown: String,
    },
    InsertBefore {
        ordinal: usize,
        markdown_blocks: Vec<String>,
    },
    InsertAfter {
        ordinal: usize,
        markdown_blocks: Vec<String>,
    },
    DeleteBlock {
        ordinal: usize,
    },
}

// `ReviewMeta` / `ReviewMetaEntry` and the endmatter readers now live in
// `quarry_collab_codec::review` (single-sourced with the slate conversion that
// needs them); imported at the top of this module.

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
    Ok(state
        .agent_presence
        .update(
            library,
            path,
            &document.id,
            agent_id,
            status,
            request.by.filter(|by| !by.trim().is_empty()),
        )
        .await)
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
    let blocks = snapshot_blocks(&markdown);
    let (_, meta) = review_meta_with_inline_comment_bodies(&markdown);
    let markers = agent_review_markers(&blocks);
    let comments = agent_review_comments(&markers.comments, &meta, include_resolved);
    let suggestions = agent_review_suggestions(&markers.suggestions, &meta);
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
                body: entry.body.clone().unwrap_or_default(),
            });
    }
    replies
}

fn agent_review_suggestions(
    markers: &[ReviewSuggestionMarker],
    meta: &ReviewMeta,
) -> Vec<AgentReviewSuggestion> {
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

        if outside_fence && !blank_outside_fence {
            if let Some(boundary) = pending_boundary.take() {
                if block_start < boundary {
                    blocks.push(markdown[block_start..boundary].to_string());
                    block_start = boundary;
                }
            }
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
        if outside_fence && !line.trim().is_empty() {
            if let Some(boundary) = pending_boundary.take() {
                if block_start < boundary {
                    blocks.push(markdown[block_start..boundary].to_string());
                    block_start = boundary;
                }
            }
        }
        if outside_fence && line.trim().is_empty() {
            pending_boundary = Some(markdown.len());
        }
    }

    if let Some(boundary) = pending_boundary {
        if boundary > block_start && boundary == markdown.len() {
            blocks.push(markdown[block_start..boundary].to_string());
            return blocks;
        }
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
    Ok(Json(
        state
            .store
            .delete_document_with_origin(&library, &path, DocumentSource::Rest, origin_id)
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
            QuarryError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            QuarryError::Conflict(_) => StatusCode::CONFLICT,
            QuarryError::Busy(_) => StatusCode::SERVICE_UNAVAILABLE,
            QuarryError::InvalidPath(_) => StatusCode::BAD_REQUEST,
            QuarryError::InvalidInput(_) => StatusCode::BAD_REQUEST,
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
        StatusCode::PRECONDITION_FAILED => "precondition_failed",
        StatusCode::CONFLICT => "conflict",
        StatusCode::SERVICE_UNAVAILABLE => "busy",
        StatusCode::BAD_REQUEST => "bad_request",
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
            origin_id: Some("browser:session-1".to_string()),
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
        assert_eq!(payload["origin_id"], "browser:session-1");
    }

    #[test]
    fn document_delete_and_move_store_events_map_to_sse_payloads_with_origin() {
        let delete = StoreEvent {
            kind: StoreEventKind::DocumentDelete,
            library_id: "library-id".to_string(),
            path: Some("notes/daily.md".to_string()),
            new_path: None,
            source: Some(DocumentSource::Rest),
            tx_id: Some("tx-1".to_string()),
            doc_id: Some("doc-1".to_string()),
            version_id: None,
            conflict_id: None,
            peer_id: None,
            applied: None,
            conflicts: None,
            origin_id: Some("browser:session-1".to_string()),
        };
        let event_type = store_event_type(&delete);
        let payload = store_event_payload("notes", &event_type, &delete);
        assert_eq!(event_type, "doc.deleted");
        assert_eq!(payload["doc_id"], "doc-1");
        assert_eq!(payload["origin_id"], "browser:session-1");

        let move_event = StoreEvent {
            kind: StoreEventKind::DocumentMove,
            library_id: "library-id".to_string(),
            path: Some("notes/daily.md".to_string()),
            new_path: Some("notes/archive.md".to_string()),
            source: Some(DocumentSource::Rest),
            tx_id: Some("tx-2".to_string()),
            doc_id: Some("doc-1".to_string()),
            version_id: None,
            conflict_id: None,
            peer_id: None,
            applied: None,
            conflicts: None,
            origin_id: Some("browser:session-1".to_string()),
        };
        let event_type = store_event_type(&move_event);
        let payload = store_event_payload("notes", &event_type, &move_event);
        assert_eq!(event_type, "doc.moved");
        assert_eq!(payload["from"], "notes/daily.md");
        assert_eq!(payload["to"], "notes/archive.md");
        assert_eq!(payload["doc_id"], "doc-1");
        assert_eq!(payload["origin_id"], "browser:session-1");
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
            origin_id: None,
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
            origin_id: None,
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
            origin_id: None,
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
            origin_id: None,
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

fn transaction_metadata_from_headers(headers: &HeaderMap) -> Result<TransactionMetadata, ApiError> {
    let mut metadata = TransactionMetadata {
        actor: optional_header(headers, "x-quarry-transaction-actor")?,
        message: optional_header(headers, "x-quarry-transaction-message")?,
        ..TransactionMetadata::default()
    };
    if let Some(value) = headers.get("x-quarry-transaction-provenance") {
        metadata.provenance = serde_json::from_str(value.to_str().map_err(|_| {
            QuarryError::InvalidPath("invalid x-quarry-transaction-provenance".to_string())
        })?)
        .map_err(|_| {
            QuarryError::InvalidPath("invalid x-quarry-transaction-provenance".to_string())
        })?;
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

fn collab_invite_revoke_path(path: &str) -> Option<(&str, &str)> {
    let path = path.strip_suffix("/revoke")?;
    let (document_path, token_id) = path.rsplit_once("/share/")?;
    if document_path.is_empty() || token_id.is_empty() || token_id.contains('/') {
        return None;
    }
    Some((document_path, token_id))
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
        if let Some(origin_id) = &event.origin_id {
            object.insert(
                "origin_id".to_string(),
                JsonValue::String(origin_id.clone()),
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

fn static_text_response(content_type: &'static str, body: &'static str) -> Response {
    let mut response = Response::new(axum::body::Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
