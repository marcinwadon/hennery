//! The caps on what items hold (plan 4a-i decision 10; the security
//! review's A-5). The item stream keeps a session's current group in
//! memory, and a page holds its groups' items: both are bounded by these.
//! Everything cut is still whole on the raw events route.

use serde_json::Value;

/// A message, a thought, a tool's output, a diff's side, a prompt's text
/// block: each at most this many bytes.
pub const TEXT_MAX_BYTES: usize = 256 * 1024;
/// A tool's input, as JSON text.
pub const INPUT_MAX_BYTES: usize = 64 * 1024;
/// An image's base64 data in a tool's content.
pub const IMAGE_MAX_BYTES: usize = 1024 * 1024;
/// An `unrecognised` item's JSON text, an unknown content block's.
pub const RAW_MAX_BYTES: usize = 16 * 1024;
/// A marker's text: a stderr tail, an error, a note, a conflicting body.
pub const NOTE_MAX_BYTES: usize = 16 * 1024;
/// A title, a name, a label, a hint, a question's message, a plan step, a
/// path.
pub const LABEL_MAX_BYTES: usize = 4 * 1024;
/// A machine word: an update kind, a status, a tool kind, a reason code.
pub const CODE_MAX_BYTES: usize = 128;
/// An id that becomes part of an item id or an answer: a `toolCallId`, a
/// `pending_id`, an option id.
pub const ID_MAX_BYTES: usize = 256;
/// The images a tool's content may carry: what a client may show inline
/// from a `data:` URL. Any other type's data is left out.
pub const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// A tool call's content blocks and locations; past them, cut.
pub const CONTENT_MAX_BLOCKS: usize = 64;
pub const LOCATIONS_MAX: usize = 64;
/// A plan's steps; past them, cut.
pub const PLAN_MAX_ENTRIES: usize = 256;
/// A form's fields and a field's options; past either, the form is one the
/// card cannot fill (decline or cancel only).
pub const FIELDS_MAX: usize = 64;
pub const FIELD_OPTIONS_MAX: usize = 64;
/// A permission's options; past them, none is offered ("cannot be answered
/// here"), so no option is ever hidden from among the others.
pub const PERMISSION_OPTIONS_MAX: usize = 16;

/// A tool call, all its fields together; past it, its last content
/// blocks are left out.
pub const TOOL_MAX_BYTES: usize = 2 * 1024 * 1024;
/// A question's parsed request; past it, an elicitation keeps its message
/// and no fields (decline or cancel only).
pub const QUESTION_MAX_BYTES: usize = 256 * 1024;

/// The items one group makes, questions apart; and its questions.
pub const GROUP_MAX_ITEMS: usize = 2000;
pub const GROUP_MAX_QUESTIONS: usize = 64;
/// What one group's items weigh together (`fold::weight`).
pub const GROUP_MAX_BYTES: usize = 16 * 1024 * 1024;

/// `s`'s longest start of at most `max` bytes, never cut inside a
/// character, and whether anything was cut.
pub fn cut(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// `cut` without the flag, for fields with no `truncated` of their own.
pub fn clip(s: &str, max: usize) -> String {
    cut(s, max).0
}

/// `value` as JSON text, cut at `max` bytes.
pub fn json_text(value: &Value, max: usize) -> (String, bool) {
    cut(&value.to_string(), max)
}

/// Append `more` to `text`, keeping it within `max` bytes: what was
/// appended, and whether anything was cut.
pub fn append(text: &mut String, more: &str, max: usize) -> (usize, bool) {
    let room = max.saturating_sub(text.len());
    let (kept, cut_off) = cut(more, room);
    text.push_str(&kept);
    (kept.len(), cut_off)
}

/// The bytes of every string in `value`, keys included: what it weighs
/// held in memory, near enough, without writing it out.
pub fn value_weight(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Array(items) => items.iter().map(value_weight).sum::<usize>() + items.len(),
        Value::Object(fields) => fields.iter().map(|(k, v)| k.len() + value_weight(v)).sum(),
        _ => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_never_splits_a_character() {
        assert_eq!(cut("abc", 3), ("abc".to_string(), false));
        // "é" is two bytes: a cut at 2 keeps "a" alone.
        assert_eq!(cut("a\u{e9}b", 2), ("a".to_string(), true));
        let mut text = "ab".to_string();
        assert_eq!(append(&mut text, "c\u{e9}", 4), (1, true));
        assert_eq!(text, "abc");
        assert_eq!(append(&mut String::new(), "", 0), (0, false));
    }

    /// A cap landing at every byte inside a 3-byte and a 4-byte character,
    /// with a byte before it and with nothing before it (the cut keeps
    /// nothing): the character is left out whole.
    #[test]
    fn a_cut_inside_a_3_or_4_byte_character_leaves_it_out() {
        for c in ["漢", "🦀"] {
            for inside in 1..c.len() {
                assert_eq!(cut(c, inside), (String::new(), true), "{c} at {inside}");
                let s = format!("a{c}b");
                assert_eq!(cut(&s, 1 + inside), ("a".to_string(), true), "{c} at {inside}");
                assert_eq!(clip(&s, 1 + inside), "a");
                // `append` with the room ending inside it: nothing of it.
                let mut text = "a".to_string();
                assert_eq!(append(&mut text, c, 1 + inside), (0, true), "{c} at {inside}");
                assert_eq!(text, "a");
                let mut text = "a".to_string();
                assert_eq!(append(&mut text, &format!("b{c}"), 2 + inside), (1, true));
                assert_eq!(text, "ab");
            }
            // At its end, it is kept.
            assert_eq!(cut(&format!("a{c}b"), 1 + c.len()), (format!("a{c}"), true));
            // No room left: nothing is appended.
            let mut text = "abc".to_string();
            assert_eq!(append(&mut text, c, 2), (0, true));
            assert_eq!(append(&mut text, c, 3), (0, true));
            assert_eq!(text, "abc");
        }
    }
}
