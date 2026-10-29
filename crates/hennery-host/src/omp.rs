//! Persisted external OMP configuration and checked storage contracts.
//! Configuration alone never establishes runtime or MCP support.

use crate::identity::{CONFIG_FILE, Paired, read_table, write_private};
use anyhow::{Context, Result, bail};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub mod probe;
pub mod session;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmpConfig {
    pub executable: PathBuf,
    pub agent_data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmpConfigStatus {
    Missing,
    Valid(OmpConfig),
    /// A bounded diagnostic without the operator's configured paths.
    Invalid(String),
}

fn path_metadata(path: &Path) -> Result<std::fs::Metadata> {
    let plain = path.components().collect::<PathBuf>().as_os_str() == path.as_os_str();
    if !path.is_absolute() || !plain || path.components().any(|part| matches!(part, Component::ParentDir)) {
        bail!("OMP paths must be absolute and contain only normal components");
    }
    path.to_str().context("OMP paths must be UTF-8")?;
    let mut prefix = PathBuf::new();
    let mut metadata = None;
    for part in path.components() {
        prefix.push(part);
        let meta = std::fs::symlink_metadata(&prefix).context("OMP path cannot be inspected")?;
        if meta.is_symlink() {
            bail!("OMP paths must not contain symlinks");
        }
        metadata = Some(meta);
    }
    metadata.context("OMP path cannot be inspected")
}

fn private_root(meta: &std::fs::Metadata, uid: u32) -> Result<()> {
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o777 != 0o700 {
        bail!("OMP agent-data root must be a private directory owned by the host user (mode 0700)");
    }
    Ok(())
}

pub(crate) fn validate(config: &OmpConfig) -> Result<()> {
    let program = path_metadata(&config.executable)?;
    if !program.is_file() || program.permissions().mode() & 0o111 == 0 || program.permissions().mode() & 0o022 != 0 {
        bail!("OMP executable must be a regular executable file, not writable by group or others");
    }
    let executable = std::ffi::CString::new(config.executable.as_os_str().as_encoded_bytes())?;
    // SAFETY: executable is NUL-terminated and valid for the call. AT_EACCESS
    // checks the same effective user that owns the OMP agent-data directory.
    if unsafe { libc::faccessat(libc::AT_FDCWD, executable.as_ptr(), libc::X_OK, libc::AT_EACCESS) } != 0 {
        bail!("OMP executable is not runnable by the host user");
    }
    // SAFETY: geteuid has no arguments or memory preconditions.
    private_root(&path_metadata(&config.agent_data_dir)?, unsafe { libc::geteuid() })
}

fn decode(value: &toml::Value) -> Result<OmpConfig> {
    let table = value.as_table().context("agents.omp must be a table")?;
    let path = |key| {
        table
            .get(key)
            .and_then(toml::Value::as_str)
            .map(PathBuf::from)
            .context("agents.omp requires string executable and agent_data_dir fields")
    };
    let config = OmpConfig {
        executable: path("executable")?,
        agent_data_dir: path("agent_data_dir")?,
    };
    validate(&config)?;
    Ok(config)
}

/// Unsafe metadata or malformed whole TOML is fatal. A malformed OMP section
/// in otherwise valid TOML is `Invalid`; no configuration means `Missing`.
pub fn read_omp_config(data_dir: &Path) -> Result<OmpConfigStatus> {
    let table = read_table(&data_dir.join(CONFIG_FILE))?;
    let Some(agents) = table.get("agents") else {
        return Ok(OmpConfigStatus::Missing);
    };
    let Some(agents) = agents.as_table() else {
        return Ok(OmpConfigStatus::Invalid("agents must be a table".into()));
    };
    let Some(value) = agents.get("omp") else {
        return Ok(OmpConfigStatus::Missing);
    };
    Ok(match decode(value) {
        Ok(config) => OmpConfigStatus::Valid(config),
        Err(error) => OmpConfigStatus::Invalid(error.to_string()),
    })
}

fn paired(data_dir: &Path) -> Result<()> {
    Paired::read(data_dir)?.context("pair this host first (`hennery host join`)")?;
    Ok(())
}

/// Replace only `agents.omp`, preserving pairing, CLI overrides and unknown
/// TOML keys. No program is spawned and no adapter set is downloaded.
pub fn set_omp_config(data_dir: &Path, config: &OmpConfig) -> Result<()> {
    paired(data_dir)?;
    validate(config)?;
    let path = data_dir.join(CONFIG_FILE);
    let mut table = read_table(&path)?;
    let agents = table
        .entry("agents")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let agents = agents.as_table_mut().context("agents must be a table")?;
    let mut omp = toml::Table::new();
    omp.insert(
        "executable".into(),
        config
            .executable
            .to_str()
            .context("OMP executable must be UTF-8")?
            .into(),
    );
    omp.insert(
        "agent_data_dir".into(),
        config
            .agent_data_dir
            .to_str()
            .context("OMP agent-data root must be UTF-8")?
            .into(),
    );
    agents.insert("omp".into(), toml::Value::Table(omp));
    write_private(&path, toml::to_string(&table)?.as_bytes())
}

/// Remove only `agents.omp`. An absent section succeeds without rewriting the
/// file; invalid whole TOML or pairing never gets repaired by this command.
pub fn remove_omp_config(data_dir: &Path) -> Result<bool> {
    paired(data_dir)?;
    let path = data_dir.join(CONFIG_FILE);
    let mut table = read_table(&path)?;
    let Some(agents) = table.get_mut("agents") else {
        return Ok(false);
    };
    let agents = agents.as_table_mut().context("agents must be a table")?;
    if agents.remove("omp").is_none() {
        return Ok(false);
    }
    write_private(&path, toml::to_string(&table)?.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{HostKey, storage_hooks};
    use std::os::unix::fs::symlink;

    struct Fixture {
        dir: tempfile::TempDir,
        host: PathBuf,
        config: OmpConfig,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            // macOS's temporary-directory spelling may contain /var's link.
            let base = dir.path().canonicalize().unwrap();
            let host = base.join("host");
            Paired {
                collector_url: "ws://127.0.0.1:1/api/hosts/ws".into(),
                trust: Default::default(),
                host_id: "host-omp".into(),
                key: HostKey::generate(),
                workspace_roots: vec!["/work".into()],
            }
            .save(&host)
            .unwrap();
            let executable = base.join("omp executable");
            std::fs::write(&executable, "#!/bin/sh\nexit 99\n").unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let agent_data_dir = base.join("agent data");
            std::fs::create_dir(&agent_data_dir).unwrap();
            std::fs::set_permissions(&agent_data_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                dir,
                host,
                config: OmpConfig {
                    executable,
                    agent_data_dir,
                },
            }
        }
        fn bytes(&self) -> Vec<u8> {
            std::fs::read(self.host.join(CONFIG_FILE)).unwrap()
        }
    }

    #[test]
    fn omp_preserves_other_keys_and_removes_only_its_section_idempotently() {
        let f = Fixture::new();
        let mut table = read_table(&f.host.join(CONFIG_FILE)).unwrap();
        table.insert("custom".into(), toml::Value::Array(vec!["keep".into(), "these".into()]));
        table.insert(
            "cli".into(),
            toml::Value::Table(toml::Table::from_iter([("claude".into(), "/bin/sh".into())])),
        );
        table.insert(
            "agents".into(),
            toml::Value::Table(toml::Table::from_iter([("other".into(), "keep".into())])),
        );
        write_private(&f.host.join(CONFIG_FILE), toml::to_string(&table).unwrap().as_bytes()).unwrap();
        assert_eq!(read_omp_config(&f.host).unwrap(), OmpConfigStatus::Missing);
        set_omp_config(&f.host, &f.config).unwrap();
        assert_eq!(
            read_omp_config(&f.host).unwrap(),
            OmpConfigStatus::Valid(f.config.clone())
        );
        let mut saved = read_table(&f.host.join(CONFIG_FILE)).unwrap();
        assert_eq!(
            saved["agents"]
                .as_table_mut()
                .unwrap()
                .remove("omp")
                .unwrap()
                .as_table()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(saved, table);
        assert_eq!(
            std::fs::metadata(f.host.join(CONFIG_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(remove_omp_config(&f.host).unwrap());
        assert_eq!(read_table(&f.host.join(CONFIG_FILE)).unwrap(), table);
        let before = f.bytes();
        assert!(!remove_omp_config(&f.host).unwrap());
        assert_eq!(before, f.bytes());
        Paired::read(&f.host).unwrap().unwrap();
        assert!(!f.host.join("adapters").exists());
    }

    #[test]
    fn omp_refuses_symlink_executable_root_and_ancestor_without_writing() {
        let f = Fixture::new();
        let before = f.bytes();
        let base = f.dir.path().canonicalize().unwrap();
        let program_link = base.join("program-link");
        symlink(&f.config.executable, &program_link).unwrap();
        let root_link = base.join("root-link");
        symlink(&f.config.agent_data_dir, &root_link).unwrap();
        let parent_link = base.join("parent-link");
        symlink(&base, &parent_link).unwrap();
        for config in [
            OmpConfig {
                executable: program_link,
                ..f.config.clone()
            },
            OmpConfig {
                agent_data_dir: root_link,
                ..f.config.clone()
            },
            OmpConfig {
                executable: parent_link.join("omp executable"),
                ..f.config.clone()
            },
        ] {
            assert!(set_omp_config(&f.host, &config).is_err(), "{config:?}");
            assert_eq!(before, f.bytes());
        }
    }

    #[test]
    fn omp_invalid_section_is_repairable_but_invalid_whole_toml_is_unchanged() {
        let f = Fixture::new();
        let original = String::from_utf8(f.bytes()).unwrap();
        for section in [
            "\n[agents]\nomp = 4\n",
            "\n[agents.omp]\nexecutable = 4\n",
            "\n[agents.omp]\nexecutable = 'relative'\nagent_data_dir = '/missing'\n",
        ] {
            write_private(&f.host.join(CONFIG_FILE), format!("{original}{section}").as_bytes()).unwrap();
            assert!(matches!(read_omp_config(&f.host).unwrap(), OmpConfigStatus::Invalid(_)));
            set_omp_config(&f.host, &f.config).unwrap();
            assert_eq!(
                read_omp_config(&f.host).unwrap(),
                OmpConfigStatus::Valid(f.config.clone())
            );
        }
        write_private(&f.host.join(CONFIG_FILE), b"[broken").unwrap();
        let before = f.bytes();
        assert!(read_omp_config(&f.host).is_err());
        assert!(set_omp_config(&f.host, &f.config).is_err());
        assert!(remove_omp_config(&f.host).is_err());
        assert_eq!(before, f.bytes());
    }

    #[test]
    fn omp_rejects_unsafe_paths_and_root_metadata() {
        let f = Fixture::new();
        let before = f.bytes();
        for path in [
            PathBuf::from("relative"),
            f.config.executable.parent().unwrap().join("./omp executable"),
            f.config.executable.parent().unwrap().join("../omp executable"),
            f.config.agent_data_dir.clone(),
        ] {
            assert!(
                set_omp_config(
                    &f.host,
                    &OmpConfig {
                        executable: path,
                        ..f.config.clone()
                    }
                )
                .is_err()
            );
            assert_eq!(before, f.bytes());
        }
        for mode in [0o600, 0o722] {
            std::fs::set_permissions(&f.config.executable, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(set_omp_config(&f.host, &f.config).is_err());
        }
        std::fs::set_permissions(&f.config.executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            set_omp_config(
                &f.host,
                &OmpConfig {
                    agent_data_dir: f.config.executable.clone(),
                    ..f.config.clone()
                }
            )
            .is_err()
        );
        for mode in [0o770, 0o707, 0o755, 0o600, 0o000] {
            std::fs::set_permissions(&f.config.agent_data_dir, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(set_omp_config(&f.host, &f.config).is_err());
        }
        std::fs::set_permissions(&f.config.agent_data_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let meta = path_metadata(&f.config.agent_data_dir).unwrap();
        assert!(private_root(&meta, meta.uid().wrapping_add(1)).is_err());
        assert_eq!(before, f.bytes());
    }

    #[test]
    fn omp_rechecks_saved_paths_and_invalid_agents_table_is_never_replaced() {
        let f = Fixture::new();
        set_omp_config(&f.host, &f.config).unwrap();
        std::fs::remove_file(&f.config.executable).unwrap();
        assert!(matches!(read_omp_config(&f.host).unwrap(), OmpConfigStatus::Invalid(_)));
        assert!(
            remove_omp_config(&f.host).unwrap(),
            "removing stale config must not require its program"
        );
        let f = Fixture::new();
        let mut table = read_table(&f.host.join(CONFIG_FILE)).unwrap();
        table.insert("agents".into(), 1.into());
        write_private(&f.host.join(CONFIG_FILE), toml::to_string(&table).unwrap().as_bytes()).unwrap();
        let before = f.bytes();
        assert!(matches!(read_omp_config(&f.host).unwrap(), OmpConfigStatus::Invalid(_)));
        assert!(set_omp_config(&f.host, &f.config).is_err());
        assert!(remove_omp_config(&f.host).is_err());
        assert_eq!(before, f.bytes());
    }

    #[test]
    fn omp_requires_the_actual_paired_key_and_refuses_unsafe_config_metadata() {
        let f = Fixture::new();
        HostKey::generate()
            .save(&f.host.join(crate::identity::KEY_FILE))
            .unwrap();
        let before = f.bytes();
        assert!(set_omp_config(&f.host, &f.config).is_err());
        assert!(remove_omp_config(&f.host).is_err());
        assert_eq!(before, f.bytes());
        assert!(set_omp_config(&f.dir.path().join("unpaired"), &f.config).is_err());
        let f = Fixture::new();
        std::fs::set_permissions(f.host.join(CONFIG_FILE), std::fs::Permissions::from_mode(0o666)).unwrap();
        let before = f.bytes();
        assert!(read_omp_config(&f.host).is_err());
        assert!(set_omp_config(&f.host, &f.config).is_err());
        assert!(remove_omp_config(&f.host).is_err());
        assert_eq!(before, f.bytes());
    }

    #[test]
    fn omp_failed_atomic_write_preserves_configuration() {
        let f = Fixture::new();
        for stage in ["before_write", "before_file_sync", "before_rename"] {
            let path = f.host.join(CONFIG_FILE);
            let before = f.bytes();
            storage_hooks::fail_once(path, stage);
            assert!(set_omp_config(&f.host, &f.config).is_err());
            assert_eq!(before, f.bytes(), "{stage}");
        }
        set_omp_config(&f.host, &f.config).unwrap();
        let before = f.bytes();
        storage_hooks::fail_once(f.host.join(CONFIG_FILE), "before_rename");
        assert!(remove_omp_config(&f.host).is_err());
        assert_eq!(before, f.bytes());
    }

    #[test]
    fn omp_command_keeps_path_arguments_literal_and_never_sets_an_environment() {
        let f = Fixture::new();
        let command = crate::runtime::agents::omp_command(&f.config).unwrap();
        assert_eq!(command.program, f.config.executable.to_str().unwrap());
        assert_eq!(
            command.args,
            ["--agent-dir", f.config.agent_data_dir.to_str().unwrap(), "acp"]
        );
        assert!(command.env.is_empty());
        let mut invalid = f.config.clone();
        invalid.executable = "relative".into();
        assert!(crate::runtime::agents::omp_command(&invalid).is_err());
    }

    #[test]
    fn omp_config_symlinks_and_non_utf8_paths_are_refused_without_changes() {
        use std::os::unix::ffi::OsStringExt;
        let f = Fixture::new();
        let before = f.bytes();
        let target = f.host.join("real.toml");
        std::fs::rename(f.host.join(CONFIG_FILE), &target).unwrap();
        symlink(&target, f.host.join(CONFIG_FILE)).unwrap();
        assert!(read_omp_config(&f.host).is_err());
        assert!(set_omp_config(&f.host, &f.config).is_err());
        assert!(remove_omp_config(&f.host).is_err());
        assert_eq!(std::fs::read(target).unwrap(), before);
        let f = Fixture::new();
        let invalid = OmpConfig {
            executable: PathBuf::from(std::ffi::OsString::from_vec(b"/invalid-\xff".to_vec())),
            ..f.config.clone()
        };
        assert!(set_omp_config(&f.host, &invalid).is_err());
        assert!(crate::runtime::agents::omp_command(&invalid).is_err());
    }

    #[test]
    fn omp_directory_sync_failure_is_reported_without_rolling_back_visible_state() {
        let f = Fixture::new();
        storage_hooks::fail_once(f.host.join(CONFIG_FILE), "directory_sync");
        let error = set_omp_config(&f.host, &f.config).unwrap_err();
        assert!(error.downcast_ref::<crate::identity::DurabilityUncertain>().is_some());
        assert_eq!(
            read_omp_config(&f.host).unwrap(),
            OmpConfigStatus::Valid(f.config.clone())
        );
        set_omp_config(&f.host, &f.config).unwrap();
        storage_hooks::fail_once(f.host.join(CONFIG_FILE), "directory_sync");
        let error = remove_omp_config(&f.host).unwrap_err();
        assert!(error.downcast_ref::<crate::identity::DurabilityUncertain>().is_some());
        assert_eq!(read_omp_config(&f.host).unwrap(), OmpConfigStatus::Missing);
    }
}
