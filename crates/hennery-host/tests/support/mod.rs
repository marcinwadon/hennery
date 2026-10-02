//! Fixtures for the managed-runtime tests: package and Node tarballs built
//! in the test, a manifest naming them under the public URLs, and a
//! loopback HTTP server standing in for the registry and nodejs.org as a
//! mirror. Nothing here reaches the network.

#![allow(dead_code)]

use base64::Engine;
use hennery_host::runtime::download::Sources;
use hennery_host::runtime::install::Layout;
use hennery_host::runtime::manifest::{Adapter, File, Manifest, Node, NodeArchive, Platform};
use sha2::{Digest, Sha256, Sha512};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A gzip tarball of `(name, mode, body)` regular files.
pub fn tgz(files: &[(&str, u32, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, mode, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_mode(*mode);
        header.set_size(body.len() as u64);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append(&header, *body).unwrap();
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    gz.finish().unwrap()
}

pub fn sri(bytes: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
    )
}

/// The `downloads/` name a package's partial download has.
pub fn part_name(bytes: &[u8]) -> String {
    format!("{}.part", &hex::encode(Sha512::digest(bytes))[..32])
}

/// An HTTP/1.1 server for fixed bodies, answering `Range: bytes=N-`.
#[derive(Clone)]
pub struct Server {
    pub base: String,
    state: Arc<Mutex<State>>,
}

/// How the server answers `Range: bytes=N-`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RangeMode {
    /// 206 from byte N, as asked.
    #[default]
    Honour,
    /// 200 with the whole body, as a server without range support.
    Ignore,
    /// 206 from byte 0, a `Content-Range` that is not the one asked for.
    WrongStart,
}

#[derive(Default)]
struct State {
    bodies: HashMap<String, Vec<u8>>,
    /// Paths whose next answer stops after this many body bytes.
    cut_once: HashMap<String, usize>,
    /// Paths answered with a redirect to this location.
    redirects: HashMap<String, String>,
    /// How a `Range` request is answered.
    range: RangeMode,
    /// Every request: its path and its `Range` header.
    requests: Vec<(String, Option<String>)>,
}

impl Server {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(stream, shared.clone()));
            }
        });
        Self { base, state }
    }

    pub fn put(&self, path: &str, body: Vec<u8>) {
        self.state.lock().unwrap().bodies.insert(path.to_string(), body);
    }

    pub fn cut_once(&self, path: &str, after: usize) {
        self.state.lock().unwrap().cut_once.insert(path.to_string(), after);
    }

    pub fn range_mode(&self, mode: RangeMode) {
        self.state.lock().unwrap().range = mode;
    }

    pub fn redirect(&self, path: &str, location: &str) {
        self.state
            .lock()
            .unwrap()
            .redirects
            .insert(path.to_string(), location.to_string());
    }

    pub fn requests(&self) -> Vec<(String, Option<String>)> {
        self.state.lock().unwrap().requests.clone()
    }

    /// The mirror settings that send every manifest URL here.
    pub fn sources(&self) -> Sources {
        Sources::new(
            Some(&format!("{}/npm/", self.base)),
            Some(&format!("{}/node/", self.base)),
        )
        .unwrap()
    }
}

async fn serve(mut stream: tokio::net::TcpStream, state: Arc<Mutex<State>>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_string();
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let range = head
        .lines()
        .find_map(|l| l.strip_prefix("range: ").or_else(|| l.strip_prefix("Range: ")))
        .map(str::to_string);
    let (body, cut, redirect, mode) = {
        let mut state = state.lock().unwrap();
        state.requests.push((path.clone(), range.clone()));
        let cut = state.cut_once.remove(&path);
        (
            state.bodies.get(&path).cloned(),
            cut,
            state.redirects.get(&path).cloned(),
            state.range,
        )
    };
    if let Some(location) = redirect {
        let _ = stream
            .write_all(
                format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await;
        return;
    }
    let Some(body) = body else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    let start = range
        .as_deref()
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<usize>().ok());
    let (status, from) = match (start, mode) {
        (Some(_), RangeMode::Ignore) => ("200 OK", 0),
        (Some(n), RangeMode::WrongStart) if n < body.len() => ("206 Partial Content", 0),
        (Some(n), RangeMode::Honour) if n < body.len() => ("206 Partial Content", n),
        _ => ("200 OK", 0),
    };
    // A 206 always names its range, from 0 too.
    let ranged = status.starts_with("206");
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len() - from
    );
    if ranged {
        response.push_str(&format!(
            "Content-Range: bytes {from}-{}/{}\r\n",
            body.len() - 1,
            body.len()
        ));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes()).await;
    let end = cut.map_or(body.len(), |c| (from + c).min(body.len()));
    let _ = stream.write_all(&body[from..end]).await;
    let _ = stream.flush().await;
}

/// Node's version in the fixtures.
pub const NODE: &str = "24.0.0";

