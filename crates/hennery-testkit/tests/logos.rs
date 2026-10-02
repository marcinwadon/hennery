//! Hat logos over HTTP (kernel spec §5.1, §8; plan 4d-B2): a PNG uploaded
//! as base64 in JSON, re-encoded, served back from the hat's logo URL with
//! headers that keep it an image even when that URL is opened directly
//! (the security review's A9), revalidated by its `ETag`, and removed. A
//! hat frozen for its purge takes no new logo but can lose its own (A6).
//! Step-up and the browser rules are pinned in `step_up.rs` and `auth.rs`.

use base64::Engine;
use hennery_kernel::hats::{HatChange, PurgeStart};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::logo::{MAX_STORED, MAX_UPLOAD, reencode};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{ApiError, HatItem, SetHatLogoRequest};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::PUBLIC_URL;
use std::net::SocketAddr;

/// What the logo URL answers with, whoever asks (the review's A9).
const POLICY: &str = "default-src 'none'; sandbox; frame-ancestors 'none'";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// The owner's client: set up if need be, stepped up for five minutes.
    fn client(&self) -> reqwest::Client {
        hennery_testkit::operator_client(&self.state.operator)
    }

    /// A signed-in session's cookie, as a browser holds it.
    fn cookie(&self) -> String {
        self.client();
        let phc = hennery_testkit::owner_phc(&self.state.operator);
        let token = self
            .state
            .operator
            .open_session("browser", &phc, unix_now())
            .unwrap()
            .unwrap();
        format!("hennery_session={token}")
    }

    fn hat(&self, name: &str) -> String {
        match self.state.hosts.create_hat(name, None, unix_now()).unwrap() {
            HatChange::Done(hat) => hat.id,
            other => panic!("expected a hat, got {other:?}"),
        }
    }

    fn logo_url(&self, hat: &str) -> String {
        self.url(&format!("/api/hats/{hat}/logo"))
    }

    async fn put_body(&self, hat: &str, body: impl Into<reqwest::Body>) -> reqwest::Response {
        self.client()
            .put(self.logo_url(hat))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap()
    }

    async fn put(&self, hat: &str, image: &[u8]) -> reqwest::Response {
        let body = serde_json::to_string(&SetHatLogoRequest { data: b64(image) }).unwrap();
        self.put_body(hat, body).await
    }

    async fn delete(&self, hat: &str) -> reqwest::Response {
        self.client().delete(self.logo_url(hat)).send().await.unwrap()
    }

    async fn get(&self, hat: &str) -> reqwest::Response {
        self.client().get(self.logo_url(hat)).send().await.unwrap()
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A `width` × `height` RGBA PNG of `shade`, as the `png` crate writes it.
fn png_of(width: u32, height: u32, shade: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&vec![shade; (width * height * 4) as usize])
        .unwrap();
    writer.finish().unwrap();
    out
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

async fn hat_item(resp: reqwest::Response) -> HatItem {
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

fn header<'a>(resp: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).map(|v| v.to_str().unwrap())
}

/// Kernel spec §5.1: the logo is stored as it was re-encoded, listed by its
/// `ETag` on the hat, served as a PNG, replaced, and removed.
#[tokio::test]
async fn a_logo_is_uploaded_served_replaced_and_removed() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    assert_eq!(code_of(c.get(&acme).await).await, (404, "not_found".into()));

    let image = png_of(2, 2, 40);
    let expected = reencode(&image, MAX_STORED).unwrap();
    let hat = hat_item(c.put(&acme, &image).await).await;
    assert_eq!(hat.logo.as_deref(), Some(expected.etag.as_str()));
    let hats: Vec<HatItem> = c
        .client()
        .get(c.url("/api/hats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Both made within the same second: listed by name here, not by age.
    let mut listed: Vec<_> = hats.iter().map(|h| (h.name.as_str(), h.logo.clone())).collect();
    listed.sort();
    assert_eq!(listed, [("Acme", Some(expected.etag.clone())), ("Personal", None)]);

    let resp = c.get(&acme).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "etag"), Some(format!("\"{}\"", expected.etag).as_str()));
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &expected.bytes[..]);

    let other = reencode(&png_of(3, 1, 200), MAX_STORED).unwrap();
    let hat = hat_item(c.put(&acme, &png_of(3, 1, 200)).await).await;
    assert_eq!(hat.logo.as_deref(), Some(other.etag.as_str()));
    assert_eq!(c.get(&acme).await.bytes().await.unwrap().as_ref(), &other.bytes[..]);

    let hat = hat_item(c.delete(&acme).await).await;
    assert_eq!(hat.logo, None);
    assert_eq!(code_of(c.get(&acme).await).await, (404, "not_found".into()));
    // Removing it again is no error.
    assert_eq!(hat_item(c.delete(&acme).await).await.logo, None);
}

