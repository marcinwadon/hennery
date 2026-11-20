//! A session's delivered hook sets (skills spec §8): rendered per agent
//! before its adapter is spawned, and, for Codex, trusted through the
//! adapter after `initialize`. Delivery is fail-closed: a set the host
//! cannot apply fails the start, never runs unloaded.

use crate::adapter::AgentCommand;
use crate::profile::Profile;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::{Agent, ErrorCode, UntypedMessage};
use hennery_proto::frames::{HookDelivery, HookSetApplied};
use hennery_proto::hooks::{CODEX_EVENTS, HookSetConfig, merge};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// What a delivery becomes for one profile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendered {
    /// Claude: `_meta.claudeCode.options.settings.hooks`.
    pub claude_meta_hooks: Option<HookSetConfig>,
    /// Codex: the adapter's `CODEX_CONFIG`, as JSON. When set, the adapter's
    /// hooks must be trusted after `initialize`.
    pub codex_config: Option<String>,
    /// Codex: the `hooks` inside `codex_config` (the operator's own first,
    /// then hennery's), which the adapter's listing must match.
    pub codex_hooks: Option<HookSetConfig>,
    /// The sets that reach the agent, in delivery order.
    pub applied: Vec<HookSetApplied>,
}

/// Why a delivery cannot be applied; the `Display` text is the
/// `start_failed` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSetsError {
    /// The profile has no way to run hooks.
    Unsupported,
    /// The agent's own `CODEX_CONFIG` is not a JSON object.
    CodexConfigNotObject,
    /// The `hooks` in the agent's own `CODEX_CONFIG` are not a hook config:
    /// serde's error category and where in the value, never the value's
    /// own text (it is the operator's and may hold a secret).
    CodexHooksInvalid {
        category: &'static str,
        line: usize,
        column: usize,
    },
    /// The agent command is an `env(1)` wrapper that drops the environment
    /// (`-i`, `-u CODEX_CONFIG`) and sets no `CODEX_CONFIG` itself, so
    /// hennery's would never reach the adapter.
    EnvironmentDropped,
    /// The adapter lacks `_codex/hooks/list` or `_codex/hooks/trust`.
    NoExtension,
    /// The adapter's session-flag hooks are not at the positions it was
    /// given (an agent command that replaces hennery's `CODEX_CONFIG`, say):
    /// handler positions `<event>:<group>:<handler>`, sorted.
    Mismatch { expected: Vec<String>, listed: Vec<String> },
    /// The adapter lists a hook at one of the expected positions that is
    /// not the one it was given: each entry is the position and what
    /// differed (`handler type`, `command`, `matcher`), never the listed
    /// command itself.
    Differs(Vec<String>),
    /// The adapter's trust call did not answer `trusted: true`.
    NotTrusted,
    /// Any other failure of the adapter's hook calls.
    Adapter(String),
}

impl std::fmt::Display for HookSetsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("this agent cannot run hook sets"),
            Self::CodexConfigNotObject => f.write_str("the agent's CODEX_CONFIG is not a JSON object"),
            Self::NoExtension => f.write_str("the adapter does not offer _codex/hooks/list and _codex/hooks/trust"),
            Self::CodexHooksInvalid { category, line, column } => write!(
                f,
                "the hooks in the agent's CODEX_CONFIG are invalid ({category} error at line {line}, column {column})"
            ),
            Self::EnvironmentDropped => f.write_str(
                "the agent command's env wrapper drops the environment and sets no CODEX_CONFIG, so the hooks cannot reach the adapter"
            ),
            Self::Mismatch { expected, listed } => write!(
                f,
                "the adapter lists other hooks than it was given: expected [{}], listed [{}]",
                expected.join(", "),
                listed.join(", ")
            ),
            Self::Differs(differences) => write!(
                f,
                "the adapter lists other hooks than it was given at [{}]",
                differences.join(", ")
            ),
            Self::NotTrusted => f.write_str("the adapter did not trust the hooks it was given"),
            Self::Adapter(message) => write!(f, "the adapter's hook call failed: {message}"),
        }
    }
}

