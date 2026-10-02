//! The project picker's routes (ACP core §7, §9; plan 6c):
//! `GET /api/hosts/{id}/projects` and `GET /api/hosts/{id}/browse?path=`,
//! answered by the host's probes, and the enumeration cache.

use crate::AppState;
use crate::api::{error, internal};
use crate::hub::RequestError;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use hennery_kernel::hosts::{MAX_ROOTS, is_displayable_path, is_displayable_text};
use hennery_proto::frames::{Capability, CollectorFrame, DirEntry, HostFrame, Project};
use hennery_proto::rest::{DirectoryListing, HostProjects, RecentProject};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a probe waits for its reply (ACP core §3.4).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long an enumeration is served from the cache (ACP core §7).
pub const CACHE_TTL: Duration = Duration::from_secs(60);

/// The most repositories an answer keeps: the host's 500 per root, for
/// every root it may have (the review's A6).
const MAX_ITEMS: usize = 500 * MAX_ROOTS;

/// The most entries a listing keeps (decision 4).
const MAX_ENTRIES: usize = 1000;

/// The longest entry name kept, in bytes.
const MAX_NAME: usize = 255;

/// An enumeration as it is answered and cached: the repositories, whether
/// the host cut it short, and its home directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Enumeration {
    pub items: Vec<Project>,
    pub partial: bool,
    pub home: Option<String>,
}

struct Cached {
    conn_id: u64,
    at: Instant,
    enumeration: Enumeration,
}

/// Enumerations by (owner, host), each served for `ttl`, and only while
/// the host's connection is the one it was filled from (decision 10): a
/// reconnect may bring other roots. Errors and listings are never cached;
/// recents are never kept here (the review's A7).
pub struct ProjectsCache {
    ttl: Duration,
    entries: Mutex<HashMap<(String, String), Cached>>,
}

impl ProjectsCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The enumeration of `owner`'s `host_id`, filled from connection
    /// `conn_id` less than `ttl` ago.
    pub fn get(&self, owner: &str, host_id: &str, conn_id: u64) -> Option<Enumeration> {
        let entries = self.entries.lock().expect("projects cache lock");
        entries
            .get(&(owner.to_string(), host_id.to_string()))
            .filter(|cached| cached.conn_id == conn_id && cached.at.elapsed() < self.ttl)
            .map(|cached| cached.enumeration.clone())
    }

    pub fn put(&self, owner: &str, host_id: &str, conn_id: u64, enumeration: Enumeration) {
        self.entries.lock().expect("projects cache lock").insert(
            (owner.to_string(), host_id.to_string()),
            Cached {
                conn_id,
                at: Instant::now(),
                enumeration,
            },
        );
    }

    /// Drop every owner's enumeration of `host_id`: it connected again.
    pub fn forget(&self, host_id: &str) {
        self.entries
            .lock()
            .expect("projects cache lock")
            .retain(|(_, host), _| host != host_id);
    }
}

/// Both routes need the operator's session (kernel spec §3.3); they change
/// nothing, so no step-up.
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/hosts/{id}/projects", get(projects))
        .route("/api/hosts/{id}/browse", get(browse));
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}

/// The paths these answers carry are the host's, and private: no cache
/// keeps them (the review's O5).
fn no_store(body: impl serde::Serialize) -> Response {
    let mut response = Json(body).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// What both routes check before anything is sent, in this order (the
/// review's A7): the host is the owner's (404 `not_found`); connected and
/// reconciled (409 `host_offline`); and it has the `projects` capability
/// (409 `projects_unsupported`). The connection's id, for the cache, or
/// the answer. `Hub::probe` checks the last two again, on the connection it
/// sends on.
fn reachable(state: &AppState, host_id: &str) -> Result<u64, Box<Response>> {
    match state.hosts.host(host_id) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(Box::new(error(StatusCode::NOT_FOUND, "not_found", "no such host"))),
        Err(err) => return Err(Box::new(internal(err))),
    }
    let Some(conn_id) = state.hub.routable_conn(host_id) else {
        return Err(Box::new(probe_failed(RequestError::NotConnected)));
    };
    if !state.hub.has_capability(host_id, Capability::Projects) {
        return Err(Box::new(probe_failed(RequestError::Unsupported)));
    }
    Ok(conn_id)
}