#[tokio::test]
async fn an_unknown_hat_has_no_logo() {
    let c = Collector::start().await;
    assert_eq!(code_of(c.get("hat-nope").await).await, (404, "not_found".into()));
    assert_eq!(
        code_of(c.put("hat-nope", &png_of(1, 1, 1)).await).await,
        (404, "not_found".into())
    );
    assert_eq!(code_of(c.delete("hat-nope").await).await, (404, "not_found".into()));
}

/// The parent's addition and the review's A9: the logo URL opened as a
/// page (`Sec-Fetch-Site: none`, or no such header at all) is still an
/// image of a fixed type, never sniffed, sandboxed, never framed, kept to
/// this origin, revalidated. Each header is asserted on its own.
#[tokio::test]
async fn the_logo_url_opened_directly_stays_a_sandboxed_image() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    assert_eq!(c.put(&acme, &png_of(1, 1, 9)).await.status(), 200);
    let cookie = c.cookie();
    for site in [Some("none"), Some("same-origin"), None] {
        let mut req = reqwest::Client::new()
            .get(c.logo_url(&acme))
            .header("cookie", &cookie)
            .header("sec-fetch-dest", "document")
            .header("accept", "text/html,application/xhtml+xml,*/*");
        if let Some(site) = site {
            req = req.header("sec-fetch-site", site);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), 200, "{site:?}");
        assert_eq!(header(&resp, "content-security-policy"), Some(POLICY), "{site:?}");
        assert_eq!(header(&resp, "x-content-type-options"), Some("nosniff"), "{site:?}");
        assert_eq!(header(&resp, "content-type"), Some("image/png"), "{site:?}");
        assert_eq!(
            header(&resp, "content-disposition"),
            Some("inline; filename=\"logo.png\""),
            "{site:?}"
        );
        assert_eq!(
            header(&resp, "cross-origin-resource-policy"),
            Some("same-origin"),
            "{site:?}"
        );
        assert_eq!(header(&resp, "cache-control"), Some("private, no-cache"), "{site:?}");
    }
    // Another site's page cannot load it (kernel spec §3.3).
    let resp = reqwest::Client::new()
        .get(c.logo_url(&acme))
        .header("cookie", &cookie)
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (403, "cross_site".into()));
}

/// The review's ruling on caching: `private, no-cache` and a strong `ETag`.
/// A request naming it (alone, in a list, weakly, or `*`) is answered 304,
/// with no body and the same headers; another tag, or a new logo, is 200.
#[tokio::test]
async fn a_logo_is_revalidated_by_its_etag() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    let first = hat_item(c.put(&acme, &png_of(1, 1, 1)).await).await.logo.unwrap();
    let tag = format!("\"{first}\"");
    let ask = |if_none_match: String| {
        c.client()
            .get(c.logo_url(&acme))
            .header("if-none-match", if_none_match)
            .send()
    };
    for matching in [
        tag.clone(),
        format!("\"x\", {tag}"),
        format!("W/{tag}"),
        "*".to_string(),
    ] {
        let resp = ask(matching.clone()).await.unwrap();
        assert_eq!(resp.status(), 304, "{matching}");
        assert_eq!(header(&resp, "etag"), Some(tag.as_str()), "{matching}");
        assert_eq!(header(&resp, "content-security-policy"), Some(POLICY), "{matching}");
        assert_eq!(header(&resp, "x-content-type-options"), Some("nosniff"), "{matching}");
        assert_eq!(header(&resp, "cache-control"), Some("private, no-cache"), "{matching}");
        assert_eq!(
            header(&resp, "content-disposition"),
            Some("inline; filename=\"logo.png\""),
            "{matching}"
        );
        assert_eq!(
            header(&resp, "cross-origin-resource-policy"),
            Some("same-origin"),
            "{matching}"
        );
        assert!(resp.bytes().await.unwrap().is_empty(), "{matching}");
    }
    for other in ["\"x\"".to_string(), first.clone(), format!("\"{}\"", &first[..31])] {
        assert_eq!(ask(other.clone()).await.unwrap().status(), 200, "{other}");
    }
    let second = hat_item(c.put(&acme, &png_of(1, 1, 2)).await).await.logo.unwrap();
    assert_ne!(first, second);
    let resp = ask(tag).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "etag"), Some(format!("\"{second}\"").as_str()));
}