impl std::error::Error for HookSetsError {}

/// The sets of `delivery` that reach a `profile` agent: every set for
/// Claude, every set with at least one event Codex runs for Codex, none
/// otherwise. In delivery order.
pub fn applied(profile: Profile, delivery: &HookDelivery) -> Vec<HookSetApplied> {
    delivery
        .sets
        .iter()
        .filter(|set| match profile {
            Profile::Claude | Profile::ClaudeOwnCli => true,
            Profile::Codex => !hennery_proto::hooks::render(&set.hooks, CODEX_EVENTS).0.is_empty(),
            Profile::Generic => false,
        })
        .map(|set| HookSetApplied {
            id: set.id.clone(),
            name: set.name.clone(),
            content_hash: set.content_hash.clone(),
        })
        .collect()
}

/// Render `delivery` for a `profile` agent: the sets merged event by event
/// in delivery order, for Codex with only the events it runs and inside
/// `existing_codex_config` (the agent's own `CODEX_CONFIG`, from
/// [`operator_codex_config`]: its environment or the `CODEX_CONFIG=`
/// argument of an `env(1)` wrapper) when it has one. Hooks already in that
/// config come first, hennery's groups after them, and the trust call after
/// `initialize` trusts both: that value comes from the operator's own agent
/// command (`host.toml`), not from a third party. The result goes back
/// where the existing value was ([`deliver_codex_config`]).
pub fn render(
    profile: Profile,
    delivery: &HookDelivery,
    existing_codex_config: Option<&str>,
) -> Result<Rendered, HookSetsError> {
    let applied = applied(profile, delivery);
    match profile {
        Profile::Generic => Err(HookSetsError::Unsupported),
        Profile::Claude | Profile::ClaudeOwnCli => {
            let merged = merge(delivery.sets.iter().map(|set| &set.hooks));
            Ok(Rendered {
                claude_meta_hooks: (!merged.0.is_empty()).then_some(merged),
                codex_config: None,
                codex_hooks: None,
                applied,
            })
        }
        Profile::Codex => {
            // codex-acp ignores an empty CODEX_CONFIG; so does hennery.
            let existing_codex_config = existing_codex_config.filter(|raw| !raw.is_empty());
            let rendered: Vec<HookSetConfig> = delivery
                .sets
                .iter()
                .map(|set| hennery_proto::hooks::render(&set.hooks, CODEX_EVENTS))
                .collect();
            let merged = merge(&rendered);
            if merged.0.is_empty() {
                return Ok(Rendered {
                    applied,
                    ..Default::default()
                });
            }
            let mut config = match existing_codex_config {
                None => Map::new(),
                Some(raw) => match serde_json::from_str::<Value>(raw) {
                    Ok(Value::Object(config)) => config,
                    _ => return Err(HookSetsError::CodexConfigNotObject),
                },
            };
            let operator = match (config.remove("hooks"), existing_codex_config) {
                (Some(_), Some(raw)) => operator_hooks(raw)?,
                _ => HookSetConfig::default(),
            };
            let hooks = merge([&operator, &merged]);
            config.insert(
                "hooks".into(),
                serde_json::to_value(&hooks).expect("hook config serialises"),
            );
            Ok(Rendered {
                claude_meta_hooks: None,
                codex_config: Some(Value::Object(config).to_string()),
                codex_hooks: Some(hooks),
                applied,
            })
        }
    }
}

