//! Adapter profiles (ACP core §6), as far as plan 8c needs them: what a
//! session's MCP servers get from the agent's adapter, decided by where the
//! agent came from, never by its name alone.

use hennery_proto::frames::{McpIsolation, McpServer, NameValue, SessionBody};
use serde_json::{Map, Value};

/// How the host treats an agent's sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// The pinned `claude-agent-acp` from the installed set with its bundled
    /// CLI: strict MCP config on every `session/new` and `session/load`.
    Claude,
    /// The pinned `claude-agent-acp` with the operator's own CLI
    /// (`--use-cli`, `CLAUDE_CODE_EXECUTABLE`): strict MCP config is still
    /// sent, but an override is unverified (ACP core §6), so the host
    /// reports no isolation.
    ClaudeOwnCli,
    /// Anything else: a `--agent` command, and Codex until its composed
    /// `CODEX_HOME` (plan 8h). No `_meta`, no isolation.
    #[default]
    Generic,
}

impl Profile {
    /// The profile of an installed set's adapter `name`; `own_cli` when
    /// `host.toml` gives it the operator's CLI.
    pub fn of_installed(name: &str, own_cli: bool) -> Self {
        match (name, own_cli) {
            ("claude", false) => Self::Claude,
            ("claude", true) => Self::ClaudeOwnCli,
            _ => Self::Generic,
        }
    }

    /// What `hello.mcp_isolation` reports for this agent.
    pub fn mcp_isolation(self) -> McpIsolation {
        match self {
            Self::Claude => McpIsolation::ClaudeStrict,
            Self::ClaudeOwnCli | Self::Generic => McpIsolation::None,
        }
    }

    /// `_meta` for `session/new` and `session/load`. Claude's strict flag
    /// goes on every one, with servers or without (umbrella §8.5, the
    /// spike's conclusion 2): the adapter does not keep it across a load,
    /// and without it the user's own MCP servers and claude.ai connectors
    /// load beside hennery's. The host sends no other `_meta` today; any
    /// later key joins this object.
    pub fn session_meta(self) -> Option<Map<String, Value>> {
        match self {
            Self::Claude | Self::ClaudeOwnCli => {
                let mut meta = Map::new();
                meta.insert(
                    "claudeCode".into(),
                    serde_json::json!({ "options": { "extraArgs": { "strict-mcp-config": "" } } }),
                );
                Some(meta)
            }
            Self::Generic => None,
        }
    }
}

/// Why a start or resume is refused before anything is spawned (plan 8c,
/// the lane's L3): it carries servers for an agent this host cannot keep to
/// them, and the collector did not waive isolation. Never dropped, never
/// passed on.
pub fn mcp_refusal(agent: &str, profile: Profile, servers: &[McpServer], waived: bool) -> Option<String> {
    (!servers.is_empty() && !waived && profile.mcp_isolation() == McpIsolation::None).then(|| {
        format!("this host cannot keep agent {agent}'s sessions to the MCP servers it is given, and isolation was not waived")
    })
}

/// The ACP `mcpServers` entries for `servers`.
pub fn acp_servers(servers: &[McpServer]) -> Vec<agent_client_protocol::schema::v1::McpServer> {
    use agent_client_protocol::schema::v1::{EnvVariable, HttpHeader, McpServer as Acp, McpServerHttp, McpServerStdio};
    servers
        .iter()
        .map(|server| match server {
            McpServer::Http { name, url, headers } => Acp::Http(
                McpServerHttp::new(name, url).headers(
                    headers
                        .iter()
                        .map(|NameValue { name, value }| HttpHeader::new(name, value))
                        .collect(),
                ),
            ),
            McpServer::Stdio {
                name,
                command,
                args,
                env,
            } => Acp::Stdio(
                McpServerStdio::new(name, command).args(args.clone()).env(
                    env.iter()
                        .map(|NameValue { name, value }| EnvVariable::new(name, value))
                        .collect(),
                ),
            ),
        })
        .collect()
}

/// The shortest value redacted from what a session reports (`Secrets`):
/// below it a value is too likely to be an ordinary word (`1`, `true`).
pub const MIN_SECRET_LEN: usize = 8;

/// Where `Secrets` cuts a value into parts, besides whitespace: a URL's
/// delimiters, and its userinfo's (so `user:password`'s password is caught
/// alone). It errs on the safe side: an ordinary word of 8 bytes or more in
/// a stdio server's argument (`Projects` in a path) is redacted too, which
/// costs a little diagnostic text, never a secret.
const SEPARATORS: &str = "/?#&=:@";

