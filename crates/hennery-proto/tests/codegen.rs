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

/// Every `$ref` in the document must resolve to a key in the document's
/// top-level `$defs`. A `$ref` into a nested (per-type) `$defs` object, or
/// one that names a definition that only exists nested, is a schema that no
/// compliant JSON Schema validator can actually use.
#[test]
fn every_ref_resolves_to_a_top_level_definition() {
    let schema: serde_json::Value = serde_json::from_str(&render_schema()).unwrap();
    let defs = schema
        .get("$defs")
        .and_then(serde_json::Value::as_object)
        .expect("document has a top-level $defs object");

    let mut unresolved = Vec::new();
    collect_unresolved_refs(&schema, defs, &mut unresolved);

    assert!(unresolved.is_empty(), "unresolved $refs: {unresolved:?}");
}

fn collect_unresolved_refs(
    value: &serde_json::Value,
    defs: &serde_json::Map<String, serde_json::Value>,
    unresolved: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(r) = map.get("$ref").and_then(serde_json::Value::as_str) {
                match r.strip_prefix("#/$defs/") {
                    Some(name) if defs.contains_key(name) => {}
                    _ => unresolved.push(r.to_string()),
                }
            }
            for v in map.values() {
                collect_unresolved_refs(v, defs, unresolved);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_unresolved_refs(v, defs, unresolved);
            }
        }
        _ => {}
    }
}
