//! The project picker on the host (ACP core §7; plan 6c): the workspace
//! roots, from `host.toml` or `--workspace-root`, and the user's home; the
//! git repositories under the roots (`list_projects`); and the browse fence
//! with its listings (`browse_directory`). Probe work runs on blocking
//! threads, bounded (`Probes`), never in the connection loop.

use crate::uplink::Uplink;
use anyhow::{Result, bail};
use hennery_proto::frames::{DirEntry, HostFrame, Project};
use hennery_proto::paths::is_within;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// The most workspace roots a host takes (decision 6).
pub const MAX_ROOTS: usize = 32;

/// The host user's home directory: `$HOME`, if it is set and absolute (the
/// review's A10). Without one, `~/` roots are refused and the browse fence
/// is the workspace roots alone.
pub fn home_dir() -> Option<PathBuf> {
    home_from(std::env::var_os("HOME"))
}

/// `home_dir`, from `$HOME`'s value.
fn home_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|home| home.is_absolute())
}

/// The workspace roots to use (decision 6): `given` (the `--workspace-root`
/// flags) if there are any, else `configured` (`host.toml`). Flags replace
/// the file's list, as `--listen` replaces `config.toml`'s (kernel spec §2).
///
/// Each root is absolute, or `~` / `~/…`, expanded against `home`, and
/// normalised lexically (no trailing slash, no `.`, no repeated `/`); at
/// most `MAX_ROOTS` once duplicates are dropped. A root is not checked for
/// existence: a missing one is tolerated (ACP core §7).
pub fn workspace_roots(given: &[String], configured: &[String], home: Option<&Path>) -> Result<Vec<PathBuf>> {
    let raw = if given.is_empty() { configured } else { given };
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in raw {
        let path = if root == "~" || root.starts_with("~/") {
            let Some(home) = home else {
                bail!("workspace root {root:?} needs $HOME, which is unset or not absolute");
            };
            match root.strip_prefix("~/") {
                // Not `join` of an absolute path, which would replace home.
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => home.to_path_buf(),
            }
        } else {
            PathBuf::from(root)
        };
        if !path.is_absolute() {
            bail!("workspace root {root:?} is not absolute; give /… or ~/…");
        }
        let path: PathBuf = path.components().collect();
        if path.to_str().is_none() {
            bail!("workspace root {root:?} is not valid UTF-8");
        }
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    if roots.len() > MAX_ROOTS {
        bail!("{} workspace roots; at most {MAX_ROOTS} are supported", roots.len());
    }
    Ok(roots)
}

/// The longest path a browse accepts, in bytes (decision 3).
pub const MAX_PATH: usize = 4096;

/// The most enumerations, and listings, a host runs at once (the review's
/// A4). One more is answered `busy`.
pub const MAX_LISTS: usize = 1;
pub const MAX_BROWSES: usize = 4;

/// The bounds of an enumeration and a listing (decisions 4 and 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Repositories kept per root (ACP core §7: 500).
    pub per_root: usize,
    /// How many levels below a root a repository may be. It defines the
    /// enumeration, so it does not make it partial.
    pub depth: usize,
    /// Directories visited per root.
    pub dirs_per_root: usize,
    /// Time for one enumeration, every root together, and for one listing.
    pub budget: Duration,
    /// Entries a listing returns.
    pub entries: usize,
    /// Entries a listing reads, before sorting.
    pub entries_read: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            per_root: 500,
            depth: 4,
            dirs_per_root: 10_000,
            budget: Duration::from_secs(5),
            entries: 1000,
            entries_read: 10_000,
        }
    }
}

/// The canonical form of `path`: absolute, every symlink resolved, as
/// UTF-8. The one canonicalisation of the fence, for `resolve_path` to
/// share (the review's A9). A path that is not UTF-8 is an error.
pub fn canonical(path: &Path) -> std::io::Result<String> {
    std::fs::canonicalize(path)?
        .into_os_string()
        .into_string()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "the path is not UTF-8"))
}

/// The browse fence (ACP core §7): the workspace roots and the home
/// directory, canonicalised when it is built, so a root made since counts
/// and a missing one is left out.
#[derive(Debug, Clone)]
pub struct Fence {
    roots: Vec<String>,
}

impl Fence {
    pub fn new(workspace_roots: &[PathBuf], home: Option<&Path>) -> Self {
        let roots = workspace_roots
            .iter()
            .map(PathBuf::as_path)
            .chain(home)
            .filter_map(|root| canonical(root).ok())
            .collect();
        Self { roots }
    }

