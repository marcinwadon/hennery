//! What a host says of its agents (ACP core §6 "Agent availability"; plan
//! 4d-B1-i): in `hello`, a cheap static view, and in `agents`, the answer
//! to `probe_agents`, a live one. Both ends bound a report the same way
//! (`bound_agents`, `RuntimeInfo::bounded`): the host before it sends one,
//! the collector again before it stores one, since a host may lie, but only
//! about itself.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// The most agents one report holds; the rest are dropped.
pub const MAX_AGENTS: usize = 16;
/// The longest agent (profile) name kept, in bytes.
pub const MAX_AGENT_NAME: usize = 64;
/// The longest adapter version kept, in bytes.
pub const MAX_AGENT_VERSION: usize = 64;
/// The longest adapter set id kept, in bytes.
pub const MAX_SET_ID: usize = 128;
/// The longest note kept, in bytes.
pub const MAX_AGENT_NOTE: usize = 512;

/// Whether an agent's CLI says it is logged in (ACP core §6): only the
/// verdict, never the account. `unknown` until a probe asked, and when the
/// host knows no CLI to ask, or it did not answer (in time, or at all: a CLI
/// ended by a signal said nothing). A host fact: agent logins
/// are per OS user in v1, every hat's included. If plan 8h ever makes
/// `auth.json` private per hat, `auth` must become per (host, hat).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentAuth {
    /// The CLI's status command exited 0.
    Ok,
    /// It exited non-zero.
    Missing,
    #[default]
    Unknown,
}

/// Which CLI an agent runs (distribution spec §3.2, §13 decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCli {
    /// The CLI bundled in the host's adapter set.
    Bundled,
    /// The operator's own CLI (`--use-cli`, recorded in `host.toml`).
    Override,
    /// A command given to `host run --agent` (e.g. Nix-provided adapters):
    /// hennery manages nothing of it.
    Given,
}

/// One agent of a host (ACP core §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AgentInfo {
    /// The profile name: what `StartSessionRequest.agent` takes.
    pub agent: String,
    /// In `hello`: launchable as configured. After a probe: its adapter
    /// started and answered `initialize`.
    pub available: bool,
    #[serde(default)]
    pub auth: AgentAuth,
    pub cli: AgentCli,
    /// In `hello`: the version the adapter set records. After a probe: the
    /// version the adapter's `initialize` answered with (`agentInfo`), if
    /// it gave a readable one. Absent for a `given` agent until a probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub adapter_version: Option<String>,
    /// Whether the agent takes images in prompts (its `initialize`'s
    /// `promptCapabilities.image`). Clients hide images only when this is
    /// `false`. Absent means no probe answered (none has run, or the
    /// adapter did not answer): a client then allows images, and the
    /// server's 409 `images_unsupported` remains the guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub images: Option<bool>,
    /// Why the agent is unavailable, or a caveat, in the host's words: never
    /// anything an agent or its CLI printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub note: Option<String>,
}

/// Where a host's agents come from (distribution spec §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSource {
    /// An adapter set hennery installed and pins.
    Managed,
    /// `host run --agent` commands.
    Given,
}

/// The host's adapter runtime. `set_id`, `pinned` and `held` are a managed
/// runtime's only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RuntimeInfo {
    pub source: RuntimeSource,
    /// The adapter set the agents launch from; absent when none is
    /// installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub set_id: Option<String>,
    /// That set is the one this host's binary pins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub pinned: Option<bool>,
    /// A rollback holds the host on its set (`hennery host adapters
    /// rollback`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub held: Option<bool>,
}

impl RuntimeInfo {
    /// Bounded as a report is (`bound_agents`): a `given` runtime keeps no
    /// managed field, and a set id that is not 1 to `MAX_SET_ID` of
    /// `[0-9A-Za-z.+-]` is dropped.
    pub fn bounded(self) -> Self {
        match self.source {
            RuntimeSource::Given => Self {
                source: RuntimeSource::Given,
                set_id: None,
                pinned: None,
                held: None,
            },
            RuntimeSource::Managed => Self {
                set_id: self.set_id.filter(|id| is_token(id, MAX_SET_ID)),
                ..self
            },
        }
    }
}

/// `s` is 1 to `max` bytes of `[0-9A-Za-z.+-]` (doctor's readable
/// version).
fn is_token(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
}

/// Whether an adapter's version can be reported: 1 to
/// `MAX_AGENT_VERSION` of `[0-9A-Za-z.+-]`.
pub fn is_readable_version(version: &str) -> bool {
    is_token(version, MAX_AGENT_VERSION)
}

/// Invisible Unicode format characters (bidi overrides and isolates,
/// zero-width characters, the byte-order mark). The kernel's host registry
/// refuses them in a host's name with this same list (the review's O4).
pub fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// `note` as a report may carry it: control and format characters replaced
/// by U+FFFD, then cut to `MAX_AGENT_NOTE` bytes on a character boundary;
/// `None` if nothing is left.
pub fn bounded_note(note: &str) -> Option<String> {
    let mut out = String::new();
    for c in note.chars() {
        let c = if c.is_control() || is_format_char(c) {
            '\u{FFFD}'
        } else {
            c
        };
        if out.len() + c.len_utf8() > MAX_AGENT_NOTE {
            break;
        }
        out.push(c);
    }
    (!out.trim().is_empty()).then_some(out)
}

/// `agents` as a report may carry them: each name 1 to `MAX_AGENT_NAME`
/// bytes of printable ASCII, else the entry is dropped; the first of each
/// name only; at most `MAX_AGENTS`; a version that is not 1 to
/// `MAX_AGENT_VERSION` of `[0-9A-Za-z.+-]` dropped; the note bounded
/// (`bounded_note`).
pub fn bound_agents(agents: Vec<AgentInfo>) -> Vec<AgentInfo> {
    let mut out: Vec<AgentInfo> = Vec::new();
    for agent in agents {
        if out.len() == MAX_AGENTS {
            break;
        }
        let name_ok = !agent.agent.is_empty()
            && agent.agent.len() <= MAX_AGENT_NAME
            && agent.agent.chars().all(|c| c.is_ascii_graphic());
        if !name_ok || out.iter().any(|a| a.agent == agent.agent) {
            continue;
        }
        out.push(AgentInfo {
            adapter_version: agent.adapter_version.filter(|v| is_readable_version(v)),
            note: agent.note.as_deref().and_then(bounded_note),
            ..agent
        });
    }
    out
}

/// A report's agents, read leniently, as `Capabilities` is: an entry this
/// build cannot read (a newer host's value, a missing field) is skipped,
/// never a reason to refuse the whole frame. An absent field is an empty
/// list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AgentList(pub Vec<AgentInfo>);

impl<'de> Deserialize<'de> for AgentList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: Vec<Value> = Deserialize::deserialize(deserializer)?;
        Ok(Self(
            raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
        ))
    }
}

/// A report's runtime, read leniently: one this build cannot read is
/// absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct MaybeRuntime(pub Option<RuntimeInfo>);

impl MaybeRuntime {
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

impl<'de> Deserialize<'de> for MaybeRuntime {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: Option<Value> = Deserialize::deserialize(deserializer)?;
        Ok(Self(raw.and_then(|v| serde_json::from_value(v).ok())))
    }
}
