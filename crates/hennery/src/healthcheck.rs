//! `hennery collector healthcheck` (distribution spec §1, §4.1): exit 0 when
//! the collector answers `GET /healthz` with 200, else 1. It is the
//! container image's `HEALTHCHECK`: the image is distroless, with no shell,
//! `curl` or `wget`, so the binary checks itself.

use anyhow::{Context, Result, anyhow, bail};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// The whole check's budget, inside the image's `--timeout=3s`, so a hung
/// collector is reported by this check and not by Docker killing it.
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// The longest status line read: the answer's status line is all that counts.
const MAX_STATUS_LINE: usize = 256;

/// Where to reach a collector listening on `listen`: a wildcard address
/// (`0.0.0.0`, `[::]`) is reached over loopback, as `up`'s host reaches it.
pub fn probe_address(listen: &str) -> Result<SocketAddr> {
    let mut address = listen
        .to_socket_addrs()
        .with_context(|| format!("listen address {listen}"))?
        .next()
        .with_context(|| format!("listen address {listen} resolves to nothing"))?;
    match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => address.set_ip(Ipv4Addr::LOCALHOST.into()),
        IpAddr::V6(ip) if ip.is_unspecified() => address.set_ip(Ipv6Addr::LOCALHOST.into()),
        _ => {}
    }
    Ok(address)
}

/// `Ok` when `GET /healthz` at `address` answers 200 within `timeout`, all
/// of it (connect, write and every read); else why not.
pub fn check(address: SocketAddr, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    // What is left of the budget; none left is a failure, not a zero timeout
    // (which the socket calls refuse).
    let left = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| anyhow!("{address} did not answer GET /healthz within {timeout:?}"))
    };
    let mut stream = TcpStream::connect_timeout(&address, left()?).with_context(|| format!("connect to {address}"))?;
    stream.set_write_timeout(Some(left()?))?;
    // One write: the whole request, so the collector never sees half of one.
    stream
        .write_all(format!("GET /healthz HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes())
        .with_context(|| format!("send to {address}"))?;
    // The status line only, up to its CRLF: a peer trickling bytes cannot
    // keep the check past its budget, since every read gets what is left.
    let mut line = Vec::new();
    let mut buf = [0u8; MAX_STATUS_LINE];
    loop {
        stream.set_read_timeout(Some(left()?))?;
        let n = stream.read(&mut buf).with_context(|| format!("read from {address}"))?;
        line.extend_from_slice(&buf[..n]);
        if let Some(end) = line.windows(2).position(|pair| pair == b"\r\n") {
            line.truncate(end);
            break;
        }
        if n == 0 || line.len() > MAX_STATUS_LINE {
            break;
        }
    }
    let status = String::from_utf8_lossy(&line);
    match status.split(' ').nth(1) {
        Some("200") => Ok(()),
        _ => bail!("{address} answered {status:?} to GET /healthz"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn a_wildcard_listen_address_is_probed_over_loopback() {
        assert_eq!(
            probe_address("0.0.0.0:8080").unwrap(),
            "127.0.0.1:8080".parse().unwrap()
        );
        assert_eq!(probe_address("[::]:8080").unwrap(), "[::1]:8080".parse().unwrap());
        assert_eq!(
            probe_address("100.64.0.7:7117").unwrap(),
            "100.64.0.7:7117".parse().unwrap()
        );
    }

    #[test]
    fn a_listen_address_without_a_port_is_refused() {
        assert!(probe_address("127.0.0.1").is_err());
    }

    /// A server that reads one request and answers `answer`, or nothing.
    fn answering(answer: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(answer.as_bytes());
        });
        address
    }

    #[test]
    fn only_a_200_is_healthy() {
        let ok = answering("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
        check(ok, TIMEOUT).unwrap();
        let unavailable = answering("HTTP/1.1 503 Service Unavailable\r\n\r\n");
        let err = check(unavailable, TIMEOUT).unwrap_err();
        assert!(format!("{err:#}").contains("503"), "{err:#}");
        let garbage = answering("200 nonsense");
        assert!(check(garbage, TIMEOUT).is_err());
    }

    #[test]
    fn nothing_listening_is_unhealthy() {
        let address = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        // The listener is dropped: nothing listens there now.
        assert!(check(address, TIMEOUT).is_err());
    }

    /// A peer that sends a byte at a time, each well inside any one read's
    /// timeout, still fails the check once the whole budget is spent.
    #[test]
    fn an_answer_trickled_byte_by_byte_is_unhealthy_within_the_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            for byte in b"HTTP/1.1 200 OK".iter().cycle() {
                if stream.write_all(&[*byte]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let started = std::time::Instant::now();
        assert!(check(address, Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn a_collector_that_never_answers_is_unhealthy_within_the_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        // Accepted by the kernel's backlog, never read or answered.
        let started = std::time::Instant::now();
        assert!(check(address, Duration::from_millis(300)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        drop(listener);
    }
}
