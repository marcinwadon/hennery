//! TLS for the disposable loopback callback. No trust-store or disk writes.
use anyhow::{Context, Result};
use rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer};
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

pub fn configuration(host: &str) -> Result<Arc<ServerConfig>> {
    let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec![host.to_string()])
        .context("cannot generate local callback certificate")?;
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )?;
    Ok(Arc::new(config))
}

pub struct Listener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    handshakes: JoinSet<Option<(TlsStream<TcpStream>, SocketAddr)>>,
}

impl Listener {
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> Self {
        Self {
            tcp,
            acceptor: TlsAcceptor::from(config),
            handshakes: JoinSet::new(),
        }
    }
}

impl axum::serve::Listener for Listener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                // A stalled or rejected handshake must not block the browser's
                // callback or kill its listener. Bound both work and duration.
                completed = self.handshakes.join_next(), if !self.handshakes.is_empty() => {
                    if let Some(Ok(Some(connection))) = completed {
                        return connection;
                    }
                }
                accepted = self.tcp.accept(), if self.handshakes.len() < 16 => {
                    match accepted {
                        Ok((stream, address)) => {
                            let acceptor = self.acceptor.clone();
                            self.handshakes.spawn(async move {
                                tokio::time::timeout(Duration::from_secs(5), acceptor.accept(stream))
                                    .await.ok()?.ok().map(|tls| (tls, address))
                            });
                        }
                        Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}
