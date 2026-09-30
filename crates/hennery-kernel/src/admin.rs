//! The admin socket (kernel spec §4.2): `<data>/admin.sock`, a Unix socket
//! only the collector's own user may use, for `hennery admin …` recovery
//! commands that work without a browser or a session: print the setup link,
//! reset the password or `public_url`, list the hosts, mint a pairing code.
//!
//! - **Who may connect:** the socket is 0600, and a peer whose user id is
//!   not the collector's is dropped without an answer. The client refuses,
//!   in turn, a socket served by another user. **Any process of the
//!   collector's own user is the operator here**, agents of the all-in-one
//!   install included (kernel spec §10): it can reset the password or
//!   `public_url` and mint pairing codes. The CLI's confirmation on a
//!   terminal stops accidents, not a process that speaks this protocol.
//! - **The protocol:** one JSON `AdminRequest` on one line, at most
//!   `MAX_REQUEST_BYTES`, within `REQUEST_TIMEOUT`; one JSON `AdminResponse`
//!   on one line back; then the connection closes.
//! - **One collector per data directory:** binding refuses a socket that
//!   still answers, replaces one that refuses the connection (a collector
//!   that was killed), and stops at any other error. The socket is removed
//!   when its `AdminSocket` is dropped.

use crate::hosts::Hosts;
use crate::operator::{Operator, Reset};
use crate::secret::unix_now;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// The socket's name in the collector's data directory (kernel spec §1).
pub const ADMIN_SOCKET: &str = "admin.sock";

/// The longest request line read, newline included.
pub const MAX_REQUEST_BYTES: u64 = 16 * 1024;

/// A connection that has not sent its whole request by then, or not taken
/// its whole answer, is dropped.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the server waits after a failed accept (out of descriptors,
/// say) before it tries again, so it does not spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// The longest response the client reads: a long host list fits.
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AdminRequest {
    /// The live setup link, or a fresh one (`Operator::setup_link`).
    SetupUrl,
    /// A new password for the owner; every session ends.
    ResetPassword {
        password: String,
    },
    ListHosts,
    MintPairingCode,
    /// A new `public_url`; every session ends (3b decision 4's recovery).
    ResetPublicUrl {
        public_url: String,
    },
}

impl AdminRequest {
    /// Whether it changes state or hands out a credential: logged at
    /// `warn`, with the peer's process id.
    pub fn changes_state(&self) -> bool {
        !matches!(self, Self::SetupUrl | Self::ListHosts)
    }

    /// The command's name, for the log: never its arguments.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SetupUrl => "setup_url",
            Self::ResetPassword { .. } => "reset_password",
            Self::ListHosts => "list_hosts",
            Self::MintPairingCode => "mint_pairing_code",
            Self::ResetPublicUrl { .. } => "reset_public_url",
        }
    }
}

/// Written by hand: the password is left out.
impl std::fmt::Debug for AdminRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResetPassword { .. } => f
                .debug_struct("ResetPassword")
                .field("password", &format_args!("<redacted>"))
                .finish(),
            Self::ResetPublicUrl { public_url } => f
                .debug_struct("ResetPublicUrl")
                .field("public_url", public_url)
                .finish(),
            other => f.write_str(other.name()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum AdminResponse {
    SetupUrl {
        url: String,
    },
    /// `setup_url` once there is an owner.
    AlreadySetUp,
    /// A reset before there is an owner: setup is the way in.
    NotSetUp,
    PasswordReset {
        sessions_ended: usize,
    },
    PublicUrlReset {
        public_url: String,
        sessions_ended: usize,
    },
    Hosts {
        hosts: Vec<AdminHost>,
    },
    PairingCode {
        code: String,
        expires_at: i64,
    },
    /// The request is not acceptable (why); nothing changed.
    Refused {
        message: String,
    },
    /// The collector failed to carry it out.
    Failed {
        message: String,
    },
}

/// Written by hand: the setup link and the pairing code are credentials,
/// and are left out.
impl std::fmt::Debug for AdminResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = format_args!("<redacted>");
        match self {
            Self::SetupUrl { .. } => f.debug_struct("SetupUrl").field("url", &redacted).finish(),
            Self::AlreadySetUp => f.write_str("AlreadySetUp"),
            Self::NotSetUp => f.write_str("NotSetUp"),
            Self::PasswordReset { sessions_ended } => f
                .debug_struct("PasswordReset")
                .field("sessions_ended", sessions_ended)
                .finish(),
            Self::PublicUrlReset {
                public_url,
                sessions_ended,
            } => f
                .debug_struct("PublicUrlReset")
                .field("public_url", public_url)
                .field("sessions_ended", sessions_ended)
                .finish(),
            Self::Hosts { hosts } => f.debug_struct("Hosts").field("hosts", hosts).finish(),
            Self::PairingCode { expires_at, .. } => f
                .debug_struct("PairingCode")
                .field("code", &redacted)
                .field("expires_at", expires_at)
                .finish(),
            Self::Refused { message } => f.debug_struct("Refused").field("message", message).finish(),
            Self::Failed { message } => f.debug_struct("Failed").field("message", message).finish(),
        }
    }
}

