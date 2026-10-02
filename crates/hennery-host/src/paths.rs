//! Resolving a typed path on the host (kernel spec §5.2, §5.4), where the
//! filesystem is: what a session's hat is decided on. The canonical form
//! is absolute, symlinks resolved, with no `.`, `..` or trailing slash
//! (umbrella §8.2), the same form the agents themselves key projects by.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// The longest path accepted, in bytes: Linux's `PATH_MAX`, as the
/// collector's rule prefixes (`hennery_kernel::hats::MAX_PATH`).
pub const MAX_PATH: usize = 4096;

/// A resolved path (`resolved_path`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub canonical: String,
    pub exists: bool,
    pub is_dir: bool,
}

/// Resolve `typed`: `~` and `~/…` are the host user's `home`, anything
/// else must be absolute. A path that exists is canonicalised by the
/// filesystem. One that does not has its deepest existing ancestor
/// canonicalised and the rest normalised by its text: a rule saved for a
/// directory not made yet then still matches it once it exists under a
/// symlinked parent. Refuses a relative path, `~user`, control characters,
/// a path longer than `MAX_PATH`, and a canonical form that is not UTF-8.
pub fn resolve(typed: &str, home: Option<&Path>) -> Result<Resolved, String> {
    if typed.len() > MAX_PATH {
        return Err(format!("a path must be at most {MAX_PATH} bytes"));
    }
    if typed.chars().any(char::is_control) {
        return Err("a path must not hold control characters".into());
    }
    let path = expand_home(typed, home)?;
    if !path.is_absolute() {
        return Err("a path must be absolute, or start with ~/".into());
    }
    let (canonical, exists) = match std::fs::canonicalize(&path) {
        Ok(canonical) => (canonical, true),
        Err(e) if e.kind() == ErrorKind::NotFound => (resolve_missing(&path)?, false),
        Err(e) => return Err(e.to_string()),
    };
    let is_dir = exists && canonical.is_dir();
    let canonical = canonical
        .into_os_string()
        .into_string()
        .map_err(|_| "the resolved path is not UTF-8".to_string())?;
    if canonical.len() > MAX_PATH {
        return Err(format!("the resolved path is longer than {MAX_PATH} bytes"));
    }
    Ok(Resolved {
        canonical,
        exists,
        is_dir,
    })
}

fn expand_home(typed: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let rest = match typed.strip_prefix('~') {
        None => return Ok(PathBuf::from(typed)),
        Some("") => "",
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/'),
        Some(_) => return Err("only ~ and ~/… name a home directory here".into()),
    };
    let home = home.ok_or("this host has no home directory to expand ~ to")?;
    Ok(home.join(rest))
}

