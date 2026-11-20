//! Hook sets (skills spec §8): the typed `hooks` object of a Claude Code
//! `settings.json`, limited to command handlers, with the rules the
//! collector and the host share: validation, per-agent rendering and merging.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Every Claude Code hook event hennery accepts in a hook set.
pub const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "Setup",
    "SessionEnd",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "Stop",
    "StopFailure",
    "PreToolUse",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "TeammateIdle",
    "ConfigChange",
    "InstructionsLoaded",
    "CwdChanged",
    "DirectoryAdded",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "PreCompact",
    "PostCompact",
    "PreModelSwitch",
    "PostModelSwitch",
    "Notification",
    "MessageDisplay",
    "Elicitation",
    "ElicitationResult",
];

/// The events Codex runs; a hook set is rendered for Codex with only these.
pub const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
];

/// Most events one hook set may name.
pub const MAX_EVENTS: usize = 32;
/// Most matcher groups under one event.
pub const MAX_GROUPS: usize = 16;
/// Most handlers in one matcher group.
pub const MAX_HANDLERS: usize = 8;
/// Longest `matcher`, in characters.
pub const MAX_MATCHER: usize = 256;
/// Longest `command`, in characters.
pub const MAX_COMMAND: usize = 4096;
/// Longest `timeout`, in seconds.
pub const MAX_TIMEOUT: u32 = 600;

/// The handler type. Only `command` exists: any other `type` value is a
/// deserialisation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum HookKind {
    #[serde(rename = "command")]
    Command,
}

/// One command handler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CommandHook {
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub kind: HookKind,
    /// The program to run; it must already exist on the host.
    pub command: String,
    /// Seconds, 1 to 600, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub timeout: Option<u32>,
}

/// A matcher and the handlers it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct MatcherGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub matcher: Option<String>,
    pub hooks: Vec<CommandHook>,
}

/// A hook set's `hooks` object: event name to matcher groups.
///
/// A newtype, so it is transparent on the wire and in the schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HookSetConfig(#[ts(type = "Record<string, MatcherGroup[]>")] pub BTreeMap<String, Vec<MatcherGroup>>);

/// Why a hook set was refused; the `Display` text is the 400 message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSetError {
    /// More than 32 events.
    TooManyEvents,
    /// The event is not a Claude Code event name.
    UnknownEvent(String),
    /// More than 16 groups under the event.
    TooManyGroups(String),
    /// More than 8 handlers in a group of the event.
    TooManyHandlers(String),
    /// A matcher of the event is over 256 characters.
    MatcherTooLong(String),
    /// A command of the event is empty.
    CommandEmpty(String),
    /// A command of the event is over 4096 characters.
    CommandTooLong(String),
    /// A timeout of the event is outside 1 to 600 seconds.
    TimeoutOutOfRange(String),
}

impl std::fmt::Display for HookSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyEvents => write!(f, "a hook set names at most {MAX_EVENTS} events"),
            Self::UnknownEvent(e) => write!(f, "unknown hook event {e:?}"),
            Self::TooManyGroups(e) => write!(f, "{e}: at most {MAX_GROUPS} matcher groups"),
            Self::TooManyHandlers(e) => write!(f, "{e}: at most {MAX_HANDLERS} handlers per group"),
            Self::MatcherTooLong(e) => write!(f, "{e}: a matcher is at most {MAX_MATCHER} characters"),
            Self::CommandEmpty(e) => write!(f, "{e}: a command must not be empty"),
            Self::CommandTooLong(e) => write!(f, "{e}: a command is at most {MAX_COMMAND} characters"),
            Self::TimeoutOutOfRange(e) => write!(f, "{e}: a timeout is 1 to {MAX_TIMEOUT} seconds"),
        }
    }
}

impl std::error::Error for HookSetError {}

/// Checks a parsed hook set against the limits; the first problem found, in
/// event order, is the error.
pub fn validate(config: &HookSetConfig) -> Result<(), HookSetError> {
    if config.0.len() > MAX_EVENTS {
        return Err(HookSetError::TooManyEvents);
    }
    for (event, groups) in &config.0 {
        if !CLAUDE_EVENTS.contains(&event.as_str()) {
            return Err(HookSetError::UnknownEvent(event.clone()));
        }
        if groups.len() > MAX_GROUPS {
            return Err(HookSetError::TooManyGroups(event.clone()));
        }
        for group in groups {
            if group.hooks.len() > MAX_HANDLERS {
                return Err(HookSetError::TooManyHandlers(event.clone()));
            }
            if group.matcher.as_ref().is_some_and(|m| m.chars().count() > MAX_MATCHER) {
                return Err(HookSetError::MatcherTooLong(event.clone()));
            }
            for hook in &group.hooks {
                match hook.command.chars().count() {
                    0 => return Err(HookSetError::CommandEmpty(event.clone())),
                    n if n > MAX_COMMAND => return Err(HookSetError::CommandTooLong(event.clone())),
                    _ => {}
                }
                if hook.timeout.is_some_and(|t| !(1..=MAX_TIMEOUT).contains(&t)) {
                    return Err(HookSetError::TimeoutOutOfRange(event.clone()));
                }
            }
        }
    }
    Ok(())
}

