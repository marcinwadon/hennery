//! The service files `hennery service install` writes (distribution spec
//! §6.2, §6.3): a launchd user agent on macOS, a systemd user unit and its
//! environment file on Linux. Both are rendered on every platform, so each
//! is tested everywhere; and read back, so `service status` knows what an
//! installed one runs.

use anyhow::{Result, bail};
use std::path::Path;

/// What the service runs (distribution spec §6): `up` on a single machine,
/// `host` on additional machines, `collector` for a dedicated collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Role {
    Up,
    Host,
    Collector,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Up, Role::Host, Role::Collector];

    pub fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Host => "host",
            Self::Collector => "collector",
        }
    }

    /// The launchd label, which is also the plist's file name.
    pub fn label(self) -> String {
        format!("dev.hennery.{}", self.name())
    }

    /// The systemd unit's name.
    pub fn unit(self) -> &'static str {
        match self {
            Self::Up => "hennery.service",
            Self::Host => "hennery-host.service",
            Self::Collector => "hennery-collector.service",
        }
    }

    /// The command line, after the binary itself.
    fn subcommand(self) -> &'static [&'static str] {
        match self {
            Self::Up => &["up"],
            Self::Host => &["host", "run"],
            Self::Collector => &["collector"],
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The service's command line: the binary's absolute path, the role's
/// subcommand and its data directory. Refused when a path is not valid
/// UTF-8 or holds a control character, which no service file can carry
/// safely (XML 1.0 has no way to write most of them at all).
pub fn command_line(role: Role, exe: &Path, data_dir: &Path) -> Result<Vec<String>> {
    let mut argv = vec![plain(exe)?];
    argv.extend(role.subcommand().iter().map(|s| s.to_string()));
    argv.push("--data-dir".into());
    argv.push(plain(data_dir)?);
    Ok(argv)
}

/// `path` as text a service file can hold.
pub fn plain(path: &Path) -> Result<String> {
    let Some(text) = path.to_str() else {
        bail!("{} is not valid UTF-8", path.display());
    };
    if !path.is_absolute() {
        bail!("{text} is not an absolute path");
    }
    if text.chars().any(char::is_control) {
        bail!("{text:?} holds a control character");
    }
    Ok(text.to_string())
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn unxml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The launchd user agent (distribution spec §6.2): started at login and
/// again whenever it fails, only in a GUI login session (`Aqua`, decision
/// 4: when the login keychain is unlocked), with the captured PATH, and its
/// output in `log`. `ExitTimeOut` gives `up` the time it takes to stop both
/// children (10 s each) before launchd kills it.
pub fn plist(role: Role, argv: &[String], path: &str, log: &str) -> String {
    let args: String = argv
        .iter()
        .map(|a| format!("\t\t<string>{}</string>\n", xml(a)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
{args}	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>{path}</string>
		<key>HENNERY_SERVICE</key>
		<string>launchd</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>ExitTimeOut</key>
	<integer>30</integer>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#,
        label = xml(&role.label()),
        path = xml(path),
        log = xml(log),
    )
}

/// The `ProgramArguments` of a plist `plist` wrote.
pub fn plist_command_line(text: &str) -> Option<Vec<String>> {
    let (_, rest) = text.split_once("<key>ProgramArguments</key>")?;
    let (array, _) = rest.split_once("</array>")?;
    Some(
        array
            .split("<string>")
            .skip(1)
            .filter_map(|s| s.split_once("</string>").map(|(arg, _)| unxml(arg)))
            .collect(),
    )
}

/// One argument of a systemd command line: double-quoted, with `\` and `"`
/// escaped, and `%` and `$` doubled so neither specifiers nor variables are
/// expanded in it.
fn systemd_word(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{escaped}\"")
}

/// The systemd user unit (distribution spec §6.3). `KillMode=mixed` lets
/// `up` or the host stop what they started (the host parks its sessions and
/// stops its adapters) before systemd kills what is left. The environment
/// file is required: a missing one would fall back to the user manager's
/// PATH, the failure D-3 describes. `EnvironmentFile=` takes its path as it
/// stands, but for `%` specifiers, so a path with `\` or `"` is refused.
pub fn systemd_unit(role: Role, argv: &[String], env_file: &str) -> Result<String> {
    if env_file.contains(['\\', '"']) {
        bail!("{env_file} holds a backslash or a double quote, which an EnvironmentFile= path cannot");
    }
    let exec: Vec<String> = argv.iter().map(|a| systemd_word(a)).collect();
    Ok(format!(
        "[Unit]
Description=hennery {role}
StartLimitIntervalSec=300
StartLimitBurst=10

[Service]
Type=exec
ExecStart={exec}
EnvironmentFile={env_file}
Environment=HENNERY_SERVICE=systemd
Restart=on-failure
RestartSec=3
KillMode=mixed
TimeoutStopSec=30

[Install]
WantedBy=default.target
",
        exec = exec.join(" "),
        env_file = env_file.replace('%', "%%"),
    ))
}

/// The `ExecStart` of a unit `systemd_unit` wrote.
pub fn systemd_command_line(text: &str) -> Option<Vec<String>> {
    let line = text.lines().find_map(|l| l.trim_end().strip_prefix("ExecStart="))?;
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if_eq(&' ').is_some() {}
        if chars.next()? != '"' {
            return None;
        }
        let mut word = String::new();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => word.push(chars.next()?),
                c @ ('%' | '$') => {
                    if chars.next()? != c {
                        return None;
                    }
                    word.push(c);
                }
                c => word.push(c),
            }
        }
        words.push(word);
        if chars.peek().is_none() {
            return Some(words);
        }
    }
}

