//! The shipped Caddy/nginx recipes, with fixture-only placeholder substitution.
use crate::tls::TestCa;
use std::fs::{File, OpenOptions};
use std::ops::RangeInclusive;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::sync::{Mutex, OnceLock};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

pub struct RecipeProxy {
    kind: String,
    root: PathBuf,
    port: u16,
    admin: u16,
    child: Child,
}
fn lease_port() -> u16 {
    lease_port_from(None)
}

// Retain leases until process exit, including while a proxy is stopped.
// flock also excludes other test binaries/dev shells; a :0 bind elsewhere
// cannot consume these ports because the ephemeral ranges are excluded.
static PORT_LEASES: Mutex<Vec<File>> = Mutex::new(Vec::new());

fn lease_port_from(preferred: Option<u16>) -> u16 {
    static RANGES: OnceLock<Vec<RangeInclusive<u16>>> = OnceLock::new();
    let ranges = RANGES.get_or_init(ephemeral_ranges);
    // Do not use temp_dir(): nix develop gives each shell its own TMPDIR.
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let root = PathBuf::from(format!("/tmp/hennery-testkit-proxy-ports-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&root) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => panic!("create proxy port lease directory: {err}"),
    }
    let metadata = std::fs::symlink_metadata(&root).unwrap();
    assert!(
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o077 == 0,
        "proxy port lease directory must be private and owned by this user"
    );
    let mut leases = PORT_LEASES.lock().unwrap();
    // The preferred candidate is a deterministic contention seam; normal
    // callers scan the same non-privileged range on every supported OS.
    for port in preferred.into_iter().chain(10000..=u16::MAX) {
        if port < 10000 || ranges.iter().any(|range| range.contains(&port)) {
            continue;
        }
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(port.to_string()))
            .unwrap();
        // SAFETY: lease owns a live descriptor. Nonblocking locking is
        // candidate selection, never a retry of a failed proxy startup.
        if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let err = std::io::Error::last_os_error();
            assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock, "proxy port lease: {err}");
            continue;
        }
        if !loopbacks_available(port) {
            continue;
        }
        leases.push(lease);
        return port;
    }
    panic!("no proxy port available outside the ephemeral ranges: {ranges:?}");
}

fn loopbacks_available(port: u16) -> bool {
    loopbacks_available_after_probe(port, || {})
}

fn loopbacks_available_after_probe(port: u16, after_probe: impl FnOnce()) -> bool {
    // Never bind a temporary listener: a concurrent fork can inherit it
    // until exec, keeping the port busy after this function returns.
    // Refusal on both families means no existing TCP listener; every
    // other outcome conservatively excludes this candidate.
    let available = ["127.0.0.1", "::1"].into_iter().all(|ip| {
        matches!(
            std::net::TcpStream::connect_timeout(
                &SocketAddr::new(ip.parse().unwrap(), port),
                Duration::from_millis(100),
            ),
            Err(err) if err.kind() == std::io::ErrorKind::ConnectionRefused
        )
    });
    after_probe();
    available
}