    /// Whether the canonical path `path` lies inside the fence, by whole
    /// segments (`is_within`).
    pub fn admits(&self, path: &str) -> bool {
        self.roots.iter().any(|root| is_within(path, root))
    }
}

/// Whether `dir` holds `.git`: a repository, or a worktree of one (whose
/// `.git` is a file). Not followed: a `.git` symlink counts too. An error,
/// such as a directory macOS will not let the host read, is no.
fn is_repo(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(".git")).is_ok()
}

/// `list_projects` (ACP core §7): the git repositories under `roots`,
/// sorted and without duplicates, and whether a bound cut the enumeration
/// short (decision 5). Dot-directories are skipped, symlinks are not
/// followed, and a repository is not looked into. A missing or unreadable
/// root is skipped.
pub fn list_projects(roots: &[PathBuf], limits: &Limits) -> (Vec<Project>, bool) {
    let deadline = Instant::now() + limits.budget;
    let mut found = BTreeSet::new();
    let mut partial = false;
    for root in roots {
        let Ok(root) = canonical(root) else {
            continue;
        };
        partial |= walk(&root, limits, deadline, &mut found);
    }
    (found.into_iter().map(|path| Project { path }).collect(), partial)
}

/// Walk one root breadth first, adding its repositories to `found`. `true`
/// if a bound cut it short.
fn walk(root: &str, limits: &Limits, deadline: Instant, found: &mut BTreeSet<String>) -> bool {
    let mut queue = VecDeque::from([(PathBuf::from(root), 0usize)]);
    let (mut repos, mut visited, mut cut) = (0usize, 0usize, false);
    while let Some((dir, depth)) = queue.pop_front() {
        if Instant::now() >= deadline {
            return true;
        }
        visited += 1;
        if is_repo(&dir) {
            if repos == limits.per_root {
                return true;
            }
            if let Some(path) = dir.to_str() {
                found.insert(path.to_string());
                repos += 1;
            }
            continue;
        }
        if depth == limits.depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        // Sorted, so the same tree gives the same answer, bounds included.
        let mut children = Vec::new();
        for entry in entries.flatten() {
            if entry.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            // `file_type` does not follow a symlink: a linked directory is
            // not walked into, so a link cannot lead out or round in a loop.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if visited + queue.len() + children.len() >= limits.dirs_per_root {
                cut = true;
                break;
            }
            children.push(entry.path());
        }
        children.sort();
        queue.extend(children.into_iter().map(|child| (child, depth + 1)));
    }
    cut
}

/// Why a browse is refused (decision 3): the host's `error` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseError {
    /// Not an absolute path of plain segments.
    Invalid,
    /// Outside the fence, or, if it does not exist, under an ancestor that
    /// is.
    OutsideWorkspace,
    PathNotFound,
    NotADirectory,
    PermissionDenied,
    /// Any other error reading it.
    Unreadable,
}

impl BrowseError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::OutsideWorkspace => "outside_workspace",
            Self::PathNotFound => "path_not_found",
            Self::NotADirectory => "not_a_directory",
            Self::PermissionDenied => "permission_denied",
            Self::Unreadable => "unreadable",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Invalid => "give an absolute path without `.` or `..` segments",
            Self::OutsideWorkspace => "the path is outside the workspace roots and the home directory",
            Self::PathNotFound => "no such directory",
            Self::NotADirectory => "the path is not a directory",
            Self::PermissionDenied => "the host may not read this directory",
            Self::Unreadable => "the host could not read this directory",
        }
    }

    /// The refusal for an I/O error on a path inside the fence.
    fn from_io(err: &std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => Self::PathNotFound,
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            std::io::ErrorKind::NotADirectory => Self::NotADirectory,
            _ => Self::Unreadable,
        }
    }
}

/// A browse's answer: `HostFrame::Directory` without its request id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
    pub truncated: bool,
}

/// Whether `path` is absolute and made of plain segments: no `.`, `..` or
/// empty segment (one trailing `/` aside), no control character, at most
/// `MAX_PATH` bytes. Read from the string itself, since `Path::components`
/// drops an inner `.` and merges `//` (the review's A2).
fn is_plain_absolute(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    path.len() <= MAX_PATH
        && !path.chars().any(char::is_control)
        && (rest.is_empty() || rest.split('/').all(|segment| !matches!(segment, "" | "." | "..")))
}

