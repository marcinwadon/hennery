//! Plan 4d-B1-i: a host's agents on the wire, in `hello` and in the answer
//! to `probe_agents`, and how both ends bound a report.

use hennery_proto::agents::{
    AgentAuth, AgentCli, AgentInfo, AgentList, MAX_AGENT_NOTE, MAX_AGENTS, MaybeRuntime, RuntimeInfo, RuntimeSource,
    bound_agents, bounded_note,
};
use hennery_proto::frames::{Capability, CollectorFrame, HostFrame};
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

fn hello(extra: serde_json::Value) -> HostFrame {
    let mut v = json!({
        "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
        "proof": "p", "attached_sessions": []
    });
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

#[test]
fn probe_agents_and_its_answer_use_the_plan_field_names() {
    let request = CollectorFrame::ProbeAgents { request_id: "r".into() };
    let wire = json!({"type": "probe_agents", "request_id": "r"});
    assert_eq!(serde_json::to_value(&request).unwrap(), wire);
    assert_eq!(serde_json::from_value::<CollectorFrame>(wire).unwrap(), request);
    assert_eq!(request.probe_capability(), Ok(Some(Capability::ProbeAgents)));
    assert_eq!(
        serde_json::to_value(Capability::ProbeAgents).unwrap(),
        json!("probe_agents")
    );

    let answer = HostFrame::Agents {
        request_id: "r".into(),
        agents: AgentList(vec![AgentInfo {
            auth: AgentAuth::Missing,
            adapter_version: Some("0.31.0".into()),
            images: Some(false),
            note: Some("a note".into()),
            ..agent("claude")
        }]),
        runtime: MaybeRuntime(Some(RuntimeInfo {
            source: RuntimeSource::Managed,
            set_id: Some("abc123".into()),
            pinned: Some(true),
            held: Some(false),
        })),
    };
    let wire = json!({
        "type": "agents", "request_id": "r",
        "agents": [{
            "agent": "claude", "available": true, "auth": "missing", "cli": "bundled",
            "adapter_version": "0.31.0", "images": false, "note": "a note"
        }],
        "runtime": {"source": "managed", "set_id": "abc123", "pinned": true, "held": false}
    });
    assert_eq!(serde_json::to_value(&answer).unwrap(), wire);
    assert_eq!(serde_json::from_value::<HostFrame>(wire).unwrap(), answer);
    assert_eq!(answer.probe_request_id(), Some("r"));
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

/// An older host sends neither field; a newer one may send values this
/// build does not know. Neither refuses the `hello`: an entry that cannot
/// be read is skipped, and a runtime that cannot be read is absent.
#[test]
fn a_hello_reads_its_agents_leniently() {
    let HostFrame::Hello { agents, runtime, .. } = hello(json!({})) else {
        panic!("not a hello")
    };
    assert!(agents.0.is_empty());
    assert_eq!(runtime.0, None);

    let HostFrame::Hello { agents, runtime, .. } = hello(json!({
        "agents": [
            {"agent": "claude", "available": true, "auth": "unknown", "cli": "bundled", "adapter_version": "0.31.0"},
            {"agent": "codex", "available": true, "auth": "expired", "cli": "bundled"},
            {"agent": "gemini", "available": true, "cli": "borrowed"},
            "not an agent",
            {"agent": "fake", "available": false, "cli": "given", "note": "n"}
        ],
        "runtime": {"source": "managed", "set_id": "abc", "pinned": false, "held": true}
    })) else {
        panic!("not a hello")
    };
    let names: Vec<&str> = agents.0.iter().map(|a| a.agent.as_str()).collect();
    assert_eq!(names, ["claude", "fake"]);
    assert_eq!(
        runtime.0,
        Some(RuntimeInfo {
            source: RuntimeSource::Managed,
            set_id: Some("abc".into()),
            pinned: Some(false),
            held: Some(true),
        })
    );

    let HostFrame::Hello { runtime, .. } = hello(json!({"runtime": {"source": "borrowed"}})) else {
        panic!("not a hello")
    };
    assert_eq!(runtime.0, None);
    // A list that is not a list is a malformed frame, as for every field.
    let wrong = json!({
        "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
        "proof": "p", "attached_sessions": [], "agents": "claude"
    });
    assert!(serde_json::from_value::<HostFrame>(wrong).is_err());
}

/// The probe's answer is read as leniently as `hello`.
#[test]
fn an_agents_answer_reads_its_agents_leniently() {
    let answer: HostFrame = serde_json::from_value(json!({
        "type": "agents", "request_id": "r",
        "agents": [{"agent": "claude", "available": true, "auth": "expired", "cli": "bundled"}, {"agent": "codex", "available": false, "cli": "override"}],
        "runtime": {"source": "elsewhere"}
    }))
    .unwrap();
    let HostFrame::Agents { agents, runtime, .. } = answer else {
        panic!("not an agents answer")
    };
    assert_eq!(
        agents.0,
        [AgentInfo {
            available: false,
            cli: AgentCli::Override,
            ..agent("codex")
        }]
    );
    assert_eq!(runtime.0, None);
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
