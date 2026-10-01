//! The JSON body extractor of every route that reads one. axum's own
//! `Json` answers a body it refuses with plain text carrying serde's
//! message, which quotes the request back and changes with serde. This one
//! keeps axum's status and answers a fixed `ApiError` instead.

use crate::auth_api::error;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

/// `axum::Json`, with its rejections answered as `ApiError`s.
pub struct ApiJson<T>(pub T);

/// A body `ApiJson` refused, answered by its status alone.
pub struct ApiJsonRejection(StatusCode);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiJsonRejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiJsonRejection::from(rejection)),
        }
    }
}

impl From<JsonRejection> for ApiJsonRejection {
    fn from(rejection: JsonRejection) -> Self {
        // The status only: serde's text quotes what the client sent, which
        // may be a secret in a field of the wrong type.
        tracing::debug!(status = %rejection.status(), "request body refused");
        Self(rejection.status())
    }
}

impl IntoResponse for ApiJsonRejection {
    fn into_response(self) -> Response {
        let (code, message) = match self.0 {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => ("unsupported_media_type", "the body must be application/json"),
            StatusCode::PAYLOAD_TOO_LARGE => ("body_too_large", "the request body is too large"),
            _ => ("invalid_body", "the request body is not what this route takes"),
        };
        error(self.0, code, message)
    }
}
