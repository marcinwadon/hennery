//! Exact, read-only OMP session identity. A returned path is a binding to
//! record before announcement, never a lease or permission to remove data.

use super::OmpConfig;
use crate::walk;
use anyhow::{Context, Result, bail, ensure};
use std::ffi::CStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

const HEADER_LIMIT: u64 = 65536;
const PROJECT_LIMIT: usize = 4096;
const FILE_LIMIT: usize = 65536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub agent_session_id: String,
    pub executable: PathBuf,
    pub agent_data_dir: PathBuf,
    pub cwd: PathBuf,
    pub jsonl: PathBuf,
}

fn os<T>(result: Result<T, i32>) -> Result<T> {
    result.map_err(|errno| std::io::Error::from_raw_os_error(errno).into())
}

fn normal_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

/// Open every component once, relative to its already-open parent.
fn root(path: &Path) -> Result<OwnedFd> {
    ensure!(normal_absolute(path), "OMP root must be an absolute normal path");
    let mut fd = os(walk::open_root(Path::new("/")))?;
    for part in path.components() {
        if let Component::Normal(name) = part {
            fd = os(walk::open_dir_at(fd.as_raw_fd(), &walk::c_name(name.as_bytes())))?;
        }
    }
    let metadata = File::from(fd.try_clone()?).metadata()?;
    // SAFETY: geteuid has no preconditions.
    super::private_root(&metadata, unsafe { libc::geteuid() })?;
    Ok(fd)
}

fn safe_directory(fd: RawFd) -> Result<()> {
    let stat = os(walk::stat_fd(fd))?;
    // SAFETY: geteuid has no preconditions.
    ensure!(
        walk::is_dir(&stat) && stat.st_uid == unsafe { libc::geteuid() } && stat.st_mode & 0o022 == 0,
        "OMP session directory is unsafe"
    );
    Ok(())
}

fn file_at(parent: RawFd, name: &CStr) -> Result<File> {
    // SAFETY: parent is open and name is NUL terminated. O_NONBLOCK prevents
    // an unexpected FIFO from blocking the start before fstat rejects it.
    let raw = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    ensure!(raw >= 0, "OMP transcript cannot be opened safely");
    // SAFETY: this call owns the descriptor just opened.
    let file = unsafe { File::from_raw_fd(raw) };
    let stat = os(walk::stat_fd(file.as_raw_fd()))?;
    // SAFETY: geteuid has no preconditions.
    ensure!(
        walk::is_file(&stat) && stat.st_uid == unsafe { libc::geteuid() } && stat.st_mode & 0o022 == 0,
        "OMP transcript is unsafe"
    );
    Ok(file)
}

#[derive(serde::Deserialize)]
struct Header {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    cwd: String,
}

fn header(file: impl Read) -> Result<Option<Header>> {
    // One lookahead byte distinguishes physical EOF from a capped prefix.
    // The limit includes the title slot and relevant newline(s); a complete
    // header ending at real EOF is also valid. Never read the whole transcript.
    let mut reader = BufReader::new(file.take(HEADER_LIMIT + 1));
    let mut consumed = 0u64;
    let mut next_line = || -> Result<Option<Vec<u8>>> {
        let mut line = Vec::new();
        let count = reader.read_until(b'\n', &mut line)?;
        consumed += count as u64;
        Ok((count != 0 && consumed <= HEADER_LIMIT).then_some(line))
    };
    let Some(first) = next_line()? else {
        return Ok(None);
    };
    let Ok(first) = serde_json::from_slice::<serde_json::Value>(&first) else {
        return Ok(None);
    };
    // OMP's optional physical title slot is not the logical session header.
    let value = if first["type"] == "title"
        && first["v"] == 1
        && first["title"].is_string()
        && first["updatedAt"].is_string()
        && first["pad"].is_string()
        && (first.get("source").is_none() || first["source"] == "auto" || first["source"] == "user")
    {
        let Some(line) = next_line()? else {
            return Ok(None);
        };
        let Ok(value) = serde_json::from_slice(&line) else {
            return Ok(None);
        };
        value
    } else {
        first
    };
    Ok(serde_json::from_value::<Header>(value)
        .ok()
        .filter(|value| value.kind == "session" && !value.id.is_empty()))
}