/// `browse_directory` (ACP core §7): the subdirectories of `path`, which
/// must be inside `fence` once canonicalised (decisions 3 and 4).
///
/// - A path that does not canonicalise is judged by its longest ancestor
///   that does: outside the fence it is `outside_workspace`, whether it
///   exists or not; only inside does the answer say why (the review's A1).
/// - The listing is of the canonical path, and each entry is judged by
///   what it resolves to (A3): a directory, or a symlink to a directory
///   inside the fence. Dot-entries, non-UTF-8 names and names with control
///   characters are left out. Sorted by name, at most `limits.entries`.
pub fn browse(fence: &Fence, path: &str, limits: &Limits) -> Result<Listing, BrowseError> {
    if !is_plain_absolute(path) {
        return Err(BrowseError::Invalid);
    }
    let canonical_path = match canonical(Path::new(path)) {
        Ok(found) => found,
        Err(err) => return Err(refusal_below(fence, Path::new(path), &err)),
    };
    if !fence.admits(&canonical_path) {
        return Err(BrowseError::OutsideWorkspace);
    }
    let dir = Path::new(&canonical_path);
    let metadata = std::fs::metadata(dir).map_err(|err| BrowseError::from_io(&err))?;
    if !metadata.is_dir() {
        return Err(BrowseError::NotADirectory);
    }
    let reader = std::fs::read_dir(dir).map_err(|err| BrowseError::from_io(&err))?;
    let deadline = Instant::now() + limits.budget;
    let (mut found, mut read, mut truncated) = (Vec::new(), 0usize, false);
    for entry in reader {
        if read == limits.entries_read || Instant::now() >= deadline {
            truncated = true;
            break;
        }
        read += 1;
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || name.chars().any(char::is_control) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let target = if kind.is_dir() {
            entry.path()
        } else if kind.is_symlink() {
            match canonical(&entry.path()) {
                Ok(resolved) if fence.admits(&resolved) && Path::new(&resolved).is_dir() => PathBuf::from(resolved),
                _ => continue,
            }
        } else {
            continue;
        };
        found.push((name, target));
    }
    found.sort();
    if found.len() > limits.entries {
        found.truncate(limits.entries);
        truncated = true;
    }
    let parent = dir
        .parent()
        .and_then(Path::to_str)
        .filter(|parent| fence.admits(parent))
        .map(str::to_string);
    let entries = found
        .into_iter()
        .map(|(name, target)| DirEntry {
            git: is_repo(&target),
            name,
        })
        .collect();
    Ok(Listing {
        path: canonical_path,
        parent,
        entries,
        truncated,
    })
}

/// The refusal for a path that does not canonicalise: decided by its
/// longest ancestor that does, so an existing and a missing path outside
/// the fence look the same.
fn refusal_below(fence: &Fence, path: &Path, err: &std::io::Error) -> BrowseError {
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if let Ok(found) = canonical(dir) {
            return if fence.admits(&found) {
                BrowseError::from_io(err)
            } else {
                BrowseError::OutsideWorkspace
            };
        }
        ancestor = dir.parent();
    }
    BrowseError::OutsideWorkspace
}

/// Runs the host's probe work (the review's A4): on blocking threads, never
/// in the connection loop, at most `MAX_LISTS` enumerations and
/// `MAX_BROWSES` listings at once. One more is answered `busy`; the
/// collector says so. The reply goes out on whatever connection is up when
/// it is ready; another connection's reply is dropped by the collector.
#[derive(Clone)]
pub struct Probes {
    lists: Arc<Semaphore>,
    browses: Arc<Semaphore>,
    limits: Limits,
}

impl Default for Probes {
    fn default() -> Self {
        Self::with_capacity(Limits::default(), MAX_LISTS, MAX_BROWSES)
    }
}

impl Probes {
    pub fn with_capacity(limits: Limits, lists: usize, browses: usize) -> Self {
        Self {
            lists: Arc::new(Semaphore::new(lists)),
            browses: Arc::new(Semaphore::new(browses)),
            limits,
        }
    }

