//! Bounded HTTP/1 connections and timed headers, including before routing.
use axum::{Extension, Router, extract::ConnectInfo};
use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
};
use hyper::{body::Incoming, server::conn::http1};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{
    collections::HashMap,
    io,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
pub struct Limits {
    pub connections: usize,
    pub header_timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            connections: 512,
            header_timeout: Duration::from_secs(10),
        }
    }
}

const MAX_HEADER_BUFFER: usize = 16 * 1024;

/// Only live reservations are kept: failed/expired clients cannot fill a
/// historical address table or evict another client's reservation.
pub struct Admission {
    total: usize,
    per_client: usize,
    clients: Mutex<HashMap<IpAddr, usize>>,
}
pub struct AdmissionPermit {
    admission: Arc<Admission>,
    client: IpAddr,
}
impl Admission {
    pub fn new(total: usize, per_client: usize) -> Self {
        assert!(total > 0 && per_client > 0);
        Self {
            total,
            per_client,
            clients: Mutex::new(HashMap::new()),
        }
    }
    pub fn try_acquire(self: &Arc<Self>, client: IpAddr) -> Option<AdmissionPermit> {
        let client = crate::ratelimit::key(client);
        let mut clients = self.clients.lock().expect("admission");
        if clients.values().sum::<usize>() >= self.total
            || clients.get(&client).copied().unwrap_or(0) >= self.per_client
        {
            return None;
        }
        *clients.entry(client).or_default() += 1;
        Some(AdmissionPermit {
            admission: self.clone(),
            client,
        })
    }
}
impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let mut clients = self.admission.clients.lock().expect("admission");
        if let Some(count) = clients.get_mut(&self.client) {
            *count -= 1;
            if *count == 0 {
                clients.remove(&self.client);
            }
        }
    }
}

