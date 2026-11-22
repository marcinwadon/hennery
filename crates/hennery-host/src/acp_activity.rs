//! Conservative host admission and parking state from ACP v2 snapshots.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Final updates can precede the first background snapshot naming an entity.
/// Keep a bounded cache so ordinary foreground tool traffic cannot grow it
/// indefinitely. Eviction is conservative: missing evidence blocks parking.
const EARLY_FINAL_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Identity {
    Terminal(String),
    Tool(String),
}

#[derive(Debug, Default)]
struct Work {
    listed: bool,
    final_seen: bool,
}

#[derive(Debug, Default)]
pub struct Activity {
    foreground_busy: bool,
    known: BTreeMap<Identity, Work>,
    future_busy: bool,
    early_finals: BTreeSet<Identity>,
    early_final_order: VecDeque<Identity>,
}

impl Activity {
    /// Accept a raw session/update envelope. A missing or malformed background
    /// list cannot establish that previously observed work finished.
    pub fn observe(&mut self, payload: &Value) {
        let Some(update) = payload.get("update") else { return };
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("terminal_update") if update.get("exitStatus").is_some_and(Value::is_object) => {
                if let Some(id) = update
                    .get("terminalId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    self.completed(Identity::Terminal(id.into()));
                }
                return;
            }
            Some("tool_call_update")
                if matches!(
                    update.get("status").and_then(Value::as_str),
                    Some("completed" | "failed" | "cancelled")
                ) =>
            {
                if let Some(id) = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    self.completed(Identity::Tool(id.into()));
                }
                return;
            }
            Some("state_update") => {}
            _ => return,
        }
        self.foreground_busy = update.get("state").and_then(Value::as_str) != Some("idle");
        if let Some(items) = update.get("background").and_then(Value::as_array) {
            let mut listed = BTreeSet::new();
            let mut future_busy = false;
            // Validate the entire snapshot before changing membership. A known
            // variant without its required identity cannot erase prior work.
            for item in items {
                let Some(kind) = item.get("type").and_then(Value::as_str).filter(|kind| !kind.is_empty()) else {
                    return;
                };
                let (field, terminal) = match kind {
                    "terminal" => ("terminalId", true),
                    "tool_call" => ("toolCallId", false),
                    _ => {
                        future_busy = true;
                        continue;
                    }
                };
                let Some(id) = item.get(field).and_then(Value::as_str).filter(|id| !id.is_empty()) else {
                    return;
                };
                listed.insert(if terminal {
                    Identity::Terminal(id.into())
                } else {
                    Identity::Tool(id.into())
                });
            }
            for (id, work) in &mut self.known {
                work.listed = listed.contains(id);
            }
            for id in listed {
                let final_seen = self.early_finals.contains(&id);
                self.known.entry(id).or_insert(Work {
                    listed: true,
                    final_seen,
                });
            }
            self.known.retain(|_, work| work.listed || !work.final_seen);
            self.future_busy = future_busy;
        }
    }

    fn completed(&mut self, id: Identity) {
        if self.early_finals.insert(id.clone()) {
            self.early_final_order.push_back(id.clone());
            if self.early_final_order.len() > EARLY_FINAL_LIMIT
                && let Some(oldest) = self.early_final_order.pop_front()
            {
                self.early_finals.remove(&oldest);
            }
        }
        if let Some(work) = self.known.get_mut(&id) {
            work.final_seen = true;
            if !work.listed {
                self.known.remove(&id);
            }
        }
    }

    pub fn foreground_busy(&self) -> bool {
        self.foreground_busy
    }

    pub fn blocks_parking(&self) -> bool {
        self.foreground_busy || !self.known.is_empty() || self.future_busy
    }
}