/// The review's ruling B: a PNG only. An SVG (with or without script), a
/// WebP, a JPEG or a GIF is 415 `unsupported_logo`, and the hat keeps the
/// logo it had.
#[tokio::test]
async fn anything_but_a_png_is_refused() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    let kept = hat_item(c.put(&acme, &png_of(1, 1, 5)).await).await.logo;
    let refused: [&[u8]; 5] = [
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(document.cookie)</script></svg>",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"><rect width=\"1\" height=\"1\"/></svg>",
        b"RIFF\x1a\0\0\0WEBPVP8L\x0d\0\0\0\x2f\0\0\0\x10\x07\x10\x11\x11\x88\x88\xfe\x07\0",
        b"\xff\xd8\xff\xe0\0\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0\xff\xd9",
        b"GIF89a\x01\0\x01\0\0\0\0;",
    ];
    for image in refused {
        assert_eq!(
            code_of(c.put(&acme, image).await).await,
            (415, "unsupported_logo".into()),
            "{:?}",
            String::from_utf8_lossy(&image[..8])
        );
    }
    assert_eq!(c.state.hosts.hat(&acme).unwrap().unwrap().logo, kept);
}

/// The review's A8: each refusal is a fixed `ApiError`, the same whatever
/// the upload held: never the decoder's words, never the upload.
#[tokio::test]
async fn each_refusal_is_a_fixed_answer() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    let mut damaged = png_of(4, 4, 7);
    let at = damaged.len() - 20;
    damaged.truncate(at);
    let mut padded = png_of(1, 1, 1);
    padded.resize(MAX_UPLOAD + 1, 0);
    let cases: [(&str, String, u16, &str, &str); 5] = [
        (
            "damaged",
            b64(&damaged),
            400,
            "invalid_logo",
            "the logo is not a PNG that can be read",
        ),
        (
            "too wide",
            b64(&png_of(1025, 1, 0)),
            400,
            "invalid_logo",
            "a logo is at most 1024 × 1024 pixels",
        ),
        (
            "not base64",
            "<svg onload=alert(1)>".into(),
            400,
            "invalid_logo",
            "the logo must be standard base64, padded, with nothing around it",
        ),
        (
            "too large",
            b64(&padded),
            413,
            "logo_too_large",
            "a logo is at most 64 KiB, and at most 256 KiB once re-encoded",
        ),
        (
            "unsupported",
            b64(b"<svg/>"),
            415,
            "unsupported_logo",
            "a logo must be a PNG: turn other images into one first",
        ),
    ];
    for (name, data, status, code, message) in cases {
        let body = serde_json::to_string(&SetHatLogoRequest { data }).unwrap();
        let resp = c.put_body(&acme, body).await;
        assert_eq!(resp.status().as_u16(), status, "{name}");
        let error: ApiError = resp.json().await.unwrap();
        assert_eq!(
            error,
            ApiError {
                code: code.into(),
                message: message.into(),
                session_id: None
            },
            "{name}"
        );
    }
    assert_eq!(c.state.hosts.hat(&acme).unwrap().unwrap().logo, None);
}

