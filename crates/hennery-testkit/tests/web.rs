//! The embedded web UI over HTTP (kernel spec §3.1, §7, §7.2; plan 4b): the
//! router's fallback serves the app for every path no route claims, never
//! for the API or the gateway's proxy, with the cache and security headers
//! each answer needs. These tests hold for both builds: the real UI (CI,
//! `HENNERY_WEB_REQUIRE=1`) and the placeholder (no `web/dist`).

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::rest::ApiError;
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

/// What only the placeholder page carries (`build.rs`).
const PLACEHOLDER_MARKER: &str = "hennery-web-ui-not-embedded";

async fn start() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = AppState::new(
        Store::open_in_memory().unwrap(),
        Hosts::open_in_memory().unwrap(),
        Operator::open_in_memory().unwrap(),
    );
    tokio::spawn(hennery_sessions::serve(listener, state));
    addr
}

fn header(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers().get(name).map(|v| v.to_str().unwrap().to_string())
}

async fn get(addr: SocketAddr, path: &str) -> reqwest::Response {
    reqwest::get(format!("http://{addr}{path}")).await.unwrap()
}

/// Every route the app shows (frontend spec §2), and any other path, is
/// `index.html`: the same page, uncached but revalidated, sending no
/// `Referer`, under the `Content-Security-Policy`.
#[tokio::test]
async fn every_other_path_is_the_app() {
    let addr = start().await;
    let index = get(addr, "/").await.text().await.unwrap();
    for path in [
        "/",
        "/login?next=%2Fhosts",
        "/sessions",
        "/sessions/s-1",
        "/new",
        "/hosts",
        "/mcp",
        "/hats",
        "/settings",
        "/no/such/page",
        "/setup.js",
        "/index.html",
    ] {
        let resp = get(addr, path).await;
        assert_eq!(resp.status(), 200, "{path}");
        assert_eq!(
            header(&resp, "content-type").as_deref(),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
        assert_eq!(header(&resp, "cache-control").as_deref(), Some("no-cache"), "{path}");
        assert_eq!(
            header(&resp, "referrer-policy").as_deref(),
            Some("no-referrer"),
            "{path}"
        );
        assert_eq!(
            header(&resp, "x-content-type-options").as_deref(),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            header(&resp, "content-security-policy").as_deref(),
            Some(hennery_kernel::csp::POLICY),
            "{path}"
        );
        assert!(header(&resp, "etag").is_some(), "{path}");
        assert_eq!(resp.text().await.unwrap(), index, "{path}");
    }
}

/// The API and the gateway's proxy are never the app: an unknown path
/// there is a JSON 404, whatever the method. A method other than `GET` or
/// `HEAD` anywhere else is one too.
#[tokio::test]
async fn the_api_and_the_proxy_are_never_the_app() {
    let addr = start().await;
    let client = reqwest::Client::new();
    for (method, path) in [
        ("GET", "/api"),
        ("GET", "/api/"),
        ("GET", "/api/no-such-route"),
        ("GET", "/api/sessions/s-1/no-such-route"),
        ("POST", "/api/no-such-route"),
        ("GET", "/mcp/some-server"),
        ("POST", "/mcp/some-server"),
        ("POST", "/sessions"),
        ("PUT", "/hosts"),
        ("DELETE", "/"),
    ] {
        let resp = client
            .request(method.parse().unwrap(), format!("http://{addr}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{method} {path}");
        assert!(
            header(&resp, "content-type").is_some_and(|t| t.starts_with("application/json")),
            "{method} {path}"
        );
        assert_eq!(
            header(&resp, "x-content-type-options").as_deref(),
            Some("nosniff"),
            "{method} {path}"
        );
        assert_eq!(
            resp.json::<ApiError>().await.unwrap().code,
            "not_found",
            "{method} {path}"
        );
    }
}

/// The app holds no data, so it is outside the browser rules: a link
/// opened from another site (mail, chat) loads it. `HEAD` answers as `GET`
/// does, without the body.
#[tokio::test]
async fn a_link_from_another_site_loads_the_app() {
    let addr = start().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/sessions/s-1"))
        .header("sec-fetch-site", "cross-site")
        .header("origin", "https://elsewhere.example")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let head = client.head(format!("http://{addr}/hosts")).send().await.unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(header(&head, "cache-control").as_deref(), Some("no-cache"));
    assert!(head.bytes().await.unwrap().is_empty());
}

/// Kernel spec §3.1: the page a setup link opens is never cached and sends
/// no `Referer` onwards. The token stays in the fragment, which no request
/// carries.
#[tokio::test]
async fn the_setup_page_is_never_cached_nor_referred() {
    let addr = start().await;
    for path in ["/setup", "/setup/"] {
        let resp = get(addr, path).await;
        assert_eq!(resp.status(), 200, "{path}");
        assert_eq!(header(&resp, "cache-control").as_deref(), Some("no-store"), "{path}");
        assert_eq!(
            header(&resp, "referrer-policy").as_deref(),
            Some("no-referrer"),
            "{path}"
        );
        assert_eq!(
            header(&resp, "x-content-type-options").as_deref(),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            header(&resp, "content-security-policy").as_deref(),
            Some(hennery_kernel::csp::POLICY),
            "{path}"
        );
        assert_eq!(header(&resp, "etag"), None, "{path}");
    }
}

/// `index.html` revalidates: its ETag answers 304, with no body.
#[tokio::test]
async fn the_page_revalidates_with_its_etag() {
    let addr = start().await;
    let first = get(addr, "/hosts").await;
    let etag = header(&first, "etag").unwrap();
    let client = reqwest::Client::new();
    for tags in [etag.clone(), format!("\"0000\", {etag}")] {
        let again = client
            .get(format!("http://{addr}/sessions"))
            .header("if-none-match", tags)
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), 304);
        assert_eq!(header(&again, "etag").as_deref(), Some(etag.as_str()));
        assert!(again.bytes().await.unwrap().is_empty());
    }
    let stale = client
        .get(format!("http://{addr}/sessions"))
        .header("if-none-match", "\"0000\"")
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 200);
}