/// Scan only direct project directories and JSONL children. Require one exact
/// ID across the tree and its canonical CWD, independently of filenames.
pub fn discover(config: &OmpConfig, id: &str, cwd: &Path) -> Result<Binding> {
    super::validate(config)?;
    ensure!(
        !id.is_empty() && cwd.canonicalize()? == cwd,
        "OMP session identity requires a canonical workspace"
    );
    let fd = root(&config.agent_data_dir)?;
    let sessions = os(walk::open_dir_at(fd.as_raw_fd(), c"sessions"))?;
    safe_directory(sessions.as_raw_fd())?;
    let projects = os(walk::list(sessions.as_raw_fd()))?;
    ensure!(
        projects.len() <= PROJECT_LIMIT,
        "OMP session tree exceeds the discovery limit"
    );
    let mut matched = None;
    let mut count = 0usize;
    for project in projects {
        let stat =
            os(walk::stat_at(sessions.as_raw_fd(), &project))?.context("OMP project disappeared during discovery")?;
        ensure!(!walk::is_link(&stat), "OMP session tree contains a symlink");
        if !walk::is_dir(&stat) {
            continue;
        }
        let dir = os(walk::open_dir_at(sessions.as_raw_fd(), &project))?;
        safe_directory(dir.as_raw_fd())?;
        let files = os(walk::list(dir.as_raw_fd()))?;
        count = count.saturating_add(files.len());
        ensure!(count <= FILE_LIMIT, "OMP session tree exceeds the discovery limit");
        for name in files {
            if !name.to_bytes().ends_with(b".jsonl") {
                continue;
            }
            let Some(header) = header(file_at(dir.as_raw_fd(), &name)?)? else {
                continue;
            };
            if header.id != id {
                continue;
            }
            ensure!(
                Path::new(&header.cwd) == cwd,
                "OMP session ID belongs to another workspace"
            );
            ensure!(matched.is_none(), "OMP session ID is ambiguous");
            let project = project.to_str().context("OMP project path is not UTF-8")?;
            let name = name.to_str().context("OMP transcript path is not UTF-8")?;
            matched = Some(config.agent_data_dir.join("sessions").join(project).join(name));
        }
    }
    let jsonl = matched.context("OMP did not persist an exact session identity")?;
    let binding = Binding {
        agent_session_id: id.into(),
        executable: config.executable.clone(),
        agent_data_dir: config.agent_data_dir.clone(),
        cwd: cwd.into(),
        jsonl,
    };
    verify(config, &binding, id, cwd)?;
    Ok(binding)
}

