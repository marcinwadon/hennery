//! Walking-skeleton request auth: a shared development bearer token.
//! Replaced by operator sessions and passkeys (kernel spec §3).

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

#[derive(Clone)]
pub struct DevToken(pub Arc<str>);

impl DevToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(Arc::from(token.into()))
    }

    /// Constant-time comparison.
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

/// Axum middleware: require `Authorization: Bearer <dev token>`.
pub async fn require_bearer(State(token): State<DevToken>, req: Request, next: Next) -> Response {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(p) if token.matches(p) => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::DevToken;

    #[test]
    fn token_comparison_rejects_prefixes_and_different_lengths() {
        let t = DevToken::new("secret");
        assert!(t.matches("secret"));
        assert!(!t.matches("secre"));
        assert!(!t.matches("secret2"));
        assert!(!t.matches(""));
    }
}
