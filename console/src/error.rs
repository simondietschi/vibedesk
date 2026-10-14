use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Fehler einer API-Anfrage. Die Meldung geht an den Benutzer, interne
/// Details nur ins Log.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Nicht angemeldet")
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "Keine Berechtigung")
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "Nicht gefunden")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("Datenbankfehler: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Interner Fehler")
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("Interner Fehler: {e:#}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "Interner Fehler")
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
