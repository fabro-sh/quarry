use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use quarry_store::{
    Actor, ActorKind, AnnotationRecord, BinaryObject, BinaryPointerResult, DeleteResult,
    DocumentSnapshotRecord, DocumentState, DocumentWriteResult, DraftResult, EventRecord,
    GitIngestResult, GitMaterializeResult, LocalStore, ObjectKind, PresenceRecord, PublishResult,
    RefRecord, RefSnapshotRecord, StoreError, TransactionRecord, TreeEntry, WorkspaceStatus,
    WriteResult,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct ApiState {
    pub store: LocalStore,
    pub event_tx: broadcast::Sender<serde_json::Value>,
}

pub fn router(store: LocalStore) -> Router {
    let (event_tx, _) = broadcast::channel(256);
    Router::new()
        .route("/", get(web_ui))
        .route("/health", get(health))
        .route("/openapi.json", get(openapi_json))
        .route("/workspaces", post(create_workspace))
        .route("/workspaces/:id/status", get(workspace_status))
        .route("/stats", get(stats))
        .route("/refs", get(list_refs).post(create_ref))
        .route("/drafts", post(create_draft))
        .route("/drafts/publish", post(publish_draft))
        .route("/tree/:ref_name", get(get_tree))
        .route(
            "/tree/:ref_name/*path",
            get(get_tree_entry)
                .put(write_tree_entry)
                .delete(delete_tree_entry),
        )
        .route("/blobs/:hash", get(get_blob))
        .route("/events", get(list_events))
        .route("/events/ws", get(events_ws))
        .route("/refs/:ref_name/snapshots", get(list_ref_snapshots))
        .route("/refs/:ref_name/restore", post(restore_ref_snapshot))
        .route("/documents", post(create_document))
        .route("/documents/:id/state", get(document_state))
        .route("/documents/:id/snapshot", get(document_snapshot))
        .route("/documents/:id/snapshots", get(document_snapshots))
        .route("/documents/:id/transactions", post(document_transaction))
        .route("/documents/:id/ops", post(document_op))
        .route(
            "/documents/:id/presence",
            get(document_presence).post(update_presence),
        )
        .route("/documents/:id/events", get(document_events))
        .route("/documents/:id/ws", get(document_ws))
        .route(
            "/binary-objects",
            get(list_binary_objects).post(create_binary_pointer),
        )
        .route("/binary-objects/:id/content", get(get_binary_content))
        .route("/binary-objects/:id", get(get_binary_object))
        .route("/transactions", get(list_transactions))
        .route("/transactions/:id", get(get_transaction))
        .route(
            "/annotations",
            get(list_annotations).post(create_annotation),
        )
        .route("/git/materialize", post(materialize_ref))
        .route("/git/ingest", post(git_ingest))
        .route("/mcp", post(mcp_rpc))
        .route("/mcp/tools/:tool", post(mcp_tool))
        .with_state(ApiState { store, event_tx })
}

pub async fn serve(store: LocalStore, addr: SocketAddr) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(store)).await
}

impl ApiState {
    fn publish_latest_event(&self) {
        if let Ok(Some(event)) = self
            .store
            .list_events(1, None)
            .map(|events| events.into_iter().next())
        {
            let _ = self
                .event_tx
                .send(json!({ "type": "event", "event": event }));
        }
    }
}

async fn web_ui() -> Html<&'static str> {
    Html(POTION_WEB_UI)
}

const POTION_WEB_UI: &str = include_str!("web_ui.html");

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "service": "quarry" }))
}

async fn openapi_json() -> Json<serde_json::Value> {
    Json(openapi_document())
}

async fn create_workspace(
    State(state): State<ApiState>,
) -> Result<Json<WorkspaceStatus>, ApiError> {
    Ok(Json(state.store.status()?))
}

async fn workspace_status(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceStatus>, ApiError> {
    let status = state.store.status()?;
    if status.workspace_id != id {
        return Err(ApiError::not_found(format!("workspace {id}")));
    }
    Ok(Json(status))
}

async fn stats(State(state): State<ApiState>) -> Result<Json<WorkspaceStatus>, ApiError> {
    Ok(Json(state.store.status()?))
}

async fn list_refs(State(state): State<ApiState>) -> Result<Json<Vec<RefRecord>>, ApiError> {
    Ok(Json(state.store.list_refs()?))
}

async fn create_ref(
    State(state): State<ApiState>,
    Json(request): Json<CreateRefRequest>,
) -> Result<Json<RefRecord>, ApiError> {
    Ok(Json(state.store.ensure_ref(&request.name)?))
}

async fn create_draft(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<CreateDraftRequest>,
) -> Result<Json<DraftResult>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result = state.store.create_draft(
        request.base_ref.as_deref().unwrap_or("published/main"),
        request.name.as_deref(),
        actor,
    )?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn publish_draft(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<PublishDraftRequest>,
) -> Result<Json<PublishResult>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result = state.store.publish_ref(
        &request.source_ref,
        request.target_ref.as_deref().unwrap_or("published/main"),
        actor,
    )?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn get_tree(
    State(state): State<ApiState>,
    Path(ref_name): Path<String>,
) -> Result<Json<RefRecord>, ApiError> {
    Ok(Json(state.store.get_ref(&ref_name)?))
}

async fn get_tree_entry(
    State(state): State<ApiState>,
    Path((ref_name, path)): Path<(String, String)>,
) -> Result<Json<TreeEntryResponse>, ApiError> {
    let entry = state.store.tree_entry(&ref_name, &path)?;
    let content_text = if matches!(
        entry.object_kind,
        ObjectKind::Blob | ObjectKind::StructuredDoc
    ) {
        state.store.read_text(&ref_name, &path).ok()
    } else {
        None
    };

    Ok(Json(TreeEntryResponse {
        entry,
        content_text,
    }))
}

async fn write_tree_entry(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((ref_name, path)): Path<(String, String)>,
    Json(request): Json<WriteTextRequest>,
) -> Result<Json<WriteResult>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result =
        state
            .store
            .write_text(&ref_name, &path, &request.content, actor, request.message)?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn delete_tree_entry(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((ref_name, path)): Path<(String, String)>,
) -> Result<Json<DeleteResult>, ApiError> {
    let result = state
        .store
        .delete_path(&ref_name, &path, actor_from_headers(&headers))?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn get_blob(
    State(state): State<ApiState>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    let record = state.store.blob_record(&hash)?;
    let bytes = state.store.read_blob(&hash)?;
    let content_type = record
        .media_type
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .map_err(|err| ApiError::internal(err.to_string()))?;
    Ok(response)
}

async fn list_events(
    State(state): State<ApiState>,
    Query(query): Query<ListEventsQuery>,
) -> Result<Json<Vec<EventRecord>>, ApiError> {
    Ok(Json(state.store.list_events(
        query.limit.unwrap_or(100),
        query.target.as_deref(),
    )?))
}

async fn events_ws(State(state): State<ApiState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_events_socket(state, socket))
}

async fn handle_events_socket(state: ApiState, mut socket: WebSocket) {
    if let Ok(events) = state.store.list_events(25, None) {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "events", "events": events }).to_string(),
            ))
            .await;
    }

    let mut rx = state.event_tx.subscribe();
    while let Ok(event) = rx.recv().await {
        if socket.send(Message::Text(event.to_string())).await.is_err() {
            break;
        }
    }
}

