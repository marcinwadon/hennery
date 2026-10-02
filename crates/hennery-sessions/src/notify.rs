//! Push triggers (ACP core §10; plan 10b): which edges a fact crosses
//! notify the owner, and with what. The store reports the edge
//! (`store::PushEdge`), from the ingest of a host fact only; this decides,
//! in one place, and the kernel applies the hat's policy and delivers
//! (`hennery_kernel::push`).

use crate::store::{EdgeSession, PushEdge, one_line};
use hennery_kernel::push::{Notice, Urgency};
use hennery_proto::frames::TurnOutcome;
use hennery_proto::rest::{TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES};

/// The notice an edge of `session` gives, `session` as the edge's fact left
/// it (`store::Edge`), or `None` for an edge that does
/// not notify:
///
/// | Edge | Title / body |
/// |---|---|
/// | activity → `blocked` | the session's / "needs your answer" (urgent) |
/// | `turn_ended{completed}` | the session's / "finished" |
/// | `turn_ended{failed}` | the session's / "failed" |
///
/// A cancelled or interrupted turn does not notify: the operator cancelled
/// it, or the host's restart already shows on the session. A question asked
/// **outside a turn** does not notify either, until the maintainer decides
/// whether it should (plan 10b's open question): it leaves the activity
/// alone, so it is not `blocked`. If it ever notifies, it needs a limit in
/// time per session: nothing the operator does paces it (10b-i's review).
pub fn notice_for(edge: &PushEdge, session: &EdgeSession) -> Option<Notice> {
    let (urgency, generic_title, body, detail) = match edge {
        PushEdge::Blocked { title, .. } => (
            Urgency::High,
            "Session needs your answer",
            "needs your answer",
            title.clone(),
        ),
        // The maintainer's open question: no trigger of its own yet.
        PushEdge::QuestionOutsideTurn { .. } => return None,
        PushEdge::TurnEnded(TurnOutcome::Completed) => (Urgency::Normal, "Session finished", "finished", None),
        PushEdge::TurnEnded(TurnOutcome::Failed) => (Urgency::Normal, "Session failed", "failed", None),
        PushEdge::TurnEnded(TurnOutcome::Cancelled | TurnOutcome::Interrupted) => return None,
    };
    Some(Notice {
        hat_id: session.hat_id.clone(),
        urgency,
        title: session_title(session),
        generic_title: generic_title.into(),
        body: body.into(),
        detail,
        url: format!("/sessions/{}", session.id),
        tag: session.id.clone(),
    })
}

/// The session's title, else the name of its project directory, else
/// "Session" (ACP core §10). The directory's name is put on one line as a
/// title is (10b-i's review, A4): it reaches a lock screen.
fn session_title(session: &EdgeSession) -> String {
    session
        .title
        .clone()
        .or_else(|| {
            let name = std::path::Path::new(&session.cwd).file_name()?;
            one_line(&name.to_string_lossy(), TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES)
        })
        .unwrap_or_else(|| "Session".into())
}
