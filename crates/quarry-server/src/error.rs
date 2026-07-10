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
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ApiErrorResponse {
    pub code: ApiErrorCode,
    pub retryable: bool,
    pub message: String,
}

impl ApiError {
    pub(crate) fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
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