fn ephemeral_ranges() -> Vec<RangeInclusive<u16>> {
    #[cfg(target_os = "linux")]
    {
        vec![parse_ephemeral_range(
            &std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range").unwrap(),
        )]
    }
    #[cfg(target_os = "macos")]
    {
        fn port(name: &std::ffi::CStr) -> u16 {
            let mut value: libc::c_int = 0;
            let mut size = std::mem::size_of_val(&value);
            // SAFETY: name is NUL-terminated and value/size are valid
            // writable buffers of the stated sizes; no new value is set.
            let result = unsafe {
                libc::sysctlbyname(
                    name.as_ptr(),
                    (&raw mut value).cast(),
                    &mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(result, 0, "read {name:?}: {}", std::io::Error::last_os_error());
            assert_eq!(size, std::mem::size_of_val(&value));
            u16::try_from(value).unwrap()
        }
        [
            (c"net.inet.ip.portrange.first", c"net.inet.ip.portrange.last"),
            (c"net.inet.ip.portrange.hifirst", c"net.inet.ip.portrange.hilast"),
        ]
        .map(|(first, last)| {
            let (first, last) = (port(first), port(last));
            first.min(last)..=first.max(last)
        })
        .into()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    panic!("proxy fixtures require a known OS ephemeral port range");
}

#[cfg(any(target_os = "linux", test))]
fn parse_ephemeral_range(text: &str) -> RangeInclusive<u16> {
    let ports: Vec<u16> = text.split_whitespace().map(|word| word.parse().unwrap()).collect();
    assert_eq!(ports.len(), 2, "kernel ephemeral port range");
    assert!(ports[0] <= ports[1], "kernel ephemeral port range order");
    ports[0]..=ports[1]
}
impl RecipeProxy {
    fn write_certificates(root: &Path, ca: &TestCa) {
        let (cert, key) = ca.server_pem();
        for (name, pem) in [("cert.pem", cert), ("key.pem", key)] {
            // nginx can inherit the old SSL object when inode+mtime match.
            // Rename a complete new file, including within one timestamp tick.
            let staged = root.join(format!("{name}.next"));
            std::fs::write(&staged, pem).unwrap();
            std::fs::rename(staged, root.join(name)).unwrap();
        }
    }
    fn command(kind: &str, root: &Path) -> Command {
        let mut command = Command::new(kind);
        if kind == "nginx" {
            command.arg("-e").arg(root.join("nginx-error.log"));
        }
        command
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env_remove("SSL_CERT_FILE")
            .env_remove("SSL_CERT_DIR")
            .stdin(Stdio::null());
        command
    }
    pub async fn start(kind: &str, backend: SocketAddr, ca: &TestCa, root: &Path) -> Self {
        Self::start_with_idle_timeout(kind, backend, ca, root, Duration::from_secs(120)).await
    }
    /// Scale the recipe's idle and request-body durations for the heartbeat
    /// check. Caddy's keepalive is for pooled upstream connections; nginx's
    /// read/send timeouts also apply to upgraded WebSocket connections.
    pub async fn start_with_idle_timeout(
        kind: &str,
        backend: SocketAddr,
        ca: &TestCa,
        root: &Path,
        idle_timeout: Duration,
    ) -> Self {
        assert!(matches!(kind, "caddy" | "nginx"));
        assert!(!idle_timeout.is_zero() && idle_timeout.subsec_nanos() == 0);
        let root = root.join(kind);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let version = Self::command(kind, &root)
            .arg(if kind == "caddy" { "version" } else { "-v" })
            .output()
            .expect("proxy binary required: run in nix develop");
        assert!(version.status.success());
        eprintln!(
            "proxy version: {}{}",
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
        let port = lease_port();
        let admin = lease_port();
        Self::write_certificates(&root, ca);
        let recipe = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/proxy")
            .join(if kind == "caddy" { "Caddyfile" } else { "nginx.conf" });
        let mut config = std::fs::read_to_string(recipe).unwrap();
        for (placeholder, value) in [
            ("@TLS_PORT@", port.to_string()),
            ("@ADMIN_PORT@", admin.to_string()),
            ("@BACKEND_PORT@", backend.port().to_string()),
            ("@ROOT@", root.display().to_string()),
            ("@CERT@", root.join("cert.pem").display().to_string()),
            ("@KEY@", root.join("key.pem").display().to_string()),
            ("@BIND_V4@", "127.0.0.1".into()),
            ("@BIND_V6@", "::1".into()),
        ] {
            config = config.replace(placeholder, &value);
        }
        assert!(!config.contains('@'), "unsubstituted recipe placeholder");
        for directive in if kind == "caddy" {
            &["keepalive"][..]
        } else {
            &["proxy_read_timeout", "proxy_send_timeout"][..]
        } {
            let shipped = format!("{directive} 120s");
            assert_eq!(config.matches(&shipped).count(), 1, "recipe idle duration changed");
            config = config.replace(&shipped, &format!("{directive} {}s", idle_timeout.as_secs()));
        }
        if kind == "caddy" {
            assert_eq!(
                config.matches("read_body 30s").count(),
                1,
                "recipe body deadline changed"
            );
            config = config.replace(
                "read_body 30s",
                &format!("read_body {}s", (idle_timeout.as_secs() / 4).max(1)),
            );
        }
        let file = root.join(if kind == "caddy" { "Caddyfile" } else { "nginx.conf" });
        std::fs::write(&file, config).unwrap();
        let mut validate = Self::command(kind, &root);
        if kind == "caddy" {
            validate.args(["validate", "--config"]).arg(&file);
        } else {
            validate.arg("-t").arg("-p").arg(&root).arg("-c").arg(&file);
        }
        let out = validate.output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let child = Self::spawn(kind, &root);
        let mut proxy = Self {
            kind: kind.into(),
            root,
            port,
            admin,
            child,
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            assert!(
                proxy.child.try_wait().unwrap().is_none(),
                "proxy exited: {}",
                proxy.logs()
            );
            if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "proxy start: {}", proxy.logs());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        proxy
    }
    fn spawn(kind: &str, root: &Path) -> Child {
        let file = root.join(if kind == "caddy" { "Caddyfile" } else { "nginx.conf" });
        let mut command = Self::command(kind, root);
        if kind == "caddy" {
            command.args(["run", "--config"]).arg(&file);
        } else {
            command
                .arg("-p")
                .arg(root)
                .arg("-c")
                .arg(&file)
                .args(["-g", "daemon off;"]);
        }
        let log = std::fs::File::create(root.join("process.log")).unwrap();
        command.stdout(log.try_clone().unwrap()).stderr(log);
        command.process_group(0).spawn().unwrap()
    }
    pub fn pause(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        // SAFETY: this fixture created and owns the entire process group.
        unsafe { libc::kill(-(self.child.id() as i32), libc::SIGTERM) };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.child.try_wait().unwrap().is_none() {
            // SAFETY: only this fixture's still-running, unreaped group.
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        }
        let _ = self.child.wait();
    }
    pub async fn resume(&mut self, ca: &TestCa) {
        assert!(self.child.try_wait().unwrap().is_some(), "pause before resume");
        Self::write_certificates(&self.root, ca);
        self.child = Self::spawn(&self.kind, &self.root);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "proxy exited: {}",
                self.logs()
            );
            if tokio::net::TcpStream::connect(("127.0.0.1", self.port)).await.is_ok() {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    /// Abrupt transport loss, with no graceful wait that could become a timeout.
    pub fn cut(&mut self) {
        assert!(self.child.try_wait().unwrap().is_none());
        // SAFETY: this fixture created and owns the unreaped process group.
        assert_eq!(unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) }, 0);
        self.child.wait().unwrap();
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn origin(&self) -> String {
        format!("https://localhost:{}", self.port)
    }
    pub fn logs(&self) -> String {
        ["process.log", "nginx-error.log"]
            .into_iter()
            .map(|file| std::fs::read_to_string(self.root.join(file)).unwrap_or_default())
            .collect()
    }
    pub async fn renew_and_reload(&mut self, ca: &TestCa) {
        let old = self.leaf(ca).await;
        Self::write_certificates(&self.root, ca);
        if self.kind == "caddy" {
            let out = Self::command("caddy", &self.root)
                .args(["reload", "--force", "--address"])
                .arg(format!("127.0.0.1:{}", self.admin))
                .arg("--config")
                .arg(self.root.join("Caddyfile"))
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        } else {
            // SAFETY: only the master process started and owned by this fixture.
            assert_eq!(unsafe { libc::kill(self.child.id() as i32, libc::SIGHUP) }, 0);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self.leaf(ca).await == old {
            assert!(
                tokio::time::Instant::now() < deadline,
                "reload did not install the renewed leaf: {}",
                self.logs()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn leaf(&self, ca: &TestCa) -> Vec<u8> {
        use rustls::pki_types::pem::PemObject;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from_pem_slice(ca.pem.as_bytes()).unwrap())
            .unwrap();
        let config =
            rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        let tls = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config))
            .connect("localhost".try_into().unwrap(), tcp)
            .await
            .unwrap();
        tls.get_ref().1.peer_certificates().unwrap()[0].as_ref().to_vec()
    }
}
impl Drop for RecipeProxy {
    fn drop(&mut self) {
        self.pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[tokio::test]
    async fn paused_caddy_admin_port_is_not_reissued() {
        paused_port_is_not_reissued("caddy", true).await;
    }

    #[tokio::test]
    async fn paused_caddy_tls_port_is_not_reissued() {
        paused_port_is_not_reissued("caddy", false).await;
    }

    #[tokio::test]
    async fn paused_nginx_tls_port_is_not_reissued() {
        paused_port_is_not_reissued("nginx", false).await;
    }

    #[test]
    fn proxy_ports_exclude_ephemeral_range_boundaries() {
        for range in ephemeral_ranges() {
            for port in [*range.start(), *range.end()] {
                assert!(!range.contains(&lease_port_from(Some(port))));
            }
        }
        assert_eq!(parse_ephemeral_range("32768\t60999\n"), 32768..=60999);
    }

    #[test]
    fn proxy_port_probe_checks_both_loopbacks() {
        let port = lease_port();
        assert!(loopbacks_available(port));
        for ip in ["127.0.0.1", "::1"] {
            let busy = std::net::TcpListener::bind((ip, port)).unwrap();
            assert!(!loopbacks_available(port), "port occupied on {ip}");
            drop(busy);
        }
    }

    #[test]
    fn a_fork_during_the_probe_cannot_keep_the_proxy_port_bound() {
        let port = lease_port();
        let mut child = None;
        let available = loopbacks_available_after_probe(port, || {
            let mut pipe = [-1; 2];
            // SAFETY: pipe points to two writable descriptors. After fork,
            // the child uses only async-signal-safe libc calls, then _exit.
            unsafe {
                assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
                let pid = libc::fork();
                assert!(pid >= 0);
                if pid == 0 {
                    libc::close(pipe[1]);
                    let mut byte = 0u8;
                    libc::read(pipe[0], (&raw mut byte).cast(), 1);
                    libc::_exit(0);
                }
                libc::close(pipe[0]);
                child = Some((pid, pipe[1]));
            }
        });
        // The child is held before exec/exit. CLOEXEC cannot close any
        // inherited probe listener yet; the real proxy must still bind.
        let bound = std::net::TcpListener::bind(("127.0.0.1", port));
        let (pid, release) = child.expect("probe completed");
        // SAFETY: only our pipe and child; release before any assertion so
        // a regression cannot leave the child blocked or unreaped.
        unsafe {
            libc::write(release, b"x".as_ptr().cast(), 1);
            libc::close(release);
            assert_eq!(libc::waitpid(pid, std::ptr::null_mut(), 0), pid);
        }
        assert!(available);
        assert!(bound.is_ok(), "a fork inherited the temporary port probe: {bound:?}");
    }

    #[test]
    fn proxy_ports_are_leased_across_processes() {
        const CANDIDATE: &str = "HENNERY_TEST_PROXY_PORT_CANDIDATE";
        if let Ok(port) = std::env::var(CANDIDATE) {
            let port = port.parse().unwrap();
            assert_ne!(
                lease_port_from(Some(port)),
                port,
                "another process owns this port lease"
            );
            return;
        }
        let port = lease_port();
        let dir = tempfile::tempdir().unwrap();
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "reverse_proxy::tests::proxy_ports_are_leased_across_processes",
            ])
            .env(CANDIDATE, port.to_string())
            .env("TMPDIR", dir.path())
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path())
            .env("XDG_DATA_HOME", dir.path())
            .env("XDG_CACHE_HOME", dir.path())
            .env("XDG_STATE_HOME", dir.path())
            .env_remove("HENNERY_DATA_DIR")
            .env_remove("HENNERY_HOST_DATA_DIR")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "child lease check: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn paused_port_is_not_reissued(kind: &str, admin: bool) {
        let dir = tempfile::tempdir().unwrap();
        let ca = TestCa::new();
        let backend = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut proxy = RecipeProxy::start(kind, backend.local_addr().unwrap(), &ca, dir.path()).await;
        let original_tls = proxy.port();
        let contested = if admin { proxy.admin } else { proxy.port() };
        proxy.pause();
        // Force another fixture's allocator to try this paused proxy's
        // port, then actually bind what it hands out before resuming.
        let issued = lease_port_from(Some(contested));
        let _competitor = std::net::TcpListener::bind(("127.0.0.1", issued)).unwrap();
        proxy.resume(&ca).await;
        assert_eq!(proxy.port(), original_tls, "the client-visible endpoint is stable");
        assert_ne!(issued, contested, "paused proxy ports must remain leased");
        assert!(!proxy.leaf(&ca).await.is_empty());
    }

    #[test]
    fn certificate_updates_replace_files_instead_of_reusing_inodes() {
        let dir = tempfile::tempdir().unwrap();
        let ca = TestCa::new();
        RecipeProxy::write_certificates(dir.path(), &ca);
        let before: Vec<_> = ["cert.pem", "key.pem"]
            .map(|name| std::fs::metadata(dir.path().join(name)).unwrap().ino())
            .into();
        RecipeProxy::write_certificates(dir.path(), &ca);
        for (name, inode) in ["cert.pem", "key.pem"].into_iter().zip(before) {
            assert_ne!(
                std::fs::metadata(dir.path().join(name)).unwrap().ino(),
                inode,
                "nginx may inherit an unchanged inode+mtime across reloads"
            );
        }
    }
}
