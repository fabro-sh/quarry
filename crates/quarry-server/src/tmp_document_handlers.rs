use crate::agent_prompt::{AgentPromptScope, agent_prompt};
use crate::discovery::request_origin;
use crate::presence::PresenceStreamGuard;
use crate::review::{DocumentReviewQuery, agent_tmp_document_review};
use crate::sse::events_for_tmp_document;
use crate::{
    AgentPresenceRequest, ApiError, AppState, ErrorResponse, PromoteTmpDocumentRequest,
    QuarryError, TmpAgentPresenceListResponse, TmpAgentPresenceResponse, TmpDocumentSubResource,
    TtlRequest, TtlResponse, agent_presence_tmp_document, bytes_response_with_expiry, gateway,
    insert_document_headers, json_response, json_with_etag, markdown_write, optional_header,
    parse_tmp_document_subresource, precondition_from_headers, require_tmp_markdown_content_type,
    tmp_metadata_from_headers, touch_agent_presence, transaction_metadata_from_headers,
};
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use quarry_core::{
    DocumentHistoryEntry, DocumentListEntry, DocumentVersion, DocumentVersionContent,
    TransactionRecord, VersionDiff, WriteOutcome, WritePrecondition,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateTmpDocumentRequest {
    pub content: Option<String>,
    pub metadata: Option<JsonValue>,
    pub content_type: Option<String>,
    pub expires_at: Option<String>,
}

/// The tmp GET query: the library shape minus the collab `token` (tmp
/// documents authenticate by capability secret, not invite token).
#[derive(Debug, Deserialize)]
pub(crate) struct TmpDocumentGetQuery {
    against: Option<String>,
    #[serde(default, flatten)]
    review: DocumentReviewQuery,
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
pub(crate) async fn create_tmp_document(
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
    path = "/v1/tmp/documents/{secret}/versions",
    params(("secret" = String, Path)),
    responses((status = 200, body = [DocumentHistoryEntry]))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn tmp_document_versions_openapi() {}

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
pub(crate) async fn tmp_document_versions_raw_openapi() {}

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
pub(crate) async fn tmp_document_version_openapi() {}

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
pub(crate) async fn tmp_document_ttl_openapi() {}

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
pub(crate) async fn tmp_document_promote_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/agent-prompt",
    params(("secret" = String, Path)),
    responses(
        (status = 200, description = "Ready-to-paste AI agent connect instructions", body = String),
        (status = 404, body = ErrorResponse)
    )
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn tmp_document_agent_prompt_openapi() {}

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
pub(crate) async fn tmp_document_blocks_openapi() {}

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
pub(crate) async fn tmp_document_block_transactions_openapi() {}

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
pub(crate) async fn tmp_document_events_stream_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}/versions/{version}/diff",
    params(("secret" = String, Path), ("version" = String, Path), ("against" = Option<String>, Query)),
    responses((status = 200, body = VersionDiff), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn tmp_document_version_diff_openapi() {}

#[utoipa::path(
    post,
    path = "/v1/tmp/documents/{secret}/versions/{version}/restore",
    params(("secret" = String, Path), ("version" = String, Path)),
    responses((status = 200, body = WriteOutcome), (status = 404, body = ErrorResponse))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn tmp_document_version_restore_openapi() {}

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
pub(crate) async fn tmp_agent_presence_list_openapi() {}

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
pub(crate) async fn tmp_agent_presence_openapi() {}

#[utoipa::path(
    get,
    path = "/v1/tmp/documents/{secret}",
    params(("secret" = String, Path)),
    responses((status = 200, body = String), (status = 410, body = ErrorResponse))
)]
pub(crate) async fn get_tmp_document(
    State(state): State<AppState>,
    Query(query): Query<TmpDocumentGetQuery>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (document_path, subresource) = parse_tmp_document_subresource(&path);
    match subresource {
        TmpDocumentSubResource::AgentPrompt => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            state.store.head_tmp_document(document_path).await?;
            let prompt = agent_prompt(
                &request_origin(&headers),
                &AgentPromptScope::Tmp {
                    secret: document_path,
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
        TmpDocumentSubResource::Blocks => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            return gateway::tmp_document_blocks(&state, document_path).await;
        }
        TmpDocumentSubResource::Review => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            let include_resolved = query.review.include_resolved()?;
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
                &crate::TmpAgentPresenceListResponse::from(
                    state.agent_presence.list(None, document_path),
                ),
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
                    document_id.to_string(),
                    agent_id,
                )
            });
            return Ok(events_for_tmp_document(
                &state.store,
                document_path.to_string(),
                document_id.to_string(),
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
        TmpDocumentSubResource::VersionDiff(version) => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .tmp_version_diff(document_path, version, query.against.as_deref())
                    .await?,
            );
        }
        TmpDocumentSubResource::Document
        | TmpDocumentSubResource::Ttl
        | TmpDocumentSubResource::Transactions
        | TmpDocumentSubResource::Promote
        | TmpDocumentSubResource::VersionRestore(_) => {}
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
pub(crate) async fn head_tmp_document(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Response, ApiError> {
    let document = state.store.head_tmp_document(&path).await?;
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
        (status = 200, body = markdown_write::PutDocumentOutcome),
        (status = 412, body = ErrorResponse),
        (status = 413, description = "Tmp Markdown body exceeds 1 MiB", body = ErrorResponse),
        (status = 415, description = "Tmp writes require a Markdown Content-Type", body = ErrorResponse)
    )
)]
pub(crate) async fn put_tmp_document(
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

pub(crate) async fn patch_tmp_document_action(
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
            expires_at: entry.expires_at.map(|value| value.to_string()),
        },
    )
}

pub(crate) async fn post_tmp_document_action(
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
        TmpDocumentSubResource::VersionRestore(version) => {
            touch_agent_presence(&state, &headers, None, document_path).await?;
            let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
            let actor = transaction_metadata_from_headers(&headers)?.actor;
            let target = state
                .store
                .tmp_document_version(document_path, version)
                .await?;
            // Tmp documents are always BlockDocuments, so restore is always a
            // whole-file write through the reconciler — no raw byte path.
            gateway::gateway_reply(
                markdown_write::restore_block_document_version(
                    &state,
                    quarry_storage::DocumentScopeRef::Tmp,
                    document_path,
                    &target,
                    origin_id,
                    actor,
                )
                .await,
            )
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
        | TmpDocumentSubResource::AgentPrompt
        | TmpDocumentSubResource::Blocks
        | TmpDocumentSubResource::Review
        | TmpDocumentSubResource::EventsStream
        | TmpDocumentSubResource::RawVersions
        | TmpDocumentSubResource::Versions
        | TmpDocumentSubResource::Version(_)
        | TmpDocumentSubResource::VersionDiff(_)
        | TmpDocumentSubResource::Ttl => Err(QuarryError::NotFound(path).into()),
    }
}

#[utoipa::path(
    delete,
    path = "/v1/tmp/documents/{secret}",
    params(("secret" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
pub(crate) async fn delete_tmp_document(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Json<TransactionRecord>, ApiError> {
    Ok(Json(state.store.delete_tmp_document(&path).await?))
}
