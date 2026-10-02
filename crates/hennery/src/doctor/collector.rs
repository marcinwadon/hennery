//! The checks that reach the collector (distribution spec §7): the host's
//! way to it, step by step, and its `hello` (check 7); the clock against
//! the collector's (check 8); and, in a collector's own directory, its
//! listeners and `public_url` (check 16). Every request goes straight to
//! the address named, never through a proxy, never following a redirect,
//! and reads a status line and a `Date` header, no body (decision 17). An
//! accepted `hello` is recorded by the collector: its Hosts view shows the
//! host as seen just now, with this binary's version.

use super::dirs::NO_HOST;
use super::service::{Holder, holder};
use super::{Doctor, Finding, Verdict};
use hennery_host::connection::Standing;
use hennery_host::identity::Paired;
use reqwest::Url;
use std::net::ToSocketAddrs;
use std::time::Duration;

/// Each step's budget.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// Clock skew that warns, and that fails (distribution §7, check 8).
pub const SKEW_WARN: u64 = 30;
pub const SKEW_FAIL: u64 = 300;

/// The most of a collector's own words printed.
const MAX_QUOTE: usize = 200;

/// Why checks 7 and 8 did not run.
const NOT_PAIRED: &str = "the host is not paired";
const NO_ANSWER: &str = "the collector did not answer (check 7)";

/// `text` cut to `MAX_QUOTE` characters: words that come from the network.
fn quoted(text: &str) -> String {
    let mut out: String = text.chars().take(MAX_QUOTE).collect();
    if text.chars().count() > MAX_QUOTE {
        out.push('…');
    }
    out
}

/// `url` as the report may show it: without a user name or password, which
/// `parse_public_url` accepts in a URL (`https://user:secret@host`).
pub fn shown_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) if !parsed.username().is_empty() || parsed.password().is_some() => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Ok(_) => url.to_string(),
        // It could still hold `user:pass@` before what broke it.
        Err(_) => "an unreadable URL".to_string(),
    }
}

/// How long the whole `hello` may take: the probe bounds its handshake and
/// its answer (15 s each), not its close.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(35);

/// `future` run to its end on a runtime of its own, on a thread of its own:
/// doctor runs inside `main`'s runtime, which cannot be blocked on.
fn block_on<T: Send + 'static>(future: impl std::future::Future<Output = T> + Send + 'static) -> T {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(future)
    })
    .join()
    .expect("the request thread")
}

/// What a `GET /healthz` came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// It answered: its status, and its `Date` header as Unix seconds.
    Answered {
        status: u16,
        date: Option<i64>,
    },
    /// The TLS handshake or the certificate failed.
    Tls(String),
    Failed(String),
}

/// `GET <base>/healthz`, straight to it (no proxy, no redirect), within
/// `STEP_TIMEOUT`; only its status and `Date` are read.
pub fn healthz(base: &Url) -> Health {
    let Ok(url) = base.join("/healthz") else {
        return Health::Failed(format!("{} cannot take a path", shown_url(base.as_str())));
    };
    let https = url.scheme() == "https";
    block_on(async move {
        let client = match reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(STEP_TIMEOUT)
            .build()
        {
            Ok(client) => client,
            Err(err) => return Health::Failed(quoted(&err.to_string())),
        };
        match client.get(url).send().await {
            Ok(response) => Health::Answered {
                status: response.status().as_u16(),
                date: response
                    .headers()
                    .get(reqwest::header::DATE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(http_date),
            },
            Err(err) => {
                // Without the URL: a name with "tls" in it is not a TLS
                // failure, and a URL's credentials are not printed.
                let err = err.without_url();
                // reqwest's own text names none of its causes: the chain is
                // walked, and the innermost cause is the one shown.
                let mut chain = err.to_string();
                let mut why = chain.clone();
                let mut cause = std::error::Error::source(&err);
                while let Some(inner) = cause {
                    why = inner.to_string();
                    chain = format!("{chain}: {why}");
                    cause = inner.source();
                }
                let lower = chain.to_ascii_lowercase();
                // Over https, step 3 has just reached this address: a
                // connect that fails, not out of time, is the handshake's,
                // even when its words name neither TLS nor a certificate
                // (plain http behind an https URL: "corrupt message").
                let handshake = https && err.is_connect() && !err.is_timeout();
                if handshake || lower.contains("certificate") || lower.contains("tls") {
                    Health::Tls(quoted(&why))
                } else {
                    Health::Failed(quoted(&why))
                }
            }
        }
    })
}

/// An HTTP date (`Thu, 02 Oct 2026 10:00:00 GMT`) as Unix seconds.
pub fn http_date(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = parts.as_slice() else {
        return None;
    };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| m == month)? as i64
        + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let hms: Vec<i64> = time.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [h, m, s] = hms.as_slice() else {
        return None;
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + m * 60 + s)
}

