//! Error type for API handlers: return [`ApiResult`] and use `?`. Anything
//! that converts to `anyhow::Error` becomes a 500 carrying its full context
//! chain; the other variants map to their status codes.

use std::borrow::Cow;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug)]
pub enum ApiError {
    NotFound(&'static str),
    BadRequest(Cow<'static, str>),
    Conflict(&'static str),
    BadGateway(String),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg).into_response(),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg).into_response(),
            Self::BadGateway(msg) => (StatusCode::BAD_GATEWAY, msg).into_response(),
            Self::Internal(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()
            }
        }
    }
}
