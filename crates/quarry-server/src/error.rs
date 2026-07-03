use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use quarry_core::QuarryError;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug)]
pub struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub(crate) error: String,
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
            QuarryError::Gone(_) => StatusCode::GONE,
            QuarryError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            QuarryError::Conflict(_) => StatusCode::CONFLICT,
            QuarryError::Busy(_) => StatusCode::SERVICE_UNAVAILABLE,
            QuarryError::InvalidPath(_) => StatusCode::BAD_REQUEST,
            QuarryError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            QuarryError::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            QuarryError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
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
        StatusCode::GONE => "gone",
        StatusCode::PRECONDITION_FAILED => "precondition_failed",
        StatusCode::CONFLICT => "conflict",
        StatusCode::SERVICE_UNAVAILABLE => "busy",
        StatusCode::BAD_REQUEST => "bad_request",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        _ => "internal_error",
    }
}
