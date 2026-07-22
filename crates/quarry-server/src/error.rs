use crate::log_redaction::redact_secret_tokens;
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use quarry_core::QuarryError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Stable machine-readable codes returned by every `/v1` HTTP error.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[non_exhaustive]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApiErrorCode {
    InvalidRequest,
    NotFound,
    Gone,
    PreconditionFailed,
    Conflict,
    MethodNotAllowed,
    UnsupportedMediaType,
    PayloadTooLarge,
    UnprocessableEntity,
    ServiceBusy,
    InternalError,
    StaleBase,
    BlockDeleted,
    AnchorNotFound,
    BlockMoveConflict,
    SuggestionInvalidated,
    SuggestionAlreadyResolved,
    UnsupportedMarkdown,
    InvalidTransaction,
    UnknownBlockType,
    UnsupportedBlockDocument,
}

impl ApiErrorCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::NotFound => "NOT_FOUND",
            Self::Gone => "GONE",
            Self::PreconditionFailed => "PRECONDITION_FAILED",
            Self::Conflict => "CONFLICT",
            Self::MethodNotAllowed => "METHOD_NOT_ALLOWED",
            Self::UnsupportedMediaType => "UNSUPPORTED_MEDIA_TYPE",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::UnprocessableEntity => "UNPROCESSABLE_ENTITY",
            Self::ServiceBusy => "SERVICE_BUSY",
            Self::InternalError => "INTERNAL_ERROR",
            Self::StaleBase => "STALE_BASE",
            Self::BlockDeleted => "BLOCK_DELETED",
            Self::AnchorNotFound => "ANCHOR_NOT_FOUND",
            Self::BlockMoveConflict => "BLOCK_MOVE_CONFLICT",
            Self::SuggestionInvalidated => "SUGGESTION_INVALIDATED",
            Self::SuggestionAlreadyResolved => "SUGGESTION_ALREADY_RESOLVED",
            Self::UnsupportedMarkdown => "UNSUPPORTED_MARKDOWN",
            Self::InvalidTransaction => "INVALID_TRANSACTION",
            Self::UnknownBlockType => "UNKNOWN_BLOCK_TYPE",
            Self::UnsupportedBlockDocument => "UNSUPPORTED_BLOCK_DOCUMENT",
        }
    }

    pub(crate) const fn retryable(self) -> bool {
        matches!(
            self,
            Self::PreconditionFailed
                | Self::ServiceBusy
                | Self::StaleBase
                | Self::BlockMoveConflict
        )
    }

    pub(crate) const fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest | Self::InvalidTransaction | Self::UnknownBlockType => {
                StatusCode::BAD_REQUEST
            }
            Self::NotFound | Self::BlockDeleted | Self::AnchorNotFound => StatusCode::NOT_FOUND,
            Self::Gone => StatusCode::GONE,
            Self::PreconditionFailed | Self::StaleBase | Self::BlockMoveConflict => {
                StatusCode::PRECONDITION_FAILED
            }
            Self::Conflict => StatusCode::CONFLICT,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnprocessableEntity
            | Self::SuggestionInvalidated
            | Self::SuggestionAlreadyResolved
            | Self::UnsupportedMarkdown
            | Self::UnsupportedBlockDocument => StatusCode::UNPROCESSABLE_ENTITY,
            Self::ServiceBusy => StatusCode::SERVICE_UNAVAILABLE,
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Debug)]
pub struct ApiError {
    code: ApiErrorCode,
    message: String,
    details: Option<Box<ApiErrorDetails>>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ApiErrorResponse {
    pub code: ApiErrorCode,
    pub retryable: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ApiErrorDetails>,
}

/// Machine-actionable context for a failed request. Transaction failures
/// identify the exact op and target; validation failures may additionally
/// identify the field, rejected value, current value, or allowed values.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
pub struct ApiErrorDetails {
    /// Zero-based index into a transaction's `ops` array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_index: Option<usize>,
    /// Operation discriminator, such as `replace_block_content`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Addressable entity involved in the failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ApiErrorTarget>,
    /// Request field that failed validation or a precondition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Rejected request value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Current server value, when useful for rebuilding a request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    /// Accepted values for a closed vocabulary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_values: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
pub struct ApiErrorTarget {
    /// Entity category, such as `block`, `review_item`, or `conflict`.
    pub kind: String,
    /// Stable identifier copied from the request or current review state.
    pub id: String,
}

impl ApiErrorDetails {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    fn redacted(mut self) -> Self {
        fn redact(value: String) -> String {
            redact_secret_tokens(&value).into_owned()
        }

        self.op = self.op.map(redact);
        self.target = self.target.map(|target| ApiErrorTarget {
            kind: redact(target.kind),
            id: redact(target.id),
        });
        self.field = self.field.map(redact);
        self.value = self.value.map(redact);
        self.current_value = self.current_value.map(redact);
        self.allowed_values = self.allowed_values.into_iter().map(redact).collect();
        self
    }
}

impl ApiError {
    pub(crate) fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub(crate) fn with_details(mut self, details: ApiErrorDetails) -> Self {
        self.details = (!details.is_empty()).then(|| Box::new(details));
        self
    }

