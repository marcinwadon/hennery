//! Bounded, normalized runtime observations. These are not billing receipts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum CacheRetention {
    #[default]
    #[serde(rename = "inherit")]
    Inherit,
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheRetention {
    pub fn is_inherit(&self) -> bool {
        *self == Self::Inherit
    }
    pub fn env_value(self) -> Option<&'static str> {
        match self {
            Self::Inherit => None,
            Self::FiveMinutes => Some("5m"),
            Self::OneHour => Some("1h"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TokenUsage {
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    #[ts(type = "number")]
    pub total_tokens: u64,
    #[ts(type = "number | null")]
    pub cache_read_tokens: Option<u64>,
    #[ts(type = "number | null")]
    pub cache_write_tokens: Option<u64>,
}

impl TokenUsage {
    pub const MAX: u64 = 9_007_199_254_740_991;
    pub fn valid(&self) -> bool {
        let parts = [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens.unwrap_or(0),
            self.cache_write_tokens.unwrap_or(0),
        ];
        self.total_tokens <= Self::MAX
            && parts
                .into_iter()
                .try_fold(0u64, u64::checked_add)
                .is_some_and(|n| n <= self.total_tokens)
    }
    /// Unknown cache categories stay unknown across aggregation.
    pub fn add(&mut self, other: &Self) -> bool {
        let exact = self
            .total_tokens
            .checked_add(other.total_tokens)
            .is_some_and(|n| n <= Self::MAX);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens).min(Self::MAX);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens).min(Self::MAX);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens).min(Self::MAX);
        self.cache_read_tokens = self
            .cache_read_tokens
            .zip(other.cache_read_tokens)
            .map(|(a, b)| a.saturating_add(b).min(Self::MAX));
        self.cache_write_tokens = self
            .cache_write_tokens
            .zip(other.cache_write_tokens)
            .map(|(a, b)| a.saturating_add(b).min(Self::MAX));
        exact
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ModelUsage {
    pub model: String,
    pub tokens: TokenUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnUsage {
    /// Adapter profile, not an asserted billing identity.
    pub source: String,
    pub tokens: TokenUsage,
    pub models: Vec<ModelUsage>,
}
impl TurnUsage {
    pub fn valid(&self) -> bool {
        matches!(self.source.as_str(), "claude" | "codex" | "acp")
            && self.tokens.valid()
            && self.models.len() <= 32
            && self
                .models
                .iter()
                .all(|m| !m.model.is_empty() && m.model.len() <= 128 && m.tokens.valid())
            && self
                .models
                .iter()
                .map(|m| &m.model)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.models.len()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ContextUsage {
    #[ts(type = "number")]
    pub used: u64,
    #[ts(type = "number")]
    pub size: u64,
    pub observed_at: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ReportedCost {
    pub amount: f64,
    pub currency: String,
    pub observed_at: String,
    /// A runtime's cumulative reading; whole-session coverage is not promised.
    pub partial: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UsageTurn {
    pub turn_id: String,
    pub outcome: String,
    pub observed_at: String,
    pub usage: Option<TurnUsage>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UsageTotals {
    pub tokens: Option<TokenUsage>,
    pub models: Vec<ModelUsage>,
    #[ts(type = "number")]
    pub turns: u64,
    #[ts(type = "number")]
    pub reported_turns: u64,
    pub since: Option<String>,
    pub cost: Option<ReportedCost>,
    pub partial: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct FamilyUsage {
    pub totals: UsageTotals,
    pub sessions: usize,
    pub reporting_sessions: usize,
    pub cost_reporting_sessions: usize,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct RetentionState {
    pub supported: bool,
    pub requested: CacheRetention,
    pub attached_request: Option<CacheRetention>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionUsage {
    pub session_id: String,
    pub source: String,
    pub context: Option<ContextUsage>,
    pub last_turn: Option<UsageTurn>,
    pub session: UsageTotals,
    pub family: FamilyUsage,
    pub retention: RetentionState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SetCacheRetention {
    pub requested: CacheRetention,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregation_preserves_unknown_categories_and_marks_wire_limit_overflow() {
        let mut total = TokenUsage {
            input_tokens: TokenUsage::MAX,
            total_tokens: TokenUsage::MAX,
            cache_read_tokens: Some(0),
            cache_write_tokens: Some(0),
            ..Default::default()
        };
        let next = TokenUsage {
            input_tokens: 1,
            total_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: Some(0),
            ..Default::default()
        };
        assert!(!total.add(&next));
        assert_eq!(total.total_tokens, TokenUsage::MAX);
        assert_eq!(total.cache_read_tokens, None);
        assert_eq!(total.cache_write_tokens, Some(0));
    }

    #[test]
    fn inconsistent_counts_and_duplicate_model_rows_are_rejected() {
        let tokens = TokenUsage {
            input_tokens: 10,
            total_tokens: 9,
            ..Default::default()
        };
        assert!(!tokens.valid());
        let row = ModelUsage {
            model: "model".into(),
            tokens: TokenUsage::default(),
        };
        let report = TurnUsage {
            source: "claude".into(),
            tokens: TokenUsage::default(),
            models: vec![row.clone(), row],
        };
        assert!(!report.valid());
    }
}
