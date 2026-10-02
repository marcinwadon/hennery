//! The host's default agents from its installed set (distribution spec
//! §3.2): `claude` and `codex` as `<runtime>/bin/node <set>/<agent>/<entry>`,
//! by absolute paths, never through `current`; and the operator's own CLIs
//! (`--use-cli`, §13 decision 5), recorded in `host.toml`.

use super::download::Sources;
use super::install::{self, InstalledSet, Layout, Selection};
use crate::adapter::AgentCommand;
use crate::identity::{CONFIG_FILE, read_table, write_private};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

/// The variable through which each pinned adapter takes another CLI.
pub const CLI_VARS: &[(&str, &str)] = &[("claude", "CLAUDE_CODE_EXECUTABLE"), ("codex", "CODEX_PATH")];

/// `host.toml`'s table of CLI overrides.
pub const CLI_TABLE: &str = "cli";

/// How long a host start spends installing the pinned set before it runs
/// on what it has.
pub const START_INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// What `--use-cli <agent>=bundled` means: back to the pinned CLI.
pub const BUNDLED: &str = "bundled";

fn cli_var(agent: &str) -> Option<&'static str> {
    CLI_VARS.iter().find(|(a, _)| *a == agent).map(|(_, v)| *v)
}

/// One `--use-cli` argument: `agent=/absolute/path`, or `agent=bundled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseCli {
    pub agent: String,
    /// `None`: the bundled CLI again.
    pub path: Option<PathBuf>,
}

impl UseCli {
    pub fn parse(arg: &str) -> Result<Self> {
        let (agent, path) = arg
            .split_once('=')
            .context("expected agent=/path/to/cli or agent=bundled")?;
        if cli_var(agent).is_none() {
            let known: Vec<&str> = CLI_VARS.iter().map(|(a, _)| *a).collect();
            bail!("--use-cli knows the agents {known:?}, not {agent:?}");
        }
        let path = if path == BUNDLED {
            None
        } else {
            Some(check_cli(Path::new(path))?)
        };
        Ok(Self {
            agent: agent.to_string(),
            path,
        })
    }
}

/// An operator's CLI: an absolute path of plain components (kept as given,
/// not resolved, so a version manager's link keeps working), naming an
/// executable regular file.
pub fn check_cli(path: &Path) -> Result<PathBuf> {
    // `components()` drops a `.` inside a path, so a path that is not its
    // own components joined again had one.
    // (`Path`'s own `==` compares components, so compare the bytes.)
    let plain = path.components().collect::<PathBuf>().as_os_str() == path.as_os_str();
    if !path.is_absolute() || !plain || path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("{} is not an absolute path without `.` or `..`", path.display());
    }
    let meta = std::fs::metadata(path).with_context(|| format!("{} cannot be read", path.display()))?;
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        bail!("{} is not an executable file", path.display());
    }
    Ok(path.to_path_buf())
}

/// The overrides `host.toml` records, by agent.
pub fn cli_overrides(data_dir: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let path = data_dir.join(CONFIG_FILE);
    let table = read_table(&path)?;
    let mut out = BTreeMap::new();
    if let Some(cli) = table.get(CLI_TABLE) {
        let cli = cli
            .as_table()
            .with_context(|| format!("{}: [{CLI_TABLE}] is not a table", path.display()))?;
        for (agent, value) in cli {
            let value = value
                .as_str()
                .with_context(|| format!("{}: {CLI_TABLE}.{agent} is not a path", path.display()))?;
            out.insert(agent.clone(), PathBuf::from(value));
        }
    }
    Ok(out)
}

/// Record (or, with `path: None`, remove) an override in `host.toml`,
/// keeping every other key. The host must be paired.
pub fn set_cli_override(data_dir: &Path, choice: &UseCli) -> Result<()> {
    let path = data_dir.join(CONFIG_FILE);
    if !path.exists() {
        bail!(
            "{} does not exist: pair this host first (`hennery host join`)",
            path.display()
        );
    }
    let mut table = read_table(&path)?;
    let cli = table
        .entry(CLI_TABLE)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .with_context(|| format!("{}: [{CLI_TABLE}] is not a table", path.display()))?;
    match &choice.path {
        Some(cli_path) => {
            let text = cli_path.to_str().context("the CLI's path is not UTF-8")?;
            cli.insert(choice.agent.clone(), toml::Value::String(text.to_string()));
        }
        None => {
            cli.remove(&choice.agent);
        }
    }
    if cli.is_empty() {
        table.remove(CLI_TABLE);
    }
    write_private(&path, toml::to_string(&table)?.as_bytes())
}

/// The agents whose bundled CLI an install leaves out.
pub fn skipped(overrides: &BTreeMap<String, PathBuf>) -> BTreeSet<String> {
    // A hand-written key for an agent with no CLI variable skips nothing:
    // it would otherwise make every pinned selection fail.
    overrides
        .keys()
        .filter(|agent| cli_var(agent).is_some())
        .cloned()
        .collect()
}

