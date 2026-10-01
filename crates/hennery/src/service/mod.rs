//! `hennery service install|uninstall|status` (distribution spec §6): a
//! per-user service for one role, a launchd user agent on macOS and a
//! systemd user unit on Linux. Everything that touches the machine goes
//! through a `Context`: its directories, its environment and, above all,
//! the service manager, which the tests replace with a fake.

pub mod path;
pub mod unit;

use crate::supervisor;
use anyhow::{Context as _, Result, bail};
use clap::{Args, Subcommand};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use unit::Role;

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    command: ServiceCommand,
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Write and start the service for a role: a launchd user agent
    /// (macOS) or a systemd user unit (Linux). One role per machine.
    Install {
        #[arg(long, value_enum, default_value_t = Role::Up)]
        role: Role,
        /// The data directory the service runs on; by default
        /// `~/Library/Application Support/hennery` (macOS) or
        /// `$XDG_DATA_HOME/hennery` (Linux). For `--role host`, the one
        /// `hennery host join` paired.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// The login shell whose PATH the service gets; by default the
        /// account's shell.
        #[arg(long)]
        shell: Option<PathBuf>,
    },
    /// Stop and remove the service; its data directory and logs are kept.
    Uninstall {
        /// By default, whichever role is installed.
        #[arg(long, value_enum)]
        role: Option<Role>,
    },
    /// Show the installed service: what it runs, whether it runs, and, for
    /// `up`, its children. Exits 1 unless it is installed and running, with
    /// no child given up on.
    Status {
        /// By default, whichever role is installed.
        #[arg(long, value_enum)]
        role: Option<Role>,
    },
}

pub fn run(args: ServiceArgs) -> Result<ExitCode> {
    let system = System;
    let cx = Context::from_process(&system)?;
    let mut out = std::io::stdout();
    match args.command {
        ServiceCommand::Install { role, data_dir, shell } => {
            install(&cx, role, data_dir.as_deref(), shell.as_deref(), &mut out)?;
            Ok(ExitCode::SUCCESS)
        }
        ServiceCommand::Uninstall { role } => {
            uninstall(&cx, role, &mut out)?;
            Ok(ExitCode::SUCCESS)
        }
        ServiceCommand::Status { role } => status(&cx, role, &mut out),
    }
}

/// The outcome of one service-manager command.
#[derive(Debug, Clone, Default)]
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `launchctl`, `systemctl` and `loginctl`.
pub trait Manager {
    fn run(&self, program: &str, args: &[&str]) -> Result<Ran>;
}

/// The machine's own service manager.
struct System;

