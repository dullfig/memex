use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Unified API error type.
pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    Forbidden(String),
    BadGateway(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, "forbidden", msg),
            Self::BadGateway(msg) => (StatusCode::BAD_GATEWAY, "bad_gateway", msg),
            Self::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "internal", msg),
        };

        let body = json!({ "error": message, "code": code });
        (status, axum::Json(body)).into_response()
    }
}

impl From<memex_ingest::IngestError> for ApiError {
    fn from(e: memex_ingest::IngestError) -> Self {
        match &e {
            memex_ingest::IngestError::ConsentDenied(_) => Self::Forbidden(e.to_string()),
            memex_ingest::IngestError::ShardNotFound(_) => Self::NotFound(e.to_string()),
            memex_ingest::IngestError::CortexUnavailable => Self::BadGateway(e.to_string()),
            memex_ingest::IngestError::Internal(_) => Self::Internal(e.to_string()),
        }
    }
}

impl From<memex_retrieval::RetrievalError> for ApiError {
    fn from(e: memex_retrieval::RetrievalError) -> Self {
        match &e {
            memex_retrieval::RetrievalError::NamespaceMismatch => Self::BadRequest(e.to_string()),
            memex_retrieval::RetrievalError::ShardNotFound(_) => Self::NotFound(e.to_string()),
            memex_retrieval::RetrievalError::CortexUnavailable => Self::BadGateway(e.to_string()),
            memex_retrieval::RetrievalError::Internal(_) => Self::Internal(e.to_string()),
        }
    }
}
