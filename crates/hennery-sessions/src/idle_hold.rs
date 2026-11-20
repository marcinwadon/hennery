//! Automatic parking holds. Snapshot and synchronous handoff retain the owner
//! gate, including after COMMIT, so an old release cannot overtake a new hold.
use crate::{hub::Hub, store::Store};
use anyhow::Result;
use hennery_proto::{frames::CollectorFrame, rest::IdleHoldState};
use rusqlite::{Connection, OptionalExtension, params};
use std::sync::{Arc, Weak};

const STATE: &str = "SELECT s.idle_pinned,
 EXISTS(SELECT 1 FROM session_families f JOIN sessions c ON c.owner_id = ?1 AND c.id=f.child_id
        WHERE f.owner_id = ?1 AND f.parent_id=s.id AND c.lifecycle NOT IN ('parked','closed','archived','failed','deleted')),
 EXISTS(SELECT 1 FROM delegation_notices n WHERE n.owner_id = ?1 AND n.parent_id=s.id
        AND n.state IN ('parent_queued','waiting_config','awaiting_reconciliation'))
 FROM sessions s WHERE s.owner_id = ?1 AND s.id=?2 AND s.lifecycle<>'deleted'
 AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)";

fn state_in(conn: &Connection, owner: &str, id: &str) -> Result<Option<IdleHoldState>> {
    Ok(conn
        .query_row(STATE, params![owner, id], |r| {
            let pinned = r.get::<_, bool>(0)?;
            let family_active = r.get::<_, bool>(1)?;
            let notices_pending = r.get::<_, bool>(2)?;
            Ok(IdleHoldState {
                pinned,
                family_active,
                notices_pending,
                held: pinned || family_active || notices_pending,
            })
        })
        .optional()?)
}

/// The ordinary coordinated guard, with committed hold invalidations flushed
/// before releasing its owner gate. Transactions/statements borrow this guard
/// and therefore finish first. Reads do not run a flush.
pub(crate) struct Guard<'a> {
    pub(crate) inner: hennery_kernel::db::ConnectionGuard<'a>,
    pub(crate) store: &'a Store,
    pub(crate) changes: u64,
}
impl std::ops::Deref for Guard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.inner
    }
}
impl std::ops::DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.inner
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.inner.is_autocommit() && self.inner.total_changes() != self.changes {
            let hub = self
                .store
                .idle_hub
                .read()
                .ok()
                .and_then(|hub| hub.as_ref().and_then(Weak::upgrade));
            if let Some(hub) = hub
                && let Err(error) = self.store.flush_idle_holds_in(&self.inner, &hub)
            {
                tracing::warn!(%error, "idle holds deferred until recovery flush");
            }
        }
    }
}

impl Store {
    pub fn set_idle_hub(&self, hub: &Arc<Hub>) {
        *self.idle_hub.write().expect("idle hub lock") = Some(Arc::downgrade(hub));
    }

    pub fn idle_hold(&self, id: &str) -> Result<Option<IdleHoldState>> {
        state_in(&self.conn(), self.owner_id(), id)
    }