/// The agents a set launches, and what kept any from it.
#[derive(Debug, Default)]
pub struct Agents {
    pub agents: HashMap<String, AgentCommand>,
    /// One line per agent left out, or launched with a caveat.
    pub notes: Vec<String>,
}

/// Each adapter of `set`, started as `<node> <set>/<agent>/<entry>`, with
/// the override's variable when `host.toml` has one (checked again now).
/// A set without an agent's bundled CLI launches that agent only with an
/// override (A6).
pub fn from_set(set: &InstalledSet, overrides: &BTreeMap<String, PathBuf>) -> Agents {
    let mut out = Agents::default();
    for (name, adapter) in &set.record.adapters {
        let entry = set.path.join(name).join(&adapter.entry);
        let mut command = AgentCommand {
            program: set.node.to_string_lossy().into_owned(),
            args: vec![entry.to_string_lossy().into_owned()],
            env: Vec::new(),
        };
        match (overrides.get(name), cli_var(name)) {
            (Some(path), Some(var)) => match check_cli(path) {
                Ok(path) => {
                    command.env.push((var.to_string(), path.to_string_lossy().into_owned()));
                    if !adapter.cli_skipped {
                        out.notes.push(format!(
                            "{name}: using {} rather than the set's bundled CLI",
                            path.display()
                        ));
                    }
                }
                Err(err) => {
                    out.notes
                        .push(format!("{name} is unavailable: its --use-cli CLI: {err:#}"));
                    continue;
                }
            },
            (Some(_), None) => {
                out.notes
                    .push(format!("{name}: no CLI override is possible; host.toml's is ignored"));
                if adapter.cli_skipped {
                    continue;
                }
            }
            (None, _) if adapter.cli_skipped => {
                out.notes.push(format!(
                    "{name} is unavailable: its set has no bundled CLI and host.toml names none; \
                     run `hennery host adapters update`"
                ));
                continue;
            }
            (None, _) => {}
        }
        out.agents.insert(name.clone(), command);
    }
    out
}

/// What `prepare` leaves the host with.
#[derive(Debug, Default)]
pub struct Prepared {
    pub agents: Agents,
    /// The set the agents launch from, held for the host's lifetime so no
    /// collection removes it.
    pub in_use: Option<std::fs::File>,
    pub set: Option<InstalledSet>,
}

/// `host run` without `--agent` (decisions 11–13): install `selection` from
/// `sources` if it is not current (unless a rollback holds the host, or
/// there are no valid sources), then take the agents from the current set.
/// Never fails: a host offline at start keeps the set it has, and one with
/// none runs with no agents; `notes` says which.
pub async fn prepare(
    data_dir: &Path,
    selection: Option<&Selection>,
    sources: Option<&Sources>,
    progress: &(dyn Fn(&str) + Sync),
) -> Prepared {
    let mut notes = Vec::new();
    let layout = match Layout::new(data_dir) {
        Ok(layout) => layout,
        Err(err) => {
            notes.push(format!("no managed agents: {err:#}"));
            return Prepared {
                agents: Agents {
                    notes,
                    ..Agents::default()
                },
                ..Prepared::default()
            };
        }
    };
    let overrides = cli_overrides(data_dir).unwrap_or_else(|err| {
        notes.push(format!("host.toml's CLI overrides are ignored: {err:#}"));
        BTreeMap::new()
    });
    let current = layout.current().ok().flatten();
    match selection {
        None => notes.push("this platform has no pinned adapter set".into()),
        Some(selection) if current.as_ref().is_some_and(|c| c.id == selection.set_id() && c.node.is_file()) => {}
        Some(_) if layout.held() => notes.push(
            "a rollback holds this host on its adapter set; `hennery host adapters update` returns it to the pinned one"
                .into(),
        ),
        Some(_) if sources.is_none() => notes.push("the pinned adapter set was not installed: no valid mirror".into()),
        Some(selection) => {
            let sources = sources.expect("checked above");
            // Bounded: a start never waits on another install, nor on a
            // download that trickles; what was fetched resumes next time.
            let attempt = install::try_install(&layout, selection, sources, progress);
            match tokio::time::timeout(START_INSTALL_TIMEOUT, attempt).await {
                Ok(Ok(Some(_))) => {}
                Ok(Ok(None)) => notes.push(
                    "the pinned adapter set was not installed: another install of it is running".into(),
                ),
                Ok(Err(err)) => notes.push(format!("the pinned adapter set was not installed: {err:#}")),
                Err(_) => notes.push(format!(
                    "the pinned adapter set was not installed within {} minutes; it resumes at the next start",
                    START_INSTALL_TIMEOUT.as_secs() / 60
                )),
            }
        }
    }
    // Read again: the install may have switched it, or a collection run by
    // `adapters update` meanwhile may have removed what was read.
    let mut lost = None;
    for _ in 0..2 {
        let set = match layout.current() {
            Ok(Some(set)) => set,
            Ok(None) => {
                notes.push("no adapter set is installed: run `hennery host adapters update`".into());
                break;
            }
            Err(err) => {
                notes.push(format!("the current adapter set is unreadable: {err:#}"));
                break;
            }
        };
        let in_use = match install::hold_in_use(&layout, &set) {
            Ok(in_use) => in_use,
            Err(err) => {
                lost = Some(err);
                continue;
            }
        };
        if let Some(selection) = selection
            && set.id != selection.set_id()
        {
            notes.push(format!(
                "running adapter set {}, not the pinned {}",
                set.id,
                selection.set_id()
            ));
        }
        if !set.node.is_file() {
            notes.push(format!(
                "the adapter set's Node ({}) is missing: no agents run; `hennery host adapters update` installs it again",
                set.node.display()
            ));
            return Prepared {
                agents: Agents {
                    notes,
                    ..Agents::default()
                },
                in_use: Some(in_use),
                set: Some(set),
            };
        }
        let mut agents = from_set(&set, &overrides);
        notes.append(&mut agents.notes);
        agents.notes = notes;
        return Prepared {
            agents,
            in_use: Some(in_use),
            set: Some(set),
        };
    }
    if let Some(err) = lost {
        notes.push(format!(
            "the current adapter set went away as the host started: {err:#}"
        ));
    }
    Prepared {
        agents: Agents {
            notes,
            ..Agents::default()
        },
        ..Prepared::default()
    }
}