/// `path` (absolute, not there) with its deepest existing ancestor
/// canonicalised and the components after it applied by their text. The
/// first of those must not be a symlink that does not resolve (the
/// review's P7): its name would stand in the result, and once the target
/// exists, paths under it would resolve to the target instead. Nothing
/// after that first missing component can exist, so a `..` there is
/// refused rather than climbed, which would otherwise land back in a
/// part of the path that does exist and so needs canonicalising, not
/// kept by its text. Any canonicalize error besides "not found", on the
/// ancestor or on the first missing component's own entry, is refused
/// with its own message rather than treated as missing (a permission
/// error must not be read as "nothing here").
fn resolve_missing(path: &Path) -> Result<PathBuf, String> {
    let components: Vec<Component<'_>> = path.components().collect();
    for split in (1..components.len()).rev() {
        let ancestor: PathBuf = components[..split].iter().collect();
        let mut out = match std::fs::canonicalize(&ancestor) {
            Ok(out) => out,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        if !out.is_dir() {
            return Err(format!("{:?} is not a directory", out.display()));
        }
        if let Component::Normal(name) = components[split] {
            match out.join(name).symlink_metadata() {
                Ok(_) => {
                    return Err(format!(
                        "{:?} is a symlink that does not resolve",
                        out.join(name).display()
                    ));
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        for component in &components[split..] {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err("a path must not climb out of a part that does not exist with `..`".into());
                }
                Component::Normal(name) => out.push(name),
                Component::RootDir | Component::Prefix(_) => return Err("not a plain absolute path".into()),
            }
        }
        return Ok(out);
    }
    Err("no part of the path exists on this host".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn canonical(p: &Path) -> String {
        std::fs::canonicalize(p)
            .unwrap()
            .into_os_string()
            .into_string()
            .unwrap()
    }

    #[test]
    fn an_existing_path_is_canonicalised_through_its_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("x")).unwrap();
        symlink(&real, dir.path().join("link")).unwrap();
        let root = canonical(dir.path());
        for typed in ["link/x", "link/x/", "link/./x", "real/x/../x", "link//x"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert_eq!(
                resolve(&typed, None),
                Ok(Resolved {
                    canonical: format!("{root}/real/x"),
                    exists: true,
                    is_dir: true
                }),
                "{typed}"
            );
        }
        std::fs::write(real.join("file"), b"").unwrap();
        let file = resolve(&format!("{}/link/file", dir.path().display()), None).unwrap();
        assert_eq!(
            (file.canonical, file.exists, file.is_dir),
            (format!("{root}/real/file"), true, false)
        );
    }

    /// Kernel spec §5.2's unverified rule, resolved as far as it exists: a
    /// directory not made yet under a symlinked parent gets the parent's
    /// canonical form.
    #[test]
    fn a_missing_path_keeps_its_existing_ancestor_resolved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let root = canonical(dir.path());
        let typed = format!("{}/link/not/yet/made/", dir.path().display());
        assert_eq!(
            resolve(&typed, None),
            Ok(Resolved {
                canonical: format!("{root}/real/not/yet/made"),
                exists: false,
                is_dir: false
            })
        );
    }

    #[test]
    fn a_dangling_symlink_is_refused_not_kept_by_its_name() {
        let dir = tempfile::tempdir().unwrap();
        symlink(dir.path().join("not-yet"), dir.path().join("link")).unwrap();
        for typed in ["link", "link/x"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert!(resolve(&typed, None).is_err(), "{typed}");
        }
    }

    /// Review finding (Important 1): a `..` after the first missing
    /// component must not climb back into a part of the path that does
    /// exist (`elsewhere`, reached only through the `link` symlink, or a
    /// dangling symlink that P7 would otherwise catch) and be kept by its
    /// text instead of canonicalised, or bypassed entirely.
    #[test]
    fn a_dotdot_after_a_missing_component_is_refused_not_climbed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, dir.path().join("link")).unwrap();
        symlink(dir.path().join("not-yet"), dir.path().join("dangling")).unwrap();
        for typed in ["missing/../link/x", "missing/../dangling/x", "missing/../link"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert!(resolve(&typed, None).is_err(), "{typed}");
        }
    }

    /// Review finding (Important 2): a file cannot hold a path under it.
    /// Through `resolve`, the top-level canonicalize of the whole path
    /// fails with `NotADirectory`, not `NotFound`, so it is refused there
    /// first, before `resolve_missing`'s own `is_dir` guard ever runs —
    /// see the direct `resolve_missing` test below for that guard in
    /// isolation.
    #[test]
    fn a_file_used_as_a_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), b"").unwrap();
        let typed = format!("{}/file/x", dir.path().display());
        assert!(resolve(&typed, None).is_err());
    }

    /// Review finding (Important 2), isolated: called directly (bypassing
    /// `resolve`'s own top-level `NotADirectory` refusal above),
    /// `resolve_missing`'s `is_dir` guard is what refuses a file used as
    /// a directory, with its own message.
    #[test]
    fn resolve_missing_refuses_a_file_used_as_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), b"").unwrap();
        let err = resolve_missing(&dir.path().join("file/x")).unwrap_err();
        assert!(err.contains("is not a directory"), "{err}");
    }

    /// Review finding (Important 2, minor): a symlink loop as the first
    /// missing component is refused, not misreported as a missing target.
    #[test]
    fn a_symlink_loop_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        symlink(dir.path().join("b"), dir.path().join("a")).unwrap();
        symlink(dir.path().join("a"), dir.path().join("b")).unwrap();
        for typed in ["a", "a/x"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert!(resolve(&typed, None).is_err(), "{typed}");
        }
    }

    /// Review finding (Important 2): a permission error part-way down the
    /// path must be refused with its own message, not read as "missing".
    /// Skipped as root, which ignores directory permissions outright.
    #[test]
    fn a_permission_denied_ancestor_is_refused_not_reported_missing() {
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: geteuid(2) cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let typed = format!("{}/blocked/rest/of/path", dir.path().display());
        let result = resolve(&typed, None);
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
    }

    /// The unverified rule (kernel spec §5.2) actually holds: once the
    /// directories a missing resolve predicted are made, canonicalizing
    /// the same typed path for real lands on the same string.
    #[test]
    fn a_missing_paths_canonical_form_matches_the_real_one_once_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        for rest in ["a/b", "c"] {
            let typed = format!("{}/link/{rest}", dir.path().display());
            let resolved = resolve(&typed, None).unwrap();
            assert!(!resolved.exists, "{typed}");
            std::fs::create_dir_all(dir.path().join("real").join(rest)).unwrap();
            assert_eq!(resolved.canonical, canonical(Path::new(&typed)), "{typed}");
        }
    }

    #[test]
    fn a_tilde_is_the_hosts_home_and_nothing_else_is_relative() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("Projects")).unwrap();
        let root = canonical(home.path());
        assert_eq!(resolve("~", Some(home.path())).unwrap().canonical, root);
        assert_eq!(
            resolve("~/Projects", Some(home.path())).unwrap().canonical,
            format!("{root}/Projects")
        );
        assert_eq!(
            resolve("~//Projects/", Some(home.path())).unwrap().canonical,
            format!("{root}/Projects")
        );
        for bad in ["~other/Projects", "Projects", "./Projects", "", "/p/\0x", "/p/\nx"] {
            assert!(resolve(bad, Some(home.path())).is_err(), "{bad:?}");
        }
        assert!(resolve("~/Projects", None).is_err());
        assert!(resolve(&format!("/{}", "a".repeat(MAX_PATH)), None).is_err());
    }

    /// On a case-insensitive filesystem (macOS by default) the canonical
    /// form has the case on disk, so a rule and a session typed in other
    /// cases resolve alike (plan 5a decision 9).
    #[cfg(target_os = "macos")]
    #[test]
    fn on_macos_the_canonical_form_has_the_case_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Acme")).unwrap();
        if !dir.path().join("ACME").exists() {
            // A case-sensitive volume: there is no other case to fold.
            return;
        }
        let root = canonical(dir.path());
        let typed = format!("{}/acme", dir.path().display());
        assert_eq!(resolve(&typed, None).unwrap().canonical, format!("{root}/Acme"));
    }
}
