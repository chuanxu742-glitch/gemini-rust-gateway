use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, GatewayError>;

/// Domain error type. Variants are split into two groups:
///
/// - **User-facing** (`InvalidRequest`, `Auth`, `FeatureDisabled`, `TaskNotFound`,
///   `QueueFull`, `Timeout`, `Provider`) carry messages that are safe to expose
///   to API clients.
/// - **Internal** (`Internal`) wraps infrastructure failures (IO, JSON,
///   upstream HTTP, dotenv). The display message is intentionally generic; the
///   underlying source is logged via `tracing` and never returned to the
///   caller.
#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    FeatureDisabled(String),
    #[error("task not found")]
    TaskNotFound,
    #[error("generation task queue is full")]
    QueueFull,
    #[error("provider request timed out")]
    Timeout,
    #[error("provider error: {0}")]
    Provider(String),
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl GatewayError {
    fn status_and_kind(&self) -> (StatusCode, &'static str) {
        match self {
            Self::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::Auth(_) => (StatusCode::UNAUTHORIZED, "authentication_error"),
            Self::FeatureDisabled(_) => (StatusCode::FORBIDDEN, "feature_disabled"),
            Self::TaskNotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::QueueFull => (StatusCode::TOO_MANY_REQUESTS, "queue_full"),
            Self::Timeout => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
            Self::Provider(_) => (StatusCode::BAD_GATEWAY, "provider_error"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "gateway_error"),
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, kind) = self.status_and_kind();
        if let Self::Internal(source) = &self {
            tracing::error!(error = %source, "internal gateway error");
        }
        let message = match &self {
            Self::Internal(_) => "internal gateway error".to_owned(),
            other => other.to_string(),
        };
        (
            status,
            Json(json!({"error": {"message": message, "type": kind}})),
        )
            .into_response()
    }
}

impl From<std::io::Error> for GatewayError {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(anyhow::Error::new(error))
    }
}

impl From<serde_json::Error> for GatewayError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(anyhow::Error::new(error))
    }
}

impl From<reqwest::Error> for GatewayError {
    fn from(error: reqwest::Error) -> Self {
        Self::Internal(anyhow::Error::new(error))
    }
}

impl From<dotenvy::Error> for GatewayError {
    fn from(error: dotenvy::Error) -> Self {
        Self::Internal(anyhow::Error::new(error))
    }
}

impl From<anyhow::Error> for GatewayError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_error_hides_source_message() {
        let error = GatewayError::Internal(anyhow::anyhow!("secret database password leaked"));
        let display = error.to_string();
        assert_eq!(display, "internal error");
        assert!(!display.contains("secret"));
    }

    #[test]
    fn user_error_preserves_message() {
        let error = GatewayError::InvalidRequest("prompt too long".into());
        assert_eq!(error.to_string(), "prompt too long");
    }
}