/// The config with only the listed events kept (`CODEX_EVENTS` for Codex;
/// Claude takes every event).
pub fn render(config: &HookSetConfig, events: &[&str]) -> HookSetConfig {
    HookSetConfig(
        config
            .0
            .iter()
            .filter(|(event, _)| events.contains(&event.as_str()))
            .map(|(event, groups)| (event.clone(), groups.clone()))
            .collect(),
    )
}

/// Merges sets event by event, appending groups in input order.
pub fn merge<'a>(sets: impl IntoIterator<Item = &'a HookSetConfig>) -> HookSetConfig {
    let mut merged = BTreeMap::<String, Vec<MatcherGroup>>::new();
    for set in sets {
        for (event, groups) in &set.0 {
            merged.entry(event.clone()).or_default().extend(groups.iter().cloned());
        }
    }
    HookSetConfig(merged)
}

/// The events a config names, sorted.
pub fn events_of(config: &HookSetConfig) -> Vec<String> {
    config.0.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(command: &str, timeout: Option<u32>) -> CommandHook {
        CommandHook {
            kind: HookKind::Command,
            command: command.into(),
            timeout,
        }
    }

    fn group(matcher: Option<&str>, hooks: Vec<CommandHook>) -> MatcherGroup {
        MatcherGroup {
            matcher: matcher.map(str::to_string),
            hooks,
        }
    }

    fn one(event: &str, g: MatcherGroup) -> HookSetConfig {
        HookSetConfig(BTreeMap::from([(event.to_string(), vec![g])]))
    }

    #[test]
    fn a_command_hook_set_parses_and_validates() {
        let c: HookSetConfig = serde_json::from_str(
            r#"{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/x.sh","timeout":10}]}]}"#,
        )
        .unwrap();
        assert!(validate(&c).is_ok());
        assert_eq!(events_of(&c), vec!["PreToolUse"]);
    }

    #[test]
    fn other_handler_types_and_unknown_fields_do_not_parse() {
        for bad in [
            r#"{"Stop":[{"hooks":[{"type":"prompt","prompt":"x"}]}]}"#,
            r#"{"Stop":[{"hooks":[{"type":"command","command":"x","if":"Bash(*)"}]}]}"#,
            r#"{"Stop":[{"hooks":[],"extra":1}]}"#,
        ] {
            assert!(serde_json::from_str::<HookSetConfig>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn validation_enforces_the_limits() {
        let ok = group(None, vec![hook("x", None)]);
        assert_eq!(
            validate(&one("Nope", ok.clone())),
            Err(HookSetError::UnknownEvent("Nope".into()))
        );
        let many = HookSetConfig(
            CLAUDE_EVENTS
                .iter()
                .map(|e| (e.to_string(), vec![ok.clone()]))
                .collect(),
        );
        assert_eq!(many.0.len(), 33);
        assert_eq!(validate(&many), Err(HookSetError::TooManyEvents));
        let stop = |g| one("Stop", g);
        assert_eq!(
            validate(&HookSetConfig(BTreeMap::from([(
                "Stop".to_string(),
                vec![ok.clone(); 17]
            )]))),
            Err(HookSetError::TooManyGroups("Stop".into()))
        );
        assert_eq!(
            validate(&stop(group(None, vec![hook("x", None); 9]))),
            Err(HookSetError::TooManyHandlers("Stop".into()))
        );
        let matcher = |n| stop(group(Some(&"m".repeat(n)), vec![hook("x", None)]));
        assert!(validate(&matcher(256)).is_ok());
        assert_eq!(
            validate(&matcher(257)),
            Err(HookSetError::MatcherTooLong("Stop".into()))
        );
        let command = |c: &str| stop(group(None, vec![hook(c, None)]));
        assert_eq!(validate(&command("")), Err(HookSetError::CommandEmpty("Stop".into())));
        assert!(validate(&command(&"c".repeat(4096))).is_ok());
        assert_eq!(
            validate(&command(&"c".repeat(4097))),
            Err(HookSetError::CommandTooLong("Stop".into()))
        );
        let timeout = |t| stop(group(None, vec![hook("x", Some(t))]));
        assert!(validate(&timeout(1)).is_ok());
        assert!(validate(&timeout(600)).is_ok());
        for t in [0, 601] {
            assert_eq!(
                validate(&timeout(t)),
                Err(HookSetError::TimeoutOutOfRange("Stop".into()))
            );
        }
    }

    #[test]
    fn render_keeps_only_codex_events_and_merge_appends_groups() {
        let a = group(Some("a"), vec![hook("a", None)]);
        let b = group(Some("b"), vec![hook("b", None)]);
        let mut both = one("Stop", a.clone());
        both.0.insert("PreToolUse".into(), vec![a.clone()]);
        let rendered = render(&both, CODEX_EVENTS);
        assert_eq!(events_of(&rendered), vec!["PreToolUse"]);
        assert_eq!(events_of(&render(&both, CLAUDE_EVENTS)), vec!["PreToolUse", "Stop"]);
        let merged = merge([&one("PreToolUse", a.clone()), &one("PreToolUse", b.clone())]);
        assert_eq!(merged.0["PreToolUse"], vec![a, b]);
    }

    #[test]
    fn the_codex_events_are_claude_events() {
        assert!(CODEX_EVENTS.iter().all(|e| CLAUDE_EVENTS.contains(e)));
    }
}
