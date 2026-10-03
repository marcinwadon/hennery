//! A host's agents on the collector (plan 4d-B1-i): the latest report, from
//! a reconciled `hello` or a `probe_agents`, stored in the host registry;
//! and `GET /api/hosts/{id}/agents[?refresh=1]`, which answers from the
//! store, after one probe of the host when asked to refresh. Concurrent
//! refreshes of a host share one probe, bounded by `AGENTS_PROBE_TIMEOUT`.

use crate::AppState;
use crate::api::{error, internal};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hennery_kernel::hosts::{AgentsRecord, ReportedIn};
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::rest::HostAgents;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;

/// How long a probe of a host's agents waits for its answer: the host's
/// own budget (15 s) and some time for the answer to arrive.
pub const AGENTS_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// The probes of agents under way, by host: a refresh that finds one joins
/// it.
#[derive(Default)]
pub struct Refreshes(Mutex<HashMap<String, watch::Receiver<bool>>>);

/// Removes its host's entry when its probe is done, however it ends.
struct Running<'a> {
    refreshes: &'a Refreshes,
    host_id: String,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.refreshes.0.lock().expect("refreshes lock").remove(&self.host_id);
    }
}

impl Refreshes {
    /// Probe `host_id`'s agents and store the answer, or join the probe
    /// already under way; return once it is done. The probe runs in a task
    /// of its own: a caller that goes away does not stop its answer from
    /// being stored. A host that is not connected, or cannot be probed, is
    /// not asked (`Hub::probe`).
    pub async fn refresh(state: &AppState, host_id: &str) {
        let mut done = {
            let mut running = state.agent_refreshes.0.lock().expect("refreshes lock");
            match running.get(host_id) {
                Some(done) => done.clone(),
                None => {
                    let (tx, done) = watch::channel(false);
                    running.insert(host_id.to_string(), done.clone());
                    let (state, host_id) = (state.clone(), host_id.to_string());
                    tokio::spawn(async move {
                        let running = Running {
                            refreshes: &state.agent_refreshes,
                            host_id: host_id.clone(),
                        };
                        probe_and_store(&state, &host_id).await;
                        // Gone before the waiters wake: a refresh after
                        // this one probes again.
                        drop(running);
                        let _ = tx.send(true);
                    });
                    done
                }
            }
        };
        // The probe is bounded itself; this only bounds a waiter whose task
        // was lost.
        let _ = tokio::time::timeout(state.agents_probe_timeout, done.wait_for(|done| *done)).await;
    }
}

/// One `probe_agents`, its answer stored.
async fn probe_and_store(state: &AppState, host_id: &str) {
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ProbeAgents {
        request_id: request_id.clone(),
    };
    match state
        .hub
        .probe(host_id, &request_id, frame, state.agents_probe_timeout)
        .await
    {
        Ok(HostFrame::Agents { agents, runtime, .. }) => {
            if let Err(err) = state
                .hosts
                .record_agents(host_id, ReportedIn::Probe, agents.0, runtime.0, unix_now())
            {
                tracing::warn!(%host_id, error = %err, "storing the host's agents failed");
            }
        }
        Ok(_) => tracing::warn!(%host_id, "the host answered probe_agents with another frame"),
        Err(err) => tracing::info!(%host_id, ?err, "no answer to probe_agents"),
    }
}

/// A host's report as the API shows it.
fn host_agents(state: &AppState, host_id: String, record: AgentsRecord) -> HostAgents {
    HostAgents {
        live: state.hub.is_ready(&host_id),
        host_id,
        agents: record.agents,
        runtime: record.runtime,
        reported_at: record.reported_at.map(rfc3339),
        source: record.source,
    }
}

#[derive(Deserialize)]
pub(crate) struct AgentsQuery {
    refresh: Option<String>,
}

/// `GET /api/hosts/{id}/agents[?refresh=1]`: the host's latest report,
/// whatever its age and whether the host is connected; with `refresh=1`,
/// after one probe of it (or that probe's timeout). 404 only for a host
/// that is not the owner's, decided from the registry before any report is
/// read or any probe sent. Never cached: a note can name a path on the
/// host.
pub(crate) async fn get_agents(
    State(state): State<AppState>,
    Path(host_id): Path<String>,
    query: Result<Query<AgentsQuery>, QueryRejection>,
) -> Response {
    let refresh = match query.as_ref().map(|Query(q)| q.refresh.as_deref()) {
        Ok(None | Some("0")) => false,
        Ok(Some("1")) => true,
        _ => return error(StatusCode::BAD_REQUEST, "invalid", "`refresh`, if given, is 0 or 1"),
    };
    match state.hosts.host(&host_id) {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => return internal(err),
    }
    if refresh {
        Refreshes::refresh(&state, &host_id).await;
    }
    match state.hosts.agents(&host_id) {
        Ok(Some(record)) => {
            let mut response = Json(host_agents(&state, host_id, record)).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => internal(err),
    }
}
