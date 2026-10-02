//! Resolving a typed path through its host (kernel spec §5.2, §5.4): the
//! host canonicalises, where the filesystem is, and the collector checks
//! the answer's form before anything is matched against it.

use crate::AppState;
use crate::api::error;
use crate::hub::RequestError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hennery_kernel::hats::is_canonical;
use hennery_proto::frames::{CollectorFrame, HostFrame};
use std::time::Duration;

/// `resolve_path`'s bound: past the host connection's read deadline, as
/// every request's (ACP core §3.4).
pub(crate) const RESOLVE_TIMEOUT: Duration = Duration::from_secs(50);

const _: () = assert!(
    RESOLVE_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis(),
    "resolve_path's timeout must exceed the host connection's read deadline"
);

/// A path as its host resolved it, its form checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnHost {
    pub canonical: String,
    pub exists: bool,
    pub is_dir: bool,
}

/// Why a path did not resolve, and the answer each gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotResolved {
    /// 409 `host_offline`: not connected, or not reconciled.
    HostOffline,
    /// 400 `invalid` with the host's message: it refused the path. Any
    /// other code the host chose is answered `host_refused` (the review's
    /// P5): a host must not make the collector say, for instance,
    /// `step_up_required`.
    Refused { code: String, message: String },
    /// 503 `delivery_unknown`.
    DeliveryUnknown,
    /// 502 `bad_host_answer`: an answer not in canonical form.
    BadAnswer,
}

impl IntoResponse for NotResolved {
    fn into_response(self) -> Response {
        match self {
            Self::HostOffline => error(StatusCode::CONFLICT, "host_offline", "the host is not connected"),
            Self::Refused { code, message } if code == "invalid" => error(StatusCode::BAD_REQUEST, "invalid", message),
            Self::Refused { code, message } => {
                tracing::warn!(?code, "resolve_path refused with a code it has no business with");
                error(StatusCode::BAD_GATEWAY, "host_refused", message)
            }
            Self::DeliveryUnknown => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery_unknown",
                "host disconnected; delivery unknown",
            ),
            Self::BadAnswer => error(
                StatusCode::BAD_GATEWAY,
                "bad_host_answer",
                "the host's answer is not a canonical path",
            ),
        }
    }
}

/// Ask `host_id` to resolve `path`.
pub(crate) async fn resolve_on_host(state: &AppState, host_id: &str, path: &str) -> Result<OnHost, NotResolved> {
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ResolvePath {
        request_id: request_id.clone(),
        path: path.to_string(),
    };
    match state.hub.probe(host_id, &request_id, frame, RESOLVE_TIMEOUT).await {
        Ok(HostFrame::ResolvedPath {
            canonical,
            exists,
            is_dir,
            ..
        }) if is_canonical(&canonical) => Ok(OnHost {
            canonical,
            exists,
            is_dir,
        }),
        Ok(other) => {
            tracing::warn!(
                %host_id,
                answer = ?other,
                "resolve_path answered with a path not in canonical form, or with another frame"
            );
            Err(NotResolved::BadAnswer)
        }
        Err(RequestError::NotConnected) => Err(NotResolved::HostOffline),
        Err(RequestError::Rejected { code, message }) => Err(NotResolved::Refused { code, message }),
        Err(RequestError::DeliveryUnknown) => Err(NotResolved::DeliveryUnknown),
    }
}