/// Install inside client-address resolution. The permit lives through the body,
/// including SSE; returning a response head does not free streaming capacity.
pub fn request_limits(app: Router) -> Router {
    // One existing gateway connection permits 64 requests plus 32 streams;
    // leave room for browser traffic beside it without lowering that contract.
    app.layer(middleware::from_fn_with_state(
        Arc::new(Admission::new(512, 128)),
        admit_request,
    ))
}
async fn admit_request(
    State(admission): State<Arc<Admission>>,
    address: crate::client_address::ClientAddress,
    request: Request,
    next: Next,
) -> Response {
    if matches!(request.uri().path(), "/healthz" | "/readyz")
        && matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD)
    {
        // Liveness stays cheap; readiness has its own blocking-work permits.
        return next.run(request).await;
    }
    let Some(permit) = admission.try_acquire(address.effective) else {
        let mut response = crate::auth_api::error(StatusCode::SERVICE_UNAVAILABLE, "busy", "request capacity reached");
        response
            .headers_mut()
            .insert("retry-after", "1".parse().expect("fixed header"));
        return response;
    };
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(AdmittedBody {
            inner: body,
            permit: Some(permit),
        }),
    )
}
struct AdmittedBody {
    inner: Body,
    permit: Option<AdmissionPermit>,
}
impl hyper::body::Body for AdmittedBody {
    type Data = axum::body::Bytes;
    type Error = axum::Error;
    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let frame = std::pin::Pin::new(&mut this.inner).poll_frame(cx);
        if matches!(frame, std::task::Poll::Ready(None | Some(Err(_)))) {
            this.permit.take();
        }
        frame
    }
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}
pub async fn serve_all(
    listeners: Vec<TcpListener>,
    app: Router,
    shutdown: CancellationToken,
    limits: Limits,
) -> io::Result<()> {
    assert!(limits.connections > 0);
    let permits = Arc::new(Semaphore::new(limits.connections));
    let mut servers = JoinSet::new();
    for listener in listeners {
        let (app, shutdown, permits) = (app.clone(), shutdown.clone(), permits.clone());
        servers.spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! { biased;
                    () = shutdown.cancelled() => break,
                    Some(_) = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept() => {
                        let (socket, peer) = match accepted {
                            Ok(accepted)=>accepted,
                            Err(error) if matches!(error.kind(),io::ErrorKind::ConnectionAborted|io::ErrorKind::ConnectionReset|io::ErrorKind::Interrupted)=>continue,
                            Err(error) if matches!(error.raw_os_error(),Some(libc::EBADF|libc::EINVAL|libc::ENOTSOCK))=>return Err(error),
                            // A temporary OS resource shortage must not turn a
                            // connection burst into a collector process exit.
                            Err(_)=>{
                                tokio::select! {
                                    () = shutdown.cancelled() => break,
                                    () = tokio::time::sleep(Duration::from_millis(100)) => {}
                                }
                                continue;
                            }
                        };
                        let Ok(permit) = permits.clone().try_acquire_owned() else { drop(socket); continue; };
                        let service = app.clone().layer(Extension(ConnectInfo(peer))).into_service::<Incoming>();
                        let shutdown = shutdown.clone();
                        connections.spawn(async move {
                            let _permit = permit;
                            let io = TokioIo::new(socket);
                            let mut builder = http1::Builder::new();
                            builder.timer(TokioTimer::new()).header_read_timeout(limits.header_timeout).max_buf_size(MAX_HEADER_BUFFER);
                            let connection = builder.serve_connection(io, TowerToHyperService::new(service)).with_upgrades();
                            tokio::pin!(connection);
                            tokio::select! {
                                _ = connection.as_mut() => {},
                                () = shutdown.cancelled() => {
                                    connection.as_mut().graceful_shutdown();
                                    let _ = connection.await;
                                }
                            }
                        });
                    }
                }
            }
            while connections.join_next().await.is_some() {}
            Ok::<_, io::Error>(())
        });
    }
    let mut result = Ok(());
    while let Some(done) = servers.join_next().await {
        if let Err(error) = done.map_err(io::Error::other).and_then(|r| r) {
            shutdown.cancel();
            if result.is_ok() {
                result = Err(error);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_is_global_per_prefix_and_releases_live_entries() {
        let admission = Arc::new(Admission::new(2, 1));
        let a = admission.try_acquire("2001:db8:1::1".parse().unwrap()).unwrap();
        assert!(admission.try_acquire("2001:db8:1::2".parse().unwrap()).is_none());
        let b = admission.try_acquire("192.0.2.2".parse().unwrap()).unwrap();
        assert!(admission.try_acquire("192.0.2.3".parse().unwrap()).is_none());
        drop(a);
        let c = admission.try_acquire("192.0.2.3".parse().unwrap()).unwrap();
        drop((b, c));
        assert!(admission.clients.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn response_body_retains_capacity_and_preserves_its_size_hint() {
        use tower::ServiceExt;
        let admission = Arc::new(Admission::new(2, 1));
        let app = Router::new()
            .route("/healthz", axum::routing::get(|| async { "ok" }))
            .route(
                "/events",
                axum::routing::get(|| async {
                    Body::from_stream(futures::stream::pending::<Result<axum::body::Bytes, io::Error>>())
                }),
            )
            .route("/fixed", axum::routing::get(|| async { "fixed" }))
            .layer(middleware::from_fn_with_state(admission.clone(), admit_request));
        let request = |ip: &str, path: &str| {
            Request::builder()
                .uri(path)
                .extension(ConnectInfo(std::net::SocketAddr::new(ip.parse().unwrap(), 1234)))
                .body(Body::empty())
                .unwrap()
        };
        let a = app.clone().oneshot(request("192.0.2.1", "/events")).await.unwrap();
        let b = app.clone().oneshot(request("192.0.2.2", "/events")).await.unwrap();
        assert_eq!(a.status(), StatusCode::OK);
        assert_eq!(b.status(), StatusCode::OK);
        assert_eq!(
            app.clone()
                .oneshot(request("192.0.2.1", "/fixed"))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            app.clone()
                .oneshot(request("192.0.2.1", "/healthz"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(request("192.0.2.3", "/fixed"))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        drop(a);
        let fixed = app.clone().oneshot(request("192.0.2.1", "/fixed")).await.unwrap();
        assert_eq!(fixed.status(), StatusCode::OK);
        assert_eq!(hyper::body::Body::size_hint(fixed.body()).exact(), Some(5));
        drop((b, fixed));
        assert!(admission.clients.lock().unwrap().is_empty());
    }
}
