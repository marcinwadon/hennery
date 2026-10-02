//! Downloads for the managed runtime (distribution spec §3.2): resumable,
//! cut off past the size the manifest pins, and verified against its
//! digest before anything is extracted.

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use sha2::{Digest, Sha256, Sha512};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::time::Duration;

/// A connection attempt gives up after this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A download with no bytes for this long gives up (and resumes on the
/// next attempt).
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Attempts per file within one install; each resumes where the last
/// stopped. A digest mismatch is never retried.
pub const ATTEMPTS: usize = 3;

/// What a file must hash to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    Sha256([u8; 32]),
    Sha512([u8; 64]),
}

/// Where the packages and Node come from (decision 7): the public registry
/// and nodejs.org, or a mirror of either. The digests are the manifest's
/// whatever the source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sources {
    npm: Option<String>,
    node: Option<String>,
}

impl Sources {
    /// `npm` replaces `https://registry.npmjs.org/`, `node`
    /// `https://nodejs.org/dist/`. A mirror must be `https://`, or plain
    /// `http://` on a loopback address.
    pub fn new(npm: Option<&str>, node: Option<&str>) -> Result<Self> {
        let base = |url: Option<&str>, what: &str| -> Result<Option<String>> {
            let Some(url) = url else { return Ok(None) };
            let parsed = reqwest::Url::parse(url).with_context(|| format!("{what} {url:?} is not a URL"))?;
            if parsed.query().is_some()
                || parsed.fragment().is_some()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                bail!("{what} {url:?} must be a plain base URL");
            }
            match parsed.scheme() {
                "https" => {}
                "http" if is_loopback(&parsed) => {}
                _ => bail!("{what} {url:?} must be https:// (plain http only on loopback)"),
            }
            let mut text = parsed.to_string();
            if !text.ends_with('/') {
                text.push('/');
            }
            Ok(Some(text))
        };
        Ok(Self {
            npm: base(npm, "the npm registry mirror")?,
            node: base(node, "the Node mirror")?,
        })
    }

    /// Where to fetch `url`, a manifest URL. One under neither pinned prefix
    /// is refused: the manifest is validated, so this never happens.
    pub fn locate(&self, url: &str) -> Result<String> {
        use super::manifest::{NODE_DIST, NPM_REGISTRY};
        let (rest, mirror) = if let Some(rest) = url.strip_prefix(NPM_REGISTRY) {
            (rest, &self.npm)
        } else if let Some(rest) = url.strip_prefix(NODE_DIST) {
            (rest, &self.node)
        } else {
            bail!("{url} is under neither {NPM_REGISTRY} nor {NODE_DIST}");
        };
        Ok(match mirror {
            Some(base) => format!("{base}{rest}"),
            None => url.to_string(),
        })
    }
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// An HTTP client for `url`: rustls, bounded, redirects only to https (or
/// to loopback), the environment's proxy for public hosts but never for
/// loopback ones.
pub fn client_for(url: &str) -> Result<reqwest::Client> {
    let parsed = reqwest::Url::parse(url)?;
    let mut builder = reqwest::Client::builder()
        .user_agent(concat!("hennery/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let next = attempt.url();
            // Plain http only from loopback to loopback: a public source
            // never sends this host's requests to its own local services.
            let from_loopback = attempt.previous().first().is_some_and(is_loopback);
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if next.scheme() == "https" || (next.scheme() == "http" && is_loopback(next) && from_loopback) {
                attempt.follow()
            } else {
                attempt.error("a redirect away from https")
            }
        }));
    if is_loopback(&parsed) {
        builder = builder.no_proxy();
    }
    Ok(builder.build()?)
}

/// Download `url` into `part` (resuming what is there), at most `size`
/// bytes, and verify it. On success `part` holds exactly the pinned bytes.
/// On a digest mismatch `part` is removed and the error names the URL;
/// on any other failure it is kept, so the next attempt resumes.
pub async fn fetch(url: &str, part: &Path, size: u64, expected: Expected) -> Result<()> {
    let client = client_for(url)?;
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match fetch_once(&client, url, part, size).await {
            Ok(()) => return verify(url, part, expected),
            // A source that sends more than pinned is not asked again.
            Err(err) if err.is::<Refused>() => return Err(err),
            Err(err) => last = Some(err),
        }
    }
    Err(last.expect("at least one attempt"))
}

