//! Extract only the standard usage and the verified bundled adapters' quota contract.
use crate::{adapter::AgentCommand, profile::Profile};
use agent_client_protocol::schema::v1::PromptResponse;
use hennery_proto::usage::{CacheRetention, ModelUsage, TokenUsage, TurnUsage};
use serde_json::Value;

pub fn retention(command: &mut AgentCommand, profile: Profile, requested: CacheRetention) -> Result<(), &'static str> {
    if requested.is_inherit() {
        return Ok(());
    }
    if profile != Profile::Claude {
        return Err("cache retention is unavailable for this runtime");
    }
    // Only the main conversation. Do not mutate global settings or the subagent policy.
    command.env.push((
        "CLAUDE_CODE_PROMPT_CACHE_TTL".into(),
        requested.env_value().unwrap().into(),
    ));
    Ok(())
}

fn count(value: &Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64().filter(|n| *n <= TokenUsage::MAX)
}
fn standard(value: &Value) -> Option<TokenUsage> {
    let tokens = TokenUsage {
        input_tokens: count(value, "inputTokens")?,
        output_tokens: count(value, "outputTokens")?,
        total_tokens: count(value, "totalTokens")?,
        cache_read_tokens: count(value, "cachedReadTokens"),
        cache_write_tokens: count(value, "cachedWriteTokens"),
    };
    tokens.valid().then_some(tokens)
}
fn quota(value: &Value) -> Option<TokenUsage> {
    let tokens = TokenUsage {
        input_tokens: count(value, "inputTokens")?,
        output_tokens: count(value, "outputTokens")?,
        total_tokens: count(value, "totalTokens")?,
        cache_read_tokens: count(value, "cachedInputTokens"),
        cache_write_tokens: count(value, "cachedWriteTokens").or_else(|| count(value, "cacheWriteInputTokens")),
    };
    tokens.valid().then_some(tokens)
}
pub fn from_response(response: &PromptResponse, profile: Profile) -> Option<Box<TurnUsage>> {
    let raw = serde_json::to_value(response).ok()?;
    let tokens = raw.get("usage").and_then(standard)?;
    let verified = matches!(profile, Profile::Claude | Profile::Codex);
    let models = if verified {
        raw.pointer("/_meta/quota/model_usage")
            .and_then(Value::as_array)
            .filter(|rows| rows.len() <= 32)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let model = row.get("model")?.as_str()?;
                if model.is_empty() || model.len() > 128 || model.chars().any(char::is_control) {
                    return None;
                }
                Some(ModelUsage {
                    model: model.into(),
                    tokens: quota(row.get("token_count")?)?,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(Box::new(TurnUsage {
        source: match profile {
            Profile::Claude => "claude",
            Profile::Codex => "codex",
            _ => "acp",
        }
        .into(),
        tokens,
        models,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn preserves_unknown_counts_and_rejects_invalid_or_foreign_model_metadata() {
        let raw = json!({"stopReason":"cancelled", "usage":{"inputTokens":10,"outputTokens":5,"totalTokens":35,"cachedReadTokens":20},
            "_meta":{"quota":{"model_usage":[{"model":"test","token_count":{"inputTokens":10,"cachedInputTokens":20,"cachedWriteTokens":0,"outputTokens":5,"totalTokens":35}}]}}});
        let response = serde_json::from_value(raw.clone()).unwrap();
        let usage = from_response(&response, Profile::Claude).unwrap();
        assert_eq!(usage.tokens.cache_write_tokens, None);
        assert_eq!(usage.models.len(), 1);
        assert!(from_response(&response, Profile::Generic).unwrap().models.is_empty());
        let mut invalid = raw;
        invalid["usage"]["totalTokens"] = json!(1);
        assert!(from_response(&serde_json::from_value(invalid).unwrap(), Profile::Claude).is_none());
    }
    #[test]
    fn retention_only_overrides_the_main_conversation_of_bundled_claude() {
        let mut cmd = AgentCommand::parse("test").unwrap();
        assert!(retention(&mut cmd, Profile::Codex, CacheRetention::OneHour).is_err());
        assert!(cmd.env.is_empty());
        retention(&mut cmd, Profile::Claude, CacheRetention::OneHour).unwrap();
        assert_eq!(cmd.env, [("CLAUDE_CODE_PROMPT_CACHE_TTL".into(), "1h".into())]);
    }
}
