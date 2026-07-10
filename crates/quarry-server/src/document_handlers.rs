use crate::agent_prompt::{AgentPromptScope, agent_prompt};
use crate::discovery::request_origin;
use crate::markdown_write;
use crate::presence::PresenceStreamGuard;
use crate::review::{DocumentReviewQuery, agent_document_review, agent_document_snapshot};
use crate::sse::events_for_library;
use crate::{
    AgentDocumentSnapshot, AgentPresenceListResponse, AgentPresenceRequest, AgentPresenceResponse,
    ApiError, ApiErrorResponse, AppState, CreateCollabInviteRequest, DocumentSubResource,
    MoveRequest, QuarryError, TtlRequest, TtlResponse, agent_id_from_headers_or_body,
    bytes_response_with_expiry, content_type, gateway, insert_document_headers, json_response,
    json_with_etag, metadata_from_headers, normalized_agent_status, optional_header,
    parse_document_subresource, precondition_from_headers,
    reject_block_document_downgrade_for_library, touch_agent_presence,
    transaction_metadata_from_headers,
};
use axum::Json;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use quarry_core::{
    CollabInviteToken, DocumentHistoryEntry, DocumentListEntry, DocumentSource, DocumentVersion,
    DocumentVersionContent, LinkCollection, TransactionRecord, VersionDiff, WriteOutcome,
};
use quarry_storage::PutDocumentRequest;
use serde::Deserialize;
use serde_json::Value as JsonValue;

#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    prefix: Option<String>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocumentGetQuery {
    against: Option<String>,
    token: Option<String>,
    #[serde(default, flatten)]
    review: DocumentReviewQuery,
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents",
    params(("library" = String, Path), ("prefix" = Option<String>, Query), ("limit" = Option<u64>, Query)),
    responses((status = 200, body = [DocumentListEntry]))
)]
pub(crate) async fn list_documents(
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
    path = "/v1/libraries/{library}/documents/{path}/backlinks",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = LinkCollection), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_backlinks_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/outgoing-links",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = LinkCollection), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_outgoing_links_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/snapshot",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = AgentDocumentSnapshot), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_snapshot_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/blocks",
    params(("library" = String, Path), ("path" = String, Path)),
    responses(
        (status = 200, body = gateway::BlockTreeResponse),
        (status = 404, body = ApiErrorResponse),
        (status = 422, body = ApiErrorResponse)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_blocks_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/transactions",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = gateway::BlockTransactionRequest,
    responses(
        (status = 200, body = gateway::BlockTransactionAck),
        (status = 400, body = ApiErrorResponse),
        (status = 404, body = ApiErrorResponse),
        (status = 412, body = ApiErrorResponse),
        (status = 422, body = ApiErrorResponse)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_block_transactions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/events/stream",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, description = "Document-scoped server-sent event stream"), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_events_stream_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/share",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [CollabInviteToken]), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_share_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/share",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = CreateCollabInviteRequest,
    responses((status = 201, body = CollabInviteToken), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_share_create_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/share/{token}/revoke",
    params(("library" = String, Path), ("path" = String, Path), ("token" = String, Path)),
    responses((status = 200, body = CollabInviteToken), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_share_revoke_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [DocumentHistoryEntry]), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_versions_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/raw",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = [DocumentVersion]), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_versions_raw_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = DocumentVersionContent), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_version_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}/diff",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path), ("against" = Option<String>, Query)),
    responses((status = 200, body = VersionDiff), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_version_diff_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/versions/{version}/restore",
    params(("library" = String, Path), ("path" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = WriteOutcome), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_version_restore_openapi() {}

#[utoipa::path(
    patch,
    path = "/v1/libraries/{library}/documents/{path}/ttl",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = TtlRequest,
    responses((status = 200, body = TtlResponse), (status = 410, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_ttl_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/presence",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = AgentPresenceListResponse), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn agent_presence_list_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/presence",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = AgentPresenceRequest,
    responses((status = 200, body = AgentPresenceResponse), (status = 404, body = ApiErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn agent_presence_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}/agent-prompt",
    params(
        ("library" = String, Path),
        ("path" = String, Path),
        ("token" = String, Query)
    ),
    responses(
        (status = 200, description = "Ready-to-paste AI agent connect instructions", body = String),
        (status = 400, body = ApiErrorResponse),
        (status = 404, body = ApiErrorResponse)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn document_agent_prompt_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = String), (status = 404, body = ApiErrorResponse))
)]
pub(crate) async fn get_document(
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
                    document.id.to_string(),
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
        DocumentSubResource::AgentPrompt => {
            let token = query.token.as_deref().filter(|token| !token.is_empty());
            let Some(token) = token else {
                return Err(QuarryError::InvalidInput(
                    "agent-prompt requires a token query parameter".to_string(),
                )
                .into());
            };
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            state.store.head_document(&library, document_path).await?;
            let prompt = agent_prompt(
                &request_origin(&headers),
                &AgentPromptScope::Library {
                    library: &library,
                    path: document_path,
                    token,
                },
            );
            return Ok((
                StatusCode::OK,
                [(
                    axum::http::header::CONTENT_TYPE,
                    "text/plain; charset=utf-8",
                )],
                prompt,
            )
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
        (status = 200, body = markdown_write::PutDocumentOutcome),
        (status = 409, description = "Existing Markdown document would be changed into a raw document without X-Quarry-Allow-Document-Kind-Change: true", body = ApiErrorResponse),
        (status = 412, body = ApiErrorResponse)
    )
)]
pub(crate) async fn put_document(
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
    let reply = markdown_write::PutDocumentOutcome {
        outcome,
        changed: true,
        conflicts: 0,
    };
    json_with_etag(StatusCode::OK, &reply, &reply.outcome.version.id)
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/documents/{path}/move",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = MoveRequest,
    responses((status = 200, body = TransactionRecord))
)]
pub(crate) async fn post_document_action(
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
                    quarry_storage::DocumentScopeRef::library(&library),
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

#[utoipa::path(
    patch,
    path = "/v1/libraries/{library}/documents/{path}/metadata",
    params(("library" = String, Path), ("path" = String, Path)),
    request_body = JsonValue,
    responses((status = 200, body = WriteOutcome))
)]
pub(crate) async fn patch_document_metadata(
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
                expires_at: entry.expires_at.map(|value| value.to_string()),
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
    head,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200), (status = 404, body = ApiErrorResponse))
)]
pub(crate) async fn head_document(
    State(state): State<AppState>,
    Path((library, path)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let document = state.store.head_document(&library, &path).await?;
    let mut response = Response::new(Body::empty());
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
    delete,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
pub(crate) async fn delete_document(
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
