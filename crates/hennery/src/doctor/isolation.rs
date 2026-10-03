//! Check 15 (distribution spec §7, kernel spec §10; plan 4d-B3): the
//! collector holds gateway credentials for more than one hat while it
//! shares its OS user with the agents of `hennery up`'s host. Doctor opens
//! no database: it asks the running collector over its admin socket
//! (`deployment`, read-only), and prints only the verdict and a count.
//! Every way of getting no answer is told by its type (`admin::Why`); none
//! passes for a quiet collector.

use super::collector::{block_on, quoted, served};
use super::{Doctor, Finding, Verdict};
use hennery_kernel::admin::{ADMIN_SOCKET, AdminRequest, AdminResponse, Why};
use hennery_kernel::deployment::{Facts, Isolation, RECOMMENDATION};
use std::path::Path;
use std::time::Duration;

/// How long the collector has to answer.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(5);

const NAME: &str = "collector isolation";

/// Asks the collector in a data directory its `deployment`.
pub type Ask<'a> = &'a dyn Fn(&Path) -> anyhow::Result<AdminResponse>;

/// Ask the collector in `collector` over its admin socket, bounded.
pub fn ask(collector: &Path) -> anyhow::Result<AdminResponse> {
    let socket = collector.join(ADMIN_SOCKET);
    block_on(
        async move { hennery_kernel::admin::request_within(&socket, &AdminRequest::Deployment, ASK_TIMEOUT).await },
    )
}

/// Check 15.
pub fn isolation(doctor: &Doctor) -> Finding {
    isolation_with(doctor, &ask)
}

/// Check 15, asking with `ask`.
pub fn isolation_with(doctor: &Doctor, ask: Ask) -> Finding {
    let Some(collector) = &doctor.dirs.collector else {
        return Finding::NotRun {
            number: 15,
            why: "no collector data directory",
        };
    };
    let mut verdict = Verdict::default();
    match ask(collector) {
        Ok(AdminResponse::Deployment { beside_host, hats }) => match (Facts { beside_host, hats }).isolation() {
            Isolation::SeveralHats => verdict.warn(
                format!(
                    "the collector runs as the OS user of hennery up's agents and holds MCP gateway credentials \
                     for {hats} hats: any of those agents can read them all"
                ),
                RECOMMENDATION,
            ),
            Isolation::OneHat => verdict.ok(format!(
                "the collector runs beside hennery up's host, with MCP gateway credentials for {hats} hat(s): at \
                 most one"
            )),
            Isolation::NotUnderUp => verdict.ok(
                "the collector was not started by hennery up; a host run by hand as its OS user is not detected \
                 (kernel spec §10)",
            ),
        },
        Ok(AdminResponse::Refused { .. }) => verdict.warn(
            "the collector does not know this question",
            "restart the collector after an upgrade",
        ),
        Ok(AdminResponse::Failed { message }) => verdict.warn(
            format!("the collector could not answer: {}", quoted(&message)),
            "see the collector's log",
        ),
        Ok(_) => verdict.warn(
            "the collector gave an answer to another question",
            "restart the collector after an upgrade",
        ),
        Err(err) => {
            let running = served(doctor, collector);
            match hennery_kernel::admin::why(&err) {
                Some(Why::NoSocket | Why::Stale) if !running => {
                    return Finding::NotRun {
                        number: 15,
                        why: "the collector is not running",
                    };
                }
                Some(Why::NoSocket | Why::Stale | Why::TimedOut | Why::Unanswered) if running => verdict.warn(
                    "the collector's service runs but its admin socket does not answer",
                    "check the collector's log, and run doctor again",
                ),
                Some(Why::TimedOut) => verdict.warn(
                    "the collector did not answer in time",
                    "run doctor again once the collector is idle",
                ),
                Some(Why::Unanswered) => verdict.warn(
                    "the collector closed the connection without answering",
                    "run doctor again",
                ),
                Some(Why::PathTooLong) => verdict.warn(
                    "cannot ask the collector: its admin socket's path is too long",
                    "move the data directory to a shorter path",
                ),
                Some(Why::Denied | Why::OtherUser) => verdict.warn(
                    "the collector's admin socket is not this user's",
                    "run doctor as the collector's user",
                ),
                Some(Why::NoSocket | Why::Stale) | None => verdict.warn(
                    format!("cannot ask the collector: {}", quoted(&format!("{err:#}"))),
                    "check the collector's log",
                ),
            }
        }
    }
    Finding::Checked(verdict.check(15, NAME))
}