async fn list_ref_snapshots(
    State(state): State<ApiState>,
    Path(ref_name): Path<String>,
    Query(query): Query<ListSnapshotsQuery>,
) -> Result<Json<Vec<RefSnapshotRecord>>, ApiError> {
    Ok(Json(state.store.list_ref_snapshots(
        &ref_name,
        query.limit.unwrap_or(50),
    )?))
}

async fn restore_ref_snapshot(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(ref_name): Path<String>,
    Json(request): Json<RestoreRefRequest>,
) -> Result<Json<RefRecord>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result = state
        .store
        .restore_ref_snapshot(&ref_name, &request.snapshot_id, actor)?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn create_document(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<CreateDocumentRequest>,
) -> Result<Json<DocumentWriteResult>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let initial_text = request.text.unwrap_or_default();
    let snapshot = request.snapshot.unwrap_or_else(|| {
        json!({
            "schema": "quarry.structured_doc.v1",
            "format": "plain_text",
            "text": initial_text.clone(),
            "blocks": [
                {
                    "type": "p",
                    "children": [{ "text": initial_text.clone() }]
                }
            ]
        })
    });
    let result = state.store.create_document(
        request.ref_name.as_deref().unwrap_or("published/main"),
        &request.path,
        request.title.as_deref(),
        snapshot,
        actor,
        request.message,
    )?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn document_state(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<DocumentState>, ApiError> {
    Ok(Json(state.store.document_state(&id)?))
}

async fn document_snapshot(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(state.store.get_document(&id)?.snapshot_json))
}

async fn document_snapshots(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<ListSnapshotsQuery>,
) -> Result<Json<Vec<DocumentSnapshotRecord>>, ApiError> {
    Ok(Json(
        state
            .store
            .list_document_snapshots(&id, query.limit.unwrap_or(50))?,
    ))
}

async fn document_transaction(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<DocumentOpRequest>,
) -> Result<Json<DocumentWriteResult>, ApiError> {
    append_document_op_from_request(state, headers, id, request).await
}

async fn document_op(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<DocumentOpRequest>,
) -> Result<Json<DocumentWriteResult>, ApiError> {
    append_document_op_from_request(state, headers, id, request).await
}

async fn append_document_op_from_request(
    state: ApiState,
    headers: HeaderMap,
    id: String,
    request: DocumentOpRequest,
) -> Result<Json<DocumentWriteResult>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result = state.store.append_document_op(&id, request.op, actor)?;
    state.publish_latest_event();
    let _ = state.event_tx.send(json!({
        "type": "document",
        "document_id": id,
        "state": result
    }));
    Ok(Json(result))
}

