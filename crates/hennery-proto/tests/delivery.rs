//! Plan 8e: the one mapping from a host's isolation to what a session gets
//! (decision E8), each outcome on its own, and the stdio servers' wire
//! types, whose `Debug`s never show an argument or a value.

use hennery_proto::frames::McpIsolation;
use hennery_proto::rest::{
    HostItem, McpAgentDelivery, McpSessionDelivery, McpSessionDeliveryMode, McpStdioEnvInput, McpStdioEnvItem,
    McpStdioServerInput, McpStdioServerItem, McpStdioServersRequest, mcp_session_delivery,
};
use serde_json::json;

#[test]
fn claude_strict_is_isolated() {
    assert_eq!(McpAgentDelivery::of(McpIsolation::ClaudeStrict), McpAgentDelivery::Isolated);
}

#[test]
fn no_isolation_is_default_hat_only() {
    assert_eq!(McpAgentDelivery::of(McpIsolation::None), McpAgentDelivery::DefaultHatOnly);
}

#[test]
fn a_host_without_the_capability_gets_nothing_whatever_it_isolates() {
    for isolation in [McpIsolation::ClaudeStrict, McpIsolation::None] {
        for (mixed, default) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(
                mcp_session_delivery(false, isolation, mixed, default),
                McpSessionDeliveryMode::Unsupported
            );
        }
    }
}

#[test]
fn an_isolated_agent_gets_its_hats_servers_in_every_hat() {
    for (mixed, default) in [(false, false), (false, true), (true, false), (true, true)] {
        assert_eq!(
            mcp_session_delivery(true, McpIsolation::ClaudeStrict, mixed, default),
            McpSessionDeliveryMode::Isolated
        );
    }
}

/// The default hat of a host that cannot isolate the agent: waived, mixed
/// (the fallback's default hat) or not (a single-hat host).
#[test]
fn the_default_hat_is_unisolated_on_a_host_that_cannot_isolate() {
    for mixed in [false, true] {
        assert_eq!(
            mcp_session_delivery(true, McpIsolation::None, mixed, true),
            McpSessionDeliveryMode::Unisolated
        );
    }
}

#[test]
fn another_hat_on_a_mixed_host_falls_back() {
    assert_eq!(
        mcp_session_delivery(true, McpIsolation::None, true, false),
        McpSessionDeliveryMode::Fallback
    );
}

/// Unreachable from the store (the session counts itself), and conservative.
#[test]
fn another_hat_on_an_unmixed_host_gets_nothing_too() {
    assert_eq!(
        mcp_session_delivery(true, McpIsolation::None, false, false),
        McpSessionDeliveryMode::Fallback
    );
}

#[test]
fn only_isolated_and_unisolated_deliver() {
    let delivers: Vec<bool> = [
        McpSessionDeliveryMode::Isolated,
        McpSessionDeliveryMode::Unisolated,
        McpSessionDeliveryMode::Fallback,
        McpSessionDeliveryMode::Unsupported,
    ]
    .map(McpSessionDeliveryMode::delivers)
    .into();
    assert_eq!(delivers, [true, true, false, false]);
}

#[test]
fn a_mode_reads_back_as_written_and_as_on_the_wire() {
    for mode in [
        McpSessionDeliveryMode::Isolated,
        McpSessionDeliveryMode::Unisolated,
        McpSessionDeliveryMode::Fallback,
        McpSessionDeliveryMode::Unsupported,
    ] {
        assert_eq!(McpSessionDeliveryMode::parse(mode.as_str()), Some(mode));
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(mode.as_str()));
    }
    assert_eq!(McpSessionDeliveryMode::parse("everything"), None);
}

#[test]
fn a_session_delivery_is_a_mode_a_count_and_a_time() {
    let delivery = McpSessionDelivery {
        mode: McpSessionDeliveryMode::Fallback,
        servers: 0,
        at: "2026-10-16T12:00:00Z".into(),
    };
    assert_eq!(
        serde_json::to_value(&delivery).unwrap(),
        json!({"mode": "fallback", "servers": 0, "at": "2026-10-16T12:00:00Z"})
    );
}

#[test]
fn a_host_without_a_recorded_delivery_leaves_it_out() {
    let item = HostItem {
        host_id: "h".into(),
        name: "h".into(),
        platform: "linux".into(),
        host_version: "1".into(),
        capabilities: Default::default(),
        default_hat_id: "hat-1".into(),
        workspace_roots: vec![],
        connected: false,
        created_at: "2026-10-16T12:00:00Z".into(),
        last_seen_at: None,
        revoked_at: None,
        mcp_delivery: None,
    };
    let value = serde_json::to_value(&item).unwrap();
    assert!(value.get("mcp_delivery").is_none(), "{value}");
    let item = HostItem {
        mcp_delivery: Some([("claude".to_string(), McpAgentDelivery::Isolated)].into()),
        ..item
    };
    assert_eq!(
        serde_json::to_value(&item).unwrap()["mcp_delivery"],
        json!({"claude": "isolated"})
    );
}

const ARG: &str = "--key=arg-secret-0123";
const VALUE: &str = "env-secret-0123";

#[test]
fn stdio_debugs_show_no_argument_and_no_value() {
    let input = McpStdioServersRequest {
        servers: vec![McpStdioServerInput {
            name: "files".into(),
            command: "files-mcp".into(),
            args: vec![ARG.into()],
            env: vec![McpStdioEnvInput {
                name: "KEY".into(),
                value: Some(VALUE.into()),
            }],
        }],
    };
    let item = McpStdioServerItem {
        name: "files".into(),
        command: "files-mcp".into(),
        args: vec![ARG.into()],
        env: vec![McpStdioEnvItem {
            name: "KEY".into(),
            has_value: true,
        }],
        created_at: String::new(),
        updated_at: String::new(),
    };
    let env = McpStdioEnvInput {
        name: "KEY".into(),
        value: Some(VALUE.into()),
    };
    for shown in [
        format!("{input:?} {input:#?}"),
        format!("{item:?} {item:#?}"),
        format!("{env:?}"),
    ] {
        assert!(!shown.contains(ARG) && !shown.contains(VALUE), "{shown}");
        assert!(shown.contains("files") || shown.contains("KEY"), "{shown}");
    }
}

#[test]
fn a_put_refuses_an_unknown_field_and_keeps_an_absent_or_null_value() {
    let body = json!({"servers": [{"name": "files", "command": "c", "env": [{"name": "A"}, {"name": "B", "value": null}, {"name": "C", "value": ""}]}]});
    let parsed: McpStdioServersRequest = serde_json::from_value(body).unwrap();
    let values: Vec<Option<&str>> = parsed.servers[0].env.iter().map(|e| e.value.as_deref()).collect();
    assert_eq!(values, [None, None, Some("")]);
    assert!(parsed.servers[0].args.is_empty());
    let unknown = json!({"servers": [{"name": "files", "command": "c", "cwd": "/"}]});
    assert!(serde_json::from_value::<McpStdioServersRequest>(unknown).is_err());
}