/// The `hooks` of the operator's own `CODEX_CONFIG` text, parsed from the
/// text itself so an error has a line and column to report.
fn operator_hooks(raw: &str) -> Result<HookSetConfig, HookSetsError> {
    #[derive(serde::Deserialize)]
    struct WithHooks {
        #[serde(default)]
        hooks: HookSetConfig,
    }
    serde_json::from_str::<WithHooks>(raw)
        .map(|config| config.hooks)
        .map_err(|err| HookSetsError::CodexHooksInvalid {
            category: match err.classify() {
                serde_json::error::Category::Io => "io",
                serde_json::error::Category::Syntax => "syntax",
                serde_json::error::Category::Data => "data",
                serde_json::error::Category::Eof => "eof",
            },
            line: err.line(),
            column: err.column(),
        })
}

/// Where an agent command sets its own `CODEX_CONFIG`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexConfigSource {
    /// `agent.env[i]`.
    Env(usize),
    /// `agent.args[i]`, a `CODEX_CONFIG=…` assignment of an `env(1)`
    /// wrapper: it overrides whatever is in `agent.env`.
    Arg(usize),
}

/// The operator's own `CODEX_CONFIG` for `agent`, the one the adapter will
/// actually run with: when the program is `env(1)`, the last
/// `CODEX_CONFIG=` among its leading `NAME=VALUE` arguments (after its
/// `-i`, `-u NAME`, `-C DIR`, `-0`, `-v` options, up to the first other
/// argument), else the last `CODEX_CONFIG` in `agent.env`. An `env` that
/// drops the environment (`-i`, `-`, `-u CODEX_CONFIG`) and sets none
/// itself would drop hennery's too: `EnvironmentDropped`.
pub fn operator_codex_config(agent: &AgentCommand) -> Result<Option<CodexConfigSource>, HookSetsError> {
    const NAME: &str = "CODEX_CONFIG";
    let in_env = agent
        .env
        .iter()
        .rposition(|(name, _)| name == NAME)
        .map(CodexConfigSource::Env);
    if Path::new(&agent.program).file_name().and_then(|name| name.to_str()) != Some("env") {
        return Ok(in_env);
    }
    let args = &agent.args;
    let mut i = 0;
    let mut dropped = false;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "-i" | "--ignore-environment" | "-" => dropped = true,
            "-u" | "--unset" => {
                i += 1;
                dropped |= args.get(i).is_some_and(|name| name == NAME);
            }
            "-C" | "--chdir" => i += 1,
            "-0" | "--null" | "-v" | "--debug" => {}
            "--" => {
                i += 1;
                break;
            }
            other => match other.strip_prefix("--unset=").or_else(|| other.strip_prefix("-u")) {
                Some(name) => dropped |= name == NAME,
                _ => break,
            },
        }
        i += 1;
    }
    let mut in_args = None;
    while let Some((name, _)) = args.get(i).and_then(|arg| arg.split_once('=')) {
        if name == NAME {
            in_args = Some(CodexConfigSource::Arg(i));
        }
        i += 1;
    }
    match (in_args, dropped) {
        (Some(source), _) => Ok(Some(source)),
        (None, true) => Err(HookSetsError::EnvironmentDropped),
        (None, false) => Ok(in_env),
    }
}

/// The text of the `CODEX_CONFIG` at `source`.
pub fn codex_config_text(agent: &AgentCommand, source: CodexConfigSource) -> &str {
    match source {
        CodexConfigSource::Env(i) => &agent.env[i].1,
        CodexConfigSource::Arg(i) => agent.args[i].split_once('=').map_or("", |(_, value)| value),
    }
}

/// Put the rendered `config` where the adapter reads it: in place of the
/// operator's own `CODEX_CONFIG=` argument, else as its only
/// `CODEX_CONFIG` in `agent.env`.
pub fn deliver_codex_config(agent: &mut AgentCommand, source: Option<CodexConfigSource>, config: String) {
    match source {
        Some(CodexConfigSource::Arg(i)) => agent.args[i] = format!("CODEX_CONFIG={config}"),
        Some(CodexConfigSource::Env(_)) | None => {
            agent.env.retain(|(name, _)| name != "CODEX_CONFIG");
            agent.env.push(("CODEX_CONFIG".into(), config));
        }
    }
}