async fn update_presence(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<PresenceRequest>,
) -> Result<Json<PresenceRecord>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result = state.store.upsert_presence(&id, actor, request.cursor)?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn document_presence(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<PresenceRecord>>, ApiError> {
    Ok(Json(state.store.list_presence(&id)?))
}

async fn document_events(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(query): Query<ListEventsQuery>,
) -> Result<Json<Vec<EventRecord>>, ApiError> {
    Ok(Json(state.store.list_events(
        query.limit.unwrap_or(100),
        Some(&format!("document:{id}")),
    )?))
}

async fn document_ws(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let actor = actor_from_headers(&headers);
    ws.on_upgrade(move |socket| handle_document_socket(state, id, actor, socket))
}

async fn handle_document_socket(
    state: ApiState,
    document_id: String,
    actor: Actor,
    mut socket: WebSocket,
) {
    if let Ok(document_state) = state.store.document_state(&document_id) {
        let _ = socket
            .send(Message::Text(
                json!({ "type": "document_state", "state": document_state }).to_string(),
            ))
            .await;
    }

    let mut rx = state.event_tx.subscribe();
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(op) = value.get("op").cloned().or_else(|| value.get("operation").cloned()) {
                                match state.store.append_document_op(&document_id, op, actor.clone()) {
                                    Ok(result) => {
                                        state.publish_latest_event();
                                        let message = json!({
                                            "type": "document",
                                            "document_id": document_id,
                                            "state": result
                                        });
                                        let _ = state.event_tx.send(message.clone());
                                        let _ = socket.send(Message::Text(message.to_string())).await;
                                    }
                                    Err(error) => {
                                        let _ = socket.send(Message::Text(json!({
                                            "type": "error",
                                            "error": error.to_string()
                                        }).to_string())).await;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(value) => {
                        if socket.send(Message::Text(value.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
        }
    }
}

async fn get_binary_object(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<BinaryObject>, ApiError> {
    Ok(Json(state.store.get_binary_object(&id)?))
}

async fn get_binary_content(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let binary = state.store.get_binary_object(&id)?;
    let bytes = state.store.read_binary_content(&id)?;
    let content_type = binary
        .media_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .map_err(|err| ApiError::internal(err.to_string()))?;
    Ok(response)
}

async fn list_binary_objects(
    State(state): State<ApiState>,
) -> Result<Json<Vec<BinaryObject>>, ApiError> {
    Ok(Json(state.store.list_binary_objects()?))
}

async fn create_binary_pointer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<CreateBinaryPointerRequest>,
) -> Result<Json<BinaryPointerResult>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let ref_name = request.ref_name.as_deref().unwrap_or("published/main");

    if let Some(file_path) = request.file_path {
        let result = state.store.add_binary_file(
            ref_name,
            &request.path,
            file_path,
            request.media_type.as_deref(),
            actor,
        )?;
        state.publish_latest_event();
        return Ok(Json(result));
    }

    let hash = request
        .hash
        .ok_or_else(|| ApiError::bad_request("hash is required without file_path"))?;
    let size = request
        .size
        .ok_or_else(|| ApiError::bad_request("size is required without file_path"))?;
    let result = state.store.add_binary_pointer(
        ref_name,
        &request.path,
        &hash,
        size,
        request.media_type.as_deref(),
        request.source_path.as_deref(),
        request.external_url.as_deref(),
        actor,
    )?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn list_transactions(
    State(state): State<ApiState>,
    Query(query): Query<ListTransactionsQuery>,
) -> Result<Json<Vec<TransactionRecord>>, ApiError> {
    Ok(Json(
        state.store.list_transactions(query.limit.unwrap_or(50))?,
    ))
}

async fn get_transaction(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<TransactionRecord>, ApiError> {
    Ok(Json(state.store.get_transaction(&id)?))
}

async fn list_annotations(
    State(state): State<ApiState>,
    Query(query): Query<ListAnnotationsQuery>,
) -> Result<Json<Vec<AnnotationRecord>>, ApiError> {
    Ok(Json(state.store.list_annotations(query.target.as_deref())?))
}

async fn create_annotation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<CreateAnnotationRequest>,
) -> Result<Json<AnnotationRecord>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result = state
        .store
        .create_annotation(&request.target, &request.body, actor)?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn materialize_ref(
    State(state): State<ApiState>,
    Json(request): Json<MaterializeRequest>,
) -> Result<Json<MaterializeResponse>, ApiError> {
    if let Some(repo_dir) = request.repo_dir {
        let result = state.store.materialize_git(
            &request.ref_name,
            &repo_dir,
            request.branch.as_deref().unwrap_or("main"),
            request.message.as_deref(),
        )?;
        state.publish_latest_event();
        return Ok(Json(MaterializeResponse::from_git(result)));
    }

    let out_dir = request
        .out_dir
        .ok_or_else(|| ApiError::bad_request("repo_dir or out_dir is required"))?;
    state.store.export_ref(&request.ref_name, &out_dir)?;
    Ok(Json(MaterializeResponse {
        ref_name: request.ref_name,
        out_dir,
        repo_dir: None,
        branch: None,
        commit: None,
        changed: true,
        mode: "raw_export".to_string(),
    }))
}

async fn git_ingest(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<GitIngestRequest>,
) -> Result<Json<GitIngestResult>, ApiError> {
    let actor = request
        .actor
        .unwrap_or_else(|| actor_from_headers(&headers));
    let result = state.store.ingest_git(
        &request.repo_dir,
        request.ref_name.as_deref().unwrap_or("published/main"),
        actor,
    )?;
    state.publish_latest_event();
    Ok(Json(result))
}

async fn mcp_tool(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(tool): Path<String>,
    Json(input): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = actor_from_headers(&headers);
    let output = run_mcp_tool(&state, actor, &tool, input)?;
    state.publish_latest_event();

    Ok(Json(json!({ "tool": tool, "result": output })))
}

async fn mcp_rpc(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<McpRpcRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = request.id.clone().unwrap_or(serde_json::Value::Null);
    let actor = actor_from_headers(&headers);

    let result = match request.method.as_str() {
        "initialize" => json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {
                "tools": {},
                "resources": {}
            },
            "serverInfo": {
                "name": "quarry",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
        "tools/list" => json!({ "tools": mcp_tool_descriptors() }),
        "tools/call" => {
            let params = request.params.unwrap_or_else(|| json!({}));
            let name = params
                .get("name")
                .and_then(|value| value.as_str())
                .ok_or_else(|| ApiError::bad_request("tools/call requires name"))?;
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let value = run_mcp_tool(&state, actor, name, arguments)?;
            state.publish_latest_event();
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&value)?
                    }
                ],
                "structuredContent": value
            })
        }
        "resources/list" => json!({
            "resources": [
                { "uri": "quarry://status", "name": "Workspace status", "mimeType": "application/json" },
                { "uri": "quarry://refs", "name": "Refs", "mimeType": "application/json" },
                { "uri": "quarry://events", "name": "Recent events", "mimeType": "application/json" }
            ]
        }),
        "resources/read" => {
            let params = request.params.unwrap_or_else(|| json!({}));
            let uri = params
                .get("uri")
                .and_then(|value| value.as_str())
                .ok_or_else(|| ApiError::bad_request("resources/read requires uri"))?;
            let value = read_mcp_resource(&state.store, uri)?;
            json!({
                "contents": [
                    {
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": serde_json::to_string_pretty(&value)?
                    }
                ]
            })
        }
        _ => {
            return Ok(Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method {} not found", request.method) }
            })))
        }
    };

    Ok(Json(
        json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    ))
}