/// The secret values of a session's servers (ACP core §8): every part
/// `McpServer::secret_values` names, each of its parts between whitespace
/// or URL delimiters (`SEPARATORS`: so a bare token echoed without its
/// `Bearer `, or one segment of a URL's path, is caught too), and the
/// JSON-escaped form of each (an adapter may echo its config as JSON), at
/// least `MIN_SECRET_LEN` bytes long. Longest first, so a value is redacted
/// before a part of it. `Debug` shows how many, never one.
#[derive(Clone, Default)]
pub struct Secrets(Vec<String>);

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secrets(<{} redacted>)", self.0.len())
    }
}

impl Secrets {
    pub fn of(servers: &[McpServer]) -> Self {
        let mut values: Vec<String> = servers
            .iter()
            .flat_map(McpServer::secret_values)
            .flat_map(|value| {
                std::iter::once(value).chain(value.split(|c: char| c.is_whitespace() || SEPARATORS.contains(c)))
            })
            .filter(|value| value.len() >= MIN_SECRET_LEN)
            .flat_map(|value| {
                let json = serde_json::to_string(value).expect("a string serializes");
                let escaped = json[1..json.len() - 1].to_string();
                [Some(value.to_string()), (escaped != value).then_some(escaped)]
            })
            .flatten()
            .collect();
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        Self(values)
    }

