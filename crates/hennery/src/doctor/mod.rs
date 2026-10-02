//! `hennery doctor` (distribution spec §7): what stands between this machine
//! and a working hennery, one check at a time, each `ok`, `warn` or `fail`
//! with a one-sentence fix. Doctor only reads (decision 2): it never
//! repairs, pairs, logs in or installs; it opens no database and takes no
//! lock a host, an install or `up` takes. The one program of the user's it
//! runs is the login shell, to compare its PATH with the service's (check
//! 5), as `service install` does. Everything it reads of the machine comes
//! through the service commands' `Context` and a `Runner`, which the tests
//! replace.

mod dirs;
mod disk;
mod env;
mod platform;
mod process;
mod runtime;
mod service;

#[cfg(test)]
mod tests;

use crate::service::{Context, Ran, System};
use anyhow::Result;
use clap::Args;
use dirs::Dirs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[derive(Args)]
pub struct DoctorArgs {
    /// The data directory to examine: `hennery up`'s, a host's or a
    /// collector's. Else `HENNERY_HOST_DATA_DIR`, else the installed
    /// service's, else the platform's default
    /// (`~/Library/Application Support/hennery` on macOS,
    /// `$XDG_DATA_HOME/hennery` on Linux).
    #[arg(long, env = "HENNERY_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

/// A check's verdict (distribution spec §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

/// One check's line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Its number in distribution spec §7's table.
    pub number: u8,
    pub name: &'static str,
    pub status: Status,
    /// What was found, in one sentence.
    pub summary: String,
    /// What to do, in one sentence: empty when `status` is `Ok`.
    pub fix: String,
}

/// What one check came to: a verdict, or nothing to look at here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    Checked(Check),
    /// Not run, and why (e.g. "no host data directory"): never counted as ok.
    NotRun {
        number: u8,
        why: &'static str,
    },
}

/// The parts of one check, folded into its line: the worst status wins, the
/// summaries are joined, and so are the fixes of the parts at that status.
#[derive(Debug, Default)]
pub struct Verdict {
    parts: Vec<(Status, String, String)>,
}

impl Verdict {
    pub fn ok(&mut self, summary: impl Into<String>) {
        self.parts.push((Status::Ok, summary.into(), String::new()));
    }

    pub fn warn(&mut self, summary: impl Into<String>, fix: impl Into<String>) {
        self.parts.push((Status::Warn, summary.into(), fix.into()));
    }

    pub fn fail(&mut self, summary: impl Into<String>, fix: impl Into<String>) {
        self.parts.push((Status::Fail, summary.into(), fix.into()));
    }

    /// The check's line. A part with no summary adds none.
    pub fn check(self, number: u8, name: &'static str) -> Check {
        let status = self.parts.iter().map(|p| p.0).max().unwrap_or(Status::Ok);
        let summary = self
            .parts
            .iter()
            .map(|p| p.1.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        let mut fixes: Vec<&str> = Vec::new();
        for (s, _, fix) in &self.parts {
            if *s == status && !fix.is_empty() && !fixes.contains(&fix.as_str()) {
                fixes.push(fix);
            }
        }
        Check {
            number,
            name,
            status,
            summary,
            fix: fixes.join(" "),
        }
    }
}

/// Runs a program for a check, bounded (decision 5): its output, or `None`
/// when it could not be run or did not finish in time.
pub type Runner<'a> = &'a dyn Fn(&Path, &[&str]) -> Option<Ran>;

/// What every check looks at.
pub struct Doctor<'a> {
    pub cx: &'a Context<'a>,
    pub dirs: Dirs,
    pub run: Runner<'a>,
}

impl Doctor<'_> {
    /// The command line of the service that runs the host examined gives
    /// its agents (`--agent`, e.g. Nix-provided adapters): that host runs no
    /// managed set, and needs neither glibc nor nix-ld for one (decision 4).
    /// A service of another directory says nothing of this one.
    pub fn agents_given(&self) -> bool {
        use crate::service::unit::Role;
        let Some(host) = self.dirs.host.as_ref().and_then(|h| h.canonicalize().ok()) else {
            return false;
        };
        self.cx.installed().into_iter().any(|role| {
            let Some(argv) = crate::service::read_command_line(self.cx, role) else {
                return false;
            };
            let served = match (role, crate::service::data_dir_of(&argv)) {
                (Role::Up, Some(data)) => data.join("host"),
                (Role::Host, Some(data)) => data,
                _ => return false,
            };
            served.canonicalize().ok() == Some(host.clone()) && argv.iter().any(|a| a == "--agent")
        })
    }
}