/// `prepare` for the set this binary pins, less the CLIs `host.toml`
/// overrides.
pub async fn prepare_pinned(data_dir: &Path, sources: Option<&Sources>, progress: &(dyn Fn(&str) + Sync)) -> Prepared {
    let skip = cli_overrides(data_dir).map(|o| skipped(&o)).unwrap_or_default();
    let (selection, why) = match Selection::pinned(&skip) {
        Ok(selection) => (Some(selection), None),
        Err(err) => (None, Some(format!("{err:#}"))),
    };
    let mut prepared = prepare(data_dir, selection.as_ref(), sources, progress).await;
    if let Some(why) = why {
        prepared.agents.notes.insert(0, format!("no pinned adapter set: {why}"));
    }
    prepared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_cli_takes_an_absolute_executable_or_bundled() {
        assert_eq!(
            UseCli::parse("claude=/bin/sh").unwrap(),
            UseCli {
                agent: "claude".into(),
                path: Some("/bin/sh".into())
            }
        );
        assert_eq!(UseCli::parse("codex=bundled").unwrap().path, None);
        for bad in [
            "claude",
            "gemini=/bin/sh",
            "claude=bin/sh",
            "claude=/bin/../bin/sh",
            "claude=/nonexistent/claude",
            "claude=/etc/hosts",
            "claude=/bin",
            "claude=/bin/./sh",
        ] {
            assert!(UseCli::parse(bad).is_err(), "{bad}");
        }
        // Refused for being relative, whatever the working directory holds.
        let err = format!("{:#}", UseCli::parse("claude=bin/sh").unwrap_err());
        assert!(err.contains("not an absolute path"), "{err}");
        // An override for an agent with no CLI variable skips nothing.
        let overrides = BTreeMap::from([
            ("gemini".to_string(), PathBuf::from("/bin/sh")),
            ("codex".to_string(), PathBuf::from("/bin/sh")),
        ]);
        assert_eq!(skipped(&overrides), BTreeSet::from(["codex".to_string()]));
    }

    #[test]
    fn an_override_is_recorded_and_cleared_keeping_the_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let use_cli = UseCli::parse("claude=/bin/sh").unwrap();
        assert!(set_cli_override(dir.path(), &use_cli).is_err(), "unpaired");
        let key = crate::identity::HostKey::from_seed([1; 32]);
        crate::identity::write_config_to(&dir.path().join(CONFIG_FILE), "ws://c/api/hosts/ws", "host-1", &key).unwrap();
        set_cli_override(dir.path(), &use_cli).unwrap();
        assert_eq!(cli_overrides(dir.path()).unwrap()["claude"], Path::new("/bin/sh"));
        let paired = read_table(&dir.path().join(CONFIG_FILE)).unwrap();
        assert_eq!(paired["host_id"].as_str(), Some("host-1"));
        set_cli_override(dir.path(), &UseCli::parse("claude=bundled").unwrap()).unwrap();
        assert!(cli_overrides(dir.path()).unwrap().is_empty());
        let text = std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
        assert!(!text.contains("[cli]"), "{text}");
        // The pairing still loads with a `[cli]` beside it.
        set_cli_override(dir.path(), &use_cli).unwrap();
        crate::identity::HostKey::from_seed([1; 32])
            .save(&dir.path().join(crate::identity::KEY_FILE))
            .unwrap();
        let paired = crate::identity::Paired::load(dir.path()).unwrap().unwrap();
        assert_eq!(paired.host_id, "host-1");
    }
}
