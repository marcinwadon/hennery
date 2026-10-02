//! Embeds the web UI's build in the binary (kernel spec §7; plan 4b).
//!
//! - `HENNERY_WEB_DIST`: the build to embed, an absolute path. When unset,
//!   `<repo>/web/dist`. Set but missing, relative, or without `index.html`:
//!   the build fails.
//! - The default build missing: a placeholder page is embedded instead, so
//!   `cargo test` never needs pnpm. Only that page carries
//!   `PLACEHOLDER_MARKER`, which release builds check is absent.
//! - `HENNERY_WEB_REQUIRE=1`: the real build is required; the placeholder
//!   fails the build. `0` or empty: not required. Anything else fails.
//!
//! It writes `$OUT_DIR/web_assets.rs`, a table of every file of the build,
//! each `include_bytes!`d, with its content type and ETag.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// In the placeholder only: release builds grep their binary for it.
const PLACEHOLDER_MARKER: &str = "hennery-web-ui-not-embedded";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=HENNERY_WEB_DIST");
    println!("cargo::rerun-if-env-changed=HENNERY_WEB_REQUIRE");
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let require = match env::var("HENNERY_WEB_REQUIRE").as_deref() {
        Err(_) | Ok("") | Ok("0") => false,
        Ok("1") => true,
        Ok(other) => panic!("HENNERY_WEB_REQUIRE must be 1, 0 or empty, not {other:?}"),
    };
    let web_dir = manifest_dir.join("../../web");
    let dist = match env::var_os("HENNERY_WEB_DIST").filter(|d| !d.is_empty()) {
        Some(given) => {
            let given = PathBuf::from(given);
            assert!(
                given.is_absolute(),
                "HENNERY_WEB_DIST must be an absolute path, not {}",
                given.display()
            );
            assert!(
                given.join("index.html").is_file(),
                "HENNERY_WEB_DIST ({}) holds no index.html: build the web UI first",
                given.display()
            );
            Some(given)
        }
        None => Some(web_dir.join("dist")).filter(|d| d.join("index.html").is_file()),
    };
    // Watch only paths that exist: cargo reruns a build script on every
    // build while a path it watches is missing.
    match &dist {
        Some(dist) => println!("cargo::rerun-if-changed={}", dist.display()),
        None if web_dir.is_dir() => println!("cargo::rerun-if-changed={}", web_dir.display()),
        None => {}
    }
    let mut table = String::new();
    match &dist {
        Some(dist) => {
            let mut files = Vec::new();
            collect(dist, dist, &mut files);
            files.sort();
            writeln!(table, "/// The real UI is embedded.\npub const EMBEDDED: bool = true;").unwrap();
            writeln!(table, "static FILES: &[Asset] = &[").unwrap();
            for (path, file) in &files {
                entry(&mut table, path, &file.display().to_string(), &fs::read(file).unwrap());
            }
            writeln!(table, "];").unwrap();
        }
        None => {
            assert!(
                !require,
                "HENNERY_WEB_REQUIRE=1, but there is no web build at {}: run `pnpm install && pnpm build` in web/",
                web_dir.join("dist").display()
            );
            let page = out_dir.join("placeholder.html");
            fs::write(&page, placeholder()).unwrap();
            writeln!(
                table,
                "/// A placeholder is embedded, not the UI.\npub const EMBEDDED: bool = false;"
            )
            .unwrap();
            writeln!(table, "static FILES: &[Asset] = &[").unwrap();
            entry(
                &mut table,
                "/index.html",
                &page.display().to_string(),
                placeholder().as_bytes(),
            );
            writeln!(table, "];").unwrap();
        }
    }
    fs::write(out_dir.join("web_assets.rs"), table).unwrap();
}

/// Every file under `dir`, as its URL path and its location, skipping dot
/// files and source maps.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".map") {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            let relative = path.strip_prefix(root).unwrap();
            let url: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push((format!("/{}", url.join("/")), path));
        }
    }
}

fn entry(table: &mut String, url: &str, file: &str, bytes: &[u8]) {
    let digest = Sha256::digest(bytes);
    let etag = format!("\"{}\"", hex(&digest[..8]));
    writeln!(
        table,
        "    Asset {{ path: {url:?}, body: include_bytes!({file:?}), content_type: {:?}, etag: {etag:?} }},",
        content_type(url)
    )
    .unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The page served in place of the UI. It holds no form and takes no secret
/// (distribution-67's conditions for the interim Nix package).
fn placeholder() -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="{PLACEHOLDER_MARKER}">
<title>hennery: no web UI in this build</title>
<h1>This build of hennery has no web UI</h1>
<p>It was built without the web UI. The API works, so hennery can still be set up:</p>
<ol>
<li>Print the one-time setup link: <code>hennery admin setup-url</code>. The token is the part after <code>#</code>.</li>
<li>Send it with the owner's password and this page's address as the public URL:
<pre>curl -X POST "$PUBLIC_URL/api/setup" -H "Origin: $PUBLIC_URL" -H "Content-Type: application/json" \
  -d '{{"token": "…", "password": "…", "public_url": "'"$PUBLIC_URL"'", "default_hat_name": "Personal"}}'</pre></li>
</ol>
<p>For the web UI, build it (<code>pnpm install &amp;&amp; pnpm build</code> in <code>web/</code>) and rebuild hennery.</p>
</html>
"#
    )
}