/// Where the host examined reaches its collector: the http(s) base and the
/// host WebSocket. For `up`'s host, as `up` hands it on (the collector's
/// first listen address loopback reaches): `host.toml`'s may name an old
/// port.
fn collector_of(doctor: &Doctor, paired: &Paired) -> Result<(String, String), String> {
    let up = doctor.dirs.collector.as_ref().filter(|_| {
        doctor
            .dirs
            .host
            .as_ref()
            .is_some_and(|h| h.parent() == Some(doctor.dirs.root.as_path()))
    });
    if let Some(collector) = up {
        let file = crate::config::FileConfig::load(collector).map_err(|e| format!("{e:#}"))?;
        let addresses = file
            .listen(&[])
            .and_then(|given| crate::listen_addresses(&given))
            .map_err(|e| format!("{e:#}"))?;
        let base = addresses
            .iter()
            .map(|a| crate::loopback_url(a))
            .find(|base| hennery_host::pairing::collector_ws_url(base).is_ok())
            .ok_or("no listen address of the collector's is reached over loopback")?;
        let ws = hennery_host::pairing::collector_ws_url(&base).map_err(|e| format!("{e:#}"))?;
        return Ok((base, ws));
    }
    let ws = paired.collector_url.clone();
    let mut url = Url::parse(&ws).map_err(|_| "host.toml's collector is not a URL".to_string())?;
    let scheme = match url.scheme() {
        "wss" => "https",
        "ws" => "http",
        other => return Err(format!("host.toml's collector has the scheme {other}")),
    };
    url.set_scheme(scheme).map_err(|()| "a scheme".to_string())?;
    url.set_path("/");
    Ok((url.to_string(), ws))
}

/// Checks 7 and 8.
pub fn collector(doctor: &Doctor) -> [Finding; 2] {
    let not_run = |why| [Finding::NotRun { number: 7, why }, Finding::NotRun { number: 8, why }];
    let Some(host) = &doctor.dirs.host else {
        return not_run(NO_HOST);
    };
    let mut verdict = Verdict::default();
    let paired = match Paired::read(host) {
        Ok(Some(paired)) => paired,
        Ok(None) => return not_run(NOT_PAIRED),
        Err(err) => {
            let fix = if host.join("host.toml.pending").exists() {
                "run `hennery host run` once (it finishes the pairing), then doctor again"
            } else {
                "fix host.toml, or pair this host again (`hennery host join`)"
            };
            verdict.warn(format!("{err}"), fix);
            return [
                Finding::Checked(verdict.check(7, "collector")),
                Finding::NotRun {
                    number: 8,
                    why: NO_ANSWER,
                },
            ];
        }
    };
    let date = reach(doctor, host, &paired, &mut verdict);
    let seven = Finding::Checked(verdict.check(7, "collector"));
    let eight = match date {
        None => Finding::NotRun {
            number: 8,
            why: NO_ANSWER,
        },
        Some(date) => Finding::Checked(skew(date, now())),
    };
    [seven, eight]
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The `/healthz` URL as the report shows it, from the shown base URL:
/// `up`'s has no path (`http://127.0.0.1:7117`), `host.toml`'s ends in `/`.
pub fn health_shown(shown: &str) -> String {
    format!("{}/healthz", shown.trim_end_matches('/'))
}

/// Check 7's steps, each stopping the check where it fails: the URL's rule,
/// DNS, TCP, TLS, `/healthz`, then the `hello`. The collector's `Date`, or
/// `None` when it did not answer (then check 8 cannot run); `Some(None)`
/// when it answered without one.
fn reach(doctor: &Doctor, host: &std::path::Path, paired: &Paired, verdict: &mut Verdict) -> Option<Option<i64>> {
    let (base, ws) = match collector_of(doctor, paired) {
        Ok(both) => both,
        Err(why) => {
            verdict.fail(why, "fix the collector's config.toml, or pair this host again");
            return None;
        }
    };
    let rejoin = "pair this host again: `hennery host join <the collector's URL>`";
    let shown = shown_url(&base);
    let health = health_shown(&shown);
    let url = match hennery_host::pairing::parse_public_url(&base) {
        Ok(url) => url,
        Err(err) => {
            // Its words quote the URL: shown without credentials.
            verdict.fail(format!("{err}").replace(&base, &shown), rejoin);
            return None;
        }
    };
    let port = url.port_or_known_default().unwrap_or(443);
    let name = url.host_str().unwrap_or_default().to_string();
    let addresses = match resolve(name.trim_start_matches('[').trim_end_matches(']'), port) {
        Some(addresses) if !addresses.is_empty() => addresses,
        Some(_) => {
            verdict.fail(
                format!("{name} does not resolve"),
                "check the name in DNS, or the collector's URL in host.toml",
            );
            return None;
        }
        None => {
            verdict.fail(
                format!("{name} did not resolve within {STEP_TIMEOUT:?}"),
                "check this machine's DNS",
            );
            return None;
        }
    };
    if let Err(err) = connect(&addresses) {
        verdict.fail(
            format!("nothing answers on {} ({})", addresses[0], err.kind()),
            "start the collector, or open the way to it (firewall, VPN)",
        );
        return None;
    }
    let date = match healthz(&url) {
        Health::Tls(why) => {
            verdict.fail(
                format!("{shown}: TLS failed: {why}"),
                "give the collector a certificate for its name (and check this machine's clock: an expired-looking certificate can be the clock)",
            );
            return None;
        }
        Health::Failed(why) => {
            verdict.fail(format!("{health}: {why}"), "check the collector's log");
            return None;
        }
        Health::Answered { status, .. } if status != 200 => {
            verdict.fail(
                format!("{health} answered {status}"),
                "check that this URL is the collector's, and its log",
            );
            return None;
        }
        Health::Answered { date, .. } => date,
    };
    verdict.ok(format!("{shown} answers"));
    hello(doctor, host, paired, &ws, verdict);
    Some(date)
}

/// `name`'s addresses, looked up on a thread of its own: `None` when the
/// lookup takes longer than `STEP_TIMEOUT` (the system's resolver has no
/// bound of its own; the thread is left to end on its own).
fn resolve(name: &str, port: u16) -> Option<Vec<std::net::SocketAddr>> {
    let name = name.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let found = (name.as_str(), port)
            .to_socket_addrs()
            .map(|a| a.collect::<Vec<_>>())
            .unwrap_or_default();
        let _ = tx.send(found);
    });
    rx.recv_timeout(STEP_TIMEOUT).ok()
}

