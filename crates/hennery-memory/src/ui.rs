//! The optional plug-in's own server-rendered management screen. All note text
//! is escaped Markdown source: no HTML interpretation or network fetch on render.
use crate::{
    management::{Edit, Manager, NewMemory},
    store::Store,
};
use anyhow::{Result, ensure};
use axum::{
    Form, Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, Extension, Path, Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use futures::StreamExt;
use hennery_kernel::{
    client_address::ClientAddress,
    operator::{Authenticated, CheckPurpose, Operator, PublicUrl, SetupOutcome},
    secret::{rfc3339, unix_now},
};
use serde::Deserialize;
use std::{
    path::Path as FsPath,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
pub const SESSION_COOKIE: &str = "__Host-hennery_memory_session";
#[derive(Clone)]
struct Ui {
    store: Arc<Store>,
    operator: Arc<Operator>,
    permits: Arc<Semaphore>,
    auth_permits: Arc<Semaphore>,
}

/// Cookies ignore ports. Deploy the UI on a hostname dedicated to this service.
/// Compare all known collector grants; standalone collectors cannot be discovered.
pub fn validate_configuration(store: &Store, directory: &FsPath, origin: &str) -> Result<()> {
    let public = PublicUrl::parse(origin).map_err(anyhow::Error::msg)?;
    ensure!(public.rp_id().is_some(), "memory UI requires a dedicated hostname");
    if store.has_feed_credentials()? {
        let key = crate::keys::load(directory, true)?;
        for collector in store.collector_origins(&key)? {
            check_collector_host(origin, &collector)?;
        }
    }
    Ok(())
}
pub fn check_collector_host(origin: &str, collector: &str) -> Result<()> {
    let public = PublicUrl::parse(origin).map_err(anyhow::Error::msg)?;
    let host = public
        .rp_id()
        .ok_or_else(|| anyhow::anyhow!("memory UI requires a dedicated hostname"))?;
    let collector = url::Url::parse(collector)?;
    ensure!(
        collector.host_str().is_some_and(|other| !host
            .trim_end_matches('.')
            .eq_ignore_ascii_case(other.trim_end_matches('.'))),
        "memory UI and collector require different hostnames, regardless of ports"
    );
    Ok(())
}

/// Local, filesystem-owner bootstrap. The caller supplies the password over
/// bounded stdin; neither mounted credentials nor a core session can do this.
pub fn bootstrap(store: &Store, directory: &FsPath, owner: &str, origin: &str, password: &str) -> Result<()> {
    validate_configuration(store, directory, origin)?;
    ensure!(!owner.is_empty() && owner.len() <= 256, "invalid owner");
    let operator = Operator::open(&directory.join("memory-auth.db"))?;
    ensure!(!operator.is_set_up()?, "UI already bootstrapped");
    let setup = operator
        .issue_setup_token(unix_now())?
        .ok_or_else(|| anyhow::anyhow!("setup unavailable"))?;
    store.bind_manager(operator.owner_id(), owner)?;
    ensure!(
        matches!(
            operator.set_up(&setup, password, origin, unix_now())?,
            SetupOutcome::Done { .. }
        ),
        "setup refused"
    );
    Ok(())
}
pub fn router(store: Arc<Store>, operator: Arc<Operator>) -> Result<Router> {
    ensure!(
        operator.is_set_up()? && operator.public_url().is_some(),
        "explicit UI bootstrap required"
    );
    ensure!(
        operator.public_url().is_some_and(|p| p.rp_id().is_some()),
        "memory UI requires a dedicated hostname"
    );
    store.manager(operator.owner_id())?;
    let state = Ui {
        store,
        operator: operator.clone(),
        permits: Arc::new(Semaphore::new(4)),
        auth_permits: Arc::new(Semaphore::new(4)),
    };
    let forms = Router::new()
        .route("/auth.js", get(client_script))
        .route("/import", get(import_page))
        .route("/import-preview", post(import_preview))
        .route("/import-apply", post(import_apply))
        .route("/", get(home))
        .route("/login", get(login_page).post(login))
        .route("/step-up", post(step_up))
        .route("/logout", post(logout))
        .route("/n/{ns}", get(list))
        .route("/n/{ns}/create", post(create))
        .route("/n/{ns}/export", post(export))
        .route("/n/{ns}/writes", post(writes))
        .route("/n/{ns}/purge", post(purge))
        .route("/n/{ns}/e/{id}", get(view))
        .route("/n/{ns}/e/{id}/edit", post(edit))
        .route("/n/{ns}/e/{id}/annotate", post(annotate))
        .route("/n/{ns}/e/{id}/retain", post(retain))
        .route("/n/{ns}/e/{id}/pin", post(pin))
        .route("/n/{ns}/e/{id}/delete", post(delete))
        .layer(DefaultBodyLimit::disable())
        .with_state(state.clone());
    // Reuse passkey registration/login/step-up/session revocation unchanged,
    // but the boundary below translates only this plug-in's private cookie.
    Ok(forms
        .merge(hennery_kernel::auth_api::router(operator))
        .layer(middleware::from_fn_with_state(state, boundary)))
}
fn cookie(token: &str, operator: &Operator) -> String {
    own_cookie(
        &hennery_kernel::operator::session_cookie(token, secure(operator)),
        operator,
    )
}
fn secure(operator: &Operator) -> bool {
    operator.public_url().is_none_or(|p| p.is_https())
}
/// The kernel's `Set-Cookie`, renamed to this plug-in's private cookie.
fn own_cookie(kernel: &str, operator: &Operator) -> String {
    let kernel_name = hennery_kernel::operator::session_cookie_name(secure(operator));
    match kernel.strip_prefix(kernel_name).filter(|rest| rest.starts_with('=')) {
        Some(rest) => format!("{}{rest}", cookie_name(operator)),
        None => kernel.to_string(),
    }
}
fn cookie_name(operator: &Operator) -> &'static str {
    if secure(operator) {
        SESSION_COOKIE
    } else {
        "hennery_memory_session"
    }
}
fn tokens(headers: &axum::http::HeaderMap, operator: &Operator) -> Vec<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| {
            let (name, value) = p.trim().split_once('=')?;
            (name == cookie_name(operator) && value.len() <= 256).then(|| value.to_string())
        })
        .take(4)
        .collect()
}
fn private(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, "no-store".parse().expect("header"));
    // Chromium serializes form POST Origin as null under no-referrer. Keep
    // same-origin forms usable while sending no referrer across origins.
    h.insert(header::REFERRER_POLICY, "same-origin".parse().expect("header"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().expect("header"));
    h.insert(header::CONTENT_SECURITY_POLICY,"default-src 'none'; style-src 'unsafe-inline'; script-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'".parse().expect("header"));
    response
}
fn error(status: StatusCode, message: &str) -> Response {
    private(
        (
            status,
            Html(page(
                "Operation unavailable",
                &format!(
                    "<p role=alert>{}</p><p><a href=\"/\">Namespaces</a> · <a href=\"/login\">Sign in</a></p>",
                    escape(message)
                ),
            )),
        )
            .into_response(),
    )
}
fn unavailable() -> Response {
    error(
        StatusCode::NOT_FOUND,
        "Unavailable, changed revision, or namespace has no live lease. Reload before retrying.",
    )
}
fn operation_error(err: anyhow::Error) -> Response {
    if err.to_string().starts_with("revision conflict") {
        error(
            StatusCode::CONFLICT,
            "This revision changed. Reload the entry and compare before saving again.",
        )
    } else {
        unavailable()
    }
}
fn needs_step_up() -> Response {
    error(
        StatusCode::FORBIDDEN,
        "Confirm your password again from Namespaces, then retry this operation. Import, export, deletion and policy changes require a check within five minutes.",
    )
}
fn sensitive(path: &str) -> bool {
    [
        "/export",
        "/delete",
        "/purge",
        "/writes",
        "/import-preview",
        "/import-apply",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}
async fn boundary(State(ui): State<Ui>, mut request: Request, next: Next) -> Response {
    let Some(public) = ui.operator.public_url() else {
        return unavailable();
    };
    let path = request.uri().path().to_string();
    let origins = request.headers().get_all(header::ORIGIN).iter().collect::<Vec<_>>();
    let origin = origins.first().and_then(|v| v.to_str().ok());
    let changing = request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD;
    if origins.len() > 1
        || (changing && origin != Some(public.origin()))
        || (!changing && origin.is_some() && origin != Some(public.origin()))
        || request
            .headers()
            .get_all("sec-fetch-site")
            .iter()
            .any(|s| s != "same-origin" && s != "none")
    {
        return error(StatusCode::FORBIDDEN, "Request origin refused.");
    }
    if request.headers().contains_key(header::AUTHORIZATION)
        || path == "/api/setup"
        || request.uri().to_string().len() > 4096
    {
        return unavailable();
    }
    let mut session = None;
    let mut token = None;
    for value in tokens(request.headers(), &ui.operator) {
        if let Ok(Some(auth)) = ui.operator.authenticate(&value, unix_now()) {
            session = Some(auth);
            token = Some(value);
            break;
        }
    }
    let api = path.starts_with("/api/auth/");
    let public_route = path == "/login" || path == "/auth.js" || api;
    let pool = if session.is_none() && public_route {
        &ui.auth_permits
    } else {
        &ui.permits
    };
    let Ok(permit) = pool.clone().try_acquire_owned() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Busy; retry later.");
    };
    if !public_route {
        let Some(auth) = session.as_ref() else {
            return error(StatusCode::UNAUTHORIZED, "Sign in to the memory plug-in.");
        };
        if sensitive(&path) && !auth.stepped_up(unix_now()) {
            return needs_step_up();
        }
        let Ok(manager) = ui.store.manager(&auth.owner_id) else {
            return unavailable();
        };
        request.extensions_mut().insert(manager);
        request.extensions_mut().insert(auth.clone());
    }
    if changing || api {
        if !api {
            let types = request
                .headers()
                .get_all(header::CONTENT_TYPE)
                .iter()
                .collect::<Vec<_>>();
            if types.len() != 1
                || !types[0]
                    .to_str()
                    .is_ok_and(|v| v.split(';').next() == Some("application/x-www-form-urlencoded"))
            {
                return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "Use the management form.");
            }
        }
        let (parts, body) = request.into_parts();
        let cap = if api {
            hennery_kernel::auth_api::MAX_BODY_BYTES
        } else if path == "/import-preview" || path == "/import-apply" {
            128 * 1024 * 1024
        } else if path.ends_with("/edit") {
            8 * 1024 * 1024
        } else {
            64 * 1024
        };
        let timeout = if cap > 64 * 1024 { 30 } else { 5 };
        let bytes = match tokio::time::timeout(Duration::from_secs(timeout), to_bytes(body, cap)).await {
            Ok(Ok(bytes)) => bytes,
            _ => return error(StatusCode::PAYLOAD_TOO_LARGE, "Form too large or slow."),
        };
        request = Request::from_parts(parts, Body::from(bytes));
        if !public_route {
            let Some(token) = token.as_ref() else {
                return unavailable();
            };
            let Ok(Some(auth)) = ui.operator.authenticate(token, unix_now()) else {
                return error(StatusCode::UNAUTHORIZED, "Session ended.");
            };
            if sensitive(&path) && !auth.stepped_up(unix_now()) {
                return needs_step_up();
            }
        }
    }
    if api {
        request.headers_mut().remove(header::COOKIE);
        if let Some(token) = token.as_ref() {
            request.headers_mut().insert(
                header::COOKIE,
                format!(
                    "{}={token}",
                    hennery_kernel::operator::session_cookie_name(secure(&ui.operator))
                )
                .parse()
                .expect("opaque cookie"),
            );
        }
    }
    let mut response = next.run(request).await;
    // Axum's default Form/Query rejection can quote an unknown field name.
    // Keep token-shaped input out of errors as well as canonical content.
    if !api
        && response.status().is_client_error()
        && !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"))
    {
        response = error(
            response.status(),
            "Form fields or query parameters were refused. Use the displayed form.",
        );
    }
    if api {
        let values = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|v| own_cookie(v, &ui.operator))
            .collect::<Vec<_>>();
        response.headers_mut().remove(header::SET_COOKIE);
        for v in values {
            response
                .headers_mut()
                .append(header::SET_COOKIE, v.parse().expect("cookie"));
        }
    }
    if session.as_ref().is_some_and(|s| s.slid)
        && !response.headers().contains_key(header::SET_COOKIE)
        && let Some(token) = token.as_ref()
        && ui.operator.authenticate(token, unix_now()).is_ok_and(|a| a.is_some())
    {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, cookie(token, &ui.operator).parse().expect("cookie"));
    }
    // A slow/unconsumed export retains its admission slot through delivery.
    let (parts, body) = private(response).into_parts();
    let stream = body.into_data_stream().map(move |part| {
        let _keep = &permit;
        part
    });
    Response::from_parts(parts, Body::from_stream(stream))
}
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content=\"width=device-width, initial-scale=1\"><title>{}</title><style>body{{overflow-wrap:anywhere;font:17px system-ui;max-width:60rem;margin:2rem auto;padding:0 1rem;color:#17251c;background:#f6f8f4}}a{{color:#145b3e}}label{{display:block;margin:.8rem 0}}input,textarea,select,button{{font:inherit;max-width:100%;box-sizing:border-box;padding:.5rem}}textarea{{width:100%;min-height:8rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;unicode-bidi:plaintext;background:#fff;padding:1rem}}input{{width:min(100%,30rem)}}button{{margin:.4rem .3rem .4rem 0;cursor:pointer}}article,section{{border-top:1px solid #b9c9bb;padding:1rem 0}}.meta{{font-size:.9rem;overflow-wrap:anywhere}}nav{{display:flex;gap:1rem;flex-wrap:wrap}}details{{margin:1rem 0}}</style><h1>{}</h1>{}</html>",
        escape(title),
        escape(title),
        body
    )
}
fn html(title: &str, body: String) -> Response {
    Html(page(title, &body)).into_response()
}
fn password_form(action: &str, label: &str) -> String {
    format!(
        "<form method=post action=\"{action}\"><label>Password <input name=password type=password required maxlength=1024 autocomplete=current-password></label><button>{label}</button></form>"
    )
}
async fn client_script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("ui-client.mjs"),
    )
        .into_response()
}
fn passkey_controls(mode: &str, label: &str) -> String {
    format!(
        "<button type=button data-passkey=\"{mode}\">{label}</button><p id=passkey-status role=status></p><script type=module src=\"/auth.js\"></script>"
    )
}
async fn login_page() -> Response {
    html(
        "Memory plug-in sign in",
        format!(
            "{}{}",
            password_form("/login", "Sign in"),
            passkey_controls("login", "Use a passkey")
        ),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Password {
    password: String,
}
async fn login(State(ui): State<Ui>, address: ClientAddress, Form(input): Form<Password>) -> Response {
    let Some(admitted) = ui.operator.admit_check(CheckPurpose::Login) else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Busy; retry later.");
    };
    if ui
        .operator
        .login_limiter
        .attempt(address.effective, Instant::now())
        .is_err()
    {
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many attempts; try later.");
    }
    let Ok(Some(phc)) = ui.operator.check_password(admitted, input.password).await else {
        return error(StatusCode::UNAUTHORIZED, "Sign in failed.");
    };
    let Ok(Some(token)) = ui.operator.open_session("Memory management browser", &phc, unix_now()) else {
        return error(StatusCode::UNAUTHORIZED, "Sign in failed.");
    };
    ui.operator.login_limiter.succeeded(address.effective);
    let mut response = Redirect::to("/").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie(&token, &ui.operator).parse().expect("cookie"),
    );
    response
}
async fn step_up(
    State(ui): State<Ui>,
    Extension(session): Extension<Authenticated>,
    address: ClientAddress,
    Form(input): Form<Password>,
) -> Response {
    let Some(admitted) = ui.operator.admit_check(CheckPurpose::StepUp) else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Busy; retry later.");
    };
    if ui
        .operator
        .step_up_limiter
        .attempt(address.effective, Instant::now())
        .is_err()
    {
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many attempts; try later.");
    }
    let Ok(Some(_)) = ui.operator.check_password(admitted, input.password).await else {
        return error(StatusCode::UNAUTHORIZED, "Confirmation failed.");
    };
    if ui.operator.step_up(&session.session_id, unix_now()).is_ok_and(|ok| ok) {
        ui.operator.step_up_limiter.succeeded(address.effective);
        Redirect::to("/").into_response()
    } else {
        unavailable()
    }
}
async fn logout(State(ui): State<Ui>, Extension(session): Extension<Authenticated>) -> Response {
    if ui.operator.revoke_session(&session.session_id, unix_now()).is_err() {
        return unavailable();
    }
    let mut response = Redirect::to("/login").into_response();
    let cleared = own_cookie(
        &hennery_kernel::operator::cleared_cookie(secure(&ui.operator)),
        &ui.operator,
    );
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cleared.parse().expect("cookie"));
    response
}
async fn home(State(ui): State<Ui>, Extension(manager): Extension<Manager>) -> Response {
    let Ok(namespaces) = ui.store.managed_namespaces(&manager) else {
        return unavailable();
    };
    let mut body = "<p>Independent memory service. Select one namespace for every operation.</p><nav>".to_string();
    for n in namespaces {
        body.push_str(&format!(
            "<a href=\"/n/{}\">{} ({})</a>",
            escape(&n.id),
            escape(&hennery_kernel::redact::text(&n.hat).unwrap_or(n.hat)),
            escape(&n.state)
        ));
    }
    body.push_str("</nav><section><h2>Confirm sensitive operations</h2>");
    body.push_str("<p><a href=\"/import\">Import a vault snapshot</a></p>");
    body.push_str(&password_form("/step-up", "Confirm password"));
    body.push_str(&passkey_controls("step-up", "Confirm with passkey"));
    body.push_str("<label>New passkey label <input id=passkey-label maxlength=64></label><button type=button data-passkey=register>Register passkey</button>");
    body.push_str("</section><form method=post action=/logout><button>Sign out</button></form>");
    html("Memory namespaces", body)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listing {
    kind: Option<String>,
    q: Option<String>,
    offset: Option<usize>,
}
async fn list(
    State(ui): State<Ui>,
    Extension(manager): Extension<Manager>,
    Path(ns): Path<String>,
    Query(q): Query<Listing>,
) -> Response {
    let kind = q.kind.as_deref().unwrap_or("memory");
    let query = q.q.as_deref().unwrap_or("");
    let offset = q.offset.unwrap_or(0);
    let Ok(entries) = ui.store.managed_list(&manager, &ns, kind, query, offset) else {
        return unavailable();
    };
    let Ok(status) = ui.store.status(&ns) else {
        return unavailable();
    };
    let enabled = status["writes"].as_bool().unwrap_or(false);
    let selected_enabled = if enabled { " selected" } else { "" };
    let selected_disabled = if enabled { "" } else { " selected" };
    let mut body = format!(
        "<p><a href=\"/\">Namespaces</a></p><nav><a href=\"/n/{ns}?kind=memory\">Memories</a><a href=\"/n/{ns}?kind=note\">Session notes</a><a href=\"/n/{ns}?kind=source\">Imported sources</a></nav><form method=get><input name=kind type=hidden value=\"{}\"><label>Search <input name=q maxlength=512 value=\"{}\"></label><button>Search</button></form>",
        escape(kind),
        escape(query)
    );
    for e in entries.iter().take(20) {
        body.push_str(&format!("<article><h2><a href=\"/n/{ns}/e/{}\">{}</a></h2><p class=meta>Author: {} · {} · revision {} · stale: {} · pinned: {}</p><pre>{}</pre></article>",escape(&e.id),escape(&e.title),escape(&e.author),escape(&rfc3339(e.updated_at)),e.revision,e.stale,e.pinned,escape(&e.body)));
    }
    if entries.len() > 20 {
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("kind", kind)
            .append_pair("q", query)
            .append_pair("offset", &(offset + 20).to_string())
            .finish();
        body.push_str(&format!("<a href=\"/n/{ns}?{}\">Next page</a>", escape(&encoded)));
    }
    body.push_str(&format!("<section><h2>Create a deliberate memory</h2><form method=post action=\"/n/{ns}/create\"><label>Title <input name=title required maxlength=160></label><label>Markdown <textarea aria-label=Markdown name=body maxlength=16384></textarea></label><button>Create memory</button></form><p>At most five memories can be pinned for optional future startup selection.</p></section><section><h2>Export this namespace</h2><p>Portable content and provenance; no credentials. Exports, backups and flash remanence are outside subsequent deletion claims.</p><form method=post action=\"/n/{ns}/export\"><button>Export Markdown and manifest</button></form></section><details><summary>Agent write policy</summary><form method=post action=\"/n/{ns}/writes\"><label>Agent writes <select name=enabled><option value=true{selected_enabled}>Enabled</option><option value=false{selected_disabled}>Disabled</option></select></label><button>Save policy</button></form></details><details><summary>Erase this namespace</summary><p>Removes all plug-in content and makes this namespace terminal. External exports/backups remain.</p><form method=post action=\"/n/{ns}/purge\"><label>Type erase <input name=confirm required pattern=erase></label><button>Erase namespace</button></form></details>"));
    html("Manage memory", body)
}
fn revision_field(revision: u64) -> String {
    format!("<input type=hidden name=revision value={revision}>")
}
async fn view(
    State(ui): State<Ui>,
    Extension(manager): Extension<Manager>,
    Path((ns, id)): Path<(String, String)>,
) -> Response {
    let Ok(e) = ui.store.managed_entry(&manager, &ns, &id) else {
        return unavailable();
    };
    let base = format!("/n/{ns}/e/{id}");
    let revision = revision_field(e.revision);
    let evidence=serde_json::to_string_pretty(&serde_json::json!({"source_refs":e.source_refs,"provenance":e.provenance,"legacy":e.legacy,"covered_through":e.covered_through,"truncated":e.truncated,"older_turns":e.older_turns})).expect("evidence");
    let mut body = format!(
        "<p><a href=\"/n/{ns}\">Back to namespace</a></p><p class=meta>Author: {} · created {} · updated {} · revision {} · stale: {} · pinned: {}</p><p>Markdown source is shown literally; embedded HTML and remote resources are inert.</p><pre>{}</pre><details><summary>Source provenance</summary><pre>{}</pre></details>",
        escape(&e.author),
        escape(&rfc3339(e.created_at)),
        escape(&rfc3339(e.updated_at)),
        e.revision,
        e.stale,
        e.pinned,
        escape(&e.body),
        escape(&evidence)
    );
    if e.kind != "note" || e.legacy.is_some() {
        body.push_str(&format!("<form method=post action=\"{base}/edit\">{revision}<label>Title <input name=title required maxlength=160 value=\"{}\"></label><label>Markdown <textarea aria-label=Markdown name=body>{}</textarea></label><button>Save revision</button></form>",escape(&e.title),escape(&e.body)));
    }
    if e.kind == "memory" {
        body.push_str(&format!("<form method=post action=\"{base}/pin\">{revision}<input type=hidden name=pinned value={}><button>{}</button></form>",!e.pinned,if e.pinned {"Unpin"}else{"Pin memory"}));
    }
    if e.kind == "note" {
        body.push_str(&format!("<form method=post action=\"{base}/annotate\">{revision}<label>Owner annotation <textarea aria-label=\"Owner annotation\" name=annotation maxlength=16384>{}</textarea></label><button>Save annotation</button></form><form method=post action=\"{base}/retain\">{revision}<label>Memory title <input name=title required maxlength=160></label><label>Exact text from this note to retain <textarea aria-label=\"Exact text from this note to retain\" name=selected required maxlength=16384></textarea></label><button>Retain selected text</button></form>",escape(&e.owner_annotation)));
    }
    body.push_str(&format!("<details><summary>Delete this entry{}</summary><form method=post action=\"{base}/delete\">{revision}<label>Type delete <input name=confirm pattern=delete required></label><button>Delete entry</button></form><p>Native note deletion also suppresses future capture for its source. Exports/backups are not erased.</p></details>",if e.kind=="note" {" / suppress its source"}else{""}));
    html(&e.title, body)
}
async fn create(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path(ns): Path<String>,
    Form(input): Form<NewMemory>,
) -> Response {
    match ui.store.managed_create(&m, &ns, input) {
        Ok(e) => Redirect::to(&format!("/n/{ns}/e/{}", e.id)).into_response(),
        Err(err) => operation_error(err),
    }
}
async fn edit(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path((ns, id)): Path<(String, String)>,
    Form(input): Form<Edit>,
) -> Response {
    match ui.store.managed_edit(&m, &ns, &id, input) {
        Ok(_) => Redirect::to(&format!("/n/{ns}/e/{id}")).into_response(),
        Err(err) => operation_error(err),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Annotation {
    revision: u64,
    annotation: String,
}
async fn annotate(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path((ns, id)): Path<(String, String)>,
    Form(input): Form<Annotation>,
) -> Response {
    match ui
        .store
        .managed_annotate(&m, &ns, &id, input.revision, &input.annotation)
    {
        Ok(_) => Redirect::to(&format!("/n/{ns}/e/{id}")).into_response(),
        Err(err) => operation_error(err),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retain {
    revision: u64,
    title: String,
    selected: String,
}
async fn retain(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path((ns, id)): Path<(String, String)>,
    Form(input): Form<Retain>,
) -> Response {
    match ui
        .store
        .managed_retain(&m, &ns, &id, input.revision, &input.title, &input.selected)
    {
        Ok(e) => Redirect::to(&format!("/n/{ns}/e/{}", e.id)).into_response(),
        Err(err) => operation_error(err),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    revision: u64,
    pinned: bool,
}
async fn pin(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path((ns, id)): Path<(String, String)>,
    Form(input): Form<Pin>,
) -> Response {
    match ui.store.managed_pin(&m, &ns, &id, input.revision, input.pinned) {
        Ok(_) => Redirect::to(&format!("/n/{ns}/e/{id}")).into_response(),
        Err(err) => operation_error(err),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    revision: u64,
    confirm: String,
}
async fn delete(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path((ns, id)): Path<(String, String)>,
    Form(input): Form<Delete>,
) -> Response {
    if input.confirm != "delete" {
        return error(StatusCode::BAD_REQUEST, "Confirmation required.");
    }
    match ui.store.managed_delete(&m, &ns, &id, input.revision) {
        Ok(()) => Redirect::to(&format!("/n/{ns}")).into_response(),
        Err(err) => operation_error(err),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Writes {
    enabled: bool,
}
async fn writes(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path(ns): Path<String>,
    Form(input): Form<Writes>,
) -> Response {
    match ui.store.managed_writes(&m, &ns, input.enabled) {
        Ok(()) => Redirect::to(&format!("/n/{ns}")).into_response(),
        Err(err) => operation_error(err),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Purge {
    confirm: String,
}
async fn purge(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Path(ns): Path<String>,
    Form(input): Form<Purge>,
) -> Response {
    if input.confirm != "erase" {
        return error(StatusCode::BAD_REQUEST, "Confirmation required.");
    }
    match ui.store.managed_purge(&m, &ns) {
        Ok(receipt) => html(
            "Deletion receipt",
            format!(
                "<pre>{}</pre><p>Receipt covers this plug-in store only. Exports, backups and flash remanence remain outside the claim.</p><a href=\"/\">Namespaces</a>",
                escape(&serde_json::to_string_pretty(&receipt).expect("receipt"))
            ),
        ),
        Err(err) => operation_error(err),
    }
}
async fn export(
    State(ui): State<Ui>,
    Extension(m): Extension<Manager>,
    Extension(session): Extension<Authenticated>,
    Path(ns): Path<String>,
) -> Response {
    let Ok(snapshot) = ui.store.managed_snapshot(&m, &ns) else {
        return unavailable();
    };
    let generation = snapshot.generation;
    let stream = futures::stream::unfold(
        (ui, m, ns, session, crate::export::files(snapshot)),
        move |(ui, m, ns, session, mut files)| async move {
            let next = files.next()?;
            let valid = export_allowed(&ui, &m, &ns, generation, &session, unix_now());
            let chunk = if valid {
                next.map(axum::body::Bytes::from)
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            };
            Some((chunk, (ui, m, ns, session, files)))
        },
    );
    (
        [
            (header::CONTENT_TYPE, "application/x-tar"),
            (header::CONTENT_DISPOSITION, "attachment; filename=memory-export.tar"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}
fn export_allowed(
    ui: &Ui,
    manager: &Manager,
    namespace: &str,
    generation: u64,
    session: &Authenticated,
    now: i64,
) -> bool {
    session.stepped_up(now)
        && ui
            .operator
            .session_expires_at(&session.session_id, now)
            .is_ok_and(|at| at.is_some())
        && ui.store.managed_deliverable(manager, namespace, generation).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_delivery_requires_current_step_up() {
        let d = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&d.path().join("memory.db")).unwrap());
        let operator = Arc::new(Operator::open_in_memory().unwrap());
        let now = unix_now();
        let setup = operator.issue_setup_token(now).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = operator
            .set_up(&setup, "synthetic password only", "https://memory.example.test", now)
            .unwrap()
        else {
            panic!()
        };
        store.bind_manager(operator.owner_id(), "owner").unwrap();
        let namespace = store.create("owner", "garden").unwrap().namespace;
        let manager = store.manager(operator.owner_id()).unwrap();
        let token = operator.open_session("fixture", &phc, now).unwrap().unwrap();
        let session = operator.authenticate(&token, now).unwrap().unwrap();
        let ui = Ui {
            store,
            operator,
            permits: Arc::new(Semaphore::new(4)),
            auth_permits: Arc::new(Semaphore::new(4)),
        };
        assert!(export_allowed(&ui, &manager, &namespace, 0, &session, now));
        assert!(!export_allowed(
            &ui,
            &manager,
            &namespace,
            0,
            &session,
            now + hennery_kernel::operator::STEP_UP_SECS + 1
        ));
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportInput {
    bundle: String,
    mapping: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportApproval {
    preview: String,
    digest: String,
    confirm: String,
}
async fn import_page(State(ui): State<Ui>, Extension(manager): Extension<Manager>) -> Response {
    let Ok(namespaces) = ui.store.managed_namespaces(&manager) else {
        return unavailable();
    };
    let mut body="<p><a href=\"/\">Namespaces</a></p><p>Nothing enters memory until a signed preview is reviewed and applied with its exact content/mapping digest. Imported statements stay unverified legacy evidence. Never submit credentials.</p><h2>Your namespace IDs</h2><ul>".to_string();
    for n in namespaces {
        body.push_str(&format!(
            "<li>{}: <code>{}</code> ({})</li>",
            escape(&hennery_kernel::redact::text(&n.hat).unwrap_or(n.hat)),
            escape(&n.id),
            escape(&n.state)
        ));
    }
    body.push_str("</ul><section><h2>Preview a bundle</h2><p>Bundle JSON: version 1, a source-vault label, and files with normalized relative path and UTF-8 body. Only notes/, memories/ and sources/ are accepted. Map each scope or file to one namespace or exclude; shared, unscoped, mixed and disagreeing files need explicit decisions. Maximum 2 MiB per file, 10,000 files and 100 MiB expanded; the HTTP form is capped at 128 MiB.</p><p>Mapping JSON has scopes, files and mixed. A target is {&quot;action&quot;:&quot;namespace&quot;,&quot;namespace&quot;:&quot;ID&quot;} with optional project_key, or {&quot;action&quot;:&quot;exclude&quot;}. mixed is a list of documents to exclude until edited for one hat. No automatic personal/shared fallback.</p><form method=post action=/import-preview><label for=bundle>Bundle JSON</label><input type=file data-fill=bundle accept=application/json aria-label=\"Load bundle JSON\"><textarea id=bundle name=bundle required></textarea><label for=mapping>Mapping JSON</label><textarea id=mapping name=mapping required>{&quot;scopes&quot;:{},&quot;files&quot;:{},&quot;mixed&quot;:[]}</textarea><button>Preview import</button></form></section><section><h2>Apply a local CLI preview</h2><p>The local importer reads an uncompressed tar snapshot and emits a redacted signed preview. Review that manifest first; this form never reads a server filesystem path. A preview expires after one hour. Changed data/mapping needs a new preview. Registered namespaces require a live lease for apply.</p><form method=post action=/import-apply><label for=preview>Reviewed preview JSON</label><input type=file data-fill=preview accept=application/json aria-label=\"Load reviewed preview JSON\"><textarea id=preview name=preview required></textarea><label for=digest>Reviewed content/mapping digest</label><input id=digest name=digest required minlength=64 maxlength=64><label for=confirm>Type apply</label><input id=confirm name=confirm required pattern=apply><button>Apply reviewed import</button></form><p id=passkey-status role=status></p><script type=module src=/auth.js></script></section>");
    html("Import vault snapshot", body)
}
async fn import_preview(
    State(ui): State<Ui>,
    Extension(manager): Extension<Manager>,
    Form(input): Form<ImportInput>,
) -> Response {
    let Ok(bundle) = serde_json::from_str::<crate::import::Bundle>(&input.bundle) else {
        return error(
            StatusCode::BAD_REQUEST,
            "Invalid bounded bundle JSON. No server paths or links are accepted.",
        );
    };
    let Ok(mapping) = serde_json::from_str::<crate::import::Mapping>(&input.mapping) else {
        return error(
            StatusCode::BAD_REQUEST,
            "Invalid mapping JSON; duplicate keys and unknown fields are refused.",
        );
    };
    let Ok(preview) = ui.store.preview_import(&manager, &bundle, &mapping) else {
        return error(
            StatusCode::BAD_REQUEST,
            "Bundle or mapping refused: check normalized paths, duplicate files, expanded size and target mapping.",
        );
    };
    let mut body = format!(
        "<p><a href=\"/import\">Back to import</a></p><p>Content/mapping digest: <code>{}</code></p><p>Blocked: {}. Preview expires {}. No entries have been written.</p><h2>Per-namespace counts</h2><ul>",
        escape(&preview.digest),
        preview.blocked,
        escape(&rfc3339(preview.expires))
    );
    for hat in &preview.hats {
        body.push_str(&format!(
            "<li><code>{}</code>: {} · conflicts: {}</li>",
            escape(&hat.namespace),
            escape(&serde_json::to_string(&hat.counts).expect("counts")),
            hat.conflict
        ));
    }
    body.push_str("</ul><h2>Files, exclusions, conflicts and redactions</h2><ul>");
    for file in &preview.reports {
        body.push_str(&format!(
            "<li><code>{}</code>: {} · changed fields: {}</li>",
            escape(&file.path),
            escape(&file.status),
            escape(&file.redactions.join(", "))
        ));
    }
    body.push_str("</ul>");
    if !preview.blocked {
        let serialized = serde_json::to_string(&preview).expect("preview");
        body.push_str(&format!("<form method=post action=/import-apply><textarea name=preview hidden>{}</textarea><input type=hidden name=digest value=\"{}\"><label for=confirm>Type apply</label><input id=confirm name=confirm pattern=apply required><button>Apply reviewed import</button></form><p>Each hat commits atomically. A conflicting or failed hat stays untouched; other hats can commit. Retrying never overwrites edits or resurrects deleted imports.</p>",escape(&serialized),escape(&preview.digest)));
    } else {
        body.push_str("<p role=alert>Resolve every ambiguous, unparseable or oversized file, or explicitly exclude it, then preview again.</p>");
    }
    html("Review import manifest", body)
}
async fn import_apply(
    State(ui): State<Ui>,
    Extension(manager): Extension<Manager>,
    Form(input): Form<ImportApproval>,
) -> Response {
    if input.confirm != "apply" {
        return error(StatusCode::BAD_REQUEST, "Explicit apply confirmation required.");
    }
    let Ok(preview) = serde_json::from_str::<crate::import::Preview>(&input.preview) else {
        return error(StatusCode::BAD_REQUEST, "Invalid preview JSON.");
    };
    let Ok(results) = ui.store.apply_import(&manager, &preview, &input.digest) else {
        return error(
            StatusCode::CONFLICT,
            "Preview signature, digest, expiry or namespace changed; preview again before applying.",
        );
    };
    html(
        "Import result",
        format!(
            "<pre>{}</pre><a href=\"/\">Namespaces</a><p>Committed and untouched hats are reported separately. Inspect representative recent/search results before cutover. Existing ACP sessions and global rules were not changed.</p>",
            escape(&serde_json::to_string_pretty(&results).expect("results"))
        ),
    )
}
