use crate::{ALLOW_DOCUMENT_KIND_CHANGE_HEADER, ApiError};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use percent_encoding::percent_decode_str;
use quarry_core::{QuarryError, WritePrecondition};
use quarry_storage::{QuarryStore, TransactionMetadata};
use serde::Serialize;
use serde_json::Value as JsonValue;

pub(crate) fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string()
}

pub(crate) fn require_tmp_markdown_content_type(headers: &HeaderMap) -> Result<String, ApiError> {
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

pub(crate) fn tmp_metadata_from_headers(
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

pub(crate) async fn reject_block_document_downgrade_for_library(
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

pub(crate) fn metadata_from_headers(
    headers: &HeaderMap,
    content_type: &str,
) -> Result<JsonValue, ApiError> {
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

pub(crate) fn precondition_from_headers(
    headers: &HeaderMap,
) -> Result<WritePrecondition, ApiError> {
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

pub(crate) fn optional_header(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<String>, ApiError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(|value| value.to_string())
                .map_err(|_| QuarryError::InvalidInput(format!("invalid {name} header")).into())
        })
        .transpose()
}

pub(crate) fn transaction_metadata_from_headers(
    headers: &HeaderMap,
) -> Result<TransactionMetadata, ApiError> {
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

pub(crate) fn agent_id_from_headers_or_body(
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

pub(crate) fn normalized_agent_status(status: &str) -> Result<String, ApiError> {
    let status = status.trim().to_ascii_lowercase();
    match status.as_str() {
        "reading" | "thinking" | "acting" | "waiting" | "completed" | "error" => Ok(status),
        _ => Err(
            QuarryError::InvalidPath(format!("unsupported agent presence status {status}")).into(),
        ),
    }
}

pub(crate) fn etag(version_id: &str) -> String {
    format!("\"{version_id}\"")
}

fn checked_header_value(name: &str, value: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value).map_err(|error| {
        QuarryError::Invariant(format!("invalid {name} response header value: {error}")).into()
    })
}

pub(crate) fn insert_document_headers(
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

pub(crate) fn bytes_response_with_expiry(
    status: StatusCode,
    content: Vec<u8>,
    content_type: &str,
    version_id: &str,
    document_id: &str,
    expires_at: Option<&str>,
) -> Result<Response, ApiError> {
    let mut response = Response::new(Body::from(content));
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

pub(crate) fn json_with_etag<T: Serialize>(
    status: StatusCode,
    value: &T,
    version_id: &str,
) -> Result<Response, ApiError> {
    let bytes = serde_json::to_vec(value).map_err(QuarryError::from)?;
    let mut response = Response::new(Body::from(bytes));
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

pub(crate) fn json_response<T: Serialize>(
    status: StatusCode,
    value: &T,
) -> Result<Response, ApiError> {
    let bytes = serde_json::to_vec(value).map_err(QuarryError::from)?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}