/// Every check this binary has, in the spec's order.
pub fn checks(doctor: &Doctor) -> Vec<Finding> {
    vec![
        runtime::binary_and_set(doctor),
        platform::platform(doctor),
        service::service_path(doctor),
        env::environment(doctor),
        disk::disk(doctor),
        service::service(doctor),
        env::hennery_on_path(doctor),
        runtime::adapter_set(doctor),
        service::host_directory(doctor),
        runtime::cli_overrides(doctor),
    ]
}

/// How long a program run for a check may take.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(5);

/// The most output read from a program run for a check.
const MAX_OUTPUT: u64 = 4096;

/// Run `program` for a check: no standard input, an environment of the base
/// system's PATH alone (nothing of the shell doctor runs in), at most
/// `MAX_OUTPUT` bytes of each output read, killed after `RUN_TIMEOUT`. What
/// it prints is parsed by the check, never printed or logged. The output is
/// read once the program exits: one that fills a pipe is killed at the
/// timeout and gives `None`, and one that leaves a child holding its pipes
/// would hold the read. The programs run (glibc's loader, `getconf`) do
/// neither.
pub fn run_bounded(program: &Path, args: &[&str]) -> Option<Ran> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + RUN_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut stdout = Vec::new();
    child.stdout.take()?.take(MAX_OUTPUT).read_to_end(&mut stdout).ok()?;
    let mut stderr = Vec::new();
    child.stderr.take()?.take(MAX_OUTPUT).read_to_end(&mut stderr).ok()?;
    Some(Ran {
        ok: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// `hennery doctor`.
pub fn run(args: DoctorArgs) -> Result<ExitCode> {
    let system = System;
    let cx = Context::from_process(&system)?;
    let dirs = Dirs::discover(&cx, args.data_dir.as_deref())?;
    let doctor = Doctor {
        cx: &cx,
        dirs,
        run: &run_bounded,
    };
    let findings = checks(&doctor);
    render(&doctor.dirs, &findings, &mut std::io::stdout())?;
    Ok(exit_code(&findings))
}

/// 1 when a check failed; warnings alone exit 0.
pub fn exit_code(findings: &[Finding]) -> ExitCode {
    let failed = findings
        .iter()
        .any(|f| matches!(f, Finding::Checked(c) if c.status == Status::Fail));
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// `text` with every control character escaped, and every character that
/// reorders text (the bidirectional marks, embeddings, overrides and
/// isolates), so no name read from the machine (a path, a file name) can
/// forge or hide a line of the report.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            let reorders = matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
            if c.is_control() || reorders {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// The report: which directory, then one line per check (and its fix),
/// then the checks that had nothing to look at.
pub fn render(dirs: &Dirs, findings: &[Finding], out: &mut dyn Write) -> Result<()> {
    writeln!(
        out,
        "hennery doctor: {} ({})",
        printable(&dirs.root.display().to_string()),
        dirs.from.describe()
    )?;
    writeln!(out, "  {}", printable(&dirs.describe()))?;
    for note in &dirs.notes {
        writeln!(out, "  note: {}", printable(note))?;
    }
    let mut not_run: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for finding in findings {
        match finding {
            Finding::Checked(check) => {
                writeln!(
                    out,
                    "{:<4} {:>2} {}: {}",
                    check.status.label(),
                    check.number,
                    check.name,
                    printable(&check.summary)
                )?;
                if check.status != Status::Ok {
                    writeln!(out, "        fix: {}", printable(&check.fix))?;
                }
            }
            Finding::NotRun { number, why } => match not_run.iter_mut().find(|(w, _)| w == why) {
                Some((_, numbers)) => numbers.push(*number),
                None => not_run.push((why, vec![*number])),
            },
        }
    }
    if !not_run.is_empty() {
        let groups: Vec<String> = not_run
            .iter()
            .map(|(why, numbers)| {
                let numbers: Vec<String> = numbers.iter().map(u8::to_string).collect();
                format!("{} ({why})", numbers.join(", "))
            })
            .collect();
        writeln!(out, "not run here: {}", groups.join("; "))?;
    }
    Ok(())
}