/// The answer to a probe that produced no reply (decision 2).
fn probe_failed(err: RequestError) -> Response {
    match err {
        RequestError::NotConnected => error(StatusCode::CONFLICT, "host_offline", "the host is not connected"),
        // A probe carries no servers: `McpUndeliverable` cannot happen.
        RequestError::Unsupported | RequestError::McpUndeliverable => error(
            StatusCode::CONFLICT,
            "projects_unsupported",
            "this host cannot list or browse projects; update it",
        ),
        RequestError::Busy => busy(),
        RequestError::DeliveryUnknown => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_answer",
            "the host did not answer in time",
        ),
        RequestError::Rejected { code, .. } => host_refused(&code),
    }
}

fn busy() -> Response {
    error(StatusCode::SERVICE_UNAVAILABLE, "busy", "the host is busy; try again")
}

fn bad_reply() -> Response {
    error(
        StatusCode::BAD_GATEWAY,
        "bad_reply",
        "the host's answer could not be used",
    )
}

/// A host's refusal (decision 3). Only codes this collector knows are
/// passed on, each with the collector's own message: the host's text is
/// not shown (the review's A6).
fn host_refused(code: &str) -> Response {
    let (status, message) = match code {
        "invalid" => (
            StatusCode::BAD_REQUEST,
            "give an absolute path without `.` or `..` segments",
        ),
        "not_a_directory" => (StatusCode::BAD_REQUEST, "the path is not a directory"),
        "outside_workspace" => (
            StatusCode::FORBIDDEN,
            "the path is outside the host's workspace roots and home directory",
        ),
        "permission_denied" => (StatusCode::FORBIDDEN, "the host may not read this directory"),
        "path_not_found" => (StatusCode::NOT_FOUND, "no such directory on the host"),
        "unreadable" => (StatusCode::BAD_GATEWAY, "the host could not read this directory"),
        "busy" => return busy(),
        _ => return bad_reply(),
    };
    error(status, code, message)
}

#[derive(Deserialize)]
struct ProjectsQuery {
    path: Option<String>,
}

/// `GET /api/hosts/{id}/projects?path=`: the recents of the hat `path`
/// resolves to (the host's default hat without one), read afresh (kernel
/// spec §5.3; the review's A7), and the repositories under the host's
/// workspace roots (ACP core §7), from the cache or a `list_projects`.
async fn projects(
    State(state): State<AppState>,
    Path(host_id): Path<String>,
    query: Result<Query<ProjectsQuery>, QueryRejection>,
) -> Response {
    // Checked before anything else, whatever the host's state (Task 6's
    // review, A2). Canonical, as hat rules are: lexically, before hats 5b.
    let path = match query {
        Ok(Query(ProjectsQuery { path: None })) => None,
        Ok(Query(ProjectsQuery { path: Some(path) }))
            if is_askable(&path) && hennery_kernel::hats::is_canonical(&path) =>
        {
            Some(path)
        }
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid",
                "`path`, if given, must be a canonical absolute path of at most 4096 bytes",
            );
        }
    };
    let conn_id = match reachable(&state, &host_id) {
        Ok(conn_id) => conn_id,
        Err(response) => return *response,
    };
    let hat_id = match path {
        Some(path) => match state.hosts.resolve_hat(&host_id, &path) {
            Ok(Some(resolution)) => resolution.hat_id,
            Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
            Err(err) => return internal(err),
        },
        None => match state.hosts.host(&host_id) {
            Ok(Some(host)) => host.default_hat_id,
            Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
            Err(err) => return internal(err),
        },
    };
    let owner = state.hosts.owner_id();
    let enumeration = match state.projects.get(owner, &host_id, conn_id) {
        Some(cached) => cached,
        None => {
            let request_id = uuid::Uuid::now_v7().to_string();
            let frame = CollectorFrame::ListProjects {
                request_id: request_id.clone(),
            };
            match state.hub.probe(&host_id, &request_id, frame, state.probe_timeout).await {
                Ok(HostFrame::Projects {
                    items, partial, home, ..
                }) => {
                    let enumeration = checked_enumeration(&host_id, items, partial, home);
                    state.projects.put(owner, &host_id, conn_id, enumeration.clone());
                    enumeration
                }
                Ok(_) => return bad_reply(),
                Err(err) => return probe_failed(err),
            }
        }
    };
    let recents = match state.hosts.recents(&host_id, &hat_id, hennery_kernel::recents::SHOWN) {
        Ok(recents) => recents,
        Err(err) => return internal(err),
    };
    no_store(HostProjects {
        recents_hat_id: hat_id,
        recents: recents
            .into_iter()
            .map(|recent| RecentProject {
                path: recent.path,
                last_used_at: hennery_kernel::secret::rfc3339(recent.last_used_at),
            })
            .collect(),
        items: enumeration.items,
        partial: enumeration.partial,
        home: enumeration.home,
    })
}