/// A TCP connection to the first of `addresses` that takes one, each
/// within `STEP_TIMEOUT`, closed at once; the first address's error else.
fn connect(addresses: &[std::net::SocketAddr]) -> std::io::Result<()> {
    let mut first = None;
    for address in addresses {
        match std::net::TcpStream::connect_timeout(address, STEP_TIMEOUT) {
            Ok(_) => return Ok(()),
            Err(err) => {
                first.get_or_insert(err);
            }
        }
    }
    Err(first.unwrap_or_else(|| std::io::ErrorKind::NotFound.into()))
}

/// The `hello`, only when no host runs on this directory (decision 17).
fn hello(doctor: &Doctor, host: &std::path::Path, paired: &Paired, ws: &str, verdict: &mut Verdict) {
    if holder(doctor, &host.join(crate::lock::HOST_LOCK)) != Holder::Nobody {
        verdict.ok("hello not sent: a host may be connected on this directory (see check 14)");
        return;
    }
    if ws.starts_with("wss://") {
        verdict.warn(
            "this binary's host cannot reach an https collector yet (no TLS for its WebSocket)",
            "reach the collector over loopback or a tunnel until hosts speak wss",
        );
        return;
    }
    let (ws, id, key) = (ws.to_string(), paired.host_id.clone(), paired.key.clone());
    let standing =
        block_on(
            async move { tokio::time::timeout(HELLO_TIMEOUT, hennery_host::connection::probe(&ws, &id, &key)).await },
        );
    let rejoin = "pair it again: remove host.key and host.toml, then `hennery host join <url>`";
    match standing {
        Err(_) => verdict.fail(
            format!("its hello did not finish within {HELLO_TIMEOUT:?}"),
            "check the collector's log",
        ),
        Ok(Ok(Standing::Accepted)) => verdict.ok(
            "its hello is accepted (the collector records this host as seen now, with this binary's version)",
        ),
        Ok(Ok(Standing::Connected)) => verdict.warn(
            "this host's key is in use by another live connection, and no host here holds host.lock",
            "run doctor again in a minute; if it stays and no other copy of this directory is yours, revoke the host in the Hosts view and pair again (`hennery host join`)",
        ),
        Ok(Ok(Standing::Revoked)) => verdict.fail(
            "the collector revoked this host",
            "re-pair it with `hennery host join <url>` (it gets a new key and id)",
        ),
        Ok(Ok(Standing::Unknown)) => verdict.fail("the collector does not know this host or its key", rejoin),
        Ok(Err(err)) => verdict.fail(
            format!("its hello failed: {}", quoted(&err.to_string())),
            "check the collector's log",
        ),
    }
}

