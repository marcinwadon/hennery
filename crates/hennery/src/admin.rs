//! `hennery admin …` (kernel spec §4.2): recovery commands, sent to the
//! running collector over its admin socket. Those that change state or
//! hand out a credential (a password reset, a `public_url` reset, a
//! pairing code) ask for confirmation, and so need a terminal on standard
//! input: without one they refuse and send nothing. That stops accidents
//! and scripts, not a process of the collector's user that speaks the
//! socket's protocol itself (kernel spec §10).

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use hennery_kernel::admin::{ADMIN_SOCKET, AdminRequest, AdminResponse, CLIENT_TIMEOUT};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Args)]
pub struct AdminArgs {
    /// The collector's data directory, or `hennery up`'s (whose collector
    /// keeps its own in `collector/`).
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    /// How long to wait for the collector, in milliseconds: for tests.
    #[arg(long, env = "HENNERY_ADMIN_TIMEOUT_MS", hide = true, value_parser = clap::value_parser!(u64).range(1..=600_000))]
    timeout_ms: Option<u64>,
    #[command(subcommand)]
    command: AdminCommand,
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Print the one-time setup link: the live one, or a fresh one once it
    /// has expired.
    SetupUrl,
    /// Set a new owner password, typed on the terminal. Signs out every
    /// session and removes every passkey.
    ResetPassword,
    /// List the paired hosts.
    Hosts,
    /// Mint a pairing code for `hennery host join`, valid for ten minutes.
    PairingCode,
    /// Set where browsers reach hennery, after moving it. Signs out every
    /// session; passkeys stop working if the host name changes, and are
    /// removed.
    ResetPublicUrl {
        /// The new public URL, e.g. https://hennery.example.
        public_url: String,
    },
}

/// The socket in `dir`, or in `dir/collector` when there is no socket in
/// `dir` and that directory exists (`dir` is then `hennery up`'s data
/// directory). A collector there whose socket path is too long has none,
/// and the directory alone says it is `up`'s: the path checked for its
/// length is then the one that collector would have used.
fn socket_path(dir: &Path) -> PathBuf {
    let own = dir.join(ADMIN_SOCKET);
    let ups = dir.join("collector");
    if !own.exists() && ups.is_dir() {
        ups.join(ADMIN_SOCKET)
    } else {
        own
    }
}

pub async fn run(args: AdminArgs) -> Result<()> {
    let socket = socket_path(&args.data_dir);
    let timeout = args.timeout_ms.map_or(CLIENT_TIMEOUT, Duration::from_millis);
    // What prompts checks first that a collector answers there, so nothing
    // is typed into a dead end (3b-ii's deferred item); the terminal is
    // checked before that, sending nothing.
    let prompts = !matches!(args.command, AdminCommand::SetupUrl | AdminCommand::Hosts);
    if prompts {
        require_terminal(args.command.name())?;
        hennery_kernel::admin::probe(&socket, timeout).await?;
    }
    let request = match args.command {
        AdminCommand::SetupUrl => AdminRequest::SetupUrl,
        AdminCommand::Hosts => AdminRequest::ListHosts,
        AdminCommand::PairingCode => {
            confirm(
                "pairing-code",
                "A pairing code pairs any machine it is given to, for ten minutes.",
            )?;
            AdminRequest::MintPairingCode
        }
        AdminCommand::ResetPassword => {
            // Said before the password is asked for (plan 3c review, A1).
            eprintln!(
                "This removes every passkey; register them again after signing in. It also signs out every session."
            );
            let password = read_secret("New password: ")?;
            if read_secret("The same again: ")? != password {
                bail!("the two passwords differ; nothing changed");
            }
            confirm(
                "reset-password",
                "This removes every passkey and signs out every session.",
            )?;
            AdminRequest::ResetPassword { password }
        }
        AdminCommand::ResetPublicUrl { public_url } => {
            confirm(
                "reset-public-url",
                &format!(
                    "Browsers will have to reach hennery at {public_url}. Passkeys stop working if the host name \
                     changes, and are then removed. This signs out every session."
                ),
            )?;
            AdminRequest::ResetPublicUrl { public_url }
        }
    };
    match hennery_kernel::admin::request_within(&socket, &request, timeout).await? {
        AdminResponse::SetupUrl { url } => println!("{url}"),
        AdminResponse::AlreadySetUp => bail!("hennery is set up already: there is no setup link"),
        AdminResponse::NotSetUp => {
            bail!("hennery is not set up yet: `hennery admin setup-url` prints the setup link")
        }
        AdminResponse::PasswordReset {
            sessions_ended,
            passkeys_removed,
        } => println!(
            "The password is reset; {sessions_ended} session(s) signed out, {passkeys_removed} passkey(s) removed."
        ),
        AdminResponse::PublicUrlReset {
            public_url,
            sessions_ended,
            passkeys_removed,
        } => println!(
            "public_url is now {public_url}; {sessions_ended} session(s) signed out, {passkeys_removed} passkey(s) removed."
        ),
        AdminResponse::Hosts { hosts } => {
            for host in hosts {
                let state = if host.revoked_at.is_some() { "revoked" } else { "paired" };
                println!("{}\t{}\t{}\t{state}", host.id, host.name, host.platform);
            }
        }
        AdminResponse::PairingCode { code, .. } => {
            println!("{code}");
            eprintln!("Valid for ten minutes: `hennery host join <public_url>` on the machine, then type it.");
        }
        AdminResponse::Refused { message } => bail!("refused: {message}"),
        AdminResponse::Failed { message } => bail!("the collector failed: {message}"),
    }
    Ok(())
}

impl AdminCommand {
    /// Its name on the command line.
    fn name(&self) -> &'static str {
        match self {
            Self::SetupUrl => "setup-url",
            Self::ResetPassword => "reset-password",
            Self::Hosts => "hosts",
            Self::PairingCode => "pairing-code",
            Self::ResetPublicUrl { .. } => "reset-public-url",
        }
    }
}

fn require_terminal(command: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("`hennery admin {command}` asks for confirmation on a terminal; run it from one (nothing was sent)");
    }
    Ok(())
}

/// Ask on the terminal, and go on only when the answer is `yes`.
fn confirm(command: &str, what: &str) -> Result<()> {
    require_terminal(command)?;
    eprint!("{what}\nType yes to go on: ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("read the answer")?;
    if answer.trim() != "yes" {
        bail!("not confirmed; nothing changed");
    }
    Ok(())
}

/// One line typed on the terminal with its echo off, so the password is not
/// shown. The echo goes off before the prompt is shown, so nothing typed
/// in answer to it is echoed, and comes back however this returns.
fn read_secret(prompt: &str) -> Result<String> {
    let _echo = EchoOff::new()?;
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read the password")?;
    eprintln!();
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// The terminal's settings with echo off, restored on drop.
struct EchoOff(libc::termios);

impl EchoOff {
    fn new() -> Result<Self> {
        // SAFETY: tcgetattr(3) into local storage; all-zero bytes are a
        // valid `termios`.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut saved) } != 0 {
            return Err(std::io::Error::last_os_error()).context("read the terminal's settings");
        }
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        // TCSANOW, not TCSAFLUSH: what was typed ahead is kept.
        // SAFETY: tcsetattr(3) with settings just read and changed.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet) } != 0 {
            return Err(std::io::Error::last_os_error()).context("turn the terminal's echo off");
        }
        Ok(Self(saved))
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: tcsetattr(3) with the settings read in `new`.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
        }
    }
}