/// A host's enumeration with what cannot be shown left out (the review's
/// A6): paths that are not absolute, too long or hold hidden characters,
/// and any past `MAX_ITEMS`.
fn checked_enumeration(host_id: &str, items: Vec<Project>, partial: bool, home: Option<String>) -> Enumeration {
    let reported = items.len();
    let items: Vec<Project> = items
        .into_iter()
        .filter(|item| is_displayable_path(&item.path))
        .take(MAX_ITEMS)
        .collect();
    if items.len() != reported {
        tracing::warn!(%host_id, dropped = reported - items.len(), "dropped projects that cannot be shown");
    }
    Enumeration {
        partial: partial || reported > MAX_ITEMS,
        items,
        home: home.filter(|home| is_displayable_path(home)),
    }
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

/// Whether `path` is worth asking a host about (decision 3): absolute, at
/// most 4096 bytes, no control character. The host checks the rest.
fn is_askable(path: &str) -> bool {
    path.starts_with('/') && path.len() <= hennery_kernel::hosts::MAX_PATH && !path.chars().any(char::is_control)
}

/// `GET /api/hosts/{id}/browse?path=`: the subdirectories of `path` on the
/// host (ACP core §7), never cached.
async fn browse(
    State(state): State<AppState>,
    Path(host_id): Path<String>,
    query: Result<Query<BrowseQuery>, QueryRejection>,
) -> Response {
    // A query that does not parse (`path` given twice) is as invalid as a
    // missing one, and answered the same way (Task 5's review).
    let Some(path) = query
        .ok()
        .and_then(|Query(query)| query.path)
        .filter(|path| is_askable(path))
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "give `path`: an absolute path of at most 4096 bytes",
        );
    };
    if let Err(response) = reachable(&state, &host_id) {
        return *response;
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::BrowseDirectory {
        request_id: request_id.clone(),
        path,
    };
    match state.hub.probe(&host_id, &request_id, frame, state.probe_timeout).await {
        Ok(HostFrame::Directory {
            path,
            parent,
            entries,
            truncated,
            ..
        }) => match checked_listing(&host_id, path, parent, entries, truncated) {
            Some(listing) => no_store(listing),
            None => bad_reply(),
        },
        Ok(_) => bad_reply(),
        Err(err) => probe_failed(err),
    }
}

/// A host's listing, checked (the review's A6): its path must be
/// displayable, or the answer is refused; a parent that is not is left
/// out, and so are entries whose name is not a plain, displayable file
/// name, and any past `MAX_ENTRIES`.
fn checked_listing(
    host_id: &str,
    path: String,
    parent: Option<String>,
    entries: Vec<DirEntry>,
    truncated: bool,
) -> Option<DirectoryListing> {
    if !is_displayable_path(&path) {
        tracing::warn!(%host_id, "a listing for a path that cannot be shown");
        return None;
    }
    let reported = entries.len();
    let entries: Vec<DirEntry> = entries
        .into_iter()
        .filter(|entry| {
            !entry.name.is_empty()
                && !matches!(entry.name.as_str(), "." | "..")
                && !entry.name.contains('/')
                && is_displayable_text(&entry.name, MAX_NAME)
        })
        .take(MAX_ENTRIES)
        .collect();
    if entries.len() != reported {
        tracing::warn!(%host_id, dropped = reported - entries.len(), "dropped entries that cannot be shown");
    }
    Some(DirectoryListing {
        path,
        parent: parent.filter(|parent| is_displayable_path(parent)),
        truncated: truncated || reported > MAX_ENTRIES,
        entries,
    })
}
