use axum::http::StatusCode;
use axum::Json;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("WebSocket error: {0}")]
    WebSocketError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::WebSocketError(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            AppError::SerializationError(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            AppError::InternalError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };

        (status, Json(json!({ "error": message } ))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
