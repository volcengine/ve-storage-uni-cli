use crate::{crypto::CryptoError, error::ProtocolError, storage::StorageError};
use axum::{http::StatusCode, response::Response};

/// Sanitized lifecycle failure; never contains secrets or backend error strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginError {
    InvalidConfiguration,
    InvalidRequest,
    AuthenticationRequired,
    Unavailable,
    ProviderContract,
    IdempotencyConflict,
    LoginPending,
    LoginDenied,
    LoginExpired,
    LoginOutcomeUnknown,
    AccessDenied,
    ScopeMissing,
    ClaimRecoveryExpired,
    RefreshInProgress,
    RefreshOutcomeUnknown,
    RefreshRecoveryExpired,
    GenerationMismatch,
    BrokerExpired,
    RefreshProviderContract,
}
impl LoginError {
    pub(crate) fn response(self, request_id: &str) -> Response {
        let (status, code, message) = self.details();
        ProtocolError::custom(request_id, status, code, message, self == Self::Unavailable)
    }
    fn details(self) -> (StatusCode, &'static str, &'static str) {
        use StatusCode as S;
        match self {
            Self::InvalidRequest => (S::BAD_REQUEST, "invalid_request", "The request is invalid."),
            Self::AuthenticationRequired => (
                S::UNAUTHORIZED,
                "authentication_required",
                "Authentication is required.",
            ),
            Self::IdempotencyConflict => (
                S::CONFLICT,
                "idempotency_conflict",
                "The operation conflicts with an existing request.",
            ),
            Self::LoginPending => (S::CONFLICT, "login_pending", "Authorization is pending."),
            Self::LoginDenied => (S::FORBIDDEN, "login_denied", "Authorization was denied."),
            Self::LoginExpired => (S::GONE, "login_expired", "The login has expired."),
            Self::LoginOutcomeUnknown => (
                S::CONFLICT,
                "login_outcome_unknown",
                "The login outcome is unknown.",
            ),
            Self::AccessDenied => (S::FORBIDDEN, "access_denied", "Access is denied."),
            Self::ScopeMissing => (S::FORBIDDEN, "scope_missing", "Required scope is missing."),
            Self::ClaimRecoveryExpired => (
                S::CONFLICT,
                "claim_recovery_expired",
                "The claim recovery window has expired.",
            ),
            Self::ProviderContract => (
                S::SERVICE_UNAVAILABLE,
                "provider_contract_error",
                "The provider response is invalid.",
            ),
            _ => self.refresh_details(),
        }
    }
    fn refresh_details(self) -> (StatusCode, &'static str, &'static str) {
        use StatusCode as S;
        match self {
            Self::RefreshInProgress => (
                S::CONFLICT,
                "refresh_in_progress",
                "A refresh is in progress.",
            ),
            Self::RefreshOutcomeUnknown => (
                S::CONFLICT,
                "refresh_outcome_unknown",
                "The refresh outcome is unknown.",
            ),
            Self::RefreshRecoveryExpired => (
                S::CONFLICT,
                "refresh_recovery_expired",
                "The refresh recovery window has expired.",
            ),
            Self::GenerationMismatch => (
                S::CONFLICT,
                "credential_generation_mismatch",
                "The credential generation does not match.",
            ),
            Self::BrokerExpired => (
                S::UNAUTHORIZED,
                "broker_expired",
                "The authorization has expired.",
            ),
            Self::RefreshProviderContract => (
                S::BAD_GATEWAY,
                "provider_contract_error",
                "The provider response is invalid.",
            ),
            _ => (
                S::SERVICE_UNAVAILABLE,
                "upstream_unavailable",
                "Authorization service is not ready.",
            ),
        }
    }
}
impl From<StorageError> for LoginError {
    fn from(_: StorageError) -> Self {
        Self::Unavailable
    }
}
impl From<CryptoError> for LoginError {
    fn from(_: CryptoError) -> Self {
        Self::Unavailable
    }
}
impl From<ProtocolError> for LoginError {
    fn from(error: ProtocolError) -> Self {
        match error {
            ProtocolError::InvalidRequest => Self::InvalidRequest,
            ProtocolError::AuthenticationRequired => Self::AuthenticationRequired,
            ProtocolError::Unavailable => Self::Unavailable,
        }
    }
}