    pub fn set_idle_pin(&self, id: &str, pinned: bool) -> Result<Option<IdleHoldState>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if state_in(&tx, self.owner_id(), id)?.is_none() {
            return Ok(None);
        }
        tx.execute(
            "UPDATE sessions SET idle_pinned=?3 WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted'",
            params![self.owner_id(), id, pinned],
        )?;
        tx.commit()?;
        state_in(&conn, self.owner_id(), id)
    }

    pub fn flush_idle_holds(&self, hub: &Hub) -> Result<()> {
        self.flush_idle_holds_in(&self.conn(), hub)
    }

    pub(crate) fn flush_idle_holds_in(&self, conn: &Connection, hub: &Hub) -> Result<()> {
        anyhow::ensure!(conn.is_autocommit(), "idle hold handoff requires committed state");
        loop {
            let rows = {
                let mut stmt = conn.prepare(
                    "SELECT s.id,s.host_id FROM sessions s WHERE s.owner_id = ?1
                AND s.idle_hold_dirty=1 AND s.lifecycle<>'deleted'
                AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)
                ORDER BY s.id LIMIT 128",
                )?;
                stmt.query_map([self.owner_id()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            if rows.is_empty() {
                break;
            }
            for (id, host) in rows {
                if let Some(state) = state_in(conn, self.owner_id(), &id)? {
                    // Offline/old hosts keep their existing behavior. Every ready
                    // handshake restores ALL current states, including false.
                    hub.notify(
                        &host,
                        CollectorFrame::SetIdleHold {
                            session_id: id.clone(),
                            held: state.held,
                        },
                    );
                    conn.execute(
                        "UPDATE sessions SET idle_hold_dirty=0 WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted'",
                        params![self.owner_id(), id],
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn resend_idle_holds(&self, host: &str, hub: &Hub) -> Result<()> {
        let conn = self.conn();
        // Snapshot/send all this host's sessions, not a bounded dirty page:
        // readiness must not leave session 129 at its stale pre-reconnect value.
        let rows = {
            let mut stmt = conn.prepare(
                "SELECT id FROM sessions WHERE owner_id = ?1 AND host_id=?2 AND lifecycle<>'deleted'
                AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=sessions.hat_id)
                ORDER BY id",
            )?;
            stmt.query_map(params![self.owner_id(), host], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in rows {
            if let Some(state) = state_in(&conn, self.owner_id(), &id)? {
                hub.notify(
                    host,
                    CollectorFrame::SetIdleHold {
                        session_id: id,
                        held: state.held,
                    },
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_proto::frames::{AgentIsolation, Capabilities, Capability};

    #[test]
    fn deleted_and_purged_sessions_scrub_idle_pin_and_dirty_state() {
        for purge in [true, false] {
            let store = Store::open_in_memory().unwrap();
            store
                .create_session("s", "offline", "fake", "/project", "hat", None)
                .unwrap();
            store
                .ingest(
                    "s",
                    1,
                    &hennery_proto::frames::SessionBody::session_started("start", "agent"),
                )
                .unwrap();
            store.set_idle_pin("s", true).unwrap().unwrap();
            store
                .ingest("s", 2, &hennery_proto::frames::SessionBody::SessionClosed)
                .unwrap();
            let deletion = if purge {
                store
                    .conn()
                    .execute(
                        "INSERT INTO purged_hats(hat_id,owner_id,purged_at) VALUES ('hat',?1,0)",
                        [store.owner_id()],
                    )
                    .unwrap();
                store.delete_session_owing_checkpoint("s", None).unwrap()
            } else {
                store.delete_session("s", None).unwrap()
            };
            assert!(matches!(deletion, crate::store::Deletion::Done { .. }));
            assert!(store.idle_hold("s").unwrap().is_none());
            let bits: (bool, bool) = store
                .conn()
                .query_row(
                    "SELECT idle_pinned,idle_hold_dirty FROM sessions WHERE owner_id = ?1 AND id='s'",
                    [store.owner_id()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(bits, (false, false), "obsolete idle state retained; purge={purge}");
        }
    }

    fn connected() -> (Store, Arc<Hub>, tokio::sync::mpsc::UnboundedReceiver<CollectorFrame>) {
        let store = Store::open_in_memory().unwrap();
        let hub = Arc::new(Hub::new());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let registration = hub
            .register(
                "ready",
                tx,
                Capabilities(vec![Capability::IdleHold]),
                AgentIsolation::default(),
            )
            .unwrap();
        hub.mark_ready("ready", registration.conn_id);
        (store, hub, rx)
    }

    #[test]
    fn park_hold_flush_and_reconnect_cover_more_than_one_page_and_skip_offline_rows() {
        let (store, hub, mut rx) = connected();
        for n in 0..260 {
            store
                .create_session(
                    &format!("{n:03}"),
                    if n < 130 { "offline" } else { "ready" },
                    "fake",
                    "/project",
                    "hat",
                    None,
                )
                .unwrap();
        }
        store.set_idle_hub(&hub);
        store.flush_idle_holds(&hub).unwrap();
        let mut count = 0;
        while let Ok(frame) = rx.try_recv() {
            assert!(matches!(frame, CollectorFrame::SetIdleHold { held: false, .. }));
            count += 1;
        }
        assert_eq!(count, 130, "offline rows cannot starve the next ready page");
        store.set_idle_pin("259", true).unwrap();
        assert!(matches!(
            rx.try_recv().unwrap(),
            CollectorFrame::SetIdleHold { held: true, .. }
        ));
        store.resend_idle_holds("ready", &hub).unwrap();
        let mut count = 0;
        let mut last = None;
        while let Ok(frame) = rx.try_recv() {
            count += 1;
            last = Some(frame);
        }
        assert_eq!(count, 130);
        assert!(
            matches!(last, Some(CollectorFrame::SetIdleHold { held: true, .. })),
            "session129 must not retain a stale release"
        );
    }

    #[test]
    fn park_hold_rollback_never_sends_transient_state_and_commit_flushes_under_gate() {
        let (store, hub, mut rx) = connected();
        store
            .create_session("s", "ready", "fake", "/project", "hat", None)
            .unwrap();
        store.set_idle_hub(&hub);
        store.flush_idle_holds(&hub).unwrap();
        rx.try_recv().unwrap();
        {
            let mut conn = store.conn();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "UPDATE sessions SET idle_pinned=1 WHERE owner_id=?1 AND id='s'",
                [store.owner_id()],
            )
            .unwrap();
            assert!(
                store.flush_idle_holds_in(&tx, &hub).is_err(),
                "uncommitted pins must not be sent"
            );
            tx.rollback().unwrap();
        }
        assert!(rx.try_recv().is_err());
        assert!(!store.idle_hold("s").unwrap().unwrap().pinned);
        store.set_idle_pin("s", true).unwrap();
        assert!(matches!(
            rx.try_recv().unwrap(),
            CollectorFrame::SetIdleHold { held: true, .. }
        ));
        assert!(rx.try_recv().is_err(), "one committed pin produces one frame");
    }
}
