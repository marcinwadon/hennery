//! Plan 4d-B1-i: a host's agents on the wire, and how both ends bound a
//! report.

use hennery_proto::agents::{
    AgentAuth, AgentCli, AgentInfo, MAX_AGENT_NOTE, MAX_AGENTS, RuntimeInfo, RuntimeSource, bound_agents, bounded_note,
};
use hennery_proto::rest::{AgentsSource, HostAgents};
use serde_json::json;

fn agent(name: &str) -> AgentInfo {
    AgentInfo {
        agent: name.into(),
        available: true,
        auth: AgentAuth::Unknown,
        cli: AgentCli::Bundled,
        adapter_version: None,
        images: None,
        note: None,
    }
}

#[test]
fn every_auth_cli_and_source_value_has_its_wire_name() {
    for (auth, name) in [
        (AgentAuth::Ok, "ok"),
        (AgentAuth::Missing, "missing"),
        (AgentAuth::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(auth).unwrap(), json!(name));
    }
    for (cli, name) in [
        (AgentCli::Bundled, "bundled"),
        (AgentCli::Override, "override"),
        (AgentCli::Given, "given"),
    ] {
        assert_eq!(serde_json::to_value(cli).unwrap(), json!(name));
    }
    for (source, name) in [(RuntimeSource::Managed, "managed"), (RuntimeSource::Given, "given")] {
        assert_eq!(serde_json::to_value(source).unwrap(), json!(name));
    }
    for (source, name) in [
        (AgentsSource::None, "none"),
        (AgentsSource::Hello, "hello"),
        (AgentsSource::Probe, "probe"),
    ] {
        assert_eq!(serde_json::to_value(source).unwrap(), json!(name));
    }
    // An absent `auth` is `unknown`.
    let read: AgentInfo = serde_json::from_value(json!({"agent": "a", "available": false, "cli": "given"})).unwrap();
    assert_eq!(read.auth, AgentAuth::Unknown);
}

#[test]
fn a_report_keeps_each_name_once_and_only_readable_names() {
    let mut second = agent("claude");
    second.available = false;
    let kept = bound_agents(vec![
        agent("claude"),
        second,
        agent(""),
        agent("has space"),
        agent("tab\tname"),
        agent("caf\u{e9}"),
        agent(&"a".repeat(65)),
        agent(&"a".repeat(64)),
    ]);
    let names: Vec<String> = kept.iter().map(|a| a.agent.clone()).collect();
    assert_eq!(names, ["claude".to_string(), "a".repeat(64)]);
    assert!(kept[0].available, "the first of a name is kept");
}

#[test]
fn a_report_holds_at_most_max_agents() {
    let many: Vec<AgentInfo> = (0..MAX_AGENTS + 5).map(|i| agent(&format!("agent-{i}"))).collect();
    let kept = bound_agents(many);
    assert_eq!(kept.len(), MAX_AGENTS);
    assert_eq!(kept.last().unwrap().agent, format!("agent-{}", MAX_AGENTS - 1));
}

#[test]
fn a_report_drops_an_unreadable_version() {
    let with = |v: &str| {
        bound_agents(vec![AgentInfo {
            adapter_version: Some(v.into()),
            ..agent("a")
        }])[0]
            .adapter_version
            .clone()
    };
    assert_eq!(with("0.81.0-beta+1"), Some("0.81.0-beta+1".into()));
    assert_eq!(with("1.2.3 see https://x"), None);
    assert_eq!(with("1_2"), None);
    assert_eq!(with(""), None);
    assert_eq!(with(&"9".repeat(64)), Some("9".repeat(64)));
    assert_eq!(with(&"9".repeat(65)), None);
}

#[test]
fn a_note_is_escaped_and_cut_on_a_character_boundary() {
    assert_eq!(bounded_note("plain"), Some("plain".into()));
    assert_eq!(
        bounded_note("a\nb\u{1b}[31mc\u{202e}d"),
        Some("a\u{fffd}b\u{fffd}[31mc\u{fffd}d".into())
    );
    assert_eq!(bounded_note("   "), None);
    assert_eq!(bounded_note(""), None);
    let long = "é".repeat(MAX_AGENT_NOTE);
    let cut = bounded_note(&long).unwrap();
    assert!(
        cut.len() <= MAX_AGENT_NOTE && cut.len() > MAX_AGENT_NOTE - 2,
        "{}",
        cut.len()
    );
    assert!(cut.chars().all(|c| c == 'é'));
    let kept = bound_agents(vec![AgentInfo {
        note: Some("x\u{0}".into()),
        ..agent("a")
    }]);
    assert_eq!(kept[0].note.as_deref(), Some("x\u{fffd}"));
}

#[test]
fn a_given_runtime_keeps_no_managed_field_and_a_bad_set_id_is_dropped() {
    let given = RuntimeInfo {
        source: RuntimeSource::Given,
        set_id: Some("abc".into()),
        pinned: Some(true),
        held: Some(true),
    };
    assert_eq!(
        given.bounded(),
        RuntimeInfo {
            source: RuntimeSource::Given,
            set_id: None,
            pinned: None,
            held: None,
        }
    );
    let managed = |id: &str| {
        RuntimeInfo {
            source: RuntimeSource::Managed,
            set_id: Some(id.into()),
            pinned: Some(false),
            held: None,
        }
        .bounded()
    };
    assert_eq!(managed("0123abcdef").set_id.as_deref(), Some("0123abcdef"));
    assert_eq!(managed("../etc").set_id, None);
    assert_eq!(managed(&"a".repeat(129)).set_id, None);
    assert_eq!(managed("a").pinned, Some(false));
}

/// Every field at its worst, every agent there: the stored report and the
/// route's answer stay small. `"` is the worst character: JSON escapes it to
/// two bytes, in a name as in a note.
#[test]
fn a_bounded_report_stays_under_32_kib() {
    let worst: Vec<AgentInfo> = (0..MAX_AGENTS + 1)
        .map(|i| AgentInfo {
            agent: format!("{i:0>2}{}", "\"".repeat(62)),
            available: true,
            auth: AgentAuth::Unknown,
            cli: AgentCli::Override,
            adapter_version: Some("9".repeat(64)),
            images: Some(true),
            note: Some("\"".repeat(4 * MAX_AGENT_NOTE)),
        })
        .collect();
    let agents = bound_agents(worst);
    assert_eq!(agents.len(), MAX_AGENTS);
    assert!(agents.iter().all(|a| a.note.as_ref().unwrap().len() == MAX_AGENT_NOTE));
    let answer = HostAgents {
        host_id: format!("host-{}", "f".repeat(16)),
        agents,
        runtime: Some(
            RuntimeInfo {
                source: RuntimeSource::Managed,
                set_id: Some("f".repeat(128)),
                pinned: Some(true),
                held: Some(true),
            }
            .bounded(),
        ),
        reported_at: Some("2026-10-02T00:00:00Z".into()),
        source: AgentsSource::Probe,
        live: true,
    };
    let bytes = serde_json::to_vec(&answer).unwrap().len();
    assert!(bytes > 16 * 1024, "every field at its limit: {bytes}");
    assert!(bytes <= 32 * 1024, "{bytes}");
}
