use hennery_proto::codegen::{render_schema, render_ts};

#[test]
fn typescript_declares_tagged_unions() {
    let ts = render_ts();
    assert!(ts.contains("export type HostFrame ="), "{ts}");
    assert!(ts.contains("\"type\": \"session\""), "{ts}");
    assert!(!ts.contains("bigint"), "u64 fields must be typed as number: {ts}");
}

#[test]
fn schema_has_a_const_tag_per_variant() {
    let schema: serde_json::Value = serde_json::from_str(&render_schema()).unwrap();
    let host = &schema["$defs"]["HostFrame"];
    let text = host.to_string();
    for tag in ["hello", "session", "error", "resend_complete"] {
        assert!(
            text.contains(&format!("\"const\":\"{tag}\"")),
            "missing tag {tag}: {text}"
        );
    }
}
