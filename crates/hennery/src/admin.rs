//! `hennery admin …` (kernel spec §4.2): recovery commands, sent to the
//! running collector over its admin socket. Those that change state or
//! hand out a credential (a password reset, a `public_url` reset, a
//! pairing code) ask for confirmation, and so need a terminal on standard
//! input: without one they refuse and send nothing. That stops accidents
//! and scripts, not a process of the collector's user that speaks the
//! socket's protocol itself (kernel spec §10).

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use hennery_kernel::admin::{ADMIN_SOCKET, AdminRequest, AdminResponse};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct AdminArgs {
    /// The collector's data directory, or `hennery up`'s (whose collector
    /// keeps its own in `collector/`).
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: AdminCommand,
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Print the one-time setup link: the live one, or a fresh one once it
    /// has expired.
    SetupUrl,
    /// Set a new owner password, typed on the terminal. Signs out every
    /// session.
    ResetPassword,
    /// List the paired hosts.
    Hosts,
    /// Mint a pairing code for `hennery host join`, valid for ten minutes.
    PairingCode,
    /// Set where browsers reach hennery, after moving it. Signs out every
    /// session.
    ResetPublicUrl {
        /// The new public URL, e.g. https://hennery.example.
        public_url: String,
    },
}

/// The socket in `dir`, or in `dir/collector` when only that one exists
/// (`dir` is then `hennery up`'s data directory).
fn socket_path(dir: &Path) -> PathBuf {
    let own = dir.join(ADMIN_SOCKET);
    let ups = dir.join("collector").join(ADMIN_SOCKET);
    if !own.exists() && ups.exists() { ups } else { own }
}

pub async fn run(args: AdminArgs) -> Result<()> {
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
            require_terminal("reset-password")?;
            let password = read_secret("New password: ")?;
            if read_secret("The same again: ")? != password {
                bail!("the two passwords differ; nothing changed");
            }
            confirm("reset-password", "This signs out every session.")?;
            AdminRequest::ResetPassword { password }
        }
        AdminCommand::ResetPublicUrl { public_url } => {
            confirm(
                "reset-public-url",
                &format!("Browsers will have to reach hennery at {public_url}. This signs out every session."),
            )?;
            AdminRequest::ResetPublicUrl { public_url }
        }
    };
    let socket = socket_path(&args.data_dir);
    match hennery_kernel::admin::request(&socket, &request).await? {
        AdminResponse::SetupUrl { url } => println!("{url}"),
        AdminResponse::AlreadySetUp => bail!("hennery is set up already: there is no setup link"),
        AdminResponse::NotSetUp => {
            bail!("hennery is not set up yet: `hennery admin setup-url` prints the setup link")
        }
        AdminResponse::PasswordReset { sessions_ended } => {
            println!("The password is reset; {sessions_ended} session(s) signed out.")
        }
        AdminResponse::PublicUrlReset {
            public_url,
            sessions_ended,
        } => println!("public_url is now {public_url}; {sessions_ended} session(s) signed out."),
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
