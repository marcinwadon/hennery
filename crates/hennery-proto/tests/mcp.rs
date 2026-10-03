//! Plan 8c: a session's MCP servers and hat on `start_session` /
//! `resume_session`, and what a host announces it can isolate (ACP core
//! §3.3, §6, §8; gateway spec §3.2, §3.4).

use hennery_proto::frames::{
    AgentIsolation, Capabilities, Capability, CollectorFrame, HostFrame, McpDelivery, McpIsolation, McpServer,
    NameValue,
};
use serde_json::json;

const TOKEN: &str = "hst_0123456789abcdef";

fn http() -> McpServer {
    McpServer::Http {
        name: "hennery-notes".into(),
        url: "https://hennery.example/mcp/notes".into(),
        headers: vec![NameValue::new("Authorization", format!("Bearer {TOKEN}"))],
    }
}

fn stdio() -> McpServer {
    McpServer::Stdio {
        name: "hennery-files".into(),
        command: "/usr/local/bin/files-mcp".into(),
        args: vec!["--root".into(), "/srv/secret-root".into()],
        env: vec![NameValue::new("FILES_KEY", "files-key-0123456789")],
    }
}

fn start(hat_id: &str, mcp: McpDelivery) -> CollectorFrame {
    CollectorFrame::StartSession {
        request_id: "r".into(),
        session_id: "s".into(),
        committed_seq: 0,
        agent: "claude".into(),
        cwd: "/tmp".into(),
        config: Default::default(),
        hat_id: hat_id.into(),
        mcp,
    }
}

#[test]
fn servers_use_the_acp_entry_shapes_with_a_type_tag() {
    assert_eq!(
        serde_json::to_value(http()).unwrap(),
        json!({
            "type": "http", "name": "hennery-notes", "url": "https://hennery.example/mcp/notes",
            "headers": [{"name": "Authorization", "value": format!("Bearer {TOKEN}")}]
        })
    );
    assert_eq!(
        serde_json::to_value(stdio()).unwrap(),
        json!({
            "type": "stdio", "name": "hennery-files", "command": "/usr/local/bin/files-mcp",
            "args": ["--root", "/srv/secret-root"],
            "env": [{"name": "FILES_KEY", "value": "files-key-0123456789"}]
        })
    );
    // Absent lists are empty ones.
    let bare: McpServer = serde_json::from_value(json!({"type": "stdio", "name": "n", "command": "/c"})).unwrap();
    assert_eq!(
        bare,
        McpServer::Stdio {
            name: "n".into(),
            command: "/c".into(),
            args: vec![],
            env: vec![]
        }
    );
}

#[test]
fn start_and_resume_carry_the_hat_and_the_servers() {
    let mcp = McpDelivery {
        mcp_servers: vec![http(), stdio()],
        isolation_waived: false,
    };
    let frame = start("hat-1", mcp.clone());
    let value = serde_json::to_value(&frame).unwrap();
    assert_eq!(value["hat_id"], json!("hat-1"));
    assert_eq!(value["mcp_servers"].as_array().unwrap().len(), 2);
    assert!(value.get("isolation_waived").is_none(), "{value}");
    assert_eq!(serde_json::from_value::<CollectorFrame>(value).unwrap(), frame);
    let resume = CollectorFrame::ResumeSession {
        request_id: "r".into(),
        session_id: "s".into(),
        committed_seq: 3,
        agent: "claude".into(),
        cwd: "/tmp".into(),
        agent_session_id: "a1".into(),
        config: Default::default(),
        hat_id: "hat-1".into(),
        mcp: McpDelivery {
            isolation_waived: true,
            ..mcp
        },
    };
    let value = serde_json::to_value(&resume).unwrap();
    assert_eq!(
        (&value["hat_id"], &value["isolation_waived"]),
        (&json!("hat-1"), &json!(true))
    );
    assert_eq!(serde_json::from_value::<CollectorFrame>(value).unwrap(), resume);
}

