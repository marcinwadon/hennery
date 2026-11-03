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
        let path = req.uri().path();
        let seconds = if path.starts_with("/api/auth/") || matches!(path, "/api/setup" | "/api/hosts/enroll") {
            5
        } else {
            30
        };
        match tokio::time::timeout(
            std::time::Duration::from_secs(seconds),
            axum::Json::<T>::from_request(req, state),
        )
        .await
        {
            Ok(Ok(axum::Json(value))) => Ok(Self(value)),
            Ok(Err(rejection)) => Err(ApiJsonRejection::from(rejection)),
            Err(_) => Err(ApiJsonRejection(StatusCode::REQUEST_TIMEOUT)),
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
            StatusCode::REQUEST_TIMEOUT => ("request_timeout", "request body deadline exceeded"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE => ("unsupported_media_type", "the body must be application/json"),
            StatusCode::PAYLOAD_TOO_LARGE => ("body_too_large", "the request body is too large"),
            _ => ("invalid_body", "the request body is not what this route takes"),
        };
        let mut response = error(self.0, code, message);
        if self.0 == StatusCode::REQUEST_TIMEOUT {
            response
                .headers_mut()
                .insert(axum::http::header::CONNECTION, "close".parse().expect("fixed header"));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn a_stalled_auth_body_has_one_deadline_and_a_fixed_error() {
        let body =
            axum::body::Body::from_stream(futures::stream::pending::<Result<axum::body::Bytes, std::io::Error>>());
        let request = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(body)
            .unwrap();
        let before = tokio::time::Instant::now();
        let result = ApiJson::<serde_json::Value>::from_request(request, &()).await;
        let Err(error) = result else {
            panic!("stalled body was accepted")
        };
        assert_eq!(tokio::time::Instant::now() - before, std::time::Duration::from_secs(5));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["code"], "request_timeout");
    }
}