impl Manager for System {
    fn run(&self, program: &str, args: &[&str]) -> Result<Ran> {
        let out = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .with_context(|| format!("run {program}"))?;
        Ok(Ran {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

/// The machine as the service commands see it.
pub struct Context<'a> {
    pub platform: Platform,
    pub home: PathBuf,
    /// This process's environment.
    pub env: BTreeMap<String, String>,
    pub uid: u32,
    pub user: String,
    /// The login shell (the account's, else `$SHELL`, else `/bin/sh`).
    pub shell: PathBuf,
    /// This binary, as the service will run it.
    pub exe: PathBuf,
    /// Where `/run` and `/proc` are (`/`, but for the tests).
    pub root: PathBuf,
    pub manager: &'a dyn Manager,
}

impl<'a> Context<'a> {
    fn from_process(manager: &'a dyn Manager) -> Result<Self> {
        let platform = if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            bail!("`hennery service` supports macOS (launchd) and Linux (systemd) only");
        };
        let env: BTreeMap<String, String> = std::env::vars().collect();
        // SAFETY: getuid(2) cannot fail.
        let uid = unsafe { libc::getuid() };
        let account = account(uid);
        let home = env
            .get("HOME")
            .map(PathBuf::from)
            .filter(|h| h.is_absolute())
            .or_else(|| account.as_ref().map(|a| a.home.clone()))
            .context("no home directory: set HOME")?;
        let user = env
            .get("USER")
            .cloned()
            .or_else(|| account.as_ref().map(|a| a.name.clone()))
            .unwrap_or_else(|| uid.to_string());
        let shell = account
            .as_ref()
            .map(|a| a.shell.clone())
            .filter(|s| s.is_absolute())
            .or_else(|| env.get("SHELL").map(PathBuf::from).filter(|s| s.is_absolute()))
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        Ok(Self {
            platform,
            home,
            uid,
            user,
            shell,
            exe: invoked_exe(&env)?,
            root: PathBuf::from("/"),
            manager,
            env,
        })
    }

    /// `$XDG_<name>_HOME` when set to an absolute path, else `home/fallback`.
    fn xdg(&self, name: &str, fallback: &str) -> PathBuf {
        self.env
            .get(&format!("XDG_{name}_HOME"))
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| self.home.join(fallback))
    }

    /// Distribution spec §8: the data directory by default.
    pub fn default_data_dir(&self) -> PathBuf {
        match self.platform {
            Platform::MacOs => self.home.join("Library/Application Support/hennery"),
            Platform::Linux => self.xdg("DATA", ".local/share").join("hennery"),
        }
    }

    /// Where `role`'s service file is.
    pub fn service_file(&self, role: Role) -> PathBuf {
        match self.platform {
            Platform::MacOs => self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{}.plist", role.label())),
            Platform::Linux => self.xdg("CONFIG", ".config").join("systemd/user").join(role.unit()),
        }
    }

    /// The systemd unit's environment file (distribution spec §8).
    fn env_file(&self) -> PathBuf {
        self.xdg("CONFIG", ".config").join("hennery/service.env")
    }

    /// Where launchd writes `role`'s output (distribution spec §8).
    fn log_file(&self, role: Role) -> PathBuf {
        self.home
            .join("Library/Logs/hennery")
            .join(format!("{}.log", role.name()))
    }

    /// The roles whose service file is there.
    fn installed(&self) -> Vec<Role> {
        Role::ALL
            .into_iter()
            .filter(|&role| self.service_file(role).exists())
            .collect()
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<Ran> {
        self.manager.run(program, args)
    }

    fn launchd_domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    fn launchd_target(&self, role: Role) -> String {
        format!("gui/{}/{}", self.uid, role.label())
    }
}

/// The account's entry in the user database.
struct Account {
    name: String,
    home: PathBuf,
    shell: PathBuf,
}

fn account(uid: u32) -> Option<Account> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: all-zero bytes are a valid `passwd`; getpwuid_r(3) fills it
    // with pointers into `buf`, which outlives every use below.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut found) };
    if rc != 0 || found.is_null() {
        return None;
    }
    // SAFETY: on success each field is a NUL-terminated string in `buf`.
    let text = |p: *const libc::c_char| unsafe { CStr::from_ptr(p) }.to_bytes().to_vec();
    Some(Account {
        name: String::from_utf8_lossy(&text(pwd.pw_name)).into_owned(),
        home: PathBuf::from(std::ffi::OsStr::from_bytes(&text(pwd.pw_dir))),
        shell: PathBuf::from(std::ffi::OsStr::from_bytes(&text(pwd.pw_shell))),
    })
}

/// This binary as it was started: the path on PATH (or given) that leads
/// to it, not where that resolves to. A package manager's link
/// (`/opt/homebrew/bin/hennery`, `~/.nix-profile/bin/hennery`) survives an
/// upgrade; the versioned file it points at does not.
fn invoked_exe(env: &BTreeMap<String, String>) -> Result<PathBuf> {
    let real = std::env::current_exe()?;
    let canonical = real.canonicalize().unwrap_or_else(|_| real.clone());
    let same = |candidate: &Path| candidate.canonicalize().is_ok_and(|c| c == canonical);
    if let Some(arg0) = std::env::args_os().next().map(PathBuf::from) {
        if arg0.components().count() > 1 {
            if let Ok(abs) = std::path::absolute(&arg0)
                && same(&abs)
            {
                return Ok(abs);
            }
        } else if let Some(path) = env.get("PATH") {
            for dir in std::env::split_paths(path).filter(|d| d.is_absolute()) {
                let candidate = dir.join(&arg0);
                if same(&candidate) {
                    return Ok(candidate);
                }
            }
        }
    }
    Ok(real)
}

/// Write `text` to `path` whole or not at all, with `mode`.
fn write_file(path: &Path, text: &str, mode: u32) -> Result<()> {
    let tmp = path.with_extension("hennery-tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .with_context(|| format!("write {}", tmp.display()))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
}

fn create_dir(dir: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(dir)
        .with_context(|| format!("create {}", dir.display()))
}

/// Why there is no systemd user manager to install into, if there is none.
fn no_systemd(cx: &Context) -> Option<String> {
    if cx.root.join("run/systemd/system").is_dir() {
        return None;
    }
    let wsl = cx.env.contains_key("WSL_DISTRO_NAME")
        || std::fs::read_to_string(cx.root.join("proc/sys/kernel/osrelease"))
            .is_ok_and(|release| release.to_ascii_lowercase().contains("microsoft"));
    Some(if wsl {
        "this WSL distribution does not run systemd: add `[boot]` and `systemd=true` to /etc/wsl.conf, run \
         `wsl --shutdown` from Windows, start the distribution again, and retry"
            .to_string()
    } else {
        "systemd is not running here (no /run/systemd/system), and `hennery service` installs systemd user units; \
         run `hennery up` or `hennery host run` under your own supervisor instead"
            .to_string()
    })
}

/// `hennery service install` (distribution spec §6).
pub fn install(
    cx: &Context,
    role: Role,
    data_dir: Option<&Path>,
    shell: Option<&Path>,
    out: &mut dyn Write,
) -> Result<()> {
    if let Some(other) = cx.installed().into_iter().find(|&r| r != role) {
        bail!(
            "the {other} service is installed ({}); one role per machine: run `hennery service uninstall --role {other}` first",
            cx.service_file(other).display()
        );
    }
    let data_dir = match data_dir {
        Some(dir) => std::path::absolute(dir)?,
        None => cx.default_data_dir(),
    };
    if role == Role::Host && !matches!(hennery_host::identity::Paired::load(&data_dir), Ok(Some(_))) {
        bail!(
            "{} holds no pairing: run `hennery host join <url> --data-dir {}` first",
            data_dir.display(),
            data_dir.display()
        );
    }
    if cx.platform == Platform::Linux
        && let Some(why) = no_systemd(cx)
    {
        bail!(why);
    }
    let argv = unit::command_line(role, &cx.exe, &data_dir)?;
    let shell = shell.unwrap_or(&cx.shell);
    let macos = cx.platform == Platform::MacOs;
    let env = path::shell_environment(&cx.env, shell, macos);
    let captured = path::login_environment(shell, &env, path::CAPTURE_TIMEOUT).with_context(|| {
        format!(
            "capture the PATH of the login shell {} (pick another with --shell)",
            shell.display()
        )
    })?;
    let service_path = path::service_path(&captured, &cx.home, macos)?;
    let file = cx.service_file(role);
    let restarted = match cx.platform {
        Platform::MacOs => install_launchd(cx, role, &argv, &service_path.path, &file)?,
        Platform::Linux => install_systemd(cx, role, &argv, &service_path.path, &file)?,
    };
    writeln!(out, "installed the {role} service: {}", file.display())?;
    writeln!(out, "  runs: {}", argv.join(" "))?;
    match cx.platform {
        Platform::MacOs => writeln!(out, "  PATH (in the plist): {}", service_path.path)?,
        Platform::Linux => writeln!(out, "  PATH (in {}): {}", cx.env_file().display(), service_path.path)?,
    }
    for note in &service_path.notes {
        writeln!(out, "  note: {note}")?;
    }
    if restarted {
        writeln!(
            out,
            "  it was running and has been restarted: its sessions are parked, and resumable"
        )?;
    }
    if macos {
        writeln!(out, "  output: {}", cx.log_file(role).display())?;
        writeln!(
            out,
            "  it runs while you are logged in to the Mac's screen (the login keychain holds the agents' credentials)"
        )?;
    } else {
        linger(cx, out)?;
    }
    Ok(())
}

/// How long `bootout` may take: the plist's `ExitTimeOut`, and a little.
const BOOTOUT_WAIT: Duration = Duration::from_secs(35);

fn loaded(cx: &Context, role: Role) -> Result<bool> {
    Ok(cx.run("launchctl", &["print", &cx.launchd_target(role)])?.ok)
}

/// Unload `role`'s agent if it is loaded, and wait until launchd has let it
/// go: `bootstrap` fails while the old job is still stopping. `true` if it
/// was loaded.
fn bootout(cx: &Context, role: Role) -> Result<bool> {
    if !loaded(cx, role)? {
        return Ok(false);
    }
    let target = cx.launchd_target(role);
    let ran = cx.run("launchctl", &["bootout", &target])?;
    let deadline = Instant::now() + BOOTOUT_WAIT;
    while loaded(cx, role)? {
        if Instant::now() >= deadline {
            bail!("launchctl bootout {target} did not finish: {}", ran.stderr.trim());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(true)
}

fn install_launchd(cx: &Context, role: Role, argv: &[String], path: &str, file: &Path) -> Result<bool> {
    // Private: the log is the service's whole output.
    let log = cx.log_file(role);
    create_dir(log.parent().expect("a log directory"), 0o700)?;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&log)
        .with_context(|| format!("create {}", log.display()))?;
    create_dir(file.parent().expect("LaunchAgents"), 0o755)?;
    let restarted = bootout(cx, role)?;
    write_file(file, &unit::plist(role, argv, path, &unit::plain(&log)?), 0o644)?;
    let plist = unit::plain(file)?;
    let ran = cx.run("launchctl", &["bootstrap", &cx.launchd_domain(), &plist])?;
    if !ran.ok {
        bail!(
            "launchctl bootstrap {} {plist} failed: {}. A launchd user agent loads only in a GUI login session \
             (log in on the Mac's screen, not only over SSH); then run `hennery service install` again",
            cx.launchd_domain(),
            ran.stderr.trim()
        );
    }
    Ok(restarted)
}

/// `systemctl --user …`, failing with its error output.
fn systemctl(cx: &Context, args: &[&str]) -> Result<Ran> {
    let mut all = vec!["--user"];
    all.extend_from_slice(args);
    let ran = cx.run("systemctl", &all)?;
    if !ran.ok {
        bail!("systemctl {} failed: {}", all.join(" "), ran.stderr.trim());
    }
    Ok(ran)
}

fn install_systemd(cx: &Context, role: Role, argv: &[String], path: &str, file: &Path) -> Result<bool> {
    let env_file = cx.env_file();
    create_dir(env_file.parent().expect("a config directory"), 0o700)?;
    write_file(&env_file, &unit::env_file(path), 0o600)?;
    create_dir(file.parent().expect("systemd/user"), 0o755)?;
    let restarted = cx
        .run("systemctl", &["--user", "is-active", "--quiet", role.unit()])?
        .ok;
    write_file(file, &unit::systemd_unit(role, argv, &unit::plain(&env_file)?)?, 0o644)?;
    systemctl(cx, &["daemon-reload"])?;
    systemctl(cx, &["enable", role.unit()])?;
    // A unit that hit its start limit refuses a plain restart.
    let _ = cx.run("systemctl", &["--user", "reset-failed", role.unit()]);
    systemctl(cx, &["restart", role.unit()])?;
    Ok(restarted)
}

/// Distribution spec §6.3: without linger the service stops at logout and
/// does not start at boot. Said, never changed: enabling it may need
/// privileges.
fn linger(cx: &Context, out: &mut dyn Write) -> Result<()> {
    let uid = cx.uid.to_string();
    let on = cx
        .run("loginctl", &["show-user", &uid, "-p", "Linger"])
        .is_ok_and(|ran| ran.ok && ran.stdout.trim() == "Linger=yes");
    if !on {
        writeln!(
            out,
            "  linger is off: the service stops when you log out and does not start at boot. To keep it running, run:\n    loginctl enable-linger {}",
            cx.user
        )?;
    }
    Ok(())
}

/// `hennery service uninstall`.
pub fn uninstall(cx: &Context, role: Option<Role>, out: &mut dyn Write) -> Result<()> {
    let roles = match role {
        Some(role) if !cx.service_file(role).exists() => {
            writeln!(out, "the {role} service is not installed")?;
            return Ok(());
        }
        Some(role) => vec![role],
        None => cx.installed(),
    };
    if roles.is_empty() {
        writeln!(out, "no hennery service is installed")?;
        return Ok(());
    }
    for role in roles {
        let file = cx.service_file(role);
        let data = read_command_line(cx, role).and_then(|argv| data_dir_of(&argv));
        match cx.platform {
            Platform::MacOs => {
                bootout(cx, role)?;
                std::fs::remove_file(&file).with_context(|| format!("remove {}", file.display()))?;
            }
            Platform::Linux => {
                if let Err(err) = systemctl(cx, &["disable", "--now", role.unit()]) {
                    writeln!(out, "  warning: {err:#}")?;
                }
                std::fs::remove_file(&file).with_context(|| format!("remove {}", file.display()))?;
                let _ = std::fs::remove_file(cx.env_file());
                systemctl(cx, &["daemon-reload"])?;
            }
        }
        writeln!(out, "removed the {role} service: {}", file.display())?;
        if let Some(data) = data {
            writeln!(out, "  its data directory is kept: {}", data.display())?;
        }
        if cx.platform == Platform::MacOs {
            writeln!(out, "  its log is kept: {}", cx.log_file(role).display())?;
        }
    }
    Ok(())
}

fn read_command_line(cx: &Context, role: Role) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(cx.service_file(role)).ok()?;
    match cx.platform {
        Platform::MacOs => unit::plist_command_line(&text),
        Platform::Linux => unit::systemd_command_line(&text),
    }
}

fn data_dir_of(argv: &[String]) -> Option<PathBuf> {
    let at = argv.iter().position(|a| a == "--data-dir")?;
    argv.get(at + 1).map(PathBuf::from)
}

/// `hennery service status`: exit 0 when the service is installed and
/// running, its binary is this one, and no child of `up` was given up on or
/// revoked; 1 otherwise.
pub fn status(cx: &Context, role: Option<Role>, out: &mut dyn Write) -> Result<ExitCode> {
    let roles = match role {
        Some(role) => vec![role],
        None => cx.installed(),
    };
    if roles.is_empty() {
        writeln!(out, "no hennery service is installed")?;
        return Ok(ExitCode::FAILURE);
    }
    if roles.len() > 1 {
        writeln!(
            out,
            "warning: more than one role is installed; keep one (`hennery service uninstall --role …`)"
        )?;
    }
    let mut healthy = roles.len() == 1;
    for role in roles {
        healthy &= status_of(cx, role, out)?;
    }
    Ok(if healthy { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn status_of(cx: &Context, role: Role, out: &mut dyn Write) -> Result<bool> {
    let file = cx.service_file(role);
    if !file.exists() {
        writeln!(out, "the {role} service is not installed ({})", file.display())?;
        return Ok(false);
    }
    writeln!(out, "the {role} service: {}", file.display())?;
    let mut healthy = true;
    let argv = read_command_line(cx, role).unwrap_or_default();
    match argv.first().map(PathBuf::from) {
        None => {
            writeln!(
                out,
                "  runs: unreadable; run `hennery service install --role {role}` again"
            )?;
            healthy = false;
        }
        Some(exe) => {
            writeln!(out, "  runs: {}", argv.join(" "))?;
            let canonical = |p: &Path| p.canonicalize().ok();
            if !exe.exists() {
                writeln!(
                    out,
                    "  its binary is missing; run `hennery service install --role {role}` again"
                )?;
                healthy = false;
            } else if canonical(&exe) != canonical(&cx.exe) {
                writeln!(
                    out,
                    "  its binary is not this one ({}); run `hennery service install --role {role}` with the one to use",
                    cx.exe.display()
                )?;
            }
        }
    }
    let running = match cx.platform {
        Platform::MacOs => {
            let ran = cx.run("launchctl", &["print", &cx.launchd_target(role)])?;
            if !ran.ok {
                writeln!(out, "  launchd: not loaded (it loads at your next GUI login)")?;
                false
            } else {
                let field = |name: &str| {
                    ran.stdout
                        .lines()
                        .find_map(|l| l.trim().strip_prefix(name)?.trim().strip_prefix('=').map(str::trim))
                };
                let state = field("state").unwrap_or("unknown");
                match field("pid") {
                    Some(pid) => writeln!(out, "  launchd: {state}, pid {pid}")?,
                    None => writeln!(out, "  launchd: {state}")?,
                }
                state == "running"
            }
        }
        Platform::Linux => {
            let active = cx.run("systemctl", &["--user", "is-active", role.unit()])?;
            let enabled = cx.run("systemctl", &["--user", "is-enabled", role.unit()])?;
            writeln!(out, "  systemd: {}, {}", active.stdout.trim(), enabled.stdout.trim())?;
            linger(cx, out)?;
            active.stdout.trim() == "active"
        }
    };
    healthy &= running;
    if role == Role::Up
        && let Some(data) = data_dir_of(&argv)
    {
        healthy &= children(&data, out)?;
    }
    Ok(healthy)
}

/// `up`'s report of its children (`supervisor.json`): `false` when one was
/// given up on or revoked.
fn children(data: &Path, out: &mut dyn Write) -> Result<bool> {
    let Some(state) = supervisor::read_state(data)? else {
        writeln!(out, "  children: no report yet in {}", data.display())?;
        return Ok(true);
    };
    let mut healthy = true;
    for (name, child) in [("collector", &state.collector), ("host", &state.host)] {
        let last = child
            .last_exit
            .as_deref()
            .map(|e| format!(", last exit: {e}"))
            .unwrap_or_default();
        let what = match child.state {
            supervisor::ChildState::Running => "running".to_string(),
            supervisor::ChildState::Restarting => "crashed; starting again".to_string(),
            supervisor::ChildState::Stopped => "stopped".to_string(),
            supervisor::ChildState::Revoked => {
                healthy = false;
                "revoked by the collector; pair it again (see `hennery up`'s log)".to_string()
            }
            supervisor::ChildState::GaveUp => {
                healthy = false;
                format!(
                    "given up on after {} crashes; restart the service once the cause is fixed",
                    child.crashes_in_window
                )
            }
        };
        writeln!(out, "  {name}: {what} ({} restarts{last})", child.restarts)?;
    }
    writeln!(out, "  (reported by pid {})", state.pid)?;
    Ok(healthy)
}

#[cfg(test)]
mod tests;