/// Trust the hooks a Codex adapter was started with (skills spec §8):
/// `_codex/hooks/list` for `cwd`, then `_codex/hooks/trust` with the
/// `{key, currentHash}` of every hook it lists from the session's flags.
/// The listing must name exactly the handler positions of `sent` (the
/// `hooks` of the `CODEX_CONFIG` hennery passed), and at each position a
/// `command` handler with the same command and matcher: an agent command
/// that sets its own `CODEX_CONFIG` replaces hennery's, and those hooks
/// must not be trusted in its place. Codex skips an untrusted hook
/// silently, so anything but `trusted: true` is an error.
pub async fn trust_codex(conn: &ConnectionTo<Agent>, cwd: &Path, sent: &HookSetConfig) -> Result<(), HookSetsError> {
    let listed = call(conn, "_codex/hooks/list", json!({ "cwd": cwd })).await?;
    let entries: Vec<(&Value, &str, &str)> = listed
        .get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|hook| hook.get("source").and_then(Value::as_str) == Some("sessionFlags"))
        .filter_map(|hook| Some((hook, hook.get("key")?.as_str()?, hook.get("currentHash")?.as_str()?)))
        .collect();
    let handlers = handlers(sent);
    let expected: Vec<String> = handlers.keys().cloned().collect();
    let mut listed: Vec<String> = entries.iter().map(|(_, key, _)| position_of(key)).collect();
    listed.sort();
    if listed != expected {
        return Err(HookSetsError::Mismatch { expected, listed });
    }
    let mut differences: Vec<String> = entries
        .iter()
        .filter_map(|(hook, key, _)| {
            let position = position_of(key);
            let (command, matcher) = handlers[&position];
            let mut what = Vec::new();
            if hook.get("handlerType").and_then(Value::as_str) != Some("command") {
                what.push("handler type");
            }
            if hook.get("command").and_then(Value::as_str) != Some(command) {
                what.push("command");
            }
            let listed_matcher = match hook.get("matcher") {
                None | Some(Value::Null) => Some(None),
                Some(Value::String(matcher)) => Some(Some(matcher.as_str())),
                Some(_) => None,
            };
            if listed_matcher != Some(matcher) {
                what.push("matcher");
            }
            (!what.is_empty()).then(|| format!("{position} ({})", what.join(", ")))
        })
        .collect();
    if !differences.is_empty() {
        differences.sort();
        return Err(HookSetsError::Differs(differences));
    }
    let hooks: Vec<Value> = entries
        .iter()
        .map(|(_, key, hash)| json!({ "key": key, "currentHash": hash }))
        .collect();
    let answer = call(conn, "_codex/hooks/trust", json!({ "cwd": cwd, "hooks": hooks })).await?;
    match answer.get("trusted").and_then(Value::as_bool) {
        Some(true) => Ok(()),
        _ => Err(HookSetsError::NotTrusted),
    }
}

/// Every handler Codex lists for `hooks`, by position (sorted): the
/// `<event>:<group>:<handler>` that ends its key, the event in snake case
/// (`PreToolUse` is `pre_tool_use`), to its command and its group's
/// matcher. Events Codex does not run are not listed.
fn handlers(hooks: &HookSetConfig) -> BTreeMap<String, (&str, Option<&str>)> {
    hooks
        .0
        .iter()
        .filter(|(event, _)| CODEX_EVENTS.contains(&event.as_str()))
        .flat_map(|(event, groups)| {
            let event = snake_case(event);
            groups.iter().enumerate().flat_map(move |(group, matcher)| {
                let event = event.clone();
                matcher.hooks.iter().enumerate().map(move |(handler, hook)| {
                    (
                        format!("{event}:{group}:{handler}"),
                        (hook.command.as_str(), matcher.matcher.as_deref()),
                    )
                })
            })
        })
        .collect()
}

