use crate::{ApiError, AppState, ErrorResponse, json_with_etag};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use quarry_core::WriteOutcome;
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
