//! The page a setup link opens (kernel spec §3.1), until the frontend
//! replaces it. The link is `…/setup#<token>`: the token is in the
//! fragment, which a browser never sends, so it reaches no server log,
//! proxy log or `Referer` (3b decision 16). The page's script reads it
//! from `location.hash` and sends it in the `POST /api/setup` body.
//!
//! The page and its script are static and hold no data, so they sit
//! outside the browser rules and the session cookie (3b decision 13). The
//! script is a file of its own, not inline, so the page's
//! `Content-Security-Policy` can be kernel spec §7.2's without a hash.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

const PAGE: &str = r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Set up hennery</title>
<h1>Set up hennery</h1>
<form id="setup">
  <p><label>Password (at least 12 characters)<br><input id="password" type="password" minlength="12" required autocomplete="new-password"></label></p>
  <p><label>Public URL (where your browser reaches hennery)<br><input id="public_url" type="url" required></label></p>
  <p><label>Name of your default hat (sessions belong to it unless a path rule says otherwise)<br><input id="hat_name" maxlength="64" required value="Personal"></label></p>
  <p><button type="submit">Set up</button></p>
</form>
<p id="result" role="status"></p>
<script src="/setup.js" defer></script>
</html>
"#;

const SCRIPT: &str = r#"// hennery setup: the token is in the fragment and leaves the browser only
// in this request's body.
const token = location.hash.slice(1);
history.replaceState(null, "", location.pathname);
const result = document.getElementById("result");
document.getElementById("public_url").value = location.origin;
if (!token) {
  result.textContent = "This page needs the setup link the collector wrote to its setup-url file.";
}
document.getElementById("setup").addEventListener("submit", async (event) => {
  event.preventDefault();
  const response = await fetch("/api/setup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      token,
      password: document.getElementById("password").value,
      public_url: document.getElementById("public_url").value,
      default_hat_name: document.getElementById("hat_name").value,
    }),
  });
  if (response.ok) {
    result.textContent = "hennery is set up, and you are signed in.";
  } else {
    const body = await response.json().catch(() => ({}));
    result.textContent = "Setup failed: " + (body.message || response.status);
  }
});
"#;

/// `GET /setup`. Its `Content-Security-Policy` is set by `csp::on_html`,
/// over the whole router.
pub(crate) async fn page() -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], PAGE).into_response()
}

/// `GET /setup.js`.
pub(crate) async fn script() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], SCRIPT).into_response()
}

/// Every setup response, the page and the API alike: never cached, and
/// never a `Referer` onwards.
pub(crate) async fn private_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
