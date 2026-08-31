//! The error response envelope shared by every route.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use design_model::ValidationError;

/// Builds the `{ "error": { "message", "details" } }` envelope.
pub fn error_response(status: StatusCode, message: &str, details: Vec<String>) -> Response {
    let body = serde_json::json!({ "error": { "message": message, "details": details } });
    (status, Json(body)).into_response()
}

/// 422 with one detail line per validation error.
pub fn validation_failed(errors: &[ValidationError]) -> Response {
    let details: Vec<String> = errors.iter().map(ToString::to_string).collect();
    tracing::info!(error_count = details.len(), "rejected invalid design");
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "design failed validation",
        details,
    )
}

/// 422 with one detail line per deck validation error.
pub fn deck_validation_failed(errors: &[ValidationError]) -> Response {
    let details: Vec<String> = errors.iter().map(ToString::to_string).collect();
    tracing::info!(error_count = details.len(), "rejected invalid deck");
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "deck failed validation",
        details,
    )
}

/// 404 for a deck id with no file behind it.
pub fn deck_not_found(id: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &format!("no deck with id `{id}`"),
        Vec::new(),
    )
}

/// 400 for a deck id that is not kebab-case.
pub fn invalid_deck_id(id: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        &format!("invalid deck id `{id}`: use lowercase letters, digits, and hyphens"),
        Vec::new(),
    )
}

/// 422 with one detail line per social validation error.
pub fn social_validation_failed(errors: &[ValidationError]) -> Response {
    let details: Vec<String> = errors.iter().map(ToString::to_string).collect();
    tracing::info!(error_count = details.len(), "rejected invalid social");
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "social failed validation",
        details,
    )
}

/// 404 for a social id with no file behind it.
pub fn social_not_found(id: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &format!("no social with id `{id}`"),
        Vec::new(),
    )
}

/// 400 for a social id that is not kebab-case.
pub fn invalid_social_id(id: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        &format!("invalid social id `{id}`: use lowercase letters, digits, and hyphens"),
        Vec::new(),
    )
}

/// 422 with one detail line per document validation error.
pub fn document_validation_failed(errors: &[ValidationError]) -> Response {
    let details: Vec<String> = errors.iter().map(ToString::to_string).collect();
    tracing::info!(error_count = details.len(), "rejected invalid document");
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "document failed validation",
        details,
    )
}

/// 404 for a document id with no file behind it.
pub fn document_not_found(id: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &format!("no document with id `{id}`"),
        Vec::new(),
    )
}

/// 400 for a document id that is not kebab-case.
pub fn invalid_document_id(id: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        &format!("invalid document id `{id}`: use lowercase letters, digits, and hyphens"),
        Vec::new(),
    )
}

/// 404 for a design id with no file behind it.
pub fn design_not_found(id: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &format!("no design with id `{id}`"),
        Vec::new(),
    )
}

/// 400 for an id that is not kebab-case.
pub fn invalid_design_id(id: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        &format!("invalid design id `{id}`: use lowercase letters, digits, and hyphens"),
        Vec::new(),
    )
}

/// 500 that logs the cause. The server is a local tool, so the message
/// carries the cause to the caller: agents fix designs from it.
pub fn internal_error(error: &anyhow::Error) -> Response {
    tracing::error!(%error, "request failed");
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &format!("internal error: {error:#}"),
        Vec::new(),
    )
}