/// An older collector sends none of the new fields, and an older host
/// reads none: empty ones are left out, absent ones are empty.
#[test]
fn the_new_fields_are_left_out_when_empty_and_default_when_absent() {
    let value = serde_json::to_value(start("", McpDelivery::default())).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "start_session", "request_id": "r", "session_id": "s", "committed_seq": 0,
            "agent": "claude", "cwd": "/tmp"
        })
    );
    let old: CollectorFrame = serde_json::from_value(json!({
        "type": "resume_session", "request_id": "r", "session_id": "s", "committed_seq": 3,
        "agent": "claude", "cwd": "/tmp", "agent_session_id": "a1"
    }))
    .unwrap();
    let CollectorFrame::ResumeSession { hat_id, mcp, .. } = old else {
        panic!("{old:?}");
    };
    assert_eq!((hat_id.as_str(), mcp), ("", McpDelivery::default()));
    assert_eq!(
        start("", McpDelivery::default()).mcp_delivery(),
        Some(&McpDelivery::default())
    );
}

/// ACP core §8: a frame's `Debug` (logged by `{other:?}` and friends) never
/// shows a header's or an env variable's value, nor a stdio server's
/// arguments.
#[test]
fn debug_never_shows_header_or_env_values_or_arguments() {
    let frame = start(
        "hat-1",
        McpDelivery {
            mcp_servers: vec![http(), stdio()],
            isolation_waived: false,
        },
    );
    let shown = format!("{frame:?} {frame:#?}");
    for secret in [TOKEN, "files-key-0123456789", "/srv/secret-root", "/mcp/notes"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    for visible in [
        "hennery-notes",
        "Authorization",
        "FILES_KEY",
        "\"https://hennery.example\"",
    ] {
        assert!(shown.contains(visible), "{shown}");
    }
    let (http, stdio) = (http(), stdio());
    let secrets: Vec<&str> = http.secret_values().into_iter().chain(stdio.secret_values()).collect();
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(
        secrets,
        [
            bearer.as_str(),
            "/mcp/notes",
            "files-key-0123456789",
            "--root",
            "/srv/secret-root"
        ]
    );
    use hennery_proto::frames::url_secrets;
    assert_eq!(url_secrets("https://u:pw@h.example:1/p?q#f"), ["u:pw", "/p?q#f"]);
    assert_eq!(url_secrets("https://h.example"), Vec::<&str>::new());
    assert_eq!(url_secrets("no scheme"), ["no scheme"]);
    // Read more than one way: all of it is secret.
    for ambiguous in [
        "https://u:ab/cd@h.example/x",
        "https://ab/cd@h.example/x",
        "https://h.example\\@evil/x",
        "https://h.example:8a/x",
    ] {
        assert_eq!(url_secrets(ambiguous), [ambiguous]);
    }
}

/// The gateway lane's L11: an upstream URL shows only as its origin, with
/// a canary in its path, query, fragment and userinfo. A URL that reads
/// more than one way shows as `<redacted>`: a userinfo with a `/` in it
/// (base64), a backslash, a host or port of other characters.
#[test]
fn a_url_shows_only_its_origin() {
    use hennery_proto::frames::url_origin;
    let canary = "canary-0123456789";
    for (url, origin) in [
        (format!("https://h.example/mcp/{canary}"), "https://h.example"),
        (format!("https://h.example:8443/?k={canary}"), "https://h.example:8443"),
        (format!("http://u:{canary}@h.example#{canary}"), "http://h.example"),
        (format!("not a url {canary}"), "<redacted>"),
        (format!("{canary}://h.example/p"), "<redacted>"),
        ("HTTPS://h.example/p".to_string(), "HTTPS://h.example"),
        (format!("https://[::1]:8443/{canary}"), "https://[::1]:8443"),
        (format!("https://u:{canary}/x@h.example/p"), "<redacted>"),
        // A userinfo with a `/` in it, whose first part reads as a host.
        (format!("https://{canary}/rest@h.example/p"), "<redacted>"),
        (format!("https://{canary}\\@h.example/p"), "<redacted>"),
        (format!("https://h.example\\{canary}"), "<redacted>"),
        (format!("https://h.example:{canary}/p"), "<redacted>"),
        (format!("https://h.example{canary}%/p"), "<redacted>"),
        (format!("https://[{canary}]/p"), "<redacted>"),
    ] {
        assert_eq!(url_origin(&url), origin);
        let server = McpServer::Http {
            name: "n".into(),
            url,
            headers: vec![],
        };
        assert!(!format!("{server:?}").contains(canary));
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

/// What a host can isolate is read leniently, like its capabilities: a
/// mechanism this build does not know counts as none (fail closed), and an
/// agent left out is not isolated.
#[test]
fn a_hello_announces_per_agent_isolation_read_leniently() {
    let HostFrame::Hello {
        capabilities,
        mcp_isolation,
        ..
    } = hello(json!({
        "capabilities": ["mcp_servers", "park"],
        "mcp_isolation": {"claude": "claude_strict", "codex": "codex_home_from_the_future", "fake": "none"}
    }))
    else {
        panic!("expected hello");
    };
    assert!(capabilities.has(Capability::McpServers));
    assert_eq!(mcp_isolation.get("claude"), McpIsolation::ClaudeStrict);
    assert_eq!(mcp_isolation.get("codex"), McpIsolation::None);
    assert_eq!(mcp_isolation.get("fake"), McpIsolation::None);
    assert_eq!(mcp_isolation.get("absent"), McpIsolation::None);
    assert!(mcp_isolation.isolates("claude"));
    assert!(!mcp_isolation.isolates("codex") && !mcp_isolation.isolates("absent"));
    // An older host sends no map: nothing is isolated.
    let HostFrame::Hello { mcp_isolation, .. } = hello(json!({})) else {
        panic!("expected hello");
    };
    assert_eq!(mcp_isolation, AgentIsolation::default());
    // Not a map at all: nothing isolated, and the `hello` still reads.
    let HostFrame::Hello { mcp_isolation, .. } = hello(json!({"mcp_isolation": ["claude"]})) else {
        panic!("expected hello");
    };
    assert_eq!(mcp_isolation, AgentIsolation::default());
    let sent = HostFrame::Hello {
        protocol_version: "1.0".into(),
        host_version: "0".into(),
        host_id: "h".into(),
        proof: "p".into(),
        capabilities: Capabilities(vec![Capability::McpServers]),
        mcp_isolation: AgentIsolation(
            [
                ("claude".to_string(), McpIsolation::ClaudeStrict),
                ("codex".to_string(), McpIsolation::None),
            ]
            .into_iter()
            .collect(),
        ),
        workspace_roots: vec![],
        attached_sessions: vec![],
        agents: Default::default(),
        runtime: Default::default(),
    };
    let value = serde_json::to_value(&sent).unwrap();
    assert_eq!(value["capabilities"], json!(["mcp_servers"]));
    assert_eq!(
        value["mcp_isolation"],
        json!({"claude": "claude_strict", "codex": "none"})
    );
}

#[test]
fn only_starts_and_resumes_carry_a_delivery() {
    let prompt = CollectorFrame::Prompt {
        request_id: "r".into(),
        session_id: "s".into(),
        turn_id: "t".into(),
        content: vec![],
    };
    assert_eq!(prompt.mcp_delivery(), None);
    let frame = start(
        "",
        McpDelivery {
            mcp_servers: vec![http()],
            isolation_waived: false,
        },
    );
    assert_eq!(frame.mcp_delivery().unwrap().mcp_servers, [http()]);
    assert_eq!(frame.agent(), Some("claude"));
}
