use crate::{
    ApiError, AppState, ErrorResponse, gateway, insert_document_headers, json_with_etag,
    markdown_write, optional_header, precondition_from_headers, require_tmp_markdown_content_type,
    tmp_metadata_from_headers, touch_agent_presence, transaction_metadata_from_headers,
};
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use quarry_core::{TransactionRecord, WriteOutcome};
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
        (status = 200, body = WriteOutcome),
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
