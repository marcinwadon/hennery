//! Tool calls (frontend §6.1): the merge of a call's updates, its output in
//! every known shape, and the fabrication warning.

use crate::cap::{
    CODE_MAX_BYTES, CONTENT_MAX_BLOCKS, ID_MAX_BYTES, IMAGE_MAX_BYTES, IMAGE_TYPES, INPUT_MAX_BYTES, LABEL_MAX_BYTES,
    LOCATIONS_MAX, RAW_MAX_BYTES, TEXT_MAX_BYTES, TOOL_MAX_BYTES, clip, cut, json_text, value_weight,
};
use crate::item::{Location, ToolCall, ToolContent};
use serde_json::Value;

/// Tool-invocation syntax that never belongs in a tool's output (F-13):
/// closed tags only, so the adapter's own `<tool_use_error>` does not trip
/// it. Matched in any case.
pub const FABRICATION_MARKERS: [&str; 8] = [
    "<tool_call>",
    "<tool_response>",
    "<tool_result>",
    "<tool_use>",
    "<bash>",
    "<function_calls>",
    "<server_name>",
    "<invoke ",
];

/// The first fabrication marker in `output`.
pub fn fabrication(output: &str) -> Option<&'static str> {
    let lower = output.to_lowercase();
    FABRICATION_MARKERS.into_iter().find(|m| lower.contains(m))
}

/// A non-empty string field of `value`.
fn text_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}

/// The `text` of every element of an array that has one, joined.
fn joined(parts: &Value) -> String {
    parts
        .as_array()
        .map(|parts| parts.iter().filter_map(|p| p.get("text")?.as_str()).collect())
        .unwrap_or_default()
}

/// A tool's text output, from whichever shape its adapter sent (F-12):
/// `rawOutput` as a string; `rawOutput` as `[{type, text}]`; `content[]`'s
/// text blocks; or the legacy `_meta.claudeCode.toolResponse` as
/// `[{type, text}]`. `None` when none gives any text.
pub fn output(update: &Value) -> Option<String> {
    let raw = update.get("rawOutput");
    if let Some(s) = raw.and_then(Value::as_str).filter(|s| !s.is_empty()) {
        return Some(s.to_string());
    }
    let from_raw = raw.map(joined).unwrap_or_default();
    if !from_raw.is_empty() {
        return Some(from_raw);
    }
    let from_content: String = update
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("content")?.get("text")?.as_str())
                .collect()
        })
        .unwrap_or_default();
    if !from_content.is_empty() {
        return Some(from_content);
    }
    let legacy = update
        .pointer("/_meta/claudeCode/toolResponse")
        .map(joined)
        .unwrap_or_default();
    (!legacy.is_empty()).then_some(legacy)
}

/// One block of a tool call's `content`, and whether a cap cut it.
fn content_block(block: &Value) -> (ToolContent, bool) {
    let other = || {
        let (raw, cut_off) = json_text(block, RAW_MAX_BYTES);
        (ToolContent::Other { raw }, cut_off)
    };
    match block.get("type").and_then(Value::as_str) {
        Some("content") => {
            let Some(inner) = block.get("content") else {
                return other();
            };
            match inner.get("type").and_then(Value::as_str) {
                Some("text") => match inner.get("text").and_then(Value::as_str) {
                    Some(text) => {
                        let (text, cut_off) = cut(text, TEXT_MAX_BYTES);
                        (ToolContent::Text { text }, cut_off)
                    }
                    None => other(),
                },
                Some("image") => {
                    let mime_type = text_of(inner, "mimeType").unwrap_or_default();
                    let data = inner.get("data").and_then(Value::as_str);
                    // Only a type a client may show inline from a `data:`
                    // URL, and only within its cap (the review's A-7).
                    let kept = data.filter(|d| d.len() <= IMAGE_MAX_BYTES && IMAGE_TYPES.contains(&mime_type));
                    let image = ToolContent::Image {
                        mime_type: clip(mime_type, CODE_MAX_BYTES),
                        data: kept.map(str::to_string),
                    };
                    (image, data.is_some() && kept.is_none())
                }
                _ => other(),
            }
        }
        Some("diff") => {
            let (Some(path), Some(new_text)) = (
                block.get("path").and_then(Value::as_str),
                block.get("newText").and_then(Value::as_str),
            ) else {
                return other();
            };
            let (new_text, new_cut) = cut(new_text, TEXT_MAX_BYTES);
            let old = block
                .get("oldText")
                .and_then(Value::as_str)
                .map(|t| cut(t, TEXT_MAX_BYTES));
            let old_cut = old.as_ref().is_some_and(|(_, c)| *c);
            let diff = ToolContent::Diff {
                path: clip(path, LABEL_MAX_BYTES),
                old_text: old.map(|(t, _)| t),
                new_text,
            };
            (diff, new_cut || old_cut)
        }
        Some("terminal") => match block
            .get("terminalId")
            .and_then(Value::as_str)
            .filter(|id| id.len() <= ID_MAX_BYTES)
        {
            Some(id) => (
                ToolContent::Terminal {
                    terminal_id: id.to_string(),
                },
                false,
            ),
            None => other(),
        },
        _ => other(),
    }
}