/// The systemd environment file: the captured PATH, double-quoted, with
/// `\`, `"`, `$` and `` ` `` escaped.
pub fn env_file(path: &str) -> String {
    let mut escaped = String::new();
    for c in path.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    format!("# Written by `hennery service install`: the login shell's PATH.\nPATH=\"{escaped}\"\n")
}

/// The `PATH` of a plist `plist` wrote: that one value of its
/// `EnvironmentVariables`. Nothing else in them is read: a user may have
/// added a secret there by hand.
pub fn plist_path(text: &str) -> Option<String> {
    let (_, rest) = text.split_once("<key>EnvironmentVariables</key>")?;
    let (dict, _) = rest.split_once("</dict>")?;
    let (_, after) = dict.split_once("<key>PATH</key>")?;
    let (between, value) = after.split_once("<string>")?;
    if !between.trim().is_empty() {
        return None;
    }
    let (value, _) = value.split_once("</string>")?;
    Some(unxml(value))
}

/// The `PATH` of an environment file `env_file` wrote: that line's value,
/// unquoted and unescaped; the last such line, as systemd takes the last.
/// No other line is read, as with the plist.
pub fn env_file_path(text: &str) -> Option<String> {
    let line = text.lines().rev().find_map(|l| l.strip_prefix("PATH="))?;
    let quoted = line.trim_end().strip_prefix('"')?.strip_suffix('"')?;
    let mut path = String::new();
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            path.push(chars.next()?);
        } else {
            path.push(c);
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A data directory with every character the files must escape.
    fn awkward() -> PathBuf {
        PathBuf::from("/tmp/a dir/with %h $HOME \"quotes\" & <tags> 'and' \\back")
    }

    #[test]
    fn each_role_runs_its_own_subcommand_with_its_data_dir() {
        let exe = Path::new("/opt/bin/hennery");
        let data = Path::new("/var/data");
        assert_eq!(
            command_line(Role::Up, exe, data).unwrap(),
            ["/opt/bin/hennery", "up", "--data-dir", "/var/data"]
        );
        assert_eq!(
            command_line(Role::Host, exe, data).unwrap(),
            ["/opt/bin/hennery", "host", "run", "--data-dir", "/var/data"]
        );
        assert_eq!(
            command_line(Role::Collector, exe, data).unwrap(),
            ["/opt/bin/hennery", "collector", "--data-dir", "/var/data"]
        );
        assert_eq!(Role::Up.unit(), "hennery.service");
        assert_eq!(Role::Host.label(), "dev.hennery.host");
    }

    #[test]
    fn paths_no_service_file_can_carry_are_refused() {
        let exe = Path::new("/opt/bin/hennery");
        for bad in ["/tmp/new\nline", "/tmp/tab\there", "relative/dir"] {
            assert!(command_line(Role::Up, exe, Path::new(bad)).is_err(), "{bad:?}");
        }
        use std::os::unix::ffi::OsStrExt;
        let invalid = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
        assert!(command_line(Role::Up, exe, invalid).is_err());
    }

    #[test]
    fn the_plist_holds_what_section_6_2_names() {
        let argv = command_line(Role::Up, Path::new("/opt/bin/hennery"), &awkward()).unwrap();
        let text = plist(
            Role::Up,
            &argv,
            "/opt/x & y/bin:/usr/bin",
            "/Users/me/Library/Logs/hennery/up.log",
        );
        for wanted in [
            "<string>dev.hennery.up</string>",
            "<key>RunAtLoad</key>\n\t<true/>",
            "<key>SuccessfulExit</key>\n\t\t<false/>",
            "<key>ThrottleInterval</key>\n\t<integer>10</integer>",
            "<key>ExitTimeOut</key>\n\t<integer>30</integer>",
            "<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>",
            "<string>/opt/x &amp; y/bin:/usr/bin</string>",
            "<key>HENNERY_SERVICE</key>\n\t\t<string>launchd</string>",
            "<key>StandardErrorPath</key>\n\t<string>/Users/me/Library/Logs/hennery/up.log</string>",
        ] {
            assert!(text.contains(wanted), "{wanted:?} not in:\n{text}");
        }
        assert!(!text.contains("CLAUDE"), "{text}");
    }

    #[test]
    fn the_unit_holds_what_section_6_3_names() {
        let exe = Path::new("/home/me/.local/bin/hen nery");
        let argv = command_line(Role::Host, exe, &awkward()).unwrap();
        let text = systemd_unit(Role::Host, &argv, "/home/me/.config/hennery 100%/service.env").unwrap();
        for wanted in [
            "Description=hennery host",
            "StartLimitIntervalSec=300",
            "StartLimitBurst=10",
            "Type=exec",
            "ExecStart=\"/home/me/.local/bin/hen nery\" \"host\" \"run\" \"--data-dir\" ",
            "EnvironmentFile=/home/me/.config/hennery 100%%/service.env\n",
            "Environment=HENNERY_SERVICE=systemd",
            "Restart=on-failure",
            "RestartSec=3",
            "KillMode=mixed",
            "TimeoutStopSec=30",
            "WantedBy=default.target",
        ] {
            assert!(text.contains(wanted), "{wanted:?} not in:\n{text}");
        }
        assert!(text.contains("with %%h $$HOME \\\"quotes\\\""), "{text}");
        assert!(systemd_unit(Role::Host, &argv, "/home/me/a\\b/service.env").is_err());
        assert!(systemd_unit(Role::Host, &argv, "/home/me/a\"b/service.env").is_err());
    }

    #[test]
    fn the_env_file_holds_the_path_alone_quoted() {
        let text = env_file("/opt/a b/bin:/opt/$x/`y`/\"z\"\\:/usr/bin");
        assert_eq!(
            text.lines().last().unwrap(),
            "PATH=\"/opt/a b/bin:/opt/\\$x/\\`y\\`/\\\"z\\\"\\\\:/usr/bin\""
        );
        assert_eq!(text.lines().filter(|l| !l.starts_with('#')).count(), 1);
    }

    /// `plutil -lint` accepts the plist (macOS; it ships with the system).
    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_the_plist() {
        let dir = tempfile::tempdir().unwrap();
        let argv = command_line(Role::Up, &std::env::current_exe().unwrap(), &awkward()).unwrap();
        let file = dir.path().join("dev.hennery.up.plist");
        std::fs::write(&file, plist(Role::Up, &argv, "/usr/bin:/bin", "/tmp/a & b/up.log")).unwrap();
        let out = std::process::Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&file)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    }

    /// `systemd-analyze verify` accepts the unit (Linux, where it is
    /// installed; CI's ubuntu job sets `HENNERY_REQUIRE_SYSTEMD_ANALYZE`, so
    /// it runs there rather than skip).
    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_analyze_accepts_the_unit() {
        let required = std::env::var_os("HENNERY_REQUIRE_SYSTEMD_ANALYZE").is_some_and(|v| !v.is_empty());
        let Ok(version) = std::process::Command::new("systemd-analyze").arg("--version").output() else {
            assert!(!required, "systemd-analyze is required but not installed");
            eprintln!("skipped: no systemd-analyze");
            return;
        };
        assert!(version.status.success());
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join("service 100%.env");
        std::fs::write(&env, env_file("/usr/bin:/bin")).unwrap();
        for role in Role::ALL {
            let argv = command_line(role, &std::env::current_exe().unwrap(), &awkward()).unwrap();
            let file = dir.path().join(role.unit());
            std::fs::write(&file, systemd_unit(role, &argv, env.to_str().unwrap()).unwrap()).unwrap();
            let out = std::process::Command::new("systemd-analyze")
                .args(["--user", "verify"])
                .arg(&file)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "{role}: {stderr}");
            assert!(!stderr.contains(role.unit()), "{role}: {stderr}");
        }
    }

    /// What `status` and `uninstall` read back is what `install` wrote, for
    /// every character the files escape.
    #[test]
    fn the_command_line_reads_back_from_either_file() {
        let exe = Path::new("/home/me/.local/bin/hen nery");
        for role in Role::ALL {
            let argv = command_line(role, exe, &awkward()).unwrap();
            let text = plist(role, &argv, "/usr/bin", "/tmp/a & b.log");
            assert_eq!(plist_command_line(&text).unwrap(), argv, "{role}");
            let text = systemd_unit(role, &argv, "/tmp/env").unwrap();
            assert_eq!(systemd_command_line(&text).unwrap(), argv, "{role}");
        }
        assert_eq!(systemd_command_line("ExecStart=/bin/true"), None);
        let argv = command_line(Role::Up, exe, &awkward()).unwrap();
        let edited = systemd_unit(Role::Up, &argv, "/tmp/env")
            .unwrap()
            .replace('\n', " \r\n");
        assert_eq!(systemd_command_line(&edited).unwrap(), argv);
        assert_eq!(plist_command_line("<plist/>"), None);
    }
}