/// A download refused outright: never retried.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

async fn fetch_once(client: &reqwest::Client, url: &str, part: &Path, size: u64) -> Result<()> {
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > size {
        std::fs::remove_file(part)?;
        have = 0;
    }
    if have == size {
        return Ok(());
    }
    let mut request = client.get(url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let response = request.send().await.with_context(|| format!("reach {url}"))?;
    let status = response.status().as_u16();
    let start = match status {
        206 if have > 0 && content_range_start(&response) == Some(have) => have,
        // The whole file again: the server ignored the range (or there was
        // none to ask for).
        200 => 0,
        // A range the server would not or could not give: start over.
        206 | 416 => {
            let _ = std::fs::remove_file(part);
            bail!("{url}: the server did not resume at byte {have}; starting over on the next attempt");
        }
        _ => bail!("{url}: HTTP {status}"),
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)
        .with_context(|| format!("open {}", part.display()))?;
    file.set_len(start)?;
    file.seek(std::io::SeekFrom::Start(start))?;
    let mut written = start;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.with_context(|| format!("download {url}"))?;
        written += chunk.len() as u64;
        if written > size {
            drop(file);
            std::fs::remove_file(part)?;
            return Err(Refused(format!("{url} sends more than the {size} bytes the manifest pins")).into());
        }
        file.write_all(&chunk)?;
    }
    super::extract::sync(&file)?;
    if written != size {
        bail!("{url}: {written} of {size} bytes, interrupted");
    }
    Ok(())
}

fn content_range_start(response: &reqwest::Response) -> Option<u64> {
    let value = response.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let range = value.strip_prefix("bytes ")?;
    range.split_once('-')?.0.parse().ok()
}

fn verify(url: &str, part: &Path, expected: Expected) -> Result<()> {
    let mut file = std::fs::File::open(part)?;
    let mut buf = vec![0u8; 1 << 16];
    let matches = match expected {
        Expected::Sha256(want) => {
            let mut hasher = Sha256::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            <[u8; 32]>::from(hasher.finalize()) == want
        }
        Expected::Sha512(want) => {
            let mut hasher = Sha512::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            <[u8; 64]>::from(hasher.finalize()) == want
        }
    };
    if !matches {
        std::fs::remove_file(part)?;
        bail!("{url} does not match the digest this binary pins; nothing was installed from it");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_replaces_only_its_own_prefix() {
        let sources = Sources::new(Some("https://npm.mirror.example/base"), None).unwrap();
        assert_eq!(
            sources
                .locate("https://registry.npmjs.org/zod/-/zod-1.0.0.tgz")
                .unwrap(),
            "https://npm.mirror.example/base/zod/-/zod-1.0.0.tgz"
        );
        assert_eq!(
            sources.locate("https://nodejs.org/dist/v24.0.0/x.tar.gz").unwrap(),
            "https://nodejs.org/dist/v24.0.0/x.tar.gz"
        );
        assert!(sources.locate("https://elsewhere.example/x.tgz").is_err());
    }

    #[test]
    fn a_mirror_is_https_or_loopback() {
        for good in [
            "https://m.example/",
            "http://127.0.0.1:8080/npm",
            "http://[::1]:1/",
            "http://localhost:9/",
        ] {
            Sources::new(Some(good), Some(good)).unwrap();
        }
        for bad in [
            "http://m.example/",
            "ftp://m.example/",
            "https://m.example/?x=1",
            "https://user@m.example/",
            "not a url",
        ] {
            assert!(Sources::new(Some(bad), None).is_err(), "{bad}");
            assert!(Sources::new(None, Some(bad)).is_err(), "{bad}");
        }
    }
}