    /// `text` with every secret value replaced by `[redacted]`.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.0 {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), crate::adapter::REDACTED);
            }
        }
        out
    }

    /// `body` with the secret values redacted from the text hennery itself
    /// writes in it: a start failure's message, a turn's error, a note, a
    /// stderr tail. Exhaustive on purpose: a new body must say whether it
    /// carries such text, or this does not compile. ACP payloads are the
    /// adapter's, verbatim (ACP core §2.3); whether a token an agent prints
    /// into one is redacted is plan 8e's open question.
    pub fn redact_body(&self, body: SessionBody) -> SessionBody {
        match body {
            SessionBody::StartFailed {
                request_id,
                code,
                message,
            } => SessionBody::StartFailed {
                request_id,
                code,
                message: self.redact(&message),
            },
            SessionBody::TurnEnded {
                turn_id,
                outcome,
                stop_reason,
                error,
            } => SessionBody::TurnEnded {
                turn_id,
                outcome,
                stop_reason,
                error: error.map(|error| self.redact(&error)),
            },
            SessionBody::HostNote { note, text } => SessionBody::HostNote {
                note,
                text: self.redact(&text),
            },
            SessionBody::AdapterExited {
                code,
                signal,
                stderr_tail,
            } => SessionBody::AdapterExited {
                code,
                signal,
                stderr_tail: self.redact(&stderr_tail),
            },
            // Ids, enums, extracts and the adapter's own payloads: no text
            // hennery writes.
            body @ (SessionBody::SessionStarted { .. }
            | SessionBody::TurnStarted { .. }
            | SessionBody::AcpUpdate { .. }
            | SessionBody::SessionParked { .. }
            | SessionBody::SessionClosed
            | SessionBody::ConfigApplied { .. }
            | SessionBody::PendingOpened { .. }
            | SessionBody::PendingResolved { .. }
            | SessionBody::AnswerResult { .. }
            | SessionBody::GitState { .. }) => body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_proto::frames::TurnOutcome;

    fn servers() -> Vec<McpServer> {
        vec![
            McpServer::Http {
                name: "hennery-notes".into(),
                url: "https://hennery.example/mcp/notes".into(),
                headers: vec![
                    NameValue::new("Authorization", "Bearer hst_token_0123456789"),
                    NameValue::new("X-Short", "abc"),
                ],
            },
            McpServer::Stdio {
                name: "hennery-files".into(),
                command: "/bin/files".into(),
                args: vec![],
                env: vec![
                    NameValue::new("KEY", "files-key-0123456789"),
                    NameValue::new("ON", "1"),
                    NameValue::new("QUOTED", "a \"quoted\" key"),
                ],
            },
        ]
    }

    #[test]
    fn only_the_pinned_claude_with_its_own_cli_is_isolated() {
        assert_eq!(Profile::of_installed("claude", false), Profile::Claude);
        assert_eq!(Profile::of_installed("claude", true), Profile::ClaudeOwnCli);
        assert_eq!(Profile::of_installed("codex", false), Profile::Generic);
        assert_eq!(Profile::Claude.mcp_isolation(), McpIsolation::ClaudeStrict);
        assert_eq!(Profile::ClaudeOwnCli.mcp_isolation(), McpIsolation::None);
        assert_eq!(Profile::Generic.mcp_isolation(), McpIsolation::None);
        // The strict flag goes to both Claude profiles, exactly as the
        // adapter reads it; nothing to any other agent.
        let strict = serde_json::json!({"claudeCode": {"options": {"extraArgs": {"strict-mcp-config": ""}}}});
        for profile in [Profile::Claude, Profile::ClaudeOwnCli] {
            assert_eq!(Value::Object(profile.session_meta().unwrap()), strict);
        }
        assert_eq!(Profile::Generic.session_meta(), None);
    }

    #[test]
    fn servers_are_refused_only_for_an_agent_not_isolated_unless_waived() {
        let servers = servers();
        assert_eq!(mcp_refusal("claude", Profile::Claude, &servers, false), None);
        assert!(mcp_refusal("fake", Profile::Generic, &servers, false).is_some());
        assert!(mcp_refusal("claude", Profile::ClaudeOwnCli, &servers, false).is_some());
        assert_eq!(mcp_refusal("fake", Profile::Generic, &servers, true), None);
        assert_eq!(mcp_refusal("fake", Profile::Generic, &[], false), None);
    }

    #[test]
    fn servers_become_acp_entries_with_stdio_untagged() {
        let acp = serde_json::to_value(acp_servers(&servers())).unwrap();
        assert_eq!(
            acp,
            serde_json::json!([
                {"type": "http", "name": "hennery-notes", "url": "https://hennery.example/mcp/notes",
                 "headers": [{"name": "Authorization", "value": "Bearer hst_token_0123456789"},
                             {"name": "X-Short", "value": "abc"}]},
                {"name": "hennery-files", "command": "/bin/files", "args": [],
                 "env": [{"name": "KEY", "value": "files-key-0123456789"}, {"name": "ON", "value": "1"},
                         {"name": "QUOTED", "value": "a \"quoted\" key"}]}
            ])
        );
    }

    #[test]
    fn secrets_are_redacted_whole_and_by_part_but_not_short_values() {
        let secrets = Secrets::of(&servers());
        let text = "sent Bearer hst_token_0123456789; echoed hst_token_0123456789 and files-key-0123456789, abc 1";
        assert_eq!(
            secrets.redact(text),
            "sent [redacted]; echoed [redacted] and [redacted], abc 1"
        );
        assert_eq!(Secrets::default().redact(text), text);
        // Echoed as JSON, the escaped form is caught too.
        let echoed = serde_json::to_string("a \"quoted\" key").unwrap();
        assert_eq!(secrets.redact(&format!("config: {echoed}")), "config: \"[redacted]\"");

        // A part of a URL's path, or its userinfo's password, echoed alone.
        let url = Secrets::of(&[McpServer::Http {
            name: "n".into(),
            url: "https://user:pw-secret-0123@h.example/mcp/url-key-0123456789?k=query-key-0123".into(),
            headers: vec![],
        }]);
        assert_eq!(
            url.redact("saw url-key-0123456789, query-key-0123 and pw-secret-0123 at /mcp"),
            "saw [redacted], [redacted] and [redacted] at /mcp"
        );
        // Its `Debug` shows none of them.
        let debug = format!("{secrets:?}");
        assert!(
            debug.starts_with("Secrets(<") && !debug.contains("0123456789"),
            "{debug}"
        );
    }

    #[test]
    fn the_text_hennery_writes_is_redacted_in_every_body_that_has_some() {
        let secrets = Secrets::of(&servers());
        let echo = "echoed hst_token_0123456789".to_string();
        let redacted = |body| serde_json::to_string(&secrets.redact_body(body)).unwrap();
        for body in [
            SessionBody::StartFailed {
                request_id: "r".into(),
                code: "start_failed".into(),
                message: echo.clone(),
            },
            SessionBody::HostNote {
                note: "config_failed".into(),
                text: echo.clone(),
            },
            SessionBody::AdapterExited {
                code: Some(3),
                signal: None,
                stderr_tail: echo.clone(),
            },
            SessionBody::TurnEnded {
                turn_id: "t".into(),
                outcome: TurnOutcome::Failed,
                stop_reason: None,
                error: Some(echo.clone()),
            },
        ] {
            let text = redacted(body);
            assert!(
                text.contains("echoed [redacted]") && !text.contains("hst_token"),
                "{text}"
            );
        }
        // An ACP payload is the adapter's, verbatim (ACP core §2.3).
        let update = SessionBody::AcpUpdate {
            indexed: Default::default(),
            payload: serde_json::json!({ "text": echo }),
        };
        assert!(redacted(update).contains("hst_token_0123456789"));
    }
}
