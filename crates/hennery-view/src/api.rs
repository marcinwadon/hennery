//! The view API's wire types (client view spec §4): what the collector's
//! view routes answer and stream, around the items.
//!
//! Both streams name a position as `<epoch>:<revision>`, the epoch being
//! `<FOLD_VERSION>.<boot>`: `boot` is made once per collector process, so a
//! restart, a restored backup or a new fold each force one resync (plan
//! 4a-ii; the review's A-1). Making `boot` is the collector's: these types
//! read no clock and no random source.
//!
//! The item stream's SSE events: `item` (an `Item`), `item_removed`
//! (`ItemRemoved`, reserved), `catalog_changed` (the protocol's
//! `SessionCatalog`, whole, after a burst's items, when one of its events
//! changed the catalogue), `session_removed` (`SessionRemoved`, last) and
//! `resync_required` (`{}`). The list stream's: `session_upsert` (a
//! `SessionSummary`), `session_removed`, `waiting_changed`
//! (`WaitingChanged`: after every resume, and whenever the count changes,
//! a burst's last message) and `resync_required`.
//!
//! Any message that ends a burst carries the id `<epoch>:<cursor>`; no
//! other carries one, and every id a stream sends resumes it.

use crate::FOLD_VERSION;
use crate::item::Item;
use hennery_proto::rest::{SessionItem, StoredBlock};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// `GET /api/view/sessions/{id}`: whole groups of a session's items,
/// oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemPage {
    pub items: Vec<Item>,
    /// The session's greatest event id when the page was read: no item
    /// here is from a later event. The item stream resumes after it.
    #[ts(type = "number")]
    pub revision: i64,
    /// The collector's epoch: `?after=<epoch>:<revision>` on the item stream.
    pub epoch: String,
    /// Groups exist before the page's first: `before_turn=<its turn_id>`
    /// gets them.
    pub older: bool,
}

/// One session in the view's list: the list item, bounded, and whether a
/// question waits for the operator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionSummary {
    #[serde(flatten)]
    pub session: SessionItem,
    /// An open question: the agent waits on it, in a turn or outside one,
    /// whether or not an answer is queued (a question outside a turn sets
    /// no `blocked` activity).
    pub question_waits: bool,
}

/// `GET /api/view/sessions`: a page of summaries, as `GET /api/sessions`
/// pages its items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SummaryPage {
    pub sessions: Vec<SessionSummary>,
    /// Where the next page starts, for `cursor` (opaque); absent on the
    /// last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub next_cursor: Option<String>,
    /// The owner's greatest event id, read before the page: the list
    /// stream resumes after it.
    #[ts(type = "number")]
    pub revision: i64,
    /// The collector's epoch: `?after=<epoch>:<revision>` on the list stream.
    pub epoch: String,
    /// How many of the owner's sessions wait for the operator: `blocked`,
    /// or with an open question (`question_waits`). Within `hat` when one
    /// is given; whatever the cursor, `q` and `lifecycle`.
    pub waiting: u32,
}

/// SSE `waiting_changed`'s data, on the list stream: `SummaryPage`'s
/// `waiting`, within the stream's `?hat=`, as of the burst it ends. Sent
/// after every resume, then whenever it changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WaitingChanged {
    pub count: u32,
}

/// SSE `item_removed`'s data. Reserved: the fold removes nothing yet, and
/// a client must still handle it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ItemRemoved {
    pub id: String,
}

/// SSE `session_removed`'s data: the session was deleted (plan 9a). On the
/// item stream it is the last message, with no id; on the list stream it
/// carries `<epoch>:<cursor>` only when it ends a burst, as any message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionRemoved {
    pub session_id: String,
}

/// `GET /api/view/sessions/{id}/turns/{turn_id}`: the prompt of a turn
/// that was not delivered (a `turn_not_delivered` marker's `about_turn`),
/// so a client can offer to send it again. A turn that was delivered has
/// its `user_turn` item instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnContent {
    pub turn_id: String,
    /// The prompt as it was stored, whole (4a-ii's security review, B-4):
    /// sent again, it is the prompt the operator wrote. Images are the
    /// owner's attachments, by `sha256`.
    pub content: Vec<StoredBlock>,
}

/// The epoch of a collector process whose boot token is `boot`.
pub fn epoch(boot: &str) -> String {
    format!("{FOLD_VERSION}.{boot}")
}

/// A stream position, `<epoch>:<revision>`, as its epoch and revision.
/// Split at the last `:`; `None` for anything else, a negative revision
/// included.
pub fn parse_anchor(anchor: &str) -> Option<(&str, i64)> {
    let (epoch, revision) = anchor.rsplit_once(':')?;
    let revision: i64 = revision.parse().ok()?;
    (!epoch.is_empty() && revision >= 0).then_some((epoch, revision))
}

/// The position `<epoch>:<revision>`, as the streams send it.
pub fn anchor(epoch: &str, revision: i64) -> String {
    format!("{epoch}:{revision}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_round_trips_and_anything_else_is_none() {
        let epoch = epoch("00ff00ff00ff00ff");
        assert_eq!(epoch, format!("{FOLD_VERSION}.00ff00ff00ff00ff"));
        assert_eq!(parse_anchor(&anchor(&epoch, 42)), Some((epoch.as_str(), 42)));
        // Split at the last `:`.
        assert_eq!(parse_anchor("a:b:7"), Some(("a:b", 7)));
        for bad in ["", "7", ":7", "e:", "e:x", "e:-1", "e:1.5", "e: 1"] {
            assert_eq!(parse_anchor(bad), None, "{bad:?}");
        }
    }

    /// A summary is the list item's fields and `question_waits`, on one
    /// level.
    #[test]
    fn a_summary_is_the_list_item_flattened() {
        let session = SessionItem {
            session_id: "s".into(),
            host_id: "h".into(),
            agent: "fake".into(),
            cwd: "/tmp".into(),
            hat_id: String::new(),
            title: None,
            lifecycle: "active".into(),
            activity: None,
            failure_reason: None,
            presumed_parked: false,
            git_branch: None,
            git_dirty: None,
            model: None,
            mode: None,
            created_at: "2026-10-02T08:00:00.000Z".into(),
            last_event_at: "2026-10-02T08:00:00.000Z".into(),
        };
        let summary = SessionSummary {
            session: session.clone(),
            question_waits: true,
        };
        let mut expected = serde_json::to_value(&session).unwrap();
        expected["question_waits"] = true.into();
        let value = serde_json::to_value(&summary).unwrap();
        assert_eq!(value, expected);
        assert_eq!(serde_json::from_value::<SessionSummary>(value).unwrap(), summary);
    }
}
