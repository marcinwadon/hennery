//! Browser-only relay. No OAuth secrets, network client or collector credentials.
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hennery_kernel::operator::PublicUrl;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use url::{Host, Url};

/// Validated local callback. Keep the input verbatim for OAuth's exact match.
pub struct LocalCallback {
    url: Url,
}

impl LocalCallback {
    pub fn parse(input: &str) -> Result<Self> {
        if input.len() > crate::model::MAX_URL || input.chars().any(|c| c.is_control() || c.is_whitespace()) {
            bail!("invalid local callback URL");
        }
        let url = Url::parse(input).map_err(|_| anyhow::anyhow!("invalid local callback URL"))?;
        let loopback = match url.host() {
            Some(Host::Domain("localhost")) => true,
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if !matches!(url.scheme(), "http" | "https")
            || !loopback
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port_or_known_default() == Some(0)
            || input.contains('\\')
        {
            bail!(
                "local callback must be http or https on localhost or a loopback IP, without credentials, query or fragment"
            );
        }
        Ok(Self { url })
    }

    pub fn is_https(&self) -> bool {
        self.url.scheme() == "https"
    }

    /// The parsed loopback host, without IPv6 brackets, for the TLS SAN.
    pub fn certificate_host(&self) -> String {
        match self.url.host().expect("validated host") {
            Host::Domain(host) => host.to_string(),
            Host::Ipv4(ip) => ip.to_string(),
            Host::Ipv6(ip) => ip.to_string(),
        }
    }

    /// Bind literals only: localhost never goes through DNS or /etc/hosts.
    /// Both localhost families are served when IPv6 is available.
    pub async fn bind(&self) -> Result<Vec<tokio::net::TcpListener>> {
        let port = self.url.port_or_known_default().expect("http and https have ports");
        let ips = match self.url.host().expect("validated host") {
            Host::Domain(_) => vec![IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)],
            Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
            Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
        };
        let mut listeners = Vec::new();
        for ip in ips {
            match tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await {
                Ok(listener) => listeners.push(listener),
                Err(err) if ip.is_ipv6() && !listeners.is_empty()
                    && err.kind() == std::io::ErrorKind::AddrNotAvailable => {}
                Err(err) => return Err(err).context(
                    "cannot bind local callback: port may be in use; stop the other receiver or choose another callback port in both the UI and CLI"),
            }
        }
        Ok(listeners)
    }

    /// The destination is fixed before listening; callback input can only
    /// supply the raw response query. The collector performs all validation.
    pub fn router(&self, collector: &str) -> Result<Router> {
        let public = PublicUrl::parse(collector).map_err(|_| anyhow::anyhow!("invalid collector origin"))?;
        let target = format!("{}{}", public.origin(), crate::api::CALLBACK_PATH);
        let authority = &self.url[url::Position::BeforeHost..url::Position::AfterPort];
        Ok(Router::new().fallback(relay).with_state(Relay {
            path: self.url.path().to_string(),
            authority: authority.to_string(),
            target,
        }))
    }
}

#[derive(Clone)]
struct Relay {
    path: String,
    authority: String,
    target: String,
}

