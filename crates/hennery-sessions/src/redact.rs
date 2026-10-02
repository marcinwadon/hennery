//! A session token the agent printed (plan 8e decision 11; the maintainer's
//! open question Q2 of plan 8c, default chosen, reversible): what hennery
//! stores of a session's ACP payloads, and what it forwards of them to the
//! timeline and its stream, has every session token in it redacted (ACP
//! core §8: tokens are never in events or SSE). The rest of the payload is
//! kept as the host sent it (§2.3).
//!
//! By shape, not by value: the gateway keeps only a token's hash, so the
//! collector cannot know the plaintext of a session's earlier tokens. Every
//! run that starts with `hnry_session_` (in any case) and goes on with at
//! least 8 hexadecimal digits is replaced whole, whoever's token it is: a
//! token cut short or upper-cased is still most of a secret. What this does
//! not catch: a token split across two updates (streamed chunks), or
//! encoded otherwise (base64, spaced out). Those reach the timeline as the
//! agent sent them; the token stops working at the session's next park.

use anyhow::{Context, Result};
use hennery_gateway::tokens::SESSION_TOKEN_PREFIX;
use hennery_proto::frames::SessionBody;
use serde_json::Value;

/// What a redacted token reads as.
pub const REDACTED: &str = "hnry_session_<redacted>";

/// The fewest hexadecimal digits after the prefix that count as a token.
const MIN_DIGITS: usize = 8;

/// `text` with every token-shaped run replaced by `REDACTED`; `None` if it
/// has none.
pub fn text(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let prefix = SESSION_TOKEN_PREFIX.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut i = 0;
    while i + prefix.len() <= bytes.len() {
        if bytes[i..i + prefix.len()].eq_ignore_ascii_case(prefix) {
            let digits = bytes[i + prefix.len()..]
                .iter()
                .take_while(|b| b.is_ascii_hexdigit())
                .count();
            if digits >= MIN_DIGITS {
                let end = i + prefix.len() + digits;
                let buffer = out.get_or_insert_with(|| String::with_capacity(text.len()));
                // Both ends are on ASCII bytes: character boundaries.
                buffer.push_str(&text[copied..i]);
                buffer.push_str(REDACTED);
                copied = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.map(|mut buffer| {
        buffer.push_str(&text[copied..]);
        buffer
    })
}

/// `text` as it may be shown: logged, or answered to the operator.
pub fn shown(text: &str) -> std::borrow::Cow<'_, str> {
    match self::text(text) {
        Some(redacted) => std::borrow::Cow::Owned(redacted),
        None => std::borrow::Cow::Borrowed(text),
    }
}

/// Redact every string of `value`, object keys too. True if any changed.
pub fn value(value: &mut Value) -> bool {
    match value {
        Value::String(s) => match text(s) {
            Some(redacted) => {
                *s = redacted;
                true
            }
            None => false,
        },
        // Every item, never stopping at the first: `any` would leave the
        // rest unredacted.
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= self::value(item);
            }
            changed
        }
        Value::Object(map) => {
            let mut changed = false;
            let keys: Vec<String> = map.keys().filter(|key| text(key).is_some()).cloned().collect();
            for key in keys {
                if let Some(mut entry) = map.remove(&key) {
                    self::value(&mut entry);
                    map.insert(text(&key).unwrap_or(key), entry);
                    changed = true;
                }
            }
            for entry in map.values_mut() {
                changed |= self::value(entry);
            }
            changed
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// `body` with its tokens redacted, read back as a body; `None` if it had
/// none, which is the common case and costs one serialisation.
pub fn body(body: &SessionBody) -> Result<Option<SessionBody>> {
    let mut json = serde_json::to_value(body)?;
    if !value(&mut json) {
        return Ok(None);
    }
    // Only strings changed, so the shape is the same; the context names no
    // content.
    serde_json::from_value(json)
        .map(Some)
        .context("a session frame with a token redacted no longer reads")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token() -> String {
        format!("{SESSION_TOKEN_PREFIX}{}", "0a".repeat(32))
    }

    #[test]
    fn a_token_is_redacted_wherever_it_is_in_a_string() {
        let t = token();
        assert_eq!(text(&t).as_deref(), Some(REDACTED));
        assert_eq!(
            text(&format!("Bearer {t}, again {t}.")).as_deref(),
            Some(format!("Bearer {REDACTED}, again {REDACTED}.").as_str())
        );
        assert_eq!(
            text(&format!("é{t}é")).as_deref(),
            Some(format!("é{REDACTED}é").as_str())
        );
    }

    #[test]
    fn a_token_cut_short_or_upper_cased_is_redacted_too() {
        let t = token();
        assert!(text(&t[..SESSION_TOKEN_PREFIX.len() + 8]).is_some());
        assert!(text(&t.to_uppercase()).is_some());
        assert!(text(&format!("{t}ff")).is_some_and(|r| r == REDACTED), "the whole run");
    }

    #[test]
    fn what_is_not_a_token_is_left_alone() {
        for kept in [
            "",
            "hnry_session_",
            "hnry_session_0a0a0a0",
            "hnry_session_<redacted>",
            "a hnry_client_0a0a0a0a0a0a0a0a",
            "plain text",
        ] {
            assert_eq!(text(kept), None, "{kept}");
        }
    }

    #[test]
    fn every_string_of_a_value_is_redacted_keys_too() {
        let t = token();
        let mut v = json!({"a": [t, {"b": format!("x{t}")}], t.clone(): 1, "n": 2});
        assert!(value(&mut v));
        assert!(!v.to_string().contains(&t), "{v}");
        assert_eq!(v["n"], 2);
        let mut clean = json!({"a": ["b"], "c": 1});
        assert!(!value(&mut clean));
    }
}