/// Apply one `tool_call` or `tool_call_update` to `call`. A field changes
/// only when the update carries a value for it, so a sparser update never
/// erases a title, input or output; an empty `rawInput` object is "no input
/// yet". Whether the call changed.
pub fn merge(call: &mut ToolCall, update: &Value) -> bool {
    let before = call.clone();
    if let Some(title) = text_of(update, "title") {
        call.title = Some(clip(title, LABEL_MAX_BYTES));
    }
    if let Some(kind) = text_of(update, "kind") {
        call.tool_kind = Some(clip(kind, CODE_MAX_BYTES));
    }
    if let Some(status) = text_of(update, "status") {
        call.status = Some(clip(status, CODE_MAX_BYTES));
    }
    match update.get("rawInput") {
        None | Some(Value::Null) => {}
        Some(Value::Object(fields)) if fields.is_empty() => {}
        Some(input) => {
            let (text, cut_off) = json_text(input, INPUT_MAX_BYTES);
            call.input = Some(if cut_off { Value::String(text) } else { input.clone() });
            call.truncated |= cut_off;
        }
    }
    if let Some(blocks) = update
        .get("content")
        .and_then(Value::as_array)
        .filter(|b| !b.is_empty())
    {
        call.truncated |= blocks.len() > CONTENT_MAX_BLOCKS;
        call.content = blocks
            .iter()
            .take(CONTENT_MAX_BLOCKS)
            .map(|block| {
                let (block, cut_off) = content_block(block);
                call.truncated |= cut_off;
                block
            })
            .collect();
    }
    if let Some(locations) = update
        .get("locations")
        .and_then(Value::as_array)
        .filter(|l| !l.is_empty())
    {
        call.truncated |= locations.len() > LOCATIONS_MAX;
        call.locations = locations
            .iter()
            .take(LOCATIONS_MAX)
            .filter_map(|l| {
                Some(Location {
                    path: clip(l.get("path")?.as_str()?, LABEL_MAX_BYTES),
                    line: l.get("line").and_then(Value::as_u64),
                })
            })
            .collect();
    }
    if let Some(text) = output(update) {
        let (text, cut_off) = cut(&text, TEXT_MAX_BYTES);
        call.fabricated = fabrication(&text).map(str::to_string);
        call.output = Some(text);
        call.truncated |= cut_off;
    }
    // The whole call within its budget (the re-confirmation's N-1): the
    // last content blocks go first; without them it fits.
    while weight(call) > TOOL_MAX_BYTES && call.content.pop().is_some() {
        call.truncated = true;
    }
    *call != before
}