/// Kernel spec §3.3 and the review's ruling on the body: JSON only, at most
/// 96 KiB however it is sent (the review's O4: chunked, with no length),
/// one `data` field. The same field spelt with a `\u` escape is the same
/// field (the fleet's differential rule); twice, or beside another, it is
/// refused.
#[tokio::test]
async fn the_body_is_one_json_field_of_at_most_96_kib() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    let data = b64(&png_of(1, 1, 3));

    let over = format!("{{\"data\":\"{}\"}}", "A".repeat(96 * 1024));
    assert_eq!(
        code_of(c.put_body(&acme, over.clone()).await).await,
        (413, "body_too_large".into())
    );
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = over.into_bytes().chunks(8192).map(|c| Ok(c.to_vec())).collect();
    let chunked = reqwest::Body::wrap_stream(futures::stream::iter(chunks));
    assert_eq!(
        code_of(c.put_body(&acme, chunked).await).await,
        (413, "body_too_large".into())
    );

    let resp = c
        .client()
        .put(c.logo_url(&acme))
        .header("content-type", "image/png")
        .body(png_of(1, 1, 3))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(resp).await, (415, "unsupported_media_type".into()));

    let twice = format!("{{\"data\":\"{data}\",\"data\":\"{data}\"}}");
    let beside = format!("{{\"data\":\"{data}\",\"mime\":\"image/svg+xml\"}}");
    for body in [twice, beside, "{}".to_string()] {
        let (status, code) = code_of(c.put_body(&acme, body.clone()).await).await;
        assert_eq!((status / 100, code.as_str()), (4, "invalid_body"), "{body}");
    }
    assert_eq!(c.state.hosts.hat(&acme).unwrap().unwrap().logo, None);

    let plain = hat_item(c.put_body(&acme, format!("{{\"data\":\"{data}\"}}")).await).await;
    let escaped = hat_item(c.put_body(&acme, format!("{{\"\\u0064ata\":\"{data}\"}}")).await).await;
    assert!(plain.logo.is_some());
    assert_eq!(plain.logo, escaped.logo);
}

/// Plan 9c decision 10c and the review's A6: a hat frozen for its purge
/// still shows its logo and can lose it, but takes no new one.
#[tokio::test]
async fn a_frozen_hat_shows_and_loses_its_logo_but_takes_no_new_one() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    let kept = hat_item(c.put(&acme, &png_of(1, 1, 1)).await).await.logo;
    assert!(matches!(
        c.state.hosts.begin_purge(&acme, unix_now()).unwrap(),
        PurgeStart::Frozen { .. }
    ));
    assert_eq!(
        code_of(c.put(&acme, &png_of(1, 1, 2)).await).await,
        (409, "hat_purging".into())
    );
    let resp = c.get(&acme).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "etag"), Some(format!("\"{}\"", kept.unwrap()).as_str()));
    let hat = hat_item(c.delete(&acme).await).await;
    assert!(hat.purging);
    assert_eq!(hat.logo, None);
}

/// Not under the browser rules alone: a request from another origin is
/// refused before the cookie is looked at, and a state change from no
/// origin at all is refused (kernel spec §3.3).
#[tokio::test]
async fn a_logo_is_changed_only_from_the_public_origin() {
    let c = Collector::start().await;
    let acme = c.hat("Acme");
    let cookie = c.cookie();
    let body = serde_json::to_string(&SetHatLogoRequest {
        data: b64(&png_of(1, 1, 1)),
    })
    .unwrap();
    for origin in [Some("https://evil.example"), None] {
        let mut req = reqwest::Client::new()
            .put(c.logo_url(&acme))
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body(body.clone());
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        assert_eq!(
            code_of(req.send().await.unwrap()).await,
            (403, "origin_mismatch".into())
        );
    }
    let resp = reqwest::Client::new()
        .put(c.logo_url(&acme))
        .header("cookie", &cookie)
        .header("origin", PUBLIC_URL)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    // A session opened as a login is: stepped up, so the change is made.
    assert_eq!(resp.status(), 200);
}