/// One paired host, as `list_hosts` shows it. Times are seconds since the
/// Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminHost {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub host_version: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// What the admin commands act on: the router's own `Operator` and
/// `Hosts`, never a second instance, since the cached `public_url` and the
/// setup token live in that one.
#[derive(Clone)]
pub struct Admin {
    pub operator: Arc<Operator>,
    pub hosts: Arc<Hosts>,
    /// The data directory, where a fresh setup link is written.
    pub dir: PathBuf,
    /// The setup link's base (kernel spec §3.1).
    pub base_url: String,
}

/// A bound admin socket. Dropping it removes the socket file, however the
/// collector stops.
pub struct AdminSocket {
    listener: tokio::net::UnixListener,
    path: PathBuf,
}

impl AdminSocket {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AdminSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The longest path a Unix socket address holds, its terminating NUL
/// included (104 bytes on macOS, 108 on Linux).
fn max_socket_path() -> usize {
    // SAFETY: all-zero bytes are a valid `sockaddr_un`.
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len()
}

/// Bind `dir/admin.sock`, mode 0600. `Ok(None)`, with a warning, when that
/// path is too long for a Unix socket: the collector then runs without it.
/// Refused when the socket there still answers: another collector serves
/// this data directory. One that refuses the connection is replaced; any
/// other error (not a socket, not ours to connect to) stops the start and
/// is named.
pub fn bind(dir: &Path) -> Result<Option<AdminSocket>> {
    let path = dir.join(ADMIN_SOCKET);
    if path.as_os_str().len() >= max_socket_path() {
        tracing::warn!(
            path = %path.display(),
            "the data directory's path is too long for a Unix socket: `hennery admin` cannot reach this collector"
        );
        return Ok(None);
    }
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(_) => bail!(
            "{} answers: another collector serves this data directory",
            path.display()
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        // Left by a collector that did not shut down cleanly.
        Err(err) if err.raw_os_error() == Some(libc::ECONNREFUSED) => {
            std::fs::remove_file(&path).with_context(|| format!("remove the stale {}", path.display()))?
        }
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "{} is there and cannot be checked; remove it if no collector serves this data directory",
                    path.display()
                )
            });
        }
    }
    let listener = std::os::unix::net::UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("make {} private", path.display()))?;
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    Ok(Some(AdminSocket { listener, path }))
}

/// Answer admin commands until `shutdown` resolves, then remove the socket
/// (by dropping it).
pub async fn serve(socket: AdminSocket, admin: Admin, shutdown: impl std::future::Future<Output = ()>) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = socket.listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let admin = admin.clone();
                    tokio::spawn(async move {
                        if let Err(err) = answer(stream, &admin).await {
                            tracing::warn!(error = %format!("{err:#}"), "admin socket connection");
                        }
                    });
                }
                Err(err) => {
                    tracing::warn!(error = %err, "admin socket accept");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
            () = &mut shutdown => break,
        }
    }
    drop(socket);
}

/// One connection: check the peer, read its request, answer it.
async fn answer(mut stream: tokio::net::UnixStream, admin: &Admin) -> Result<()> {
    let peer = stream.peer_cred().context("the peer's credentials")?;
    // SAFETY: geteuid(2) cannot fail.
    let own = unsafe { libc::geteuid() };
    if peer.uid() != own {
        tracing::warn!(
            peer = peer.uid(),
            "admin socket: refused a connection from another user"
        );
        return Ok(());
    }
    let peer_pid = peer.pid();
    let (read, mut write) = stream.split();
    let mut line = String::new();
    let mut reader = BufReader::new(read.take(MAX_REQUEST_BYTES));
    let response = match tokio::time::timeout(REQUEST_TIMEOUT, reader.read_line(&mut line)).await {
        Err(_) => bail!("no request within {REQUEST_TIMEOUT:?}"),
        Ok(Err(err)) => refused(format!("the request is not a line of UTF-8: {err}")),
        Ok(Ok(_)) if !line.ends_with('\n') => refused(format!(
            "the request must be one line of at most {MAX_REQUEST_BYTES} bytes"
        )),
        Ok(Ok(_)) => match serde_json::from_str::<AdminRequest>(&line) {
            Ok(request) => {
                if request.changes_state() {
                    tracing::warn!(command = request.name(), ?peer_pid, "admin command");
                } else {
                    tracing::info!(command = request.name(), ?peer_pid, "admin command");
                }
                carry_out(request, admin).await
            }
            Err(err) => refused(format!("not an admin request: {err}")),
        },
    };
    let mut out = serde_json::to_vec(&response)?;
    out.push(b'\n');
    let sent = tokio::time::timeout(REQUEST_TIMEOUT, async {
        write.write_all(&out).await?;
        write.shutdown().await
    });
    match sent.await {
        Ok(result) => result?,
        Err(_) => bail!("the answer was not taken within {REQUEST_TIMEOUT:?}"),
    }
    Ok(())
}

