//! Which data directory doctor examines, and what it holds (decision 1):
//! the one given, else `HENNERY_HOST_DATA_DIR`'s host, else the installed
//! service's, else the platform's default; then a host's directory, a
//! collector's, or both under `hennery up`'s root. Judged from the names in
//! it alone: nothing is opened, and an interrupted pairing is not rolled
//! forward (only its host may do that, under `host.lock`).

use crate::service::Context;
use crate::service::unit::Role;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The variable `host run`, `host join` and `host adapters` read their
/// directory from.
pub const HOST_DIR_VAR: &str = "HENNERY_HOST_DATA_DIR";

/// Where the directory came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// `--data-dir` or `HENNERY_DATA_DIR`.
    Given,
    /// `HENNERY_HOST_DATA_DIR`: a host's directory.
    HostVariable,
    /// The installed service's command line.
    Service(Role),
    /// The platform's default (distribution spec §8).
    Default,
}

impl Found {
    pub fn describe(self) -> String {
        match self {
            Self::Given => "given with --data-dir or HENNERY_DATA_DIR".to_string(),
            Self::HostVariable => format!("{HOST_DIR_VAR}'s"),
            Self::Service(role) => format!("the {role} service's"),
            Self::Default => "the default".to_string(),
        }
    }
}

/// The directory doctor examines, and what is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub root: PathBuf,
    pub from: Found,
    /// A host's data directory: `root` itself, or `up`'s `root/host`.
    pub host: Option<PathBuf>,
    /// A collector's: `root` itself, or `up`'s `root/collector`.
    pub collector: Option<PathBuf>,
    /// What the report's header should also say.
    pub notes: Vec<String>,
}

/// Why the host checks did not run.
pub const NO_HOST: &str = "no host data directory";

impl Dirs {
    /// The directory to examine (decision 1).
    pub fn discover(cx: &Context, given: Option<&Path>) -> Result<Self> {
        let host_var = cx
            .env
            .get(HOST_DIR_VAR)
            .filter(|dir| !dir.is_empty())
            .map(std::path::absolute)
            .transpose()?;
        if let Some(dir) = given {
            let mut dirs = Self::by_contents(std::path::absolute(dir)?, Found::Given);
            if let Some(host) = host_var.filter(|host| Some(host) != dirs.host.as_ref()) {
                dirs.notes.push(format!(
                    "{HOST_DIR_VAR} names another host directory, {}: `hennery doctor --data-dir {}` examines it",
                    host.display(),
                    host.display()
                ));
            }
            return Ok(dirs);
        }
        if let Some(host) = host_var {
            return Ok(Self {
                host: host.is_dir().then(|| host.clone()),
                root: host,
                from: Found::HostVariable,
                collector: None,
                notes: Vec::new(),
            });
        }
        if let [role] = cx.installed()[..]
            && let Some(dir) =
                crate::service::read_command_line(cx, role).and_then(|argv| crate::service::data_dir_of(&argv))
        {
            return Ok(Self::by_role(dir, role));
        }
        Ok(Self::by_contents(cx.default_data_dir(), Found::Default))
    }

    /// What `role`'s service keeps in `root`.
    pub fn by_role(root: PathBuf, role: Role) -> Self {
        let dir = |p: PathBuf| p.is_dir().then_some(p);
        let (host, collector) = match role {
            Role::Up => (dir(root.join("host")), dir(root.join("collector"))),
            Role::Host => (dir(root.clone()), None),
            Role::Collector => (None, dir(root.clone())),
        };
        Self {
            root,
            from: Found::Service(role),
            host,
            collector,
            notes: Vec::new(),
        }
    }

    /// What `root` holds, told by the names in it.
    pub fn by_contents(root: PathBuf, from: Found) -> Self {
        let (host, collector) = if is_host(&root) {
            (Some(root.clone()), None)
        } else {
            let host = root.join("host");
            let collector = root.join("collector");
            match (is_host(&host), collector.is_dir()) {
                (false, false) if is_collector(&root) => (None, Some(root.clone())),
                (h, c) => (h.then_some(host), c.then_some(collector)),
            }
        };
        Self {
            root,
            from,
            host,
            collector,
            notes: Vec::new(),
        }
    }

    /// One line on what is in the directory.
    pub fn describe(&self) -> String {
        let at_root = |p: &PathBuf| *p == self.root;
        match (&self.host, &self.collector) {
            (Some(h), None) if at_root(h) => "a host's".to_string(),
            (None, Some(c)) if at_root(c) => "a collector's".to_string(),
            (Some(_), Some(_)) => "hennery up's: a host in host/ and a collector in collector/".to_string(),
            (Some(_), None) => "hennery up's: a host in host/, no collector".to_string(),
            (None, Some(_)) => "hennery up's: a collector in collector/, no host".to_string(),
            (None, None) if self.root.is_dir() => "nothing of hennery's is in it".to_string(),
            (None, None) => "it does not exist".to_string(),
        }
    }
}

/// A host's directory: a pairing, a key, or a managed runtime is in it.
fn is_host(dir: &Path) -> bool {
    use hennery_host::identity::{CONFIG_FILE, KEY_FILE};
    crate::pairing_in(dir) != crate::Pairing::None
        || dir.join(KEY_FILE).exists()
        || dir.join(CONFIG_FILE).exists()
        || dir.join("adapters").is_dir()
}

/// A collector's directory: its database, its settings or its socket.
fn is_collector(dir: &Path) -> bool {
    [
        "hennery.db",
        crate::config::CONFIG_FILE,
        hennery_kernel::admin::ADMIN_SOCKET,
    ]
    .iter()
    .any(|name| dir.join(name).exists())
}
