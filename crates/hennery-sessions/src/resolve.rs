//! Resolving a typed path through its host (kernel spec §5.2, §5.4): the
//! host canonicalises, where the filesystem is, and the collector checks
//! the answer's form before anything is matched against it.

use crate::AppState;
use crate::api::error;
use crate::hub::RequestError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hennery_kernel::hats::is_canonical;
use hennery_kernel::hosts::is_displayable_text;
use hennery_proto::frames::{CollectorFrame, HostFrame};

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
    /// 400 `invalid` with the host's message: it refused the path. `busy`
    /// is the probes' own (503). Any other code the host chose is answered
    /// `host_refused` (the review's P5): a host must not make the
    /// collector say, for instance, `step_up_required`.
    Refused { code: String, message: String },
    /// 503 `busy`: the connection has its most probes in flight, or the
    /// host its most resolutions; nothing was resolved.
    Busy,
    /// 503 `no_answer`: the probe timed out, keeping the connection, or it
    /// dropped before answering; either way, no answer came.
    NoAnswer,
    /// 502 `bad_host_answer`: an answer not in canonical form.
    BadAnswer,
    /// 409 `resolve_unsupported`: connected, but its build does not
    /// announce `Capability::ResolvePath` (ACP core §3.3: the collector
    /// never sends a frame that needs a capability to a host that lacks
    /// it).
    Unsupported,
}

impl IntoResponse for NotResolved {
    fn into_response(self) -> Response {
        match self {
            Self::HostOffline => error(StatusCode::CONFLICT, "host_offline", "the host is not connected"),
            // The host's own text is shown only within bounds (6c's A6): a
            // host that cannot be trusted with its workspace roots is not
            // trusted with the bytes it puts in an error either.
            Self::Refused { code, message } if code == "invalid" => {
                let message = if is_displayable_text(&message, 256) {
                    message
                } else {
                    "the host refused the path".to_string()
                };
                error(StatusCode::BAD_REQUEST, "invalid", message)
            }
            Self::Refused { code, .. } if code == "busy" => Self::Busy.into_response(),
            Self::Refused { code, message } => {
                tracing::warn!(
                    ?code,
                    ?message,
                    "resolve_path refused with a code it has no business with"
                );
                error(StatusCode::BAD_GATEWAY, "host_refused", "the host refused the request")
            }
            Self::NoAnswer => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "no_answer",
                "the host did not answer in time",
            ),
            Self::BadAnswer => error(
                StatusCode::BAD_GATEWAY,
                "bad_host_answer",
                "the host's answer is not a canonical path",
            ),
            Self::Unsupported => error(
                StatusCode::CONFLICT,
                "resolve_unsupported",
                "this host cannot resolve paths; update it",
            ),
            Self::Busy => error(StatusCode::SERVICE_UNAVAILABLE, "busy", "the host is busy; try again"),
        }
    }
}

/// Ask `host_id` to resolve `path`, a probe (ACP core §3.3): only to a
/// host that announced `resolve_path` (`Hub::probe` checks it), with the
/// probes' own timeout, which keeps the connection.
pub(crate) async fn resolve_on_host(state: &AppState, host_id: &str, path: &str) -> Result<OnHost, NotResolved> {
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ResolvePath {
        request_id: request_id.clone(),
        path: path.to_string(),
    };
    match state.hub.probe(host_id, &request_id, frame, state.probe_timeout).await {
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
        Err(RequestError::DeliveryUnknown) => Err(NotResolved::NoAnswer),
        Err(RequestError::Unsupported) => Err(NotResolved::Unsupported),
        Err(RequestError::Busy) => Err(NotResolved::Busy),
    }
}