fn refused(message: String) -> AdminResponse {
    AdminResponse::Refused { message }
}

async fn carry_out(request: AdminRequest, admin: &Admin) -> AdminResponse {
    let now = unix_now();
    let outcome = match request {
        AdminRequest::SetupUrl => admin
            .operator
            .setup_link(&admin.dir, &admin.base_url, now)
            .map(|link| match link {
                Some(link) => AdminResponse::SetupUrl { url: link.url },
                None => AdminResponse::AlreadySetUp,
            }),
        AdminRequest::ResetPassword { password } => {
            admin
                .operator
                .reset_password(password, now)
                .await
                .map(|reset| match reset {
                    Reset::Done { sessions_ended } => AdminResponse::PasswordReset { sessions_ended },
                    Reset::NotSetUp => AdminResponse::NotSetUp,
                    Reset::Invalid(message) => AdminResponse::Refused { message },
                })
        }
        AdminRequest::ListHosts => admin.hosts.list().map(|records| AdminResponse::Hosts {
            hosts: records
                .into_iter()
                .map(|r| AdminHost {
                    id: r.id,
                    name: r.name,
                    platform: r.platform,
                    host_version: r.host_version,
                    created_at: r.created_at,
                    last_seen_at: r.last_seen_at,
                    revoked_at: r.revoked_at,
                })
                .collect(),
        }),
        AdminRequest::MintPairingCode => admin
            .hosts
            .mint_pairing_code(now)
            .map(|code| AdminResponse::PairingCode {
                code: code.code,
                expires_at: code.expires_at,
            }),
        AdminRequest::ResetPublicUrl { public_url } => {
            admin.operator.reset_public_url(&public_url).map(|reset| match reset {
                Reset::Done { sessions_ended } => AdminResponse::PublicUrlReset {
                    public_url: admin
                        .operator
                        .public_url()
                        .map(|url| url.origin().to_string())
                        .unwrap_or_default(),
                    sessions_ended,
                },
                Reset::NotSetUp => AdminResponse::NotSetUp,
                Reset::Invalid(message) => AdminResponse::Refused { message },
            })
        }
    };
    outcome.unwrap_or_else(|err| AdminResponse::Failed {
        message: format!("{err:#}"),
    })
}

/// The client's side: send `request` to the collector at `socket` and read
/// its answer, one line. Not to the end of the stream: a collector that
/// closes with part of a request unread resets the connection on Linux,
/// after its answer.
pub async fn request(socket: &Path, request: &AdminRequest) -> Result<AdminResponse> {
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect to {} (is the collector running?)", socket.display()))?;
    // The other way round too: a password is sent only to a collector of
    // this user's.
    let server = stream.peer_cred().context("the collector's credentials")?.uid();
    // SAFETY: geteuid(2) cannot fail.
    let own = unsafe { libc::geteuid() };
    if server != own {
        bail!(
            "{} is served by user id {server}, not by this user ({own}); nothing was sent",
            socket.display()
        );
    }
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line).await?;
    read_answer(&mut stream).await
}

/// One `AdminResponse` line from `stream`, at most `MAX_RESPONSE_BYTES`.
pub async fn read_answer(stream: &mut tokio::net::UnixStream) -> Result<AdminResponse> {
    let mut answer = String::new();
    let read = BufReader::new(stream.take(MAX_RESPONSE_BYTES))
        .read_line(&mut answer)
        .await
        .context("read the collector's answer")?;
    // The connection closed before a single byte arrived: a reset in flight
    // when the collector stopped, or a peer dropped for a uid mismatch.
    // Serde's own message for an empty input ("EOF while parsing a value")
    // reads like a parse bug, not like what happened.
    if read == 0 {
        bail!(
            "the collector closed the connection without answering (it may have stopped); \
             the command's outcome is unknown"
        );
    }
    serde_json::from_str(&answer).context("the collector's answer")
}