    pub(crate) fn status(&self) -> StatusCode {
        self.code.status()
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl From<QuarryError> for ApiError {
    fn from(value: QuarryError) -> Self {
        let code = match &value {
            QuarryError::NotFound(_) => ApiErrorCode::NotFound,
            QuarryError::Gone(_) => ApiErrorCode::Gone,
            QuarryError::PreconditionFailed(_) => ApiErrorCode::PreconditionFailed,
            QuarryError::Conflict(_) => ApiErrorCode::Conflict,
            QuarryError::Busy(_) => ApiErrorCode::ServiceBusy,
            QuarryError::InvalidPath(_) | QuarryError::InvalidInput(_) => {
                ApiErrorCode::InvalidRequest
            }
            QuarryError::UnsupportedMediaType(_) => ApiErrorCode::UnsupportedMediaType,
            QuarryError::PayloadTooLarge(_) => ApiErrorCode::PayloadTooLarge,
            QuarryError::UnsupportedMarkdown(_) => ApiErrorCode::UnsupportedMarkdown,
            _ => ApiErrorCode::InternalError,
        };
        Self::new(code, value.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code;
        // Logs and 4xx bodies may echo the message, which for tmp documents can
        // contain the secret (e.g. `not found: <secret>`); redact it everywhere
        // it crosses the server boundary.
        let reason = redact_secret_tokens(&self.message);
        tracing::debug!(
            event = "api.error.returned",
            status = status.as_u16(),
            outcome = "error",
            reason_code = code.as_str(),
            reason = %reason,
            "API error returned"
        );
        if status == StatusCode::PRECONDITION_FAILED {
            tracing::debug!(
                event = "api.precondition.failed",
                status = status.as_u16(),
                outcome = "rejected",
                reason_code = code.as_str(),
                reason = %reason,
                "API precondition failed"
            );
        }
        if status == StatusCode::SERVICE_UNAVAILABLE {
            tracing::warn!(
                event = "api.busy.returned",
                status = status.as_u16(),
                outcome = "busy",
                reason_code = code.as_str(),
                reason = %reason,
                "API busy response returned"
            );
        }
        let message = match code {
            ApiErrorCode::ServiceBusy => "service temporarily unavailable".to_string(),
            ApiErrorCode::InternalError => "internal error".to_string(),
            _ => reason.into_owned(),
        };
        let payload = ApiErrorResponse {
            code,
            retryable: code.retryable(),
            message,
            details: self.details.map(|details| (*details).redacted()),
        };
        let mut response = (status, Json(payload)).into_response();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

pub(crate) fn fallback_error_for_status(status: StatusCode) -> ApiError {
    let code = match status {
        StatusCode::BAD_REQUEST => ApiErrorCode::InvalidRequest,
        StatusCode::NOT_FOUND => ApiErrorCode::NotFound,
        StatusCode::GONE => ApiErrorCode::Gone,
        StatusCode::PRECONDITION_FAILED => ApiErrorCode::PreconditionFailed,
        StatusCode::CONFLICT => ApiErrorCode::Conflict,
        StatusCode::METHOD_NOT_ALLOWED => ApiErrorCode::MethodNotAllowed,
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiErrorCode::UnsupportedMediaType,
        StatusCode::PAYLOAD_TOO_LARGE => ApiErrorCode::PayloadTooLarge,
        StatusCode::UNPROCESSABLE_ENTITY => ApiErrorCode::UnprocessableEntity,
        StatusCode::SERVICE_UNAVAILABLE => ApiErrorCode::ServiceBusy,
        status if status.is_client_error() => ApiErrorCode::InvalidRequest,
        _ => ApiErrorCode::InternalError,
    };
    let message = match code {
        ApiErrorCode::InvalidRequest => "invalid request",
        ApiErrorCode::NotFound => "not found",
        ApiErrorCode::Gone => "gone",
        ApiErrorCode::PreconditionFailed => "precondition failed",
        ApiErrorCode::Conflict => "conflict",
        ApiErrorCode::MethodNotAllowed => "method not allowed",
        ApiErrorCode::UnsupportedMediaType => "unsupported media type",
        ApiErrorCode::PayloadTooLarge => "payload too large",
        ApiErrorCode::UnprocessableEntity => "unprocessable entity",
        ApiErrorCode::ServiceBusy => "service temporarily unavailable",
        _ => "internal error",
    };
    ApiError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdefABCDEF0123456789";

    #[test]
    fn structured_error_details_redact_tmp_document_secrets() {
        let details = ApiErrorDetails {
            op_index: Some(3),
            op: Some(format!("operation {SECRET}")),
            target: Some(ApiErrorTarget {
                kind: SECRET.to_string(),
                id: SECRET.to_string(),
            }),
            field: Some(SECRET.to_string()),
            value: Some(format!("rejected {SECRET}")),
            current_value: Some(SECRET.to_string()),
            allowed_values: vec![SECRET.to_string(), "safe".to_string()],
        }
        .redacted();

        assert_eq!(details.op_index, Some(3));
        assert_eq!(details.op.as_deref(), Some("operation <tmp-secret>"));
        assert_eq!(
            details.target,
            Some(ApiErrorTarget {
                kind: "<tmp-secret>".to_string(),
                id: "<tmp-secret>".to_string(),
            })
        );
        assert_eq!(details.field.as_deref(), Some("<tmp-secret>"));
        assert_eq!(details.value.as_deref(), Some("rejected <tmp-secret>"));
        assert_eq!(details.current_value.as_deref(), Some("<tmp-secret>"));
        assert_eq!(details.allowed_values, ["<tmp-secret>", "safe"]);
    }
}