fn run_mcp_tool(
    state: &ApiState,
    actor: Actor,
    tool: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, ApiError> {
    let output = match tool {
        "quarry_status" => serde_json::to_value(state.store.status()?)?,
        "quarry_list" => {
            let request: RefToolRequest = serde_json::from_value(input)?;
            serde_json::to_value(
                state
                    .store
                    .get_ref(request.ref_name.as_deref().unwrap_or("published/main"))?,
            )?
        }
        "quarry_read" => {
            let request: PathToolRequest = serde_json::from_value(input)?;
            json!({
                "content": state.store.read_text(
                    request.ref_name.as_deref().unwrap_or("published/main"),
                    &request.path
                )?
            })
        }
        "quarry_write" => {
            let request: WriteToolRequest = serde_json::from_value(input)?;
            serde_json::to_value(state.store.write_text(
                request.ref_name.as_deref().unwrap_or("published/main"),
                &request.path,
                &request.content,
                request.actor.unwrap_or(actor),
                request.message,
            )?)?
        }
        "quarry_comment" => {
            let request: CommentToolRequest = serde_json::from_value(input)?;
            serde_json::to_value(state.store.create_annotation(
                &request.target,
                &request.body,
                request.actor.unwrap_or(actor),
            )?)?
        }
        "quarry_start_draft" => {
            let request: CreateDraftRequest = serde_json::from_value(input)?;
            serde_json::to_value(state.store.create_draft(
                request.base_ref.as_deref().unwrap_or("published/main"),
                request.name.as_deref(),
                request.actor.unwrap_or(actor),
            )?)?
        }
        "quarry_publish_draft" => {
            let request: PublishDraftRequest = serde_json::from_value(input)?;
            serde_json::to_value(state.store.publish_ref(
                &request.source_ref,
                request.target_ref.as_deref().unwrap_or("published/main"),
                request.actor.unwrap_or(actor),
            )?)?
        }
        "quarry_events" => {
            let request: ListEventsQuery = serde_json::from_value(input)?;
            serde_json::to_value(
                state
                    .store
                    .list_events(request.limit.unwrap_or(100), request.target.as_deref())?,
            )?
        }
        "quarry_snapshots" => {
            let request: RefSnapshotsToolRequest = serde_json::from_value(input)?;
            serde_json::to_value(
                state
                    .store
                    .list_ref_snapshots(&request.ref_name, request.limit.unwrap_or(50))?,
            )?
        }
        "quarry_restore" => {
            let request: RestoreToolRequest = serde_json::from_value(input)?;
            serde_json::to_value(state.store.restore_ref_snapshot(
                &request.ref_name,
                &request.snapshot_id,
                request.actor.unwrap_or(actor),
            )?)?
        }
        "quarry_create_document" => {
            let request: CreateDocumentRequest = serde_json::from_value(input)?;
            let snapshot = request.snapshot.unwrap_or_else(|| {
                let text = request.text.unwrap_or_default();
                json!({
                    "schema": "quarry.structured_doc.v1",
                    "format": "plain_text",
                    "text": text.clone(),
                    "blocks": [{ "type": "p", "children": [{ "text": text.clone() }] }]
                })
            });
            serde_json::to_value(state.store.create_document(
                request.ref_name.as_deref().unwrap_or("published/main"),
                &request.path,
                request.title.as_deref(),
                snapshot,
                request.actor.unwrap_or(actor),
                request.message,
            )?)?
        }
        "quarry_document_state" => {
            let request: DocumentToolRequest = serde_json::from_value(input)?;
            serde_json::to_value(state.store.document_state(&request.document_id)?)?
        }
        "quarry_document_op" => {
            let request: DocumentOpToolRequest = serde_json::from_value(input)?;
            serde_json::to_value(state.store.append_document_op(
                &request.document_id,
                request.op,
                request.actor.unwrap_or(actor),
            )?)?
        }
        "quarry_git_sync" => {
            let request: MaterializeRequest = serde_json::from_value(input)?;
            let repo_dir = request
                .repo_dir
                .ok_or_else(|| ApiError::bad_request("repo_dir is required"))?;
            serde_json::to_value(state.store.materialize_git(
                &request.ref_name,
                &repo_dir,
                request.branch.as_deref().unwrap_or("main"),
                request.message.as_deref(),
            )?)?
        }
        _ => return Err(ApiError::not_found(format!("MCP tool {tool}"))),
    };

    Ok(output)
}

fn read_mcp_resource(store: &LocalStore, uri: &str) -> Result<serde_json::Value, ApiError> {
    match uri {
        "quarry://status" => Ok(serde_json::to_value(store.status()?)?),
        "quarry://refs" => Ok(serde_json::to_value(store.list_refs()?)?),
        "quarry://events" => Ok(serde_json::to_value(store.list_events(100, None)?)?),
        _ => Err(ApiError::not_found(format!("resource {uri}"))),
    }
}

fn mcp_tool_descriptors() -> serde_json::Value {
    let names = [
        "quarry_status",
        "quarry_list",
        "quarry_read",
        "quarry_write",
        "quarry_comment",
        "quarry_start_draft",
        "quarry_publish_draft",
        "quarry_events",
        "quarry_snapshots",
        "quarry_restore",
        "quarry_create_document",
        "quarry_document_state",
        "quarry_document_op",
        "quarry_git_sync",
    ];
    serde_json::Value::Array(
        names
            .into_iter()
            .map(|name| json!({ "name": name, "inputSchema": { "type": "object" } }))
            .collect(),
    )
}

#[derive(Debug, Deserialize)]
pub struct CreateRefRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateDraftRequest {
    pub base_ref: Option<String>,
    pub name: Option<String>,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct PublishDraftRequest {
    pub source_ref: String,
    pub target_ref: Option<String>,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct WriteTextRequest {
    pub content: String,
    pub message: Option<String>,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAnnotationRequest {
    pub target: String,
    pub body: String,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBinaryPointerRequest {
    #[serde(rename = "ref")]
    pub ref_name: Option<String>,
    pub path: String,
    pub file_path: Option<String>,
    pub hash: Option<String>,
    pub size: Option<u64>,
    pub media_type: Option<String>,
    pub source_path: Option<String>,
    pub external_url: Option<String>,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct ListAnnotationsQuery {
    pub target: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListTransactionsQuery {
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ListEventsQuery {
    pub limit: Option<u64>,
    pub target: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListSnapshotsQuery {
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RestoreRefRequest {
    pub snapshot_id: String,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDocumentRequest {
    #[serde(rename = "ref")]
    pub ref_name: Option<String>,
    pub path: String,
    pub title: Option<String>,
    pub text: Option<String>,
    pub snapshot: Option<serde_json::Value>,
    pub message: Option<String>,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct DocumentOpRequest {
    pub op: serde_json::Value,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct PresenceRequest {
    pub cursor: serde_json::Value,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct MaterializeRequest {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub out_dir: Option<String>,
    pub repo_dir: Option<String>,
    pub branch: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitIngestRequest {
    #[serde(rename = "ref")]
    pub ref_name: Option<String>,
    pub repo_dir: String,
    pub actor: Option<Actor>,
}

#[derive(Debug, Serialize)]
pub struct MaterializeResponse {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub out_dir: String,
    pub repo_dir: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub changed: bool,
    pub mode: String,
}

impl MaterializeResponse {
    fn from_git(result: GitMaterializeResult) -> Self {
        Self {
            ref_name: result.ref_name,
            out_dir: result.repo_dir.clone(),
            repo_dir: Some(result.repo_dir),
            branch: Some(result.branch),
            commit: result.commit,
            changed: result.changed,
            mode: "git_materialize".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RefToolRequest {
    #[serde(rename = "ref")]
    pub ref_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PathToolRequest {
    #[serde(rename = "ref")]
    pub ref_name: Option<String>,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct WriteToolRequest {
    #[serde(rename = "ref")]
    pub ref_name: Option<String>,
    pub path: String,
    pub content: String,
    pub message: Option<String>,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct CommentToolRequest {
    pub target: String,
    pub body: String,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct RefSnapshotsToolRequest {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RestoreToolRequest {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub snapshot_id: String,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct DocumentToolRequest {
    pub document_id: String,
}

#[derive(Debug, Deserialize)]
pub struct DocumentOpToolRequest {
    pub document_id: String,
    pub op: serde_json::Value,
    pub actor: Option<Actor>,
}

#[derive(Debug, Deserialize)]
pub struct McpRpcRequest {
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct TreeEntryResponse {
    pub entry: TreeEntry,
    pub content_text: Option<String>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
        }
    }

    fn internal(message: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message,
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(value: StoreError) -> Self {
        match value {
            StoreError::NotFound(message) => Self {
                status: StatusCode::NOT_FOUND,
                message,
            },
            StoreError::InvalidPath(message) => Self {
                status: StatusCode::BAD_REQUEST,
                message,
            },
            StoreError::PolicyDenied(message) => Self {
                status: StatusCode::FORBIDDEN,
                message,
            },
            other => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: other.to_string(),
            },
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self::bad_request(value.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

fn actor_from_headers(headers: &HeaderMap) -> Actor {
    let id = header_value(headers, "x-quarry-actor-id").unwrap_or_else(|| "anonymous".to_string());
    let display_name = header_value(headers, "x-quarry-actor-name").unwrap_or_else(|| id.clone());
    let avatar_url = header_value(headers, "x-quarry-actor-avatar-url");
    let kind = match header_value(headers, "x-quarry-actor-kind")
        .unwrap_or_else(|| "human".to_string())
        .as_str()
    {
        "agent" => ActorKind::Agent,
        "git_import" => ActorKind::GitImport,
        "system" => ActorKind::System,
        "integration" => ActorKind::Integration,
        _ => ActorKind::Human,
    };

    Actor {
        id,
        display_name,
        kind,
        avatar_url,
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn openapi_document() -> serde_json::Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Quarry Local API",
            "version": env!("CARGO_PKG_VERSION")
        },
        "paths": {
            "/health": { "get": { "summary": "Health check" } },
            "/": { "get": { "summary": "Embedded local web UI" } },
            "/stats": { "get": { "summary": "Workspace stats and diagnostics" } },
            "/refs": {
                "get": { "summary": "List refs" },
                "post": { "summary": "Create or return a ref" }
            },
            "/drafts": { "post": { "summary": "Create a draft ref from a base ref" } },
            "/drafts/publish": { "post": { "summary": "Publish one ref into another ref" } },
            "/tree/{ref}": { "get": { "summary": "Read a ref tree" } },
            "/tree/{ref}/{path}": {
                "get": { "summary": "Read a tree entry" },
                "put": { "summary": "Write text to a path and auto-commit a transaction" },
                "delete": { "summary": "Delete a path subject to product guardrails" }
            },
            "/blobs/{hash}": { "get": { "summary": "Read immutable blob bytes" } },
            "/events": { "get": { "summary": "List append-only Quarry events" } },
            "/events/ws": { "get": { "summary": "Subscribe to local event stream over WebSocket" } },
            "/refs/{ref}/snapshots": { "get": { "summary": "List ref snapshots" } },
            "/refs/{ref}/restore": { "post": { "summary": "Restore a ref to a prior snapshot" } },
            "/documents": { "post": { "summary": "Create a structured document and attach it to a ref" } },
            "/documents/{id}/state": { "get": { "summary": "Read document state, snapshots, ops, and presence" } },
            "/documents/{id}/snapshot": { "get": { "summary": "Read latest structured document snapshot" } },
            "/documents/{id}/snapshots": { "get": { "summary": "List document snapshots" } },
            "/documents/{id}/transactions": { "post": { "summary": "Apply a document transaction" } },
            "/documents/{id}/ops": { "post": { "summary": "Apply a document op" } },
            "/documents/{id}/presence": {
                "get": { "summary": "List document presence" },
                "post": { "summary": "Update self-attested document presence" }
            },
            "/documents/{id}/events": { "get": { "summary": "List document events" } },
            "/documents/{id}/ws": { "get": { "summary": "Collaborate on a document over WebSocket" } },
            "/binary-objects": {
                "get": { "summary": "List opaque binary pointer records" },
                "post": { "summary": "Attach an opaque binary pointer to a ref path" }
            },
            "/binary-objects/{id}": { "get": { "summary": "Read an opaque binary pointer record" } },
            "/binary-objects/{id}/content": { "get": { "summary": "Read local binary bytes without text extraction" } },
            "/transactions": { "get": { "summary": "List transactions" } },
            "/transactions/{id}": { "get": { "summary": "Read a transaction" } },
            "/annotations": {
                "get": { "summary": "List annotations, optionally filtered by target" },
                "post": { "summary": "Create an annotation" }
            },
            "/git/materialize": { "post": { "summary": "Materialize a ref to a Git repo or raw export directory" } },
            "/git/ingest": { "post": { "summary": "Ingest a Git/worktree directory into a Quarry ref, preserving conflicts as drafts" } },
            "/mcp": { "post": { "summary": "MCP JSON-RPC endpoint for tools and resources" } },
            "/mcp/tools/{tool}": { "post": { "summary": "Invoke a Quarry MCP-style tool" } }
        }
    })
}

#[allow(dead_code)]
const WEB_UI: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Quarry</title>
  <style>
    :root {
      color-scheme: light dark;
      font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      line-height: 1.4;
    }
    body {
      margin: 0;
      background: Canvas;
      color: CanvasText;
    }
    header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 16px;
      padding: 14px 20px;
      border-bottom: 1px solid color-mix(in srgb, CanvasText 16%, transparent);
    }
    main {
      display: grid;
      grid-template-columns: minmax(220px, 320px) 1fr;
      min-height: calc(100vh - 57px);
    }
    aside {
      border-right: 1px solid color-mix(in srgb, CanvasText 16%, transparent);
      padding: 16px;
      overflow: auto;
    }
    section {
      padding: 16px;
      overflow: auto;
    }
    label {
      display: grid;
      gap: 5px;
      margin-bottom: 10px;
      font-size: 13px;
    }
    input, textarea, select, button {
      box-sizing: border-box;
      width: 100%;
      font: inherit;
      border: 1px solid color-mix(in srgb, CanvasText 22%, transparent);
      border-radius: 6px;
      background: Canvas;
      color: CanvasText;
      padding: 8px 10px;
    }
    button {
      width: auto;
      cursor: pointer;
    }
    textarea {
      min-height: 28vh;
      resize: vertical;
      font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      font-size: 13px;
    }
    .row {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      align-items: center;
      margin: 10px 0 14px;
    }
    .muted {
      color: color-mix(in srgb, CanvasText 62%, transparent);
      font-size: 13px;
    }
    .tree {
      display: grid;
      gap: 4px;
      margin-top: 12px;
    }
    .tree button {
      width: 100%;
      text-align: left;
      border-color: transparent;
      background: color-mix(in srgb, CanvasText 5%, transparent);
    }
    .status {
      white-space: pre-wrap;
      font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      font-size: 12px;
    }
    .panel {
      border-top: 1px solid color-mix(in srgb, CanvasText 16%, transparent);
      margin-top: 14px;
      padding-top: 14px;
    }
    .grid {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 10px;
    }
    @media (max-width: 760px) {
      main {
        grid-template-columns: 1fr;
      }
      aside {
        border-right: 0;
        border-bottom: 1px solid color-mix(in srgb, CanvasText 16%, transparent);
      }
    }
  </style>
</head>
<body>
  <header>
    <strong>Quarry</strong>
    <span id="workspace" class="muted">Loading workspace</span>
  </header>
  <main>
    <aside>
      <label>Ref
        <select id="ref"></select>
      </label>
      <div class="row">
        <button id="refresh" type="button">Refresh</button>
      </div>
      <div id="tree" class="tree"></div>
      <h3>Stats</h3>
      <pre id="stats" class="status"></pre>
      <h3>Transactions</h3>
      <pre id="transactions" class="status"></pre>
    </aside>
    <section>
      <div class="grid">
        <label>Path
          <input id="path" value="notes/hello.md" autocomplete="off">
        </label>
        <label>Actor
          <input id="actor" value="web-ui" autocomplete="off">
        </label>
      </div>
      <label>Content
        <textarea id="content"># Hello from Quarry
</textarea>
      </label>
      <div class="row">
        <button id="read" type="button">Read</button>
        <button id="write" type="button">Write</button>
      </div>
      <p id="message" class="muted"></p>
      <div class="panel">
        <h3>Draft</h3>
        <div class="grid">
          <label>Draft Name
            <input id="draftName" value="draft/web-review" autocomplete="off">
          </label>
          <label>Publish Target
            <input id="publishTarget" value="published/main" autocomplete="off">
          </label>
        </div>
        <div class="row">
          <button id="startDraft" type="button">Start Draft</button>
          <button id="publishDraft" type="button">Publish Ref</button>
        </div>
      </div>
      <div class="panel">
        <h3>Comments</h3>
        <label>Comment
          <input id="commentBody" value="Please revise this section" autocomplete="off">
        </label>
        <div class="row">
          <button id="addComment" type="button">Add Comment</button>
        </div>
        <pre id="annotations" class="status"></pre>
      </div>
      <div class="panel">
        <h3>Binary Pointer</h3>
        <div class="grid">
          <label>Hash
            <input id="binaryHash" value="demo-sha256" autocomplete="off">
          </label>
          <label>Size
            <input id="binarySize" value="1234" autocomplete="off">
          </label>
        </div>
        <div class="grid">
          <label>Media Type
            <input id="binaryMediaType" value="application/pdf" autocomplete="off">
          </label>
          <label>External URL
            <input id="binaryUrl" value="" autocomplete="off">
          </label>
        </div>
        <div class="row">
          <button id="addBinary" type="button">Add Pointer</button>
        </div>
      </div>
      <div class="panel">
        <h3>Git Sync</h3>
        <div class="grid">
          <label>Repo Dir
            <input id="repoDir" value="/tmp/quarry-web-demo-git" autocomplete="off">
          </label>
          <label>Branch
            <input id="branch" value="main" autocomplete="off">
          </label>
        </div>
        <div class="row">
          <button id="materialize" type="button">Materialize</button>
          <button id="ingest" type="button">Ingest</button>
        </div>
      </div>
    </section>
  </main>
  <script>
    const refEl = document.getElementById("ref");
    const pathEl = document.getElementById("path");
    const contentEl = document.getElementById("content");
    const messageEl = document.getElementById("message");
    const treeEl = document.getElementById("tree");
    const statsEl = document.getElementById("stats");
    const transactionsEl = document.getElementById("transactions");
    const annotationsEl = document.getElementById("annotations");
    const workspaceEl = document.getElementById("workspace");

    async function json(path, options) {
      const response = await fetch(path, options);
      const body = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(body.error || response.statusText);
      return body;
    }

    function encodedRef() {
      return encodeURIComponent(refEl.value || "published/main");
    }

    function encodedPath() {
      return pathEl.value.split("/").map(encodeURIComponent).join("/");
    }

    async function refresh() {
      const selectedRef = refEl.value;
      const [stats, refs, transactions] = await Promise.all([
        json("/stats"),
        json("/refs"),
        json("/transactions?limit=8")
      ]);
      workspaceEl.textContent = `${stats.workspace_id} · ${stats.git_sync}`;
      statsEl.textContent = JSON.stringify(stats, null, 2);
      transactionsEl.textContent = JSON.stringify(transactions, null, 2);
      refEl.innerHTML = refs.map(ref => `<option>${ref.name}</option>`).join("");
      const current = refs.find(ref => ref.name === selectedRef) || refs[0];
      if (!current) return;
      refEl.value = current.name;
      renderTree(current.entries || []);
      await refreshAnnotations();
    }

    function renderTree(entries) {
      treeEl.innerHTML = "";
      if (!entries.length) {
        treeEl.innerHTML = "<span class='muted'>No files yet</span>";
        return;
      }
      for (const entry of entries) {
        const button = document.createElement("button");
        button.type = "button";
        button.textContent = entry.path;
        button.onclick = () => {
          pathEl.value = entry.path;
          readFile();
        };
        treeEl.appendChild(button);
      }
    }

    async function readFile() {
      const result = await json(`/tree/${encodedRef()}/${encodedPath()}`);
      contentEl.value = result.content_text || "";
      messageEl.textContent = `Read ${result.entry.path}`;
    }

    async function writeFile() {
      const result = await json(`/tree/${encodedRef()}/${encodedPath()}`, {
        method: "PUT",
        headers: {
          "content-type": "application/json",
          "x-quarry-actor-id": document.getElementById("actor").value || "web-ui",
          "x-quarry-actor-kind": "human"
        },
        body: JSON.stringify({ content: contentEl.value, message: `web edit ${pathEl.value}` })
      });
      messageEl.textContent = `Wrote ${result.transaction.id}`;
      await refresh();
    }

    async function startDraft() {
      const result = await json("/drafts", {
        method: "POST",
        headers: { "content-type": "application/json", "x-quarry-actor-id": document.getElementById("actor").value || "web-ui" },
        body: JSON.stringify({ base_ref: refEl.value, name: document.getElementById("draftName").value })
      });
      messageEl.textContent = `Started ${result.draft_ref.name}`;
      await refresh();
      refEl.value = result.draft_ref.name;
      renderTree(result.draft_ref.entries || []);
    }

    async function publishDraft() {
      const result = await json("/drafts/publish", {
        method: "POST",
        headers: { "content-type": "application/json", "x-quarry-actor-id": document.getElementById("actor").value || "web-ui" },
        body: JSON.stringify({ source_ref: refEl.value, target_ref: document.getElementById("publishTarget").value || "published/main" })
      });
      messageEl.textContent = `Published to ${result.target_ref.name}`;
      await refresh();
    }

    function currentTarget() {
      return `ref:${refEl.value}:path:${pathEl.value}`;
    }

    async function refreshAnnotations() {
      const annotations = await json(`/annotations?target=${encodeURIComponent(currentTarget())}`);
      annotationsEl.textContent = JSON.stringify(annotations, null, 2);
    }

    async function addComment() {
      const result = await json("/annotations", {
        method: "POST",
        headers: { "content-type": "application/json", "x-quarry-actor-id": document.getElementById("actor").value || "web-ui" },
        body: JSON.stringify({ target: currentTarget(), body: document.getElementById("commentBody").value })
      });
      messageEl.textContent = `Commented ${result.id}`;
      await refreshAnnotations();
    }

    async function addBinary() {
      const result = await json("/binary-objects", {
        method: "POST",
        headers: { "content-type": "application/json", "x-quarry-actor-id": document.getElementById("actor").value || "web-ui" },
        body: JSON.stringify({
          ref: refEl.value,
          path: pathEl.value,
          hash: document.getElementById("binaryHash").value,
          size: Number(document.getElementById("binarySize").value || 0),
          media_type: document.getElementById("binaryMediaType").value,
          external_url: document.getElementById("binaryUrl").value || null
        })
      });
      messageEl.textContent = `Added binary pointer ${result.binary.id}`;
      await refresh();
    }

    async function materialize() {
      const result = await json("/git/materialize", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          ref: refEl.value,
          repo_dir: document.getElementById("repoDir").value,
          branch: document.getElementById("branch").value
        })
      });
      messageEl.textContent = `Materialized ${result.commit || "no changes"}`;
    }

    async function ingest() {
      const result = await json("/git/ingest", {
        method: "POST",
        headers: { "content-type": "application/json", "x-quarry-actor-id": "git", "x-quarry-actor-kind": "git_import" },
        body: JSON.stringify({
          ref: document.getElementById("publishTarget").value || "published/main",
          repo_dir: document.getElementById("repoDir").value
        })
      });
      messageEl.textContent = result.conflict_ref ? `Conflict draft ${result.conflict_ref}` : `Imported ${result.imported_paths.length} paths`;
      await refresh();
    }

    document.getElementById("refresh").onclick = () => refresh().catch(showError);
    document.getElementById("read").onclick = () => readFile().catch(showError);
    document.getElementById("write").onclick = () => writeFile().catch(showError);
    document.getElementById("startDraft").onclick = () => startDraft().catch(showError);
    document.getElementById("publishDraft").onclick = () => publishDraft().catch(showError);
    document.getElementById("addComment").onclick = () => addComment().catch(showError);
    document.getElementById("addBinary").onclick = () => addBinary().catch(showError);
    document.getElementById("materialize").onclick = () => materialize().catch(showError);
    document.getElementById("ingest").onclick = () => ingest().catch(showError);
    refEl.onchange = () => refresh().catch(showError);
    pathEl.onchange = () => refreshAnnotations().catch(showError);

    function showError(error) {
      messageEl.textContent = error.message;
    }

    refresh().catch(showError);
  </script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn writes_and_reads_tree_entries() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(LocalStore::open(dir.path()).unwrap());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/tree/published%2Fmain/docs/hello.md")
                    .header("content-type", "application/json")
                    .header("x-quarry-actor-id", "codex")
                    .header("x-quarry-actor-kind", "agent")
                    .body(Body::from(r##"{"content":"# Hello\n"}"##))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/tree/published%2Fmain/docs/hello.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["content_text"], "# Hello\n");
        assert_eq!(value["entry"]["path"], "docs/hello.md");
    }

    #[tokio::test]
    async fn stats_include_workspace_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(LocalStore::open(dir.path()).unwrap());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_ui_serves_potion_style_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(LocalStore::open(dir.path()).unwrap());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("class=\"sidebar\""));
        assert!(html.contains("class=\"right-panel\""));
        assert!(html.contains("Start draft"));
        assert!(html.contains("Transaction history"));
    }

    #[tokio::test]
    async fn mcp_tool_flow_writes_reads_comments_and_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(LocalStore::open(dir.path()).unwrap());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp/tools/quarry_start_draft")
                    .header("content-type", "application/json")
                    .header("x-quarry-actor-id", "codex")
                    .header("x-quarry-actor-kind", "agent")
                    .body(Body::from(
                        r##"{"base_ref":"published/main","name":"draft/codex"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp/tools/quarry_write")
                    .header("content-type", "application/json")
                    .header("x-quarry-actor-id", "codex")
                    .header("x-quarry-actor-kind", "agent")
                    .body(Body::from(
                        r##"{"ref":"draft/codex","path":"docs/v1.md","content":"done"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp/tools/quarry_comment")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"target":"ref:draft/codex:path:docs/v1.md","body":"ship it"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp/tools/quarry_publish_draft")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"source_ref":"draft/codex","target_ref":"published/main"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp/tools/quarry_read")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"ref":"published/main","path":"docs/v1.md"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["result"]["content"], "done");
    }

    #[tokio::test]
    async fn binary_pointer_endpoint_attaches_to_ref() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(LocalStore::open(dir.path()).unwrap());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/binary-objects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"ref":"published/main","path":"assets/design.pdf","hash":"abc","size":42,"media_type":"application/pdf"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/tree/published%2Fmain/assets/design.pdf")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["entry"]["object_kind"], "binary_object");
        assert!(value["content_text"].is_null());
    }

    #[tokio::test]
    async fn document_events_and_restore_routes_work() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(LocalStore::open(dir.path()).unwrap());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/documents")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"ref":"published/main","path":"docs/rich.md","title":"Rich","text":"hello"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let document_id = created["document"]["id"].as_str().unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/documents/{document_id}/ops"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"op":{"kind":"replace_text","text":"updated"}}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/documents/{document_id}/state"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let state: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(state["document"]["snapshot_json"]["text"], "updated");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/events?limit=20")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let events: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(events
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "document_op"));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/refs/published%2Fmain/snapshots?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let snapshots: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let snapshot_id = snapshots[0]["id"].as_str().unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/refs/published%2Fmain/restore")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"snapshot_id":"{snapshot_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mcp_json_rpc_lists_and_calls_tools() {
        let dir = tempfile::tempdir().unwrap();
        let app = router(LocalStore::open(dir.path()).unwrap());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "quarry_document_op"));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r##"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"quarry_status","arguments":{}}}"##,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
