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
use pulldown_cmark::{
    Event as MarkdownEvent, Options as MarkdownOptions, Parser as MarkdownParser, Tag,
};
use quarry_collab_codec::{
    build_nodes, review_block_to_slate, review_blocks_to_slate, split_review_endmatter, ReviewMeta,
    ReviewMetaEntry, Unsupported,
};
use quarry_core::{
    now_timestamp, CollabInviteToken, ConflictRecord, DocumentLink, DocumentListEntry,
    DocumentSource, DocumentVersion, DocumentVersionContent, GcReport, GitPeer, GraphEdge,
    GraphNode, GraphResponse, Library, LinkCollection, QuarryError, ReindexReport, SearchResponse,
    SearchResult, SearchSuggestion, TransactionRecord, VersionDiff, WriteOutcome,
    WritePrecondition,
};
use quarry_git::{
    export_worktree, import_worktree, pull_peer, push_peer, sync_peer, GitExportOptions,
    GitExportResult, GitImportResult, GitSyncResult,
};
use quarry_storage::{QuarryStore, StoreEvent, StoreEventKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex;
use utoipa::{OpenApi, ToSchema};

use crate::collab::{InjectionBatch, InjectionOp, LiveMutation};

#[derive(Clone)]
pub struct AppState {
    store: QuarryStore,
    collab: collab::CollabHub,
    agent_idempotency: AgentIdempotencyCache,
    agent_events: AgentEventJournal,
    agent_presence: AgentPresenceRegistry,
}

#[derive(Clone, Default)]
struct AgentIdempotencyCache {
    entries: Arc<Mutex<HashMap<String, CachedAgentEdit>>>,
}

#[derive(Clone)]
struct CachedAgentEdit {
    request_hash: String,
    response: AgentEditResponse,
    version_id: Option<String>,
}

impl AgentIdempotencyCache {
    async fn get(
        &self,
        key: &str,
        request_hash: &str,
    ) -> Result<Option<CachedAgentEdit>, ApiError> {
        let entries = self.entries.lock().await;
        let Some(cached) = entries.get(key) else {
            return Ok(None);
        };
        if cached.request_hash != request_hash {
            return Err(QuarryError::Conflict(
                "idempotency key already used for a different edit".to_string(),
            )
            .into());
        }
        Ok(Some(cached.clone()))
    }

    async fn insert(
        &self,
        key: String,
        request_hash: String,
        response: AgentEditResponse,
        version_id: Option<String>,
    ) {
        let mut entries = self.entries.lock().await;
        entries.insert(
            key,
            CachedAgentEdit {
                request_hash,
                response,
                version_id,
            },
        );
    }
}

const AGENT_EVENT_JOURNAL_CAPACITY: usize = 4096;

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
                        tracing::warn!(skipped, "agent event journal lagged");
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

pub fn router(store: QuarryStore) -> Router {
    let agent_events = AgentEventJournal::default();
    agent_events.spawn_ingest(store.clone());

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

    let collab = collab::CollabHub::new(store.clone());
    router.with_state(AppState {
        store,
        collab,
        agent_idempotency: AgentIdempotencyCache::default(),
        agent_events,
        agent_presence: AgentPresenceRegistry::default(),
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
        document_snapshot_openapi,
        document_events_stream_openapi,
        document_share_openapi,
        document_share_create_openapi,
        document_share_revoke_openapi,
        agent_presence_list_openapi,
        agent_presence_openapi,
        agent_events_pending,
        agent_events_ack,
        document_ops_openapi,
        document_versions_openapi,
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
        DocumentVersion,
        DocumentVersionContent,
        WriteOutcome,
        AgentDocumentSnapshot,
        AgentSnapshotBlock,
        AgentBlockRef,
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
        AgentOpsResponse,
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
    events_for_library(&state.store, &query.library, None).await
}

async fn events_for_library(
    store: &QuarryStore,
    library: &str,
    document_path: Option<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let library = store.get_library(library).await?;
    let receiver = store.subscribe_events();
    let stream = stream::unfold(
        (receiver, library.id, library.slug, document_path),
        |(mut receiver, library_id, library_slug, document_path)| async move {
            loop {
                match receiver.recv().await {
                    Ok(event)
                        if event.library_id == library_id
                            && event_matches_document_filter(&event, document_path.as_deref()) =>
                    {
                        let event_type = store_event_type(&event);
                        let payload = store_event_payload(&library_slug, &event_type, &event);
                        let event = Event::default().event(event_type).data(payload.to_string());
                        return Some((
                            Ok(event),
                            (receiver, library_id, library_slug, document_path),
                        ));
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
                        return Some((
                            Ok(event),
                            (receiver, library_id, library_slug, document_path),
                        ));
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
}

#[derive(Debug, Deserialize)]
struct DocumentActionQuery {
    #[serde(default, rename = "dryRun", alias = "dry_run")]
    dry_run: Option<String>,
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

impl DocumentActionQuery {
    fn dry_run(&self) -> Result<bool, ApiError> {
        let Some(value) = self.dry_run.as_deref() else {
            return Ok(false);
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Ok(true),
            "0" | "false" | "no" => Ok(false),
            _ => Err(QuarryError::InvalidPath("invalid dryRun value".to_string()).into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentBlockRef {
    #[serde(rename = "baseToken")]
    pub base_token: String,
    pub ordinal: usize,
    #[serde(rename = "contentHash")]
    pub content_hash: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<WriteOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Whether the edit was injected into a live collab room, or why it fell
    /// back to a plain write: `injected` | `no_live_room` | `not_codec_eligible`
    /// | `gate_rejected`. Absent for dry runs.
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
pub struct AgentOpsRequest {
    #[serde(rename = "baseToken")]
    pub base_token: String,
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
    pub by: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, alias = "suggestionType")]
    #[schema(value_type = Option<AgentSuggestionKind>)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgentOpsResponse {
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<WriteOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Whether the ops mutation reached a live collab room, or why it used the
    /// non-live write path: `injected` | `metadata_only_injected` |
    /// `no_live_room` | `gate_rejected` | `not_codec_eligible`. Absent for dry
    /// runs.
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
    if let Some(path) = path.strip_suffix("/snapshot") {
        return json_response(
            StatusCode::OK,
            &agent_document_snapshot(&state.store, &library, path).await?,
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
    if headers.contains_key("x-quarry-collab-session-id") {
        if let Err(error) = state
            .store
            .mark_collab_recovery_state_clean(
                &outcome.document.id,
                Some(outcome.version.id.clone()),
            )
            .await
        {
            if !matches!(error, QuarryError::NotFound(_)) {
                tracing::warn!(
                    %error,
                    document_id = %outcome.document.id,
                    version_id = %outcome.version.id,
                    "failed to mark collab recovery state clean after browser flush"
                );
            }
        }
    }
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
    Query(query): Query<DocumentActionQuery>,
    headers: HeaderMap,
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

    if let Some(path) = path.strip_suffix("/share") {
        let request: CreateCollabInviteRequest = serde_json::from_value(request)
            .map_err(|error| QuarryError::InvalidPath(format!("invalid share request: {error}")))?;
        let token = state
            .store
            .create_collab_invite_token(&library, path, &request.role, request.by_hint)
            .await?;
        return json_response(StatusCode::CREATED, &token);
    }

    if let Some((_, token_id)) = collab_invite_revoke_path(&path) {
        let token = state.store.revoke_collab_invite_token(token_id).await?;
        return json_response(StatusCode::OK, &token);
    }

    if let Some(path) = path.strip_suffix("/edit") {
        let request: AgentEditRequest = serde_json::from_value(request)
            .map_err(|error| QuarryError::InvalidPath(format!("invalid edit request: {error}")))?;
        let response =
            agent_edit_document(&state, &headers, &query, &library, path, request).await?;
        return agent_edit_response(response);
    }

    if let Some(path) = path.strip_suffix("/ops") {
        let request: AgentOpsRequest = serde_json::from_value(request)
            .map_err(|error| QuarryError::InvalidPath(format!("invalid ops request: {error}")))?;
        let response =
            agent_ops_document(&state, &headers, &query, &library, path, request).await?;
        return agent_ops_response(response);
    }

    if let Some(path) = path.strip_suffix("/presence") {
        let request: AgentPresenceRequest = serde_json::from_value(request).map_err(|error| {
            QuarryError::InvalidPath(format!("invalid presence request: {error}"))
        })?;
        let response = agent_presence_document(&state, &headers, &library, path, request).await?;
        return json_response(StatusCode::OK, &response);
    }

    Err(QuarryError::NotFound(path).into())
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/edit",
    params(("library" = String, Path), ("path" = String, Path), ("dryRun" = Option<DryRunValue>, Query)),
    request_body = AgentEditRequest,
    responses((status = 200, body = AgentEditResponse), (status = 412, body = ErrorResponse))
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
    params(("library" = String, Path), ("path" = String, Path), ("dryRun" = Option<DryRunValue>, Query)),
    request_body = AgentOpsRequest,
    responses((status = 200, body = AgentOpsResponse), (status = 409, body = ErrorResponse), (status = 412, body = ErrorResponse))
)]
#[allow(dead_code)]
async fn document_ops_openapi() {}

#[derive(Clone)]
struct AgentEditResult {
    response: AgentEditResponse,
    version_id: Option<String>,
}

#[derive(Clone, Debug)]
struct AgentEditPlan {
    markdown: String,
    blocks: Vec<String>,
    ops: Vec<PlannedAgentEditOp>,
    original_blocks: Vec<String>,
}

#[derive(Clone, Debug)]
enum PlannedAgentEditOp {
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

#[derive(Clone)]
struct AgentOpsResult {
    response: AgentOpsResponse,
    version_id: Option<String>,
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

async fn agent_ops_document(
    state: &AppState,
    headers: &HeaderMap,
    query: &DocumentActionQuery,
    library: &str,
    path: &str,
    request: AgentOpsRequest,
) -> Result<AgentOpsResult, ApiError> {
    let dry_run = query.dry_run()?;
    let document = state.store.get_document(library, path).await?;
    let markdown = document_markdown(&document)?;
    let base_token = etag(&document.version.id);
    let author = request
        .by
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| optional_header(headers, "x-agent-id").ok().flatten())
        .unwrap_or_else(|| "unknown".to_string());
    let applied = apply_agent_ops(&markdown, &base_token, &request, &author)?;

    if dry_run {
        return Ok(AgentOpsResult {
            response: AgentOpsResponse {
                dry_run: true,
                id: applied.id,
                outcome: None,
                markdown: Some(applied.markdown),
                injection: None,
            },
            version_id: None,
        });
    }

    let base_version_id = version_id_from_base_token(&request.base_token)?;
    let request_hash = agent_ops_request_hash(&request, false)?;
    let live_room = state.collab.live_room(&document.id).await;
    let (injection_guard, injection_status, injected_session_id) = if let Some(room) = live_room {
        let plan =
            match agent_ops_live_mutation_plan(&markdown, &applied.markdown, &request, &applied) {
                Ok(plan) => plan,
                Err(error) => {
                    tracing::debug!(
                        document_id = %document.id,
                        path,
                        reason = %error,
                        "agent ops live mutation rejected because operation is not codec eligible"
                    );
                    return Err(live_gate_rejected_error());
                }
            };
        let status = if plan.batch.is_some() {
            "injected"
        } else {
            "metadata_only_injected"
        };
        let mutation = match (plan.batch, plan.review) {
            (Some(batch), review) => LiveMutation::content(batch, review),
            (None, Some(review)) => LiveMutation::metadata(review),
            (None, None) => {
                tracing::debug!(
                    document_id = %document.id,
                    path,
                    "agent ops live mutation rejected because it was empty"
                );
                return Err(live_gate_rejected_error());
            }
        };
        let Some(guard) = room
            .begin_live_mutation(mutation, &plan.original_blocks, base_version_id.clone())
            .await
        else {
            tracing::debug!(
                document_id = %document.id,
                path,
                "agent ops live mutation gate rejected live room"
            );
            return Err(live_gate_rejected_error());
        };
        (
            Some(guard),
            status.to_string(),
            Some(format!("agent-injected:{request_hash}")),
        )
    } else {
        tracing::debug!(
            document_id = %document.id,
            path,
            "agent ops skipped live injection because no live collab room exists"
        );
        (None, "no_live_room".to_string(), None)
    };

    let outcome = if let Some(guard) = injection_guard {
        let outcome = state
            .store
            .commit_document_with_collab_session(
                library,
                path,
                applied.markdown.clone().into_bytes(),
                document.version.metadata.clone(),
                &document.version.content_type,
                DocumentSource::Rest,
                WritePrecondition::IfMatch(base_version_id.clone()),
            )
            .await?;
        let _commit_outcome = guard.commit(outcome.version.id.clone()).await;
        state
            .store
            .emit_document_put_events(&outcome, injected_session_id);
        outcome
    } else {
        state
            .store
            .put_document(
                library,
                path,
                applied.markdown.into_bytes(),
                document.version.metadata.clone(),
                &document.version.content_type,
                DocumentSource::Rest,
                WritePrecondition::IfMatch(base_version_id),
            )
            .await?
    };
    let version_id = outcome.version.id.clone();
    Ok(AgentOpsResult {
        response: AgentOpsResponse {
            dry_run: false,
            id: applied.id,
            outcome: Some(outcome),
            markdown: None,
            injection: Some(injection_status),
        },
        version_id: Some(version_id),
    })
}

fn agent_ops_response(result: AgentOpsResult) -> Result<Response, ApiError> {
    if let Some(version_id) = result.version_id {
        json_with_etag(StatusCode::OK, &result.response, &version_id)
    } else {
        json_response(StatusCode::OK, &result.response)
    }
}

async fn agent_document_snapshot(
    store: &QuarryStore,
    library: &str,
    path: &str,
) -> Result<AgentDocumentSnapshot, ApiError> {
    let document = store.get_document(library, path).await?;
    let markdown = document_markdown(&document)?;
    let base_token = etag(&document.version.id);
    let blocks = snapshot_blocks(&markdown, &base_token);
    Ok(AgentDocumentSnapshot {
        document_id: document.id,
        base_token,
        blocks,
    })
}

async fn agent_edit_document(
    state: &AppState,
    headers: &HeaderMap,
    query: &DocumentActionQuery,
    library: &str,
    path: &str,
    request: AgentEditRequest,
) -> Result<AgentEditResult, ApiError> {
    let dry_run = query.dry_run()?;
    let request_hash = agent_edit_request_hash(&request, dry_run)?;
    let idempotency_key = optional_header(headers, "idempotency-key")?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let cache_key = idempotency_key
        .as_ref()
        .filter(|_| !dry_run)
        .map(|key| format!("agent-edit\0{library}\0{path}\0{key}"));

    if let Some(cache_key) = &cache_key {
        if let Some(cached) = state
            .agent_idempotency
            .get(cache_key, &request_hash)
            .await?
        {
            return Ok(AgentEditResult {
                response: cached.response,
                version_id: cached.version_id,
            });
        }
    }

    let document = state.store.get_document(library, path).await?;
    let markdown = document_markdown(&document)?;
    let base_token = etag(&document.version.id);
    let plan = apply_agent_edit(&markdown, &base_token, &request)?;

    if dry_run {
        return Ok(AgentEditResult {
            response: AgentEditResponse {
                dry_run: true,
                outcome: None,
                markdown: Some(plan.markdown),
                injection: None,
            },
            version_id: None,
        });
    }

    let base_version_id = version_id_from_base_token(&request.base_token)?;
    let injected_session_id = format!("agent-injected:{request_hash}");
    let review_snapshot = review_meta_patch_from_markdown(&plan.markdown);
    // Track why an edit did or didn't reach the live room, so the response can
    // report it (observability for agents and tests) instead of silently
    // falling back to a plain write.
    let (injection_guard, injection_status) = match agent_edit_injection_batch(&plan) {
        Ok(batch) => match state.collab.live_room(&document.id).await {
            Some(room) => match room
                .begin_live_mutation(
                    LiveMutation::content(batch, review_snapshot.clone()),
                    &plan.original_blocks,
                    base_version_id.clone(),
                )
                .await
            {
                Some(guard) => (Some(guard), "injected"),
                None => (None, "gate_rejected"),
            },
            None => {
                tracing::debug!(
                    document_id = %document.id,
                    path,
                    "agent edit skipped live injection because no live collab room exists"
                );
                (None, "no_live_room")
            }
        },
        Err(error) => {
            tracing::debug!(
                document_id = %document.id,
                path,
                reason = %error,
                "agent edit skipped live injection because edit is not codec eligible"
            );
            (None, "not_codec_eligible")
        }
    };

    let outcome = if let Some(guard) = injection_guard {
        let outcome = state
            .store
            .commit_document_with_collab_session(
                library,
                path,
                plan.markdown.clone().into_bytes(),
                document.version.metadata.clone(),
                &document.version.content_type,
                DocumentSource::Rest,
                WritePrecondition::IfMatch(base_version_id.clone()),
            )
            .await?;
        let _commit_outcome = guard.commit(outcome.version.id.clone()).await;
        state
            .store
            .emit_document_put_events(&outcome, Some(injected_session_id));
        outcome
    } else {
        state
            .store
            .put_document(
                library,
                path,
                plan.markdown.into_bytes(),
                document.version.metadata.clone(),
                &document.version.content_type,
                DocumentSource::Rest,
                WritePrecondition::IfMatch(base_version_id),
            )
            .await?
    };
    let version_id = outcome.version.id.clone();
    let response = AgentEditResponse {
        dry_run: false,
        outcome: Some(outcome),
        markdown: None,
        injection: Some(injection_status.to_string()),
    };

    if let Some(cache_key) = cache_key {
        state
            .agent_idempotency
            .insert(
                cache_key,
                request_hash,
                response.clone(),
                Some(version_id.clone()),
            )
            .await;
    }

    Ok(AgentEditResult {
        response,
        version_id: Some(version_id),
    })
}

fn agent_edit_response(result: AgentEditResult) -> Result<Response, ApiError> {
    if let Some(version_id) = result.version_id {
        json_with_etag(StatusCode::OK, &result.response, &version_id)
    } else {
        json_response(StatusCode::OK, &result.response)
    }
}

fn agent_edit_request_hash(request: &AgentEditRequest, dry_run: bool) -> Result<String, ApiError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(if dry_run { b"dry-run:1" } else { b"dry-run:0" });
    hasher.update(&serde_json::to_vec(request).map_err(QuarryError::from)?);
    Ok(hasher.finalize().to_hex().to_string())
}

fn agent_ops_request_hash(request: &AgentOpsRequest, dry_run: bool) -> Result<String, ApiError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(if dry_run { b"dry-run:1" } else { b"dry-run:0" });
    hasher.update(&serde_json::to_vec(request).map_err(QuarryError::from)?);
    Ok(hasher.finalize().to_hex().to_string())
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

fn snapshot_blocks(markdown: &str, base_token: &str) -> Vec<AgentSnapshotBlock> {
    split_markdown_blocks(markdown)
        .into_iter()
        .enumerate()
        .map(|(ordinal, markdown)| AgentSnapshotBlock {
            block_ref: AgentBlockRef {
                base_token: base_token.to_string(),
                ordinal,
                content_hash: block_hash(&markdown),
            },
            markdown,
        })
        .collect()
}

fn apply_agent_edit(
    markdown: &str,
    current_base_token: &str,
    request: &AgentEditRequest,
) -> Result<AgentEditPlan, ApiError> {
    if request.operations.is_empty() {
        return Err(QuarryError::InvalidPath(
            "edit request must include at least one operation".to_string(),
        )
        .into());
    }

    let request_base_version_id = version_id_from_base_token(&request.base_token)?;
    let current_base_version_id = version_id_from_base_token(current_base_token)?;
    if request_base_version_id != current_base_version_id {
        return Err(stale_base_error());
    }

    let original_blocks = split_markdown_blocks(markdown);
    let mut blocks = original_blocks
        .iter()
        .cloned()
        .enumerate()
        .map(|(ordinal, markdown)| (Some(ordinal), markdown))
        .collect::<Vec<_>>();
    let mut targeted_ordinals = HashSet::new();
    let mut ops = Vec::new();

    for operation in &request.operations {
        let block_ref = operation.block_ref.as_ref().ok_or_else(|| {
            QuarryError::InvalidPath(format!("{} operation missing ref", operation.op))
        })?;
        validate_block_ref(block_ref, &request_base_version_id, &original_blocks)?;
        if !targeted_ordinals.insert(block_ref.ordinal) {
            return Err(QuarryError::InvalidPath(format!(
                "multiple operations target block ordinal {}; use one insert operation with blocks when inserting multiple blocks at the same ref",
                block_ref.ordinal
            ))
            .into());
        }
        let current_index = blocks
            .iter()
            .position(|(ordinal, _)| *ordinal == Some(block_ref.ordinal))
            .ok_or_else(stale_base_error)?;

        match operation.op.as_str() {
            "replace_block" => {
                reject_operation_blocks(operation)?;
                let markdown = required_operation_block(operation)?;
                validate_single_markdown_block(markdown)?;
                blocks[current_index] = (Some(block_ref.ordinal), markdown.to_string());
                ops.push(PlannedAgentEditOp::ReplaceBlock {
                    ordinal: block_ref.ordinal,
                    markdown: markdown.to_string(),
                });
            }
            "insert_before" => {
                let markdown_blocks = insert_operation_blocks(operation)?;
                blocks.splice(
                    current_index..current_index,
                    markdown_blocks
                        .iter()
                        .cloned()
                        .map(|markdown| (None, markdown)),
                );
                ops.push(PlannedAgentEditOp::InsertBefore {
                    ordinal: block_ref.ordinal,
                    markdown_blocks,
                });
            }
            "insert_after" => {
                let markdown_blocks = insert_operation_blocks(operation)?;
                blocks.splice(
                    current_index + 1..current_index + 1,
                    markdown_blocks
                        .iter()
                        .cloned()
                        .map(|markdown| (None, markdown)),
                );
                ops.push(PlannedAgentEditOp::InsertAfter {
                    ordinal: block_ref.ordinal,
                    markdown_blocks,
                });
            }
            "delete_block" => {
                reject_operation_block(operation)?;
                reject_operation_blocks(operation)?;
                blocks.remove(current_index);
                ops.push(PlannedAgentEditOp::DeleteBlock {
                    ordinal: block_ref.ordinal,
                });
            }
            other => {
                return Err(QuarryError::InvalidPath(format!(
                    "unsupported edit operation {other}"
                ))
                .into());
            }
        }
    }

    let final_blocks = blocks
        .into_iter()
        .map(|(_, markdown)| markdown)
        .collect::<Vec<_>>();
    let next_markdown = final_blocks.iter().cloned().collect::<String>();
    validate_markdown_roundtrip(&next_markdown)?;
    Ok(AgentEditPlan {
        markdown: next_markdown,
        blocks: final_blocks,
        ops,
        original_blocks,
    })
}

fn agent_edit_injection_batch(plan: &AgentEditPlan) -> Result<InjectionBatch, Unsupported> {
    // Per-original-block live node counts. Comment/suggestion marks are
    // reproduced and the trailing endmatter block contributes no live nodes, so
    // this now succeeds for review documents (previously bailed on endmatter).
    let original_nodes = review_blocks_to_slate(&plan.original_blocks)
        .map_err(|error| error.context("original blocks"))?;

    let mut prefix = Vec::with_capacity(original_nodes.len());
    let mut total = 0u32;
    for nodes in &original_nodes {
        prefix.push(total);
        let node_count = u32::try_from(nodes.len())
            .map_err(|_| Unsupported::new("original node count exceeds u32"))?;
        total = total
            .checked_add(node_count)
            .ok_or_else(|| Unsupported::new("original node count overflow"))?;
    }
    let original_node_count = |ordinal: usize| -> Result<u32, Unsupported> {
        let nodes = original_nodes
            .get(ordinal)
            .ok_or_else(|| Unsupported::new(format!("missing original block ordinal {ordinal}")))?;
        u32::try_from(nodes.len()).map_err(|_| Unsupported::new("original node count exceeds u32"))
    };

    // New blocks are reproduced against the edited document's own endmatter.
    let (_, new_meta) = split_review_endmatter(&plan.markdown);
    for (ordinal, block) in plan.blocks.iter().enumerate() {
        review_block_to_slate(block, &new_meta)
            .map_err(|error| error.context(format!("edited block {ordinal}")))?;
    }

    let mut ops = Vec::with_capacity(plan.ops.len());
    for op in &plan.ops {
        match op {
            PlannedAgentEditOp::ReplaceBlock { ordinal, markdown } => {
                ops.push(InjectionOp::ReplaceSpan {
                    start: *prefix.get(*ordinal).ok_or_else(|| {
                        Unsupported::new(format!("missing original block ordinal {ordinal}"))
                    })?,
                    old_node_count: original_node_count(*ordinal)?,
                    new_nodes: built_nodes_for_block(markdown, &new_meta)?,
                });
            }
            PlannedAgentEditOp::InsertBefore {
                ordinal,
                markdown_blocks,
            } => {
                ops.push(InjectionOp::InsertAt {
                    index: *prefix.get(*ordinal).ok_or_else(|| {
                        Unsupported::new(format!("missing original block ordinal {ordinal}"))
                    })?,
                    new_nodes: built_nodes_for_blocks(markdown_blocks, &new_meta)?,
                });
            }
            PlannedAgentEditOp::InsertAfter {
                ordinal,
                markdown_blocks,
            } => {
                let start = *prefix.get(*ordinal).ok_or_else(|| {
                    Unsupported::new(format!("missing original block ordinal {ordinal}"))
                })?;
                ops.push(InjectionOp::InsertAt {
                    index: start
                        .checked_add(original_node_count(*ordinal)?)
                        .ok_or_else(|| Unsupported::new("injection index overflow"))?,
                    new_nodes: built_nodes_for_blocks(markdown_blocks, &new_meta)?,
                });
            }
            PlannedAgentEditOp::DeleteBlock { ordinal } => {
                ops.push(InjectionOp::DeleteSpan {
                    start: *prefix.get(*ordinal).ok_or_else(|| {
                        Unsupported::new(format!("missing original block ordinal {ordinal}"))
                    })?,
                    old_node_count: original_node_count(*ordinal)?,
                });
            }
        }
    }
    InjectionBatch::new(ops).ok_or_else(|| Unsupported::new("empty injection batch"))
}

fn built_nodes_for_block(
    markdown: &str,
    meta: &ReviewMeta,
) -> Result<Vec<quarry_collab_codec::BuiltNode>, Unsupported> {
    let nodes = review_block_to_slate(markdown, meta)?;
    build_nodes(&nodes)
}

fn built_nodes_for_blocks(
    markdown_blocks: &[String],
    meta: &ReviewMeta,
) -> Result<Vec<quarry_collab_codec::BuiltNode>, Unsupported> {
    let mut built = Vec::new();
    for markdown in markdown_blocks {
        built.extend(built_nodes_for_block(markdown, meta)?);
    }
    Ok(built)
}

struct AgentOpsLiveMutationPlan {
    batch: Option<InjectionBatch>,
    original_blocks: Vec<String>,
    review: Option<JsonValue>,
}

fn agent_ops_live_mutation_plan(
    original_markdown: &str,
    edited_markdown: &str,
    request: &AgentOpsRequest,
    applied: &AppliedAgentOps,
) -> Result<AgentOpsLiveMutationPlan, Unsupported> {
    let id = applied
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Unsupported::new("agent ops mutation missing applied review id"))?;
    let review = agent_ops_review_meta_patch(edited_markdown, request, applied)?;
    match request.op.as_str() {
        "comment.add" | "suggestion.add" => {
            let ordinal = request
                .block_ref
                .as_ref()
                .ok_or_else(|| Unsupported::new(format!("{} operation missing ref", request.op)))?
                .ordinal;
            let plan = block_replace_injection_plan(original_markdown, edited_markdown, ordinal)?;
            Ok(AgentOpsLiveMutationPlan {
                batch: Some(plan.batch),
                original_blocks: plan.original_blocks,
                review,
            })
        }
        "suggestion.accept" | "accept" | "suggestion.reject" | "reject" => {
            let (body, _) = split_review_endmatter(original_markdown);
            let ordinal = review_marker_block_ordinal(&body, id)?;
            let plan = block_replace_injection_plan(original_markdown, edited_markdown, ordinal)?;
            Ok(AgentOpsLiveMutationPlan {
                batch: Some(plan.batch),
                original_blocks: plan.original_blocks,
                review,
            })
        }
        "comment.reply" => Ok(AgentOpsLiveMutationPlan {
            batch: None,
            original_blocks: split_markdown_blocks(original_markdown),
            review,
        }),
        "comment.delete" => {
            let (original_body, _) = split_review_endmatter(original_markdown);
            let (edited_body, _) = split_review_endmatter(edited_markdown);
            if original_body == edited_body {
                Ok(AgentOpsLiveMutationPlan {
                    batch: None,
                    original_blocks: split_markdown_blocks(original_markdown),
                    review,
                })
            } else {
                let ordinal = review_marker_block_ordinal(&original_body, id)?;
                let plan =
                    block_replace_injection_plan(original_markdown, edited_markdown, ordinal)?;
                Ok(AgentOpsLiveMutationPlan {
                    batch: Some(plan.batch),
                    original_blocks: plan.original_blocks,
                    review,
                })
            }
        }
        "comment.resolve" => Ok(AgentOpsLiveMutationPlan {
            batch: None,
            original_blocks: split_markdown_blocks(original_markdown),
            review,
        }),
        other => Err(Unsupported::new(format!(
            "unsupported ops operation {other}"
        ))),
    }
}

struct AgentOpsBlockReplacementPlan {
    batch: InjectionBatch,
    original_blocks: Vec<String>,
}

fn block_replace_injection_plan(
    original_markdown: &str,
    edited_markdown: &str,
    ordinal: usize,
) -> Result<AgentOpsBlockReplacementPlan, Unsupported> {
    let original_blocks = split_markdown_blocks(original_markdown);
    let original_nodes = review_blocks_to_slate(&original_blocks)
        .map_err(|error| error.context("original blocks"))?;

    let mut prefix = Vec::with_capacity(original_nodes.len());
    let mut total = 0u32;
    for nodes in &original_nodes {
        prefix.push(total);
        let node_count = u32::try_from(nodes.len())
            .map_err(|_| Unsupported::new("original node count exceeds u32"))?;
        total = total
            .checked_add(node_count)
            .ok_or_else(|| Unsupported::new("original node count overflow"))?;
    }

    let start = *prefix
        .get(ordinal)
        .ok_or_else(|| Unsupported::new(format!("missing original block ordinal {ordinal}")))?;
    let old_node_count = original_nodes
        .get(ordinal)
        .ok_or_else(|| Unsupported::new(format!("missing original block ordinal {ordinal}")))
        .and_then(|nodes| {
            u32::try_from(nodes.len())
                .map_err(|_| Unsupported::new("original node count exceeds u32"))
        })?;

    let (edited_body, edited_meta) = split_review_endmatter(edited_markdown);
    let edited_blocks = split_markdown_blocks(&edited_body);
    let new_nodes = if let Some(edited_block) = edited_blocks.get(ordinal) {
        built_nodes_for_block(edited_block, &edited_meta)?
    } else if edited_body.trim().is_empty() {
        Vec::new()
    } else {
        return Err(Unsupported::new(format!(
            "missing edited block ordinal {ordinal}"
        )));
    };
    let batch = InjectionBatch::new(vec![InjectionOp::ReplaceSpan {
        start,
        old_node_count,
        new_nodes,
    }])
    .ok_or_else(|| Unsupported::new("empty injection batch"))?;

    Ok(AgentOpsBlockReplacementPlan {
        batch,
        original_blocks,
    })
}

fn agent_ops_review_meta_patch(
    edited_markdown: &str,
    request: &AgentOpsRequest,
    applied: &AppliedAgentOps,
) -> Result<Option<JsonValue>, Unsupported> {
    let id = applied
        .id
        .as_deref()
        .ok_or_else(|| Unsupported::new("agent ops mutation missing applied review id"))?;
    match request.op.as_str() {
        "comment.add" | "comment.reply" => {
            let (_, meta) = review_meta_with_inline_comment_bodies(edited_markdown);
            let entry = meta
                .comments
                .get(id)
                .ok_or_else(|| Unsupported::new(format!("missing comment metadata {id}")))?;
            review_patch_entry("comments", id, entry).map(Some)
        }
        "suggestion.add" => {
            let (_, meta) = split_review_endmatter(edited_markdown);
            let entry = meta
                .suggestions
                .get(id)
                .ok_or_else(|| Unsupported::new(format!("missing suggestion metadata {id}")))?;
            review_patch_entry("suggestions", id, entry).map(Some)
        }
        "suggestion.accept" | "accept" | "suggestion.reject" | "reject" => {
            Ok(Some(serde_json::json!({ "removeSuggestions": [id] })))
        }
        "comment.resolve" => {
            let (_, meta) = review_meta_with_inline_comment_bodies(edited_markdown);
            let entry = meta
                .comments
                .get(id)
                .ok_or_else(|| Unsupported::new(format!("missing comment metadata {id}")))?;
            review_patch_entry("comments", id, entry).map(Some)
        }
        "comment.delete" => {
            if applied.removed_comment_ids.is_empty() {
                return Err(Unsupported::new(
                    "comment.delete missing removed comment ids",
                ));
            }
            Ok(Some(serde_json::json!({
                "removeComments": applied.removed_comment_ids.clone()
            })))
        }
        other => Err(Unsupported::new(format!(
            "unsupported ops operation {other}"
        ))),
    }
}

fn review_meta_with_inline_comment_bodies(markdown: &str) -> (String, ReviewMeta) {
    let (body, mut meta) = split_review_endmatter(markdown);
    hydrate_inline_comment_bodies(&body, &mut meta);
    (body, meta)
}

fn review_patch_entry(
    section: &str,
    id: &str,
    entry: &ReviewMetaEntry,
) -> Result<JsonValue, Unsupported> {
    let mut entries = serde_json::Map::new();
    entries.insert(
        id.to_string(),
        serde_json::to_value(entry).map_err(|error| {
            Unsupported::new(format!("review metadata serialization failed: {error}"))
        })?,
    );
    let mut patch = serde_json::Map::new();
    patch.insert(section.to_string(), JsonValue::Object(entries));
    Ok(JsonValue::Object(patch))
}

fn review_marker_block_ordinal(body: &str, id: &str) -> Result<usize, Unsupported> {
    let marker = format!("{{#{id}}}");
    let matches = split_markdown_blocks(body)
        .into_iter()
        .enumerate()
        .filter_map(|(ordinal, block)| block.contains(&marker).then_some(ordinal))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [ordinal] => Ok(*ordinal),
        [] => Err(Unsupported::new(format!("review marker {id} not found"))),
        _ => Err(Unsupported::new(format!("review marker {id} is ambiguous"))),
    }
}

struct AppliedAgentOps {
    markdown: String,
    id: Option<String>,
    removed_comment_ids: Vec<String>,
}

fn apply_agent_ops(
    markdown: &str,
    current_base_token: &str,
    request: &AgentOpsRequest,
    author: &str,
) -> Result<AppliedAgentOps, ApiError> {
    let request_base_version_id = version_id_from_base_token(&request.base_token)?;
    let current_base_version_id = version_id_from_base_token(current_base_token)?;
    if request_base_version_id != current_base_version_id {
        return Err(stale_base_error());
    }

    let (body, mut meta) = split_review_endmatter(markdown);
    let (next_body, applied_id, removed_comment_ids) = match request.op.as_str() {
        "comment.add" => {
            let id = review_id(request.id.as_deref(), "c", markdown, &request.op)?;
            ensure_review_id_available(&body, &meta, &id)?;
            let block_ref = request.block_ref.as_ref().ok_or_else(|| {
                QuarryError::InvalidPath("comment.add operation missing ref".to_string())
            })?;
            let body_text = required_ops_text(request.body.as_deref(), "comment.add missing body")?;
            let at = now_timestamp();
            let next = splice_block_anchor(
                &body,
                &request_base_version_id,
                block_ref,
                request.quote.as_deref(),
                |anchor| format!("{{=={anchor}==}}{{>>{body_text}<<}}{{#{id}}}"),
            )?;
            meta.comments.insert(
                id.clone(),
                ReviewMetaEntry {
                    by: author.to_string(),
                    at: at.clone(),
                    body: None,
                    re: None,
                    status: None,
                    resolved: None,
                },
            );
            (next, Some(id), Vec::new())
        }
        "comment.reply" => {
            let parent_id = required_ops_text(
                request.parent_id.as_deref(),
                "comment.reply missing parentId",
            )?;
            validate_review_id(parent_id)?;
            {
                let parent = meta.comments.get(parent_id).ok_or_else(|| {
                    QuarryError::InvalidPath(format!("review comment {parent_id} not found"))
                })?;
                if parent.re.is_some() {
                    return Err(QuarryError::InvalidPath(format!(
                        "review comment {parent_id} is a reply"
                    ))
                    .into());
                }
            }
            let id = review_id(request.id.as_deref(), "r", markdown, &request.op)?;
            ensure_review_id_available(&body, &meta, &id)?;
            let body_text =
                required_ops_text(request.body.as_deref(), "comment.reply missing body")?;
            meta.comments.insert(
                id.clone(),
                ReviewMetaEntry {
                    by: author.to_string(),
                    at: now_timestamp(),
                    body: Some(body_text.to_string()),
                    re: Some(parent_id.to_string()),
                    status: None,
                    resolved: None,
                },
            );
            (body, Some(id), Vec::new())
        }
        "comment.delete" => {
            let id = required_ops_text(request.id.as_deref(), "comment.delete missing id")?;
            validate_review_id(id)?;
            let is_reply = meta
                .comments
                .get(id)
                .ok_or_else(|| QuarryError::InvalidPath(format!("review comment {id} not found")))?
                .re
                .is_some();
            let (next, removed_ids) = if is_reply {
                (body, vec![id.to_string()])
            } else {
                let mut removed_ids = vec![id.to_string()];
                removed_ids.extend(meta.comments.iter().filter_map(|(reply_id, entry)| {
                    (entry.re.as_deref() == Some(id)).then_some(reply_id.clone())
                }));
                (transform_comment_by_id(&body, id)?, removed_ids)
            };
            for removed_id in &removed_ids {
                meta.comments.remove(removed_id);
            }
            (next, Some(id.to_string()), removed_ids)
        }
        "suggestion.add" => {
            let id = review_id(request.id.as_deref(), "s", markdown, &request.op)?;
            ensure_review_id_available(&body, &meta, &id)?;
            let block_ref = request.block_ref.as_ref().ok_or_else(|| {
                QuarryError::InvalidPath("suggestion.add operation missing ref".to_string())
            })?;
            let kind = request
                .kind
                .as_deref()
                .unwrap_or("substitution")
                .trim()
                .to_ascii_lowercase();
            let next = match kind.as_str() {
                "insert" => {
                    let content =
                        required_ops_text(request.content.as_deref(), "insert missing content")?;
                    splice_block_insertion(
                        &body,
                        &request_base_version_id,
                        block_ref,
                        request.quote.as_deref(),
                        &format!("{{++{content}++}}{{#{id}}}"),
                    )?
                }
                "delete" | "remove" => splice_block_anchor(
                    &body,
                    &request_base_version_id,
                    block_ref,
                    request.quote.as_deref(),
                    |anchor| format!("{{--{anchor}--}}{{#{id}}}"),
                )?,
                "replace" | "substitution" => {
                    let content = required_ops_text(
                        request.content.as_deref(),
                        "substitution missing content",
                    )?;
                    splice_block_anchor(
                        &body,
                        &request_base_version_id,
                        block_ref,
                        request.quote.as_deref(),
                        |anchor| format!("{{~~{anchor}~>{content}~~}}{{#{id}}}"),
                    )?
                }
                other => {
                    return Err(QuarryError::InvalidPath(format!(
                        "unsupported suggestion kind {other}"
                    ))
                    .into());
                }
            };
            meta.suggestions.insert(
                id.clone(),
                ReviewMetaEntry {
                    by: author.to_string(),
                    at: now_timestamp(),
                    body: None,
                    re: None,
                    status: None,
                    resolved: None,
                },
            );
            (next, Some(id), Vec::new())
        }
        "suggestion.accept" | "accept" => {
            let id = required_ops_text(request.id.as_deref(), "accept missing id")?;
            let next = transform_suggestion_by_id(&body, id, true)?;
            meta.suggestions.remove(id);
            (next, Some(id.to_string()), Vec::new())
        }
        "suggestion.reject" | "reject" => {
            let id = required_ops_text(request.id.as_deref(), "reject missing id")?;
            let next = transform_suggestion_by_id(&body, id, false)?;
            meta.suggestions.remove(id);
            (next, Some(id.to_string()), Vec::new())
        }
        "comment.resolve" => {
            let id = required_ops_text(request.id.as_deref(), "comment.resolve missing id")?;
            let entry = meta.comments.get_mut(id).ok_or_else(|| {
                QuarryError::InvalidPath(format!("review comment {id} not found"))
            })?;
            entry.status = Some("resolved".to_string());
            (body, Some(id.to_string()), Vec::new())
        }
        other => {
            return Err(
                QuarryError::InvalidPath(format!("unsupported ops operation {other}")).into(),
            );
        }
    };

    Ok(AppliedAgentOps {
        markdown: assemble_review_document(&next_body, &meta)?,
        id: applied_id,
        removed_comment_ids,
    })
}

fn review_meta_patch_from_markdown(markdown: &str) -> Option<JsonValue> {
    let (body, mut meta) = split_review_endmatter(markdown);
    hydrate_inline_comment_bodies(&body, &mut meta);
    let value = serde_json::to_value(meta).ok()?;
    if value.as_object().is_some_and(|object| object.is_empty()) {
        None
    } else {
        Some(value)
    }
}

fn hydrate_inline_comment_bodies(body: &str, meta: &mut ReviewMeta) {
    for captures in inline_comment_body().captures_iter(body) {
        let Some(comment_body) = captures.get(2) else {
            continue;
        };
        let Some(id) = captures.get(3) else {
            continue;
        };
        if let Some(entry) = meta.comments.get_mut(id.as_str()) {
            entry.body = Some(comment_body.as_str().to_string());
        }
    }
}

fn inline_comment_body() -> &'static Regex {
    static INLINE_COMMENT_BODY: OnceLock<Regex> = OnceLock::new();
    INLINE_COMMENT_BODY.get_or_init(|| {
        Regex::new(r"\{==(?s:(.*?))==\}\{>>(?s:(.*?))<<\}\{#([A-Za-z0-9_-]+)\}")
            .expect("inline comment body regex is valid")
    })
}

fn required_operation_block(operation: &AgentBlockOperation) -> Result<&str, ApiError> {
    operation
        .block
        .as_ref()
        .map(|block| block.markdown.as_str())
        .ok_or_else(|| {
            QuarryError::InvalidPath(format!("{} operation missing block", operation.op)).into()
        })
}

fn reject_operation_block(operation: &AgentBlockOperation) -> Result<(), ApiError> {
    if operation.block.is_some() {
        return Err(QuarryError::InvalidPath(format!(
            "{} operation does not accept block",
            operation.op
        ))
        .into());
    }
    Ok(())
}

fn reject_operation_blocks(operation: &AgentBlockOperation) -> Result<(), ApiError> {
    if operation.blocks.is_some() {
        return Err(QuarryError::InvalidPath(format!(
            "{} operation does not accept blocks",
            operation.op
        ))
        .into());
    }
    Ok(())
}

fn insert_operation_blocks(operation: &AgentBlockOperation) -> Result<Vec<String>, ApiError> {
    match (&operation.block, &operation.blocks) {
        (Some(block), None) => {
            validate_single_markdown_block(&block.markdown)?;
            Ok(vec![block.markdown.clone()])
        }
        (None, Some(blocks)) => {
            if blocks.is_empty() {
                return Err(QuarryError::InvalidPath(format!(
                    "{} operation blocks must not be empty",
                    operation.op
                ))
                .into());
            }
            blocks
                .iter()
                .map(|block| {
                    validate_single_markdown_block(&block.markdown)?;
                    Ok(normalize_bulk_insert_block(&block.markdown))
                })
                .collect()
        }
        _ => Err(QuarryError::InvalidPath(format!(
            "{} operation requires exactly one of block or blocks",
            operation.op
        ))
        .into()),
    }
}

fn normalize_bulk_insert_block(markdown: &str) -> String {
    let mut normalized = markdown.to_string();
    while !normalized.ends_with("\n\n") {
        normalized.push('\n');
    }
    normalized
}

fn required_ops_text<'a>(value: Option<&'a str>, message: &str) -> Result<&'a str, ApiError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| QuarryError::InvalidPath(message.to_string()).into())
}

fn review_id(
    supplied: Option<&str>,
    prefix: &str,
    markdown: &str,
    operation: &str,
) -> Result<String, ApiError> {
    if let Some(id) = supplied.map(str::trim).filter(|id| !id.is_empty()) {
        validate_review_id(id)?;
        return Ok(id.to_string());
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(markdown.as_bytes());
    hasher.update(operation.as_bytes());
    hasher.update(now_timestamp().as_bytes());
    Ok(format!(
        "{}{}",
        prefix,
        &hasher.finalize().to_hex().to_string()[..12]
    ))
}

fn validate_review_id(id: &str) -> Result<(), ApiError> {
    if id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        Ok(())
    } else {
        Err(QuarryError::InvalidPath(format!("invalid review id {id}")).into())
    }
}

fn ensure_review_id_available(body: &str, meta: &ReviewMeta, id: &str) -> Result<(), ApiError> {
    if meta.comments.contains_key(id)
        || meta.suggestions.contains_key(id)
        || body.contains(&format!("{{#{id}}}"))
    {
        return Err(QuarryError::Conflict(format!("review id {id} already exists")).into());
    }
    Ok(())
}

fn splice_block_anchor<F>(
    body: &str,
    request_base_version_id: &str,
    block_ref: &AgentBlockRef,
    quote: Option<&str>,
    replacement: F,
) -> Result<String, ApiError>
where
    F: FnOnce(&str) -> String,
{
    let mut blocks = split_markdown_blocks(body);
    validate_block_ref(block_ref, request_base_version_id, &blocks)?;
    let block = blocks
        .get_mut(block_ref.ordinal)
        .ok_or_else(stale_base_error)?;
    let range = anchor_range(block, quote)?;
    let anchor = block[range.clone()].to_string();
    block.replace_range(range, &replacement(&anchor));
    Ok(blocks.concat())
}

fn splice_block_insertion(
    body: &str,
    request_base_version_id: &str,
    block_ref: &AgentBlockRef,
    quote: Option<&str>,
    insertion: &str,
) -> Result<String, ApiError> {
    let mut blocks = split_markdown_blocks(body);
    validate_block_ref(block_ref, request_base_version_id, &blocks)?;
    let block = blocks
        .get_mut(block_ref.ordinal)
        .ok_or_else(stale_base_error)?;
    let index = if let Some(quote) = quote {
        unique_quote_range(block, quote)?.end
    } else {
        block.trim_end().len()
    };
    block.insert_str(index, insertion);
    Ok(blocks.concat())
}

fn anchor_range(block: &str, quote: Option<&str>) -> Result<std::ops::Range<usize>, ApiError> {
    if let Some(quote) = quote {
        unique_quote_range(block, quote)
    } else {
        let end = block.trim_end().len();
        if end == 0 {
            return Err(QuarryError::InvalidPath("anchor block is empty".to_string()).into());
        }
        Ok(0..end)
    }
}

fn unique_quote_range(block: &str, quote: &str) -> Result<std::ops::Range<usize>, ApiError> {
    if quote.is_empty() {
        return Err(QuarryError::InvalidPath("quote must not be empty".to_string()).into());
    }
    let matches = block.match_indices(quote).collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(QuarryError::InvalidPath("ANCHOR_NOT_FOUND".to_string()).into()),
        [(start, matched)] => Ok(*start..(*start + matched.len())),
        _ => Err(QuarryError::InvalidPath("AMBIGUOUS_ANCHOR".to_string()).into()),
    }
}

fn transform_suggestion_by_id(body: &str, id: &str, accept: bool) -> Result<String, ApiError> {
    validate_review_id(id)?;
    let suffix = format!("{{#{id}}}");
    let matches = body.match_indices(&suffix).collect::<Vec<_>>();
    let suffix_start = match matches.as_slice() {
        [] => {
            return Err(
                QuarryError::InvalidPath(format!("review suggestion {id} not found")).into(),
            );
        }
        [(start, _)] => *start,
        _ => {
            return Err(
                QuarryError::InvalidPath(format!("review suggestion {id} is ambiguous")).into(),
            );
        }
    };
    let suffix_end = suffix_start + suffix.len();
    let before = &body[..suffix_start];

    let (marker_start, replacement) = if before.ends_with("++}") {
        let marker_start = before
            .rfind("{++")
            .ok_or_else(|| invalid_review_marker(id))?;
        let text = &body[marker_start + 3..suffix_start - 3];
        (marker_start, if accept { text } else { "" }.to_string())
    } else if before.ends_with("--}") {
        let marker_start = before
            .rfind("{--")
            .ok_or_else(|| invalid_review_marker(id))?;
        let text = &body[marker_start + 3..suffix_start - 3];
        (marker_start, if accept { "" } else { text }.to_string())
    } else if before.ends_with("~~}") {
        let marker_start = before
            .rfind("{~~")
            .ok_or_else(|| invalid_review_marker(id))?;
        let inner = &body[marker_start + 3..suffix_start - 3];
        let (old, new) = inner
            .split_once("~>")
            .ok_or_else(|| invalid_review_marker(id))?;
        (marker_start, if accept { new } else { old }.to_string())
    } else {
        return Err(invalid_review_marker(id));
    };

    let mut next = body.to_string();
    next.replace_range(marker_start..suffix_end, &replacement);
    Ok(next)
}

fn transform_comment_by_id(body: &str, id: &str) -> Result<String, ApiError> {
    validate_review_id(id)?;
    let matches = inline_comment_body()
        .captures_iter(body)
        .filter_map(|captures| {
            let marker_id = captures.get(3)?;
            if marker_id.as_str() != id {
                return None;
            }
            let full = captures.get(0)?;
            let anchor = captures.get(1)?;
            Some((full.start(), full.end(), anchor.as_str().to_string()))
        })
        .collect::<Vec<_>>();
    let (start, end, anchor) = match matches.as_slice() {
        [] => {
            return Err(QuarryError::InvalidPath(format!("review comment {id} not found")).into());
        }
        [(start, end, anchor)] => (*start, *end, anchor.clone()),
        _ => {
            return Err(
                QuarryError::InvalidPath(format!("review comment {id} is ambiguous")).into(),
            );
        }
    };
    let mut next = body.to_string();
    next.replace_range(start..end, &anchor);
    Ok(next)
}

fn invalid_review_marker(id: &str) -> ApiError {
    QuarryError::InvalidPath(format!("review marker {id} is not a suggestion")).into()
}

// `split_review_endmatter` / `has_review_endmatter` and their helpers now live
// in `quarry_collab_codec::review`; imported at the top of this module.

fn assemble_review_document(body: &str, meta: &ReviewMeta) -> Result<String, ApiError> {
    let body = body.trim_end();
    if meta.comments.is_empty() && meta.suggestions.is_empty() {
        return Ok(if body.is_empty() {
            String::new()
        } else {
            format!("{body}\n")
        });
    }
    let mut yaml = serde_yaml::to_string(meta).map_err(QuarryError::from)?;
    if let Some(stripped) = yaml.strip_prefix("---\n") {
        yaml = stripped.to_string();
    }
    Ok(format!("{body}\n\n---\n{yaml}"))
}

fn validate_block_ref(
    block_ref: &AgentBlockRef,
    request_base_version_id: &str,
    original_blocks: &[String],
) -> Result<(), ApiError> {
    if version_id_from_base_token(&block_ref.base_token)? != request_base_version_id {
        return Err(stale_base_error());
    }
    let Some(block) = original_blocks.get(block_ref.ordinal) else {
        return Err(stale_base_error());
    };
    if block_hash(block) != block_ref.content_hash {
        return Err(stale_base_error());
    }
    Ok(())
}

fn validate_single_markdown_block(markdown: &str) -> Result<(), ApiError> {
    if markdown.trim().is_empty() {
        return Err(
            QuarryError::InvalidPath("edit block markdown must not be empty".to_string()).into(),
        );
    }
    let parsed_blocks = parsed_top_level_markdown_blocks(markdown);
    if parsed_blocks != 1 {
        return Err(QuarryError::InvalidPath(
            "edit block markdown must parse as one top-level block".to_string(),
        )
        .into());
    }
    let blocks = split_markdown_blocks(markdown);
    if blocks.len() != 1 || blocks.concat() != markdown {
        return Err(QuarryError::InvalidPath(
            "edit block markdown must be one top-level block".to_string(),
        )
        .into());
    }
    Ok(())
}

fn validate_markdown_roundtrip(markdown: &str) -> Result<(), ApiError> {
    parsed_top_level_markdown_blocks(markdown);
    if split_markdown_blocks(markdown).concat() != markdown {
        return Err(QuarryError::InvalidPath(
            "spliced markdown failed block round-trip validation".to_string(),
        )
        .into());
    }
    Ok(())
}

fn parsed_top_level_markdown_blocks(markdown: &str) -> usize {
    let parser = MarkdownParser::new_ext(markdown, MarkdownOptions::all());
    let mut stack = Vec::new();
    let mut depth = 0usize;
    let mut count = 0usize;

    for event in parser {
        match event {
            MarkdownEvent::Start(tag) => {
                let is_block = is_markdown_block_tag(&tag);
                stack.push(is_block);
                if is_block {
                    if depth == 0 {
                        count += 1;
                    }
                    depth += 1;
                }
            }
            MarkdownEvent::End(_) => {
                if stack.pop().unwrap_or(false) && depth > 0 {
                    depth -= 1;
                }
            }
            MarkdownEvent::Rule if depth == 0 => {
                count += 1;
            }
            _ => {}
        }
    }

    count
}

fn is_markdown_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::Item
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::MetadataBlock(_)
    )
}

fn version_id_from_base_token(base_token: &str) -> Result<String, ApiError> {
    let token = base_token
        .trim()
        .strip_prefix("W/")
        .unwrap_or_else(|| base_token.trim())
        .trim()
        .trim_matches('"')
        .to_string();
    if token.is_empty() {
        return Err(stale_base_error());
    }
    Ok(token)
}

fn stale_base_error() -> ApiError {
    QuarryError::PreconditionFailed("STALE_BASE".to_string()).into()
}

fn live_gate_rejected_error() -> ApiError {
    ApiError {
        status: StatusCode::CONFLICT,
        message: "LIVE_GATE_REJECTED".to_string(),
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
    fn injection_batch_supports_documents_with_review_comments() {
        // A document carrying a CriticMarkup comment + endmatter used to disable
        // live injection entirely. Replacing the commented prose block must now
        // produce a batch (the endmatter block contributes no live nodes).
        let original = "Quote {==highlight==}{>>note<<}{#c1} tail\n\n---\ncomments:\n  c1:\n    by: ai:codex\n    at: 2026-06-05T02:41:00.480Z\n";
        let original_blocks = split_markdown_blocks(original);
        assert_eq!(original_blocks.len(), 2, "prose block + endmatter block");

        let new_block = "Replaced prose\n\n".to_string();
        let mut blocks = original_blocks.clone();
        blocks[0] = new_block.clone();
        let plan = AgentEditPlan {
            markdown: blocks.concat(),
            blocks,
            ops: vec![PlannedAgentEditOp::ReplaceBlock {
                ordinal: 0,
                markdown: new_block,
            }],
            original_blocks,
        };

        assert!(
            agent_edit_injection_batch(&plan).is_ok(),
            "review-comment edit should produce a live injection batch"
        );
    }

    #[test]
    fn injection_batch_reports_codec_rejection_reason() {
        let original = "Add {++x++} now\n";
        let original_blocks = split_markdown_blocks(original);
        let new_block = "Plain replacement\n".to_string();
        let plan = AgentEditPlan {
            markdown: new_block.clone(),
            blocks: vec![new_block.clone()],
            ops: vec![PlannedAgentEditOp::ReplaceBlock {
                ordinal: 0,
                markdown: new_block,
            }],
            original_blocks,
        };

        let error = agent_edit_injection_batch(&plan).unwrap_err();

        assert_eq!(
            error.0,
            "original blocks: block 0: review marker without {#id}"
        );
    }

    #[test]
    fn review_meta_patch_from_markdown_includes_inline_comment_bodies() {
        let markdown = "Quote {==highlight==}{>>Needs support.<<}{#c1} tail\n\n---\ncomments:\n  c1:\n    by: ai:codex\n    at: 2026-06-05T02:41:00.480Z\n";

        let patch = review_meta_patch_from_markdown(markdown).unwrap();

        assert_eq!(patch["comments"]["c1"]["by"], "ai:codex");
        assert_eq!(patch["comments"]["c1"]["body"], "Needs support.");
    }

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

fn static_text_response(content_type: &'static str, body: &'static str) -> Response {
    let mut response = Response::new(axum::body::Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