/// Check 8's verdict: the collector's `date` against `now`.
pub fn skew(date: Option<i64>, now: i64) -> super::Check {
    let mut verdict = Verdict::default();
    match date {
        None => verdict.warn(
            "the collector's answer had no Date",
            "check this machine's clock by hand (`date`)",
        ),
        Some(date) => {
            let off = now.abs_diff(date);
            let fix = "set this machine's clock from the network (NTP)";
            if off > SKEW_FAIL {
                verdict.fail(format!("this clock is {off} s off the collector's"), fix);
            } else if off > SKEW_WARN {
                verdict.warn(format!("this clock is {off} s off the collector's"), fix);
            } else {
                verdict.ok(format!("within {off} s of the collector's"));
            }
        }
    }
    verdict.check(8, "clock")
}

/// Whether the one installed service runs the collector in `dir`.
fn served(doctor: &Doctor, dir: &std::path::Path) -> bool {
    use crate::service::unit::Role;
    let cx = doctor.cx;
    let [role] = cx.installed()[..] else {
        return false;
    };
    let Some(data) = crate::service::read_command_line(cx, role).and_then(|a| crate::service::data_dir_of(&a)) else {
        return false;
    };
    let collector = match role {
        Role::Up => data.join("collector"),
        Role::Collector => data,
        Role::Host => return false,
    };
    collector.canonicalize().ok() == dir.canonicalize().ok()
        && crate::service::managed(cx, role)
            .ok()
            .flatten()
            .is_some_and(|m| m.running)
}

/// Check 16 (kernel §7): every listen address answers, and `public_url`
/// (`config.toml`'s; the one stored at setup is in the database, which is
/// not opened) answers `/healthz`.
pub fn listeners(doctor: &Doctor) -> Finding {
    let Some(collector) = &doctor.dirs.collector else {
        return Finding::NotRun {
            number: 16,
            why: "no collector data directory",
        };
    };
    let mut verdict = Verdict::default();
    let file = match crate::config::FileConfig::load(collector) {
        Ok(file) => file,
        Err(err) => {
            verdict.fail(format!("{err}"), "fix the collector's config.toml");
            return Finding::Checked(verdict.check(16, "listeners"));
        }
    };
    let addresses = match file.listen(&[]).and_then(|given| crate::listen_addresses(&given)) {
        Ok(addresses) => addresses,
        Err(err) => {
            verdict.fail(format!("{err}"), "fix listen in the collector's config.toml");
            return Finding::Checked(verdict.check(16, "listeners"));
        }
    };
    let running = served(doctor, collector);
    for address in &addresses {
        let answered = crate::healthcheck::probe_address(address)
            .and_then(|a| crate::healthcheck::check(a, crate::healthcheck::TIMEOUT));
        match answered {
            Ok(()) => verdict.ok(format!("{address} answers")),
            Err(_) if running => verdict.fail(
                format!("{address}: nothing answers, though the service runs"),
                "check listen in config.toml against the collector's log (a taken port fails its start)",
            ),
            Err(_) => verdict.warn(
                format!("{address}: no collector answers (is it running?)"),
                "start it (`hennery up`, `hennery collector`, or the service)",
            ),
        }
    }
    if doctor.cx.env.contains_key("HENNERY_LISTEN") {
        verdict.ok("HENNERY_LISTEN is set in this shell: a service does not see it");
    }
    match file.public_url(None) {
        None => verdict.ok("no public_url in config.toml (the one stored at setup is not read)"),
        Some(public) => match hennery_host::pairing::parse_public_url(&public) {
            Err(err) => verdict.warn(
                format!("{err}").replace(&public, &shown_url(&public)),
                "fix public_url in config.toml",
            ),
            Ok(url) => match healthz(&url) {
                Health::Answered { status: 200, .. } => {
                    verdict.ok(format!("public_url {} answers /healthz", shown_url(&public)))
                }
                Health::Answered { status, .. } => verdict.warn(
                    format!("public_url {} answers /healthz with {status}", shown_url(&public)),
                    "point public_url, or the proxy in front, at this collector",
                ),
                Health::Tls(why) | Health::Failed(why) => verdict.warn(
                    format!("public_url {} reaches no listener or proxy: {why}", shown_url(&public)),
                    "point public_url, or the proxy in front, at this collector",
                ),
            },
        },
    }
    Finding::Checked(verdict.check(16, "listeners"))
}