/// The `<event>:<group>:<handler>` that ends a listed key
/// (`/<session-flags>/config.toml:pre_tool_use:0:0`); the whole key when
/// it has no such ending.
fn position_of(key: &str) -> String {
    let parts: Vec<&str> = key.rsplitn(4, ':').collect();
    match parts.as_slice() {
        [handler, group, event, _] => format!("{event}:{group}:{handler}"),
        _ => key.to_string(),
    }
}

fn snake_case(event: &str) -> String {
    let mut out = String::with_capacity(event.len() + 4);
    for (i, c) in event.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

async fn call(conn: &ConnectionTo<Agent>, method: &str, params: Value) -> Result<Value, HookSetsError> {
    let request = UntypedMessage::new(method, params).expect("JSON value is serializable");
    conn.send_request(request)
        .block_task()
        .await
        .map_err(|err| match err.code {
            ErrorCode::METHOD_NOT_FOUND => HookSetsError::NoExtension,
            _ => HookSetsError::Adapter(err.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_proto::frames::HookSetDelivery;

    fn set(id: &str, hooks: Value) -> HookSetDelivery {
        HookSetDelivery {
            id: id.into(),
            name: format!("{id}-name"),
            content_hash: format!("sha256:{id}"),
            hooks: serde_json::from_value(hooks).unwrap(),
        }
    }

    fn delivery() -> HookDelivery {
        HookDelivery {
            sets: vec![
                set(
                    "a",
                    json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "/bin/a"}]}],
                           "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "/bin/a2"}]}]}),
                ),
                set(
                    "b",
                    json!({"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "/bin/b"}]}]}),
                ),
                set(
                    "c",
                    json!({"PreToolUse": [{"hooks": [{"type": "command", "command": "/bin/c", "timeout": 5}]}]}),
                ),
            ],
        }
    }

    fn ids(applied: &[HookSetApplied]) -> Vec<&str> {
        applied.iter().map(|a| a.id.as_str()).collect()
    }

    #[test]
    fn claude_takes_every_set_merged_in_delivery_order() {
        for profile in [Profile::Claude, Profile::ClaudeOwnCli] {
            let rendered = render(profile, &delivery(), Some("ignored")).unwrap();
            assert_eq!(ids(&rendered.applied), ["a", "b", "c"]);
            assert_eq!(rendered.codex_config, None);
            assert_eq!(
                serde_json::to_value(rendered.claude_meta_hooks.unwrap()).unwrap(),
                json!({
                    "PreToolUse": [
                        {"matcher": "Bash", "hooks": [{"type": "command", "command": "/bin/a"}]},
                        {"hooks": [{"type": "command", "command": "/bin/c", "timeout": 5}]}
                    ],
                    "UserPromptSubmit": [
                        {"hooks": [{"type": "command", "command": "/bin/a2"}]},
                        {"hooks": [{"type": "command", "command": "/bin/b"}]}
                    ]
                })
            );
        }
    }

    #[test]
    fn codex_takes_only_the_events_it_runs_inside_its_own_config() {
        let rendered = render(Profile::Codex, &delivery(), None).unwrap();
        assert_eq!(ids(&rendered.applied), ["a", "c"]);
        assert_eq!(rendered.claude_meta_hooks, None);
        let pre = json!([
            {"matcher": "Bash", "hooks": [{"type": "command", "command": "/bin/a"}]},
            {"hooks": [{"type": "command", "command": "/bin/c", "timeout": 5}]}
        ]);
        let config: Value = serde_json::from_str(&rendered.codex_config.unwrap()).unwrap();
        assert_eq!(config, json!({"hooks": {"PreToolUse": pre}}));

        let operator = json!({"type": "command", "command": "/bin/operator"});
        let rendered = render(
            Profile::Codex,
            &delivery(),
            Some(&json!({"model": "x", "hooks": {"PreToolUse": [{"hooks": [operator]}]}}).to_string()),
        )
        .unwrap();
        let config: Value = serde_json::from_str(&rendered.codex_config.unwrap()).unwrap();
        let mut merged = vec![json!({"hooks": [operator]})];
        merged.extend(pre.as_array().unwrap().iter().cloned());
        assert_eq!(config, json!({"model": "x", "hooks": {"PreToolUse": merged}}));
        assert_eq!(
            serde_json::to_value(rendered.codex_hooks.unwrap()).unwrap(),
            config["hooks"]
        );
        let invalid = render(
            Profile::Codex,
            &delivery(),
            Some(r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/bin/x --token=s3cret","bogus":1}]}]}}"#),
        )
        .unwrap_err();
        assert!(
            matches!(
                invalid,
                HookSetsError::CodexHooksInvalid {
                    category: "data",
                    line: 1,
                    ..
                }
            ),
            "{invalid:?}"
        );
        assert!(!invalid.to_string().contains("s3cret"), "{invalid}");
        assert!(!invalid.to_string().contains("bogus"), "{invalid}");
        assert!(invalid.to_string().contains("line 1, column"), "{invalid}");

        for bad in ["[]", "\"x\"", "not json", "null"] {
            assert_eq!(
                render(Profile::Codex, &delivery(), Some(bad)),
                Err(HookSetsError::CodexConfigNotObject),
                "{bad}"
            );
        }
    }

    #[test]
    fn codex_treats_an_empty_config_as_none_like_the_adapter() {
        // codex-acp reads `CODEX_CONFIG ? JSON.parse(CODEX_CONFIG) : undefined`,
        // so an empty value is no config, not a malformed one.
        assert_eq!(
            render(Profile::Codex, &delivery(), Some("")),
            render(Profile::Codex, &delivery(), None)
        );
    }

    #[test]
    fn codex_with_nothing_it_runs_gets_no_config_and_no_trust() {
        let only_prompts = HookDelivery {
            sets: vec![delivery().sets.remove(1)],
        };
        let rendered = render(Profile::Codex, &only_prompts, Some("[]")).unwrap();
        assert_eq!(rendered, Rendered::default());
    }

    #[test]
    fn codex_lists_handlers_by_snake_case_event_group_and_handler() {
        let hooks: HookSetConfig = serde_json::from_value(json!({
            "PreToolUse": [
                {"hooks": [{"type": "command", "command": "/bin/a"}, {"type": "command", "command": "/bin/b"}]},
                {"hooks": [{"type": "command", "command": "/bin/c"}]}
            ],
            "SessionStart": [{"hooks": [{"type": "command", "command": "/bin/d"}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "/bin/e"}]}]
        }))
        .unwrap();
        let listed = handlers(&hooks);
        assert_eq!(
            listed.keys().collect::<Vec<_>>(),
            [
                "pre_tool_use:0:0",
                "pre_tool_use:0:1",
                "pre_tool_use:1:0",
                "session_start:0:0"
            ]
        );
        assert_eq!(listed["pre_tool_use:0:1"], ("/bin/b", None));
        assert_eq!(
            position_of("/<session-flags>/config.toml:pre_tool_use:0:1"),
            "pre_tool_use:0:1"
        );
        assert_eq!(position_of("odd"), "odd");
    }

    fn command(program: &str, args: &[&str], env: &[(&str, &str)]) -> AgentCommand {
        AgentCommand {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn the_operator_codex_config_is_found_where_the_adapter_reads_it() {
        use CodexConfigSource::{Arg, Env};
        let dropped = Err(HookSetsError::EnvironmentDropped);
        // A command line, whether `agent.env` sets `CODEX_CONFIG` too, and
        // where the adapter's `CODEX_CONFIG` is.
        let cases = [
            ("codex-acp", false, Ok(None)),
            ("codex-acp CODEX_CONFIG={}", true, Ok(Some(Env(1)))),
            ("/usr/bin/env CODEX_PATH=/x codex-acp", true, Ok(Some(Env(1)))),
            (
                "/usr/bin/env CODEX_PATH=/x CODEX_CONFIG={} codex-acp",
                true,
                Ok(Some(Arg(1))),
            ),
            ("env CODEX_CONFIG={} CODEX_CONFIG={} codex-acp", false, Ok(Some(Arg(1)))),
            (
                "/usr/bin/env -i -C /tmp CODEX_CONFIG={} codex-acp",
                false,
                Ok(Some(Arg(3))),
            ),
            ("/usr/bin/env -- CODEX_CONFIG={} codex-acp", false, Ok(Some(Arg(1)))),
            // After the program, an assignment is the program's argument.
            ("/usr/bin/env codex-acp CODEX_CONFIG={}", true, Ok(Some(Env(1)))),
            ("/usr/bin/env -v codex-acp", false, Ok(None)),
            ("/usr/bin/env -u OTHER codex-acp", false, Ok(None)),
            ("/usr/bin/env -i codex-acp", true, dropped.clone()),
            ("/usr/bin/env - codex-acp", false, dropped.clone()),
            ("/usr/bin/env -u CODEX_CONFIG codex-acp", true, dropped.clone()),
            ("/usr/bin/env --unset=CODEX_CONFIG codex-acp", false, dropped.clone()),
            ("/usr/bin/env -uCODEX_CONFIG codex-acp", false, dropped),
            ("/usr/bin/env -i CODEX_CONFIG={} codex-acp", false, Ok(Some(Arg(1)))),
            // Not env(1): its arguments are the adapter's.
            ("/usr/bin/envoy -i CODEX_CONFIG={}", false, Ok(None)),
        ];
        for (line, in_env, expected) in cases {
            let mut agent = AgentCommand::parse(line).unwrap();
            agent.env.push(("A".into(), "1".into()));
            if in_env {
                agent.env.push(("CODEX_CONFIG".into(), "{}".into()));
            }
            assert_eq!(operator_codex_config(&agent), expected, "{line} (env: {in_env})");
        }
    }

    #[test]
    fn the_merged_codex_config_goes_back_where_it_was() {
        let mut agent = command("/usr/bin/env", &["CODEX_CONFIG={\"model\":\"x\"}", "codex-acp"], &[]);
        let source = operator_codex_config(&agent).unwrap();
        assert_eq!(codex_config_text(&agent, source.unwrap()), "{\"model\":\"x\"}");
        deliver_codex_config(&mut agent, source, "{\"hooks\":{}}".into());
        assert_eq!(agent.args, ["CODEX_CONFIG={\"hooks\":{}}", "codex-acp"]);
        assert!(agent.env.is_empty());

        let mut agent = command("codex-acp", &[], &[("CODEX_CONFIG", "{}"), ("A", "1")]);
        let source = operator_codex_config(&agent).unwrap();
        assert_eq!(codex_config_text(&agent, source.unwrap()), "{}");
        deliver_codex_config(&mut agent, source, "{\"hooks\":{}}".into());
        assert_eq!(
            agent.env,
            [
                ("A".to_string(), "1".to_string()),
                ("CODEX_CONFIG".into(), "{\"hooks\":{}}".into())
            ]
        );

        let mut agent = command("codex-acp", &[], &[]);
        deliver_codex_config(&mut agent, None, "{}".into());
        assert_eq!(agent.env, [("CODEX_CONFIG".to_string(), "{}".to_string())]);
    }

    #[test]
    fn a_generic_agent_cannot_run_hook_sets() {
        assert_eq!(
            render(Profile::Generic, &delivery(), None),
            Err(HookSetsError::Unsupported)
        );
        assert!(applied(Profile::Generic, &delivery()).is_empty());
    }
}
