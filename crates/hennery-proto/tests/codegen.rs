use hennery_proto::codegen::{SCHEMA_PATH, TS_PATH, render_schema, render_ts};
use std::path::Path;

/// The generated output must not depend on which packages this test binary
/// was built with: `cargo test --workspace` unifies `serde_json`'s
/// `preserve_order` feature across the whole build graph (because
/// `agent-client-protocol-schema` enables it), while `cargo run -p
/// hennery-proto --bin gen` never does. Both must still match the committed
/// files.
#[test]
fn generated_schema_matches_the_checked_in_copy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let committed = std::fs::read_to_string(root.join(SCHEMA_PATH)).expect("read committed schema");
    assert_eq!(
        render_schema(),
        committed,
        "{SCHEMA_PATH} is stale for this build's feature set; run `cargo run -p hennery-proto --bin gen`"
    );
}

#[test]
fn generated_ts_matches_the_checked_in_copy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let committed = std::fs::read_to_string(root.join(TS_PATH)).expect("read committed ts");
    assert_eq!(
        render_ts(),
        committed,
        "{TS_PATH} is stale for this build's feature set; run `cargo run -p hennery-proto --bin gen`"
    );
}

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

#[test]
fn config_fields_are_flat_in_typescript() {
    let ts = render_ts();
    assert!(ts.contains("export type ConfigValue = boolean | string;"), "{ts}");
    let start = ts
        .lines()
        .find(|l| l.starts_with("export type StartSessionRequest ="))
        .expect("StartSessionRequest");
    assert!(start.contains("model?") && start.contains("axes?"), "{start}");
}

/// Plan 6a: the frontend renders stored images from these, so they are
/// exported, with sizes as numbers.
#[test]
fn stored_images_and_their_usage_are_exported() {
    let ts = render_ts();
    // A declaration runs to the next one (doc comments split it in lines).
    let decl = |name: &str| {
        let start = ts.find(&format!("export type {name} =")).expect(name);
        let rest = &ts[start + 1..];
        rest[..rest.find("export type").unwrap_or(rest.len())].to_string()
    };
    let block = decl("StoredBlock");
    assert!(
        block.contains("\"type\": \"image\", mimeType: string,")
            && block.contains("sha256: string")
            && block.contains("size: number"),
        "{block}"
    );
    let usage = decl("AttachmentUsage");
    assert!(
        usage.contains("count: number") && usage.contains("bytes: number"),
        "{usage}"
    );
}