/// A pinned set to serve: the manifest and every body it names.
#[derive(Clone)]
pub struct Fixture {
    pub manifest: Manifest,
    /// Server path → body.
    pub bodies: BTreeMap<String, Vec<u8>>,
}

impl Fixture {
    /// Both agents, with a nested package, an executable entry and a CLI
    /// package each, on every platform; `version` tells sets apart.
    pub fn new(version: &str) -> Self {
        Self::with_node(version, NODE, &format!("echo v{NODE}"))
    }

    /// As `new`, with this Node version and stub `bin/node` script body.
    pub fn with_node(version: &str, node_version: &str, node_script: &str) -> Self {
        let mut fixture = Fixture {
            manifest: Manifest {
                schema: 1,
                node: Node {
                    version: node_version.to_string(),
                    platforms: BTreeMap::new(),
                },
                adapters: BTreeMap::new(),
            },
            bodies: BTreeMap::new(),
        };
        for platform in Platform::ALL {
            let key = platform.key();
            let root = format!("node-v{node_version}-{key}");
            let script = format!("#!/bin/sh\n{node_script}\n");
            let archive = tgz(&[
                (&format!("{root}/README.md"), 0o644, b"node"),
                (&format!("{root}/bin/node"), 0o755, script.as_bytes()),
            ]);
            let rest = format!("v{node_version}/{root}.tar.gz");
            fixture.manifest.node.platforms.insert(
                key.to_string(),
                NodeArchive {
                    url: format!("https://nodejs.org/dist/{rest}"),
                    sha256: hex::encode(Sha256::digest(&archive)),
                    archive_size: archive.len() as u64,
                    node_size: script.len() as u64,
                },
            );
            fixture.bodies.insert(format!("/node/{rest}"), archive);
        }
        for agent in ["claude", "codex"] {
            let mut platforms = BTreeMap::new();
            for platform in Platform::ALL {
                let key = platform.key();
                let entry = format!("// {agent} {version}\n");
                let files = vec![
                    fixture.package(
                        &format!("node_modules/@acp/{agent}"),
                        &format!("@acp/{agent}"),
                        version,
                        &[
                            ("package/package.json", 0o644, b"{}".as_slice()),
                            ("package/dist/index.js", 0o755, entry.as_bytes()),
                        ],
                        false,
                    ),
                    fixture.package(
                        &format!("node_modules/@acp/{agent}/node_modules/nested"),
                        "nested",
                        version,
                        &[("package/index.js", 0o644, b"nested".as_slice())],
                        false,
                    ),
                    fixture.package(
                        &format!("node_modules/@vendor/{agent}-cli-{key}"),
                        &format!("@vendor/{agent}-cli-{key}"),
                        version,
                        &[("package/cli", 0o755, format!("#!/bin/sh\n# {agent} cli\n").as_bytes())],
                        true,
                    ),
                ];
                platforms.insert(key.to_string(), files);
            }
            fixture.manifest.adapters.insert(
                agent.to_string(),
                Adapter {
                    package: format!("@acp/{agent}"),
                    version: version.to_string(),
                    entry: format!("node_modules/@acp/{agent}/dist/index.js"),
                    platforms,
                },
            );
        }
        fixture.manifest.validate().unwrap();
        fixture
    }

    fn package(&mut self, path: &str, name: &str, version: &str, files: &[(&str, u32, &[u8])], cli: bool) -> File {
        let body = tgz(files);
        let rest = format!("{name}/-/{}-{version}.tgz", name.rsplit('/').next().unwrap());
        let file = File {
            path: path.to_string(),
            url: format!("https://registry.npmjs.org/{rest}"),
            integrity: sri(&body),
            archive_size: body.len() as u64,
            unpacked_size: files.iter().map(|(_, _, b)| b.len() as u64).sum(),
            cli,
            install_script: false,
        };
        self.bodies.insert(format!("/npm/{rest}"), body);
        file
    }

    /// Serve every body of this fixture.
    pub fn serve(&self, server: &Server) {
        for (path, body) in &self.bodies {
            server.put(path, body.clone());
        }
    }

    /// The server path of `agent`'s file at `path` on this platform.
    pub fn server_path(&self, agent: &str, path: &str) -> String {
        let file = self.file(agent, path);
        format!("/npm/{}", file.url.strip_prefix("https://registry.npmjs.org/").unwrap())
    }

    pub fn file(&self, agent: &str, path: &str) -> &File {
        self.manifest.adapters[agent].platforms[here().key()]
            .iter()
            .find(|f| f.path == path)
            .unwrap()
    }

    pub fn hash(&self) -> String {
        Manifest::hash_of(&self.manifest.to_json())
    }
}

/// The platform these tests run on.
pub fn here() -> Platform {
    Platform::current().expect("tests run on a supported platform")
}

/// A host data directory and its layout.
pub fn data_dir() -> (tempfile::TempDir, Layout) {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(&dir.path().join("host")).unwrap();
    (dir, layout)
}

pub fn quiet(_: &str) {}
