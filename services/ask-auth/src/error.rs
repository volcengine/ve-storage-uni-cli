use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

#[derive(Clone, Copy)]
pub(crate) enum ProtocolError {
    InvalidRequest,
    AuthenticationRequired,
    Unavailable,
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    schema_version: u8,
    request_id: &'a str,
    error: ErrorDetails,
}

#[derive(Serialize)]
struct ErrorDetails {
    code: &'static str,
    message: &'static str,
    retryable: bool,
    retry_after_seconds: Option<u16>,
}

impl ProtocolError {
    pub(crate) fn custom(
        request_id: &str,
        status: StatusCode,
        code: &'static str,
        message: &'static str,
        retryable: bool,
    ) -> Response {
        (
            status,
            Json(ErrorResponse {
                schema_version: 1,
                request_id,
                error: ErrorDetails {
                    code,
                    message,
                    retryable,
                    retry_after_seconds: None,
                },
            }),
        )
            .into_response()
    }
    pub(crate) fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::AuthenticationRequired => StatusCode::UNAUTHORIZED,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub(crate) fn response(self, request_id: &str) -> Response {
        self.with_status(request_id, self.status())
    }

    pub(crate) fn with_status(self, request_id: &str, status: StatusCode) -> Response {
        let (code, message) = match self {
            Self::InvalidRequest => ("invalid_request", "The request is invalid."),
            Self::AuthenticationRequired => {
                ("authentication_required", "Authentication is required.")
            }
            Self::Unavailable => (
                "upstream_unavailable",
                "Authorization service is not ready.",
            ),
        };
        let payload = ErrorResponse {
            schema_version: 1,
            request_id,
            error: ErrorDetails {
                code,
                message,
                retryable: matches!(self, Self::Unavailable),
                retry_after_seconds: None,
            },
        };
        (status, Json(payload)).into_response()
    }
}