/// A missing hashed file is a plain 404, never the page: a browser that
/// asks for a stale chunk must not run HTML as a script. The real build's
/// own hashed files are cached for good.
#[tokio::test]
async fn hashed_files_are_immutable_and_a_missing_one_is_a_plain_404() {
    let addr = start().await;
    let missing = get(addr, "/assets/index-00000000.js").await;
    assert_eq!(missing.status(), 404);
    assert!(header(&missing, "content-type").is_some_and(|t| t.starts_with("text/plain")));
    assert_eq!(header(&missing, "x-content-type-options").as_deref(), Some("nosniff"));

    let index = get(addr, "/").await.text().await.unwrap();
    let assets: Vec<&str> = index.split('"').filter(|part| part.starts_with("/assets/")).collect();
    assert_eq!(assets.is_empty(), !hennery_kernel::web::ui_embedded(), "{index}");
    for path in assets {
        let resp = get(addr, path).await;
        assert_eq!(resp.status(), 200, "{path}");
        assert_eq!(
            header(&resp, "cache-control").as_deref(),
            Some("public, max-age=31536000, immutable"),
            "{path}"
        );
        assert_eq!(
            header(&resp, "x-content-type-options").as_deref(),
            Some("nosniff"),
            "{path}"
        );
        let kind = header(&resp, "content-type").unwrap();
        assert!(
            (path.ends_with(".js") && kind == "text/javascript; charset=utf-8")
                || (path.ends_with(".css") && kind == "text/css; charset=utf-8"),
            "{path}: {kind}"
        );
    }
}

/// The page runs no inline script and holds no inline style (kernel spec
/// §7.2: `script-src 'self'`, `default-src 'self'`). The placeholder holds
/// no script and no form, takes no secret, and is the only page with the
/// marker release builds grep for.
#[tokio::test]
async fn the_page_runs_no_inline_script() {
    let addr = start().await;
    let index = get(addr, "/").await.text().await.unwrap();
    for tag in index.split("<script").skip(1) {
        let open = &tag[..tag.find('>').unwrap()];
        assert!(open.contains("src=\"/"), "an inline script: <script{open}>");
    }
    assert!(!index.contains("<style"), "{index}");
    if hennery_kernel::web::ui_embedded() {
        assert!(!index.contains(PLACEHOLDER_MARKER));
        assert!(index.contains("<script src=\"/theme.js\"></script>"), "{index}");
    } else {
        assert!(index.contains(PLACEHOLDER_MARKER));
        assert!(!index.contains("<script") && !index.contains("<form"), "{index}");
        assert!(index.contains("hennery admin setup-url") && index.contains("/api/setup"));
    }
}

/// A build told to require the UI (CI, release builds) embeds it.
#[test]
fn a_build_that_requires_the_ui_embeds_it() {
    if option_env!("HENNERY_WEB_REQUIRE") == Some("1") {
        assert!(hennery_kernel::web::ui_embedded());
    }
}