async fn relay(State(relay): State<Relay>, request: Request) -> Response {
    // A fixed Host also refuses DNS rebinding and requests to another local
    // virtual host. No trace/log middleware: the URI carries credentials.
    let mut response = if request.uri().path() != relay.path
        || request.headers().get(header::HOST).and_then(|v| v.to_str().ok()) != Some(relay.authority.as_str())
    {
        (StatusCode::NOT_FOUND, "Use the callback address shown by the receiver.").into_response()
    } else if request.method() != Method::GET {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "OAuth callback requires browser GET navigation.",
        )
            .into_response()
    } else if request.uri().query().is_none() {
        // A certificate exception can be accepted before OAuth puts a code
        // in this URL. This page neither starts nor completes an OAuth flow.
        (
            StatusCode::OK,
            "Local OAuth callback ready. Return to Hennery and click Connect (or Reconnect).",
        )
            .into_response()
    } else {
        let query = request.uri().query().unwrap_or_default();
        // Copy bytes, including duplicates, unknown keys and encoding. Only
        // the collector may interpret state, issuer, code and provider errors.
        let location = format!("{}?{query}", relay.target);
        match HeaderValue::from_str(&location) {
            Ok(location) => (StatusCode::FOUND, [(header::LOCATION, location)]).into_response(),
            Err(_) => (
                StatusCode::BAD_REQUEST,
                "Invalid callback response. Start Connect again.",
            )
                .into_response(),
        }
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    #[tokio::test]
    async fn listeners_bind_only_loopback_on_the_requested_port() {
        for host in ["localhost", "127.0.0.1"] {
            let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = reserved.local_addr().unwrap().port();
            drop(reserved);
            let local = LocalCallback::parse(&format!("http://{host}:{port}/callback")).unwrap();
            let listeners = local.bind().await.unwrap();
            assert!(!listeners.is_empty());
            for listener in listeners {
                let address = listener.local_addr().unwrap();
                assert!(
                    address.ip().is_loopback(),
                    "never expose callback responses on a LAN interface"
                );
                assert_eq!(address.port(), port);
            }
        }
    }

    #[test]
    fn https_callbacks_select_tls_for_the_loopback_certificate_host() {
        for (input, host) in [
            ("https://127.0.0.1:9882/callback", "127.0.0.1"),
            ("https://localhost:443/result", "localhost"),
            ("https://[::1]:9882/callback", "::1"),
        ] {
            let local = LocalCallback::parse(input).unwrap();
            assert!(local.is_https());
            assert_eq!(local.certificate_host(), host);
        }
        assert!(
            !LocalCallback::parse("http://localhost:8976/callback")
                .unwrap()
                .is_https()
        );
    }

    #[test]
    fn accepts_only_local_http_or_https_without_response_parameters() {
        for input in [
            "http://localhost:8999/result",
            "http://127.0.0.2:8999/result",
            "http://[::1]:8999/result",
            "http://localhost:80/result",
        ] {
            assert!(LocalCallback::parse(input).is_ok(), "{input}");
        }
        for input in [
            "https://localhost.evil:8999/result",
            "http://localhost.evil:8999/result",
            "http://0.0.0.0:8999/result",
            "http://[::]:8999/result",
            "http://192.168.1.1:8999/result",
            "http://user@localhost:8999/result",
            "http://localhost:8999/result?",
            "http://localhost:8999/result#",
            "http://localhost:0/result",
            "https://localhost:0/result",
            "ftp://localhost/result",
            "https://user@localhost/result",
            "https://127.0.0.1/result?code=secret",
            "https://127.0.0.1/result#fragment",
            "http://localhost:8999/\nresult",
            "http://localhost\\@evil.example/result",
        ] {
            assert!(LocalCallback::parse(input).is_err(), "{input:?}");
        }
    }

    #[tokio::test]
    async fn fixed_destination_preserves_raw_query_and_privacy_on_every_response() {
        let app = LocalCallback::parse("http://127.0.0.1:8999/result")
            .unwrap()
            .router("https://collector.example")
            .unwrap();
        let query =
            "code=c%2B%2F&state=a&state=b&iss=https%3A%2F%2Fissuer.example&target=https://evil.example&extra=%2520";
        for (method, host, path, status) in [
            ("GET", "127.0.0.1:8999", format!("/result?{query}"), StatusCode::FOUND),
            ("GET", "evil.example", "/result".into(), StatusCode::NOT_FOUND),
            ("GET", "127.0.0.1:8999", "/other".into(), StatusCode::NOT_FOUND),
            (
                "POST",
                "127.0.0.1:8999",
                "/result".into(),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header("host", host)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
            if status == StatusCode::FOUND {
                assert_eq!(
                    response.headers()[header::LOCATION],
                    format!("https://collector.example/api/mcp/oauth/callback?{query}")
                );
            } else {
                assert!(!response.headers().contains_key(header::LOCATION));
            }
        }
    }
}
