use hennery_proto::frames::{Capabilities, Capability, McpDelivery, McpIsolation};
use hennery_proto::mcp_source::{McpSourcePolicy, McpSourceSnapshot};

#[test]
fn explicit_empty_policy_roundtrips_and_omission_remains_legacy() {
    let value = McpDelivery {
        source_policy: Some(McpSourceSnapshot {
            policy: McpSourcePolicy::ManagedOnly,
            revision: 2,
            request_id: "attach".into(),
        }),
        ..Default::default()
    };
    let json = serde_json::to_value(&value).unwrap();
    assert!(json.get("mcp_servers").is_none());
    assert!(json.get("isolation_waived").is_none());
    assert_eq!(serde_json::from_value::<McpDelivery>(json).unwrap(), value);
    assert!(
        serde_json::from_str::<McpDelivery>("{}")
            .unwrap()
            .source_policy
            .is_none()
    );
    assert!(
        serde_json::from_str::<McpDelivery>(r#"{"source_policy":{"policy":"future","revision":0,"request_id":"a"}}"#)
            .is_err()
    );
}

#[test]
fn capability_never_promotes_an_unverified_profile_or_native_sources_by_omission() {
    let caps = Capabilities(vec![Capability::McpServers, Capability::McpSourcePolicy]);
    assert!(McpSourcePolicy::ManagedOnly.supported(&caps, McpIsolation::ClaudeStrict));
    assert!(!McpSourcePolicy::ManagedOnly.supported(&caps, McpIsolation::None));
    assert!(!McpSourcePolicy::ManagedPlusNative.supported(&caps, McpIsolation::ClaudeStrict));
    let mut native = caps;
    native.0.push(Capability::McpNativeSources);
    assert!(McpSourcePolicy::ManagedPlusNative.supported(&native, McpIsolation::ClaudeStrict));
    assert!(!McpSourcePolicy::ManagedPlusNative.supported(&native, McpIsolation::None));
}