/// What a tool call weighs held in memory, near enough: its strings' bytes.
pub fn weight(call: &ToolCall) -> usize {
    let opt = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    call.tool_call_id.len()
        + opt(&call.title)
        + opt(&call.tool_kind)
        + opt(&call.status)
        + call.input.as_ref().map_or(0, value_weight)
        + opt(&call.output)
        + opt(&call.fabricated)
        + call.locations.iter().map(|l| l.path.len() + 16).sum::<usize>()
        + call
            .content
            .iter()
            .map(|block| {
                16 + match block {
                    ToolContent::Text { text } => text.len(),
                    ToolContent::Diff {
                        path,
                        old_text,
                        new_text,
                    } => path.len() + opt(old_text) + new_text.len(),
                    ToolContent::Image { mime_type, data } => mime_type.len() + opt(data),
                    ToolContent::Terminal { terminal_id } => terminal_id.len(),
                    ToolContent::Other { raw } => raw.len(),
                }
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_output_shape_gives_its_text() {
        let cases = [
            (json!({"rawOutput": "plain"}), Some("plain")),
            (
                json!({"rawOutput": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]}),
                Some("ab"),
            ),
            (
                json!({"content": [{"type": "content", "content": {"type": "text", "text": "from content"}}]}),
                Some("from content"),
            ),
            (
                json!({"_meta": {"claudeCode": {"toolResponse": [{"type": "text", "text": "legacy"}]}}}),
                Some("legacy"),
            ),
            // The newer adapter's toolResponse is an object: no text, no failure.
            (
                json!({"_meta": {"claudeCode": {"toolResponse": {"type": "create", "filePath": "/x"}}}}),
                None,
            ),
            // A string wins over the content blocks it repeats.
            (
                json!({"rawOutput": "raw", "content": [{"type": "content", "content": {"type": "text", "text": "x"}}]}),
                Some("raw"),
            ),
            (json!({"rawOutput": ""}), None),
            (json!({}), None),
        ];
        for (update, expected) in cases {
            assert_eq!(output(&update).as_deref(), expected, "{update}");
        }
    }

    #[test]
    fn fabrication_needs_a_closed_tag() {
        assert_eq!(fabrication("ran <tool_use> x"), Some("<tool_use>"));
        assert_eq!(fabrication("<FUNCTION_CALLS>"), Some("<function_calls>"));
        assert_eq!(fabrication("<invoke name=\"Bash\">"), Some("<invoke "));
        assert_eq!(fabrication("<tool_use_error>No such tool</tool_use_error>"), None);
        assert_eq!(fabrication("<tool_use"), None);
        assert_eq!(fabrication("<invoke>"), None);
        for marker in FABRICATION_MARKERS {
            assert_eq!(fabrication(&format!("x{marker}y")), Some(marker));
        }
    }

    #[test]
    fn a_sparser_update_erases_nothing() {
        let mut call = ToolCall::default();
        merge(
            &mut call,
            &json!({"title": "Write a.txt", "kind": "edit", "rawInput": {"path": "a.txt"}, "status": "pending",
                    "locations": [{"path": "a.txt", "line": 3}], "rawOutput": "done",
                    "content": [{"type": "content", "content": {"type": "text", "text": "done"}}]}),
        );
        let before = call.clone();
        assert!(!merge(
            &mut call,
            &json!({"title": "", "rawInput": {}, "content": [], "locations": []})
        ));
        assert_eq!(call, before);
        assert!(merge(&mut call, &json!({"status": "completed"})));
        assert_eq!(call.status.as_deref(), Some("completed"));
        assert_eq!(call.input, Some(json!({"path": "a.txt"})));
        assert_eq!(call.output.as_deref(), Some("done"));
        assert_eq!(
            call.locations,
            vec![Location {
                path: "a.txt".into(),
                line: Some(3)
            }]
        );
    }

    #[test]
    fn content_blocks_are_typed_and_capped() {
        let mut call = ToolCall::default();
        let big = "A".repeat(IMAGE_MAX_BYTES + 1);
        merge(
            &mut call,
            &json!({"content": [
                {"type": "content", "content": {"type": "text", "text": "t"}},
                {"type": "diff", "path": "p", "oldText": null, "newText": "n"},
                {"type": "content", "content": {"type": "image", "mimeType": "image/png", "data": "QQ=="}},
                {"type": "content", "content": {"type": "image", "mimeType": "image/png", "data": big}},
                {"type": "content", "content": {"type": "image", "mimeType": "image/svg+xml", "data": "PHN2Zz4="}},
                {"type": "terminal", "terminalId": "term-1"},
                {"type": "audio", "data": "x"},
            ]}),
        );
        assert_eq!(
            call.content,
            vec![
                ToolContent::Text { text: "t".into() },
                ToolContent::Diff {
                    path: "p".into(),
                    old_text: None,
                    new_text: "n".into()
                },
                ToolContent::Image {
                    mime_type: "image/png".into(),
                    data: Some("QQ==".into())
                },
                ToolContent::Image {
                    mime_type: "image/png".into(),
                    data: None
                },
                // Not a type a client shows inline: no data.
                ToolContent::Image {
                    mime_type: "image/svg+xml".into(),
                    data: None
                },
                ToolContent::Terminal {
                    terminal_id: "term-1".into()
                },
                ToolContent::Other {
                    raw: r#"{"type":"audio","data":"x"}"#.into()
                },
            ]
        );
        assert!(call.truncated);
    }

    #[test]
    fn content_and_locations_past_their_count_are_cut() {
        let mut call = ToolCall::default();
        let blocks: Vec<Value> = (0..CONTENT_MAX_BLOCKS + 1)
            .map(|i| json!({"type": "content", "content": {"type": "text", "text": i.to_string()}}))
            .collect();
        let locations: Vec<Value> = (0..LOCATIONS_MAX + 1).map(|i| json!({"path": i.to_string()})).collect();
        merge(&mut call, &json!({"content": blocks, "locations": locations}));
        assert_eq!(call.content.len(), CONTENT_MAX_BLOCKS);
        assert_eq!(call.locations.len(), LOCATIONS_MAX);
        assert!(call.truncated);
    }

    #[test]
    fn a_call_stays_within_its_budget() {
        let mut call = ToolCall::default();
        let image = json!({"type": "content", "content": {"type": "image", "mimeType": "image/png", "data": "A".repeat(IMAGE_MAX_BYTES)}});
        let blocks: Vec<Value> = (0..CONTENT_MAX_BLOCKS).map(|_| image.clone()).collect();
        merge(
            &mut call,
            &json!({"content": blocks, "rawOutput": "o".repeat(TEXT_MAX_BYTES)}),
        );
        assert!(weight(&call) <= TOOL_MAX_BYTES);
        assert_eq!(call.content.len(), 1);
        assert!(call.truncated);
        // A terminal id past its cap is no terminal.
        let mut call = ToolCall::default();
        merge(
            &mut call,
            &json!({"content": [{"type": "terminal", "terminalId": "t".repeat(ID_MAX_BYTES + 1)}]}),
        );
        assert!(matches!(call.content[0], ToolContent::Other { .. }));
    }

    #[test]
    fn an_input_past_its_cap_becomes_the_start_of_its_text() {
        let mut call = ToolCall::default();
        merge(
            &mut call,
            &json!({"rawInput": {"content": "x".repeat(INPUT_MAX_BYTES)}}),
        );
        let Some(Value::String(text)) = &call.input else {
            panic!("a string: {:?}", call.input);
        };
        assert_eq!(text.len(), INPUT_MAX_BYTES);
        assert!(text.starts_with(r#"{"content":"xxx"#));
        assert!(call.truncated);
    }

    #[test]
    fn a_fabricated_output_is_flagged_and_a_later_clean_one_clears_it() {
        let mut call = ToolCall::default();
        merge(&mut call, &json!({"rawOutput": "<tool_result>fake</tool_result>"}));
        assert_eq!(call.fabricated.as_deref(), Some("<tool_result>"));
        merge(&mut call, &json!({"rawOutput": "real"}));
        assert_eq!(call.fabricated, None);
    }
}