/// Resume the recorded file only. Never replace a vanished binding by an ID
/// lookup elsewhere in the root (OMP's own load has a global fallback).
pub fn verify(config: &OmpConfig, binding: &Binding, id: &str, cwd: &Path) -> Result<()> {
    super::validate(config)?;
    ensure!(
        normal_absolute(&binding.jsonl),
        "OMP transcript path is not an absolute normal path"
    );
    ensure!(
        binding.agent_session_id == id
            && binding.executable == config.executable
            && binding.agent_data_dir == config.agent_data_dir
            && binding.cwd == cwd
            && cwd.canonicalize()? == cwd,
        "OMP session binding changed"
    );
    let relative = binding.jsonl.strip_prefix(config.agent_data_dir.join("sessions"))?;
    let parts: Vec<_> = relative.components().collect();
    let [Component::Normal(project), Component::Normal(name)] = parts.as_slice() else {
        bail!("OMP transcript path is outside its recorded session tree")
    };
    ensure!(name.as_bytes().ends_with(b".jsonl"), "OMP transcript must be JSONL");
    let fd = root(&config.agent_data_dir)?;
    let sessions = os(walk::open_dir_at(fd.as_raw_fd(), c"sessions"))?;
    safe_directory(sessions.as_raw_fd())?;
    let project = os(walk::open_dir_at(
        sessions.as_raw_fd(),
        &walk::c_name(project.as_bytes()),
    ))?;
    safe_directory(project.as_raw_fd())?;
    let header = header(file_at(project.as_raw_fd(), &walk::c_name(name.as_bytes()))?)?
        .context("OMP session header is missing or malformed")?;
    ensure!(
        header.id == id && Path::new(&header.cwd) == cwd,
        "OMP transcript identity changed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture {
        _dir: tempfile::TempDir,
        config: OmpConfig,
        cwd: PathBuf,
        file: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let agent_data_dir = root.join("agent");
            let project = agent_data_dir.join("sessions/opaque-project");
            std::fs::create_dir_all(&project).unwrap();
            std::fs::set_permissions(&agent_data_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            let executable = root.join("omp");
            std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let cwd = root.join("workspace");
            std::fs::create_dir(&cwd).unwrap();
            let file = project.join("unrelated-name.jsonl");
            let fixture = Self {
                _dir: dir,
                config: OmpConfig {
                    executable,
                    agent_data_dir,
                },
                cwd,
                file,
            };
            fixture.write("exact-id", false);
            fixture
        }

        fn write(&self, id: &str, title: bool) {
            let header = serde_json::json!({"type":"session", "id":id, "cwd":self.cwd});
            let title = if title {
                "{\"type\":\"title\",\"v\":1,\"title\":\"hello\",\"updatedAt\":\"now\",\"pad\":\" \"}\n"
            } else {
                ""
            };
            std::fs::write(
                &self.file,
                format!("{title}{header}\n{{\"type\":\"message\",\"text\":\"exact-id\"}}\n"),
            )
            .unwrap();
        }

        fn discover(&self) -> Result<Binding> {
            discover(&self.config, "exact-id", &self.cwd)
        }
    }

    #[test]
    fn logical_header_identifies_the_file_without_guessing_its_name() {
        let fixture = Fixture::new();
        for title in [false, true] {
            fixture.write("exact-id", title);
            let before = std::fs::read(&fixture.file).unwrap();
            let binding = fixture.discover().unwrap();
            assert_eq!(binding.jsonl, fixture.file);
            assert_eq!(std::fs::read(&fixture.file).unwrap(), before);
        }
        fixture.write("other-id", true);
        assert!(fixture.discover().is_err(), "message text is not a session header");
    }

    #[test]
    fn ambiguous_id_and_wrong_workspace_are_refused() {
        let fixture = Fixture::new();
        std::fs::copy(&fixture.file, fixture.file.with_file_name("duplicate.jsonl")).unwrap();
        assert!(fixture.discover().unwrap_err().to_string().contains("ambiguous"));
        std::fs::remove_file(fixture.file.with_file_name("duplicate.jsonl")).unwrap();
        let other = fixture.cwd.parent().unwrap().join("another-workspace");
        std::fs::create_dir(&other).unwrap();
        assert!(
            discover(&fixture.config, "exact-id", &other)
                .unwrap_err()
                .to_string()
                .contains("another workspace")
        );
    }

    #[test]
    fn malformed_or_oversized_header_cannot_be_found_by_later_content() {
        let fixture = Fixture::new();
        for prefix in ["not-json\n".to_string(), "x".repeat(HEADER_LIMIT as usize + 1)] {
            std::fs::write(
                &fixture.file,
                format!(
                    "{prefix}{}\n",
                    serde_json::json!({"type":"session", "id":"exact-id", "cwd":fixture.cwd})
                ),
            )
            .unwrap();
            assert!(fixture.discover().is_err());
        }
    }

    fn assert_truncated_header_is_refused(title: bool) {
        let fixture = Fixture::new();
        let binding = fixture.discover().unwrap();
        fixture.write("exact-id", title);
        let original = std::fs::read_to_string(&fixture.file).unwrap();
        let lines: Vec<_> = original.lines().collect();
        let prefix = if title {
            format!("{}\n{}", lines[0], lines[1])
        } else {
            lines[0].to_string()
        };
        let mut capped = prefix.into_bytes();
        capped.resize(HEADER_LIMIT as usize, b' ');
        capped.extend_from_slice(b"x\n");
        std::fs::write(&fixture.file, &capped).unwrap();
        let discovered = fixture.discover().is_ok();
        let verified = verify(&fixture.config, &binding, "exact-id", &fixture.cwd).is_ok();
        assert!(
            !discovered && !verified,
            "accepted truncated header, title={title}, discovered={discovered}, verified={verified}"
        );
        assert_eq!(std::fs::read(&fixture.file).unwrap(), capped);
    }

    #[test]
    fn valid_prefix_at_cap_cannot_hide_invalid_plain_header_continuation() {
        assert_truncated_header_is_refused(false);
    }

    #[test]
    fn valid_prefix_at_cap_cannot_hide_invalid_title_slot_header_continuation() {
        assert_truncated_header_is_refused(true);
    }

    #[test]
    fn complete_header_boundary_at_limit_is_distinguished_from_overflow() {
        let fixture = Fixture::new();
        for title in [false, true] {
            for newline in [false, true] {
                for size in [HEADER_LIMIT - 1, HEADER_LIMIT, HEADER_LIMIT + 1] {
                    fixture.write("exact-id", title);
                    let original = std::fs::read_to_string(&fixture.file).unwrap();
                    let lines: Vec<_> = original.lines().collect();
                    let prefix = if title {
                        format!("{}\n{}", lines[0], lines[1])
                    } else {
                        lines[0].to_string()
                    };
                    let mut bytes = prefix.into_bytes();
                    bytes.resize(size as usize - usize::from(newline), b' ');
                    if newline {
                        bytes.extend_from_slice(b"\n{\"type\":\"message\"}\n");
                    }
                    std::fs::write(&fixture.file, bytes).unwrap();
                    assert_eq!(
                        fixture.discover().is_ok(),
                        size <= HEADER_LIMIT,
                        "size={size}, title={title}, newline={newline}"
                    );
                }
            }
        }
    }

    #[test]
    fn unterminated_header_read_is_bounded_to_limit_plus_one() {
        struct Counted {
            bytes: std::io::Repeat,
            read: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl Read for Counted {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let count = self.bytes.read(buf)?;
                self.read.set(self.read.get() + count);
                Ok(count)
            }
        }
        let read = std::rc::Rc::new(std::cell::Cell::new(0));
        assert!(
            header(Counted {
                bytes: std::io::repeat(b' '),
                read: read.clone(),
            })
            .unwrap()
            .is_none()
        );
        assert!(read.get() > 0 && read.get() <= HEADER_LIMIT as usize + 1);
    }

    #[test]
    fn symlink_transcript_project_and_root_are_not_followed() {
        let fixture = Fixture::new();
        let target = fixture.file.with_extension("saved");
        std::fs::rename(&fixture.file, &target).unwrap();
        symlink(&target, &fixture.file).unwrap();
        assert!(fixture.discover().is_err());
        std::fs::remove_file(&fixture.file).unwrap();
        std::fs::rename(&target, &fixture.file).unwrap();
        let project = fixture.file.parent().unwrap();
        let moved = project.with_file_name("saved-project");
        std::fs::rename(project, &moved).unwrap();
        symlink(&moved, project).unwrap();
        assert!(fixture.discover().is_err());
        std::fs::remove_file(project).unwrap();
        std::fs::rename(&moved, project).unwrap();
        let root_link = fixture.cwd.parent().unwrap().join("linked-root");
        symlink(&fixture.config.agent_data_dir, &root_link).unwrap();
        let mut config = fixture.config.clone();
        config.agent_data_dir = root_link;
        assert!(discover(&config, "exact-id", &fixture.cwd).is_err());
    }

    #[test]
    fn resume_checks_the_recorded_file_instead_of_finding_a_replacement() {
        let fixture = Fixture::new();
        let binding = fixture.discover().unwrap();
        std::fs::rename(&fixture.file, fixture.file.with_file_name("moved.jsonl")).unwrap();
        assert!(verify(&fixture.config, &binding, "exact-id", &fixture.cwd).is_err());
        assert!(
            fixture.discover().is_ok(),
            "new discovery could find it, but resume must not replace the binding"
        );
    }

    #[test]
    fn resume_refuses_changed_header_command_root_and_mode() {
        let fixture = Fixture::new();
        let binding = fixture.discover().unwrap();
        fixture.write("changed-id", false);
        assert!(verify(&fixture.config, &binding, "exact-id", &fixture.cwd).is_err());
        fixture.write("exact-id", false);
        let mut changed = binding.clone();
        changed.executable = changed.executable.with_file_name("another-program");
        assert!(verify(&fixture.config, &changed, "exact-id", &fixture.cwd).is_err());
        let mut changed = binding.clone();
        changed.agent_data_dir = changed.agent_data_dir.with_file_name("another-root");
        assert!(verify(&fixture.config, &changed, "exact-id", &fixture.cwd).is_err());
        std::fs::set_permissions(&fixture.file, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(verify(&fixture.config, &binding, "exact-id", &fixture.cwd).is_err());
    }

    #[test]
    fn binding_and_home_survive_reopen_and_cannot_be_rebound() {
        let fixture = Fixture::new();
        let binding = fixture.discover().unwrap();
        let db = fixture.cwd.parent().unwrap().join("homes.db");
        let registry = crate::agent_home::Registry::open(&db).unwrap();
        let home = registry.record_omp(&binding).unwrap();
        assert!(registry.contains(crate::agent_home::OMP, "exact-id", &home).unwrap());
        registry.record_omp(&binding).unwrap();
        drop(registry);
        let registry = crate::agent_home::Registry::open(&db).unwrap();
        assert_eq!(registry.omp_binding("exact-id").unwrap(), Some(binding.clone()));
        let mut changed = binding.clone();
        changed.jsonl = fixture.file.with_file_name("another.jsonl");
        std::fs::copy(&fixture.file, &changed.jsonl).unwrap();
        assert!(registry.record_omp(&changed).is_err());
        assert_eq!(registry.omp_binding("exact-id").unwrap(), Some(binding));
    }

    #[test]
    fn home_insert_failure_rolls_back_the_exact_binding() {
        let fixture = Fixture::new();
        let binding = fixture.discover().unwrap();
        let db = fixture.cwd.parent().unwrap().join("homes.db");
        let registry = crate::agent_home::Registry::open(&db).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_home BEFORE INSERT ON agent_homes BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .unwrap();
        assert!(registry.record_omp(&binding).is_err());
        assert_eq!(registry.omp_binding("exact-id").unwrap(), None);
        conn.execute_batch("DROP TRIGGER reject_home;").unwrap();
        let home = registry.record_omp(&binding).unwrap();
        assert!(registry.contains(crate::agent_home::OMP, "exact-id", &home).unwrap());
    }
}