    /// Answer `list_projects`: the repositories under `roots`, and `home`.
    pub fn list(&self, uplink: &Uplink, request_id: String, roots: Vec<PathBuf>, home: Option<PathBuf>) {
        let Ok(permit) = self.lists.clone().try_acquire_owned() else {
            uplink.reply(busy(request_id));
            return;
        };
        let (uplink, limits) = (uplink.clone(), self.limits);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let (items, partial) = list_projects(&roots, &limits);
            let home = home.and_then(|home| canonical(&home).ok());
            uplink.reply(HostFrame::Projects {
                request_id,
                items,
                partial,
                home,
            });
        });
    }

    /// Answer `browse_directory` for `path`, fenced by `roots` and `home`.
    pub fn browse(
        &self,
        uplink: &Uplink,
        request_id: String,
        path: String,
        roots: Vec<PathBuf>,
        home: Option<PathBuf>,
    ) {
        let Ok(permit) = self.browses.clone().try_acquire_owned() else {
            uplink.reply(busy(request_id));
            return;
        };
        let (uplink, limits) = (uplink.clone(), self.limits);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let fence = Fence::new(&roots, home.as_deref());
            uplink.reply(match browse(&fence, &path, &limits) {
                Ok(listing) => HostFrame::Directory {
                    request_id,
                    path: listing.path,
                    parent: listing.parent,
                    entries: listing.entries,
                    truncated: listing.truncated,
                },
                Err(refused) => HostFrame::Error {
                    request_id,
                    code: refused.code().into(),
                    message: refused.message().into(),
                },
            });
        });
    }
}

fn busy(request_id: String) -> HostFrame {
    HostFrame::Error {
        request_id,
        code: "busy".into(),
        message: "the host is busy with other probes; try again".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(roots: &[&str]) -> Vec<String> {
        roots.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn the_flags_replace_the_files_roots() {
        let home = Path::new("/home/u");
        let file = strings(&["/srv/a", "~/src"]);
        assert_eq!(
            workspace_roots(&[], &file, Some(home)).unwrap(),
            [PathBuf::from("/srv/a"), PathBuf::from("/home/u/src")]
        );
        assert_eq!(
            workspace_roots(&strings(&["/p"]), &file, Some(home)).unwrap(),
            [PathBuf::from("/p")]
        );
        assert!(workspace_roots(&[], &[], Some(home)).unwrap().is_empty());
    }

    #[test]
    fn a_root_is_absolute_or_under_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            workspace_roots(&strings(&["~", "~/", "/p", "/p"]), &[], Some(home)).unwrap(),
            [PathBuf::from("/home/u"), PathBuf::from("/p")],
            "`~/` is `~` again, so it is dropped as a duplicate"
        );
        for bad in ["src", "./src", "", "~other/src"] {
            let err = workspace_roots(&strings(&[bad]), &[], Some(home)).unwrap_err();
            assert!(err.to_string().contains("not absolute"), "{bad:?}: {err}");
        }
        let err = workspace_roots(&strings(&["~/src"]), &[], None).unwrap_err();
        assert!(err.to_string().contains("$HOME"), "{err}");
        assert_eq!(
            workspace_roots(&strings(&["/p"]), &[], None).unwrap(),
            [PathBuf::from("/p")]
        );
    }

    #[test]
    fn at_most_max_roots_are_taken() {
        let many: Vec<String> = (0..=MAX_ROOTS).map(|n| format!("/r{n}")).collect();
        let err = workspace_roots(&many, &[], None).unwrap_err();
        assert!(err.to_string().contains("at most"), "{err}");
        assert_eq!(workspace_roots(&many[1..], &[], None).unwrap().len(), MAX_ROOTS);
        // Duplicates do not count.
        let mut with_duplicate = many[1..].to_vec();
        with_duplicate.push("/r1/".into());
        assert_eq!(workspace_roots(&with_duplicate, &[], None).unwrap().len(), MAX_ROOTS);
    }

    /// Task 2's review: roots are normalised lexically, and `~//x` stays
    /// under home.
    #[test]
    fn roots_are_normalised() {
        let home = Path::new("/home/u");
        assert_eq!(
            workspace_roots(&strings(&["~//etc", "/p/", "/p//q/./r", "~/src/"]), &[], Some(home)).unwrap(),
            [
                PathBuf::from("/home/u/etc"),
                PathBuf::from("/p"),
                PathBuf::from("/p/q/r"),
                PathBuf::from("/home/u/src")
            ]
        );
    }

    #[test]
    fn home_counts_only_when_absolute_and_a_non_utf8_root_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(home_from(Some("/home/u".into())), Some(PathBuf::from("/home/u")));
        assert_eq!(home_from(Some("relative/home".into())), None);
        assert_eq!(home_from(Some("".into())), None);
        assert_eq!(home_from(None), None);
        let odd = Path::new(std::ffi::OsStr::from_bytes(b"/h\xff"));
        let err = workspace_roots(&strings(&["~/src"]), &[], Some(odd)).unwrap_err();
        assert!(err.to_string().contains("UTF-8"), "{err}");
    }
}
