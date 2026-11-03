//! Owner/hat/agent launcher preferences. All writes recheck authorization,
//! freeze and compare-and-swap revision under the same IMMEDIATE transaction.
use crate::{
    hosts::Hosts,
    operator::{Authenticated, STEP_UP_SECS},
    secret::unix_now,
};
use anyhow::Result;
use hennery_proto::{
    frames::ConfigValue,
    session_defaults::{HatSessionDefaults, PutHatSessionDefaults},
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

/// JavaScript can represent every revision exactly; reserve one for increment.
pub const MAX_REVISION: i64 = 9_007_199_254_740_990;

#[derive(Debug, PartialEq)]
pub enum DefaultsChange {
    Done(HatSessionDefaults),
    NotFound,
    Purging,
    Changed,
    StepUpRequired,
    Invalid,
}

pub fn valid_id(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

pub fn valid_defaults(agent: &str, request: &PutHatSessionDefaults) -> bool {
    valid_id(agent, 128)
        && (0..=MAX_REVISION).contains(&request.revision)
        && request.config.model.as_ref().is_none_or(|v| valid_id(v, 256))
        && request.config.mode.as_ref().is_none_or(|v| valid_id(v, 256))
        && request.config.axes.len() <= 64
        && request.config.axes.iter().all(|(id, value)| {
            valid_id(id, 256)
                && id != "model"
                && id != "mode"
                && match value {
                    ConfigValue::Bool(_) => true,
                    ConfigValue::Id(v) => valid_id(v, 256),
                }
        })
}

impl Hosts {
    pub fn session_defaults(&self, hat: &str, agent: &str) -> Result<Option<HatSessionDefaults>> {
        let conn = self.conn();
        let row: Option<(Option<String>, Option<i64>)> = conn
            .query_row(
                "SELECT d.config_json, d.revision FROM hats h
             LEFT JOIN hat_session_defaults d ON d.hat_id = h.id AND d.owner_id = h.owner_id AND d.agent = ?3
             WHERE h.id = ?1 AND h.owner_id = ?2",
                params![hat, self.owner_id(), agent],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(json, revision)| {
            Ok(HatSessionDefaults {
                hat_id: hat.into(),
                agent: agent.into(),
                revision: revision.unwrap_or(0),
                config: match json {
                    Some(json) => serde_json::from_str(&json)?,
                    None => Default::default(),
                },
            })
        })
        .transpose()
    }

    pub fn put_session_defaults(
        &self,
        hat: &str,
        agent: &str,
        request: &PutHatSessionDefaults,
        auth: &Authenticated,
    ) -> Result<DefaultsChange> {
        if !valid_defaults(agent, request) {
            return Ok(DefaultsChange::Invalid);
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_now();
        let authorized = auth.owner_id == self.owner_id()
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE id_hash = ?1 AND owner_id = ?2
             AND expires_at > ?3 AND last_step_up_at > ?4)",
                params![auth.session_id, self.owner_id(), now, now - STEP_UP_SECS],
                |r| r.get::<_, bool>(0),
            )?;
        if !authorized {
            return Ok(DefaultsChange::StepUpRequired);
        }
        let hat_state: Option<bool> = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM purged_hats p WHERE p.hat_id = h.id AND p.owner_id = ?2)
             FROM hats h WHERE h.id = ?1 AND h.owner_id = ?2",
                params![hat, self.owner_id()],
                |r| r.get(0),
            )
            .optional()?;
        match hat_state {
            None => return Ok(DefaultsChange::NotFound),
            Some(true) => return Ok(DefaultsChange::Purging),
            Some(false) => {}
        }
        let revision: i64 = tx
            .query_row(
                "SELECT revision FROM hat_session_defaults WHERE hat_id = ?1 AND owner_id = ?2 AND agent = ?3",
                params![hat, self.owner_id(), agent],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if revision != request.revision {
            return Ok(DefaultsChange::Changed);
        }
        let stored = HatSessionDefaults {
            hat_id: hat.into(),
            agent: agent.into(),
            revision: revision + 1,
            config: request.config.clone(),
        };
        tx.execute(
            "INSERT INTO hat_session_defaults(owner_id, hat_id, agent, config_json, revision) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(owner_id, hat_id, agent) DO UPDATE SET config_json = excluded.config_json, revision = excluded.revision
             WHERE hat_session_defaults.owner_id = ?1",
            params![self.owner_id(), hat, agent, serde_json::to_string(&request.config)?, stored.revision],
        )?;
        tx.commit()?;
        Ok(DefaultsChange::Done(stored))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, schema};
    use hennery_proto::frames::SessionConfig;
    use rusqlite::Connection;
    use std::sync::{Arc, Barrier};

    struct Fixture {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        hosts: Hosts,
        hat: String,
        auth: Authenticated,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("hennery.db");
            // Upgrade the previous kernel version, then open and reopen it.
            let mut conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
            db::migrate_component(
                &mut conn,
                schema::COMPONENT,
                &schema::MIGRATIONS[..schema::MIGRATIONS.len() - 1],
            )
            .unwrap();
            drop(conn);
            let hosts = Hosts::open(&path).unwrap();
            let hat = hosts.hats().unwrap()[0].id.clone();
            let now = unix_now();
            hosts.conn().execute("INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, last_step_up_at, expires_at) VALUES ('auth', ?1, 'test', ?2, ?2, ?2, ?3)", params![hosts.owner_id(), now, now+3600]).unwrap();
            let auth = Authenticated {
                session_id: "auth".into(),
                owner_id: hosts.owner_id().into(),
                last_step_up_at: Some(now),
                expires_at: now + 3600,
                slid: false,
            };
            Self {
                _dir: dir,
                path,
                hosts,
                hat,
                auth,
            }
        }
        fn put(&self, revision: i64, config: SessionConfig) -> DefaultsChange {
            self.hosts
                .put_session_defaults(
                    &self.hat,
                    "custom",
                    &PutHatSessionDefaults { revision, config },
                    &self.auth,
                )
                .unwrap()
        }
    }
    fn config() -> SessionConfig {
        SessionConfig {
            cache_retention: Default::default(),
            model: Some("model-a".into()),
            mode: Some("full".into()),
            axes: [("fast".into(), ConfigValue::Bool(false))].into(),
        }
    }

    #[test]
    fn upgrade_reopen_false_sparse_reset_and_revision_aba() {
        let f = Fixture::new();
        assert_eq!(f.hosts.session_defaults(&f.hat, "custom").unwrap().unwrap().revision, 0);
        assert!(matches!(f.put(0, config()), DefaultsChange::Done(v) if v.config == config() && v.revision == 1));
        let reopened = Hosts::open(&f.path).unwrap();
        assert_eq!(
            reopened.session_defaults(&f.hat, "custom").unwrap().unwrap().config,
            config()
        );
        assert!(
            matches!(f.put(1, SessionConfig::default()), DefaultsChange::Done(v) if v.revision == 2 && v.config.is_empty())
        );
        assert_eq!(f.put(0, config()), DefaultsChange::Changed);
        assert_eq!(f.hosts.session_defaults(&f.hat, "other").unwrap().unwrap().revision, 0);
    }
    #[test]
    fn ownership_freeze_cascade_and_authorization_at_write_boundary() {
        let f = Fixture::new();
        f.hosts.conn().execute_batch("INSERT INTO owners(id,created_at) VALUES ('foreign', 0); INSERT INTO hats(id,owner_id,name,colour,created_at) VALUES ('foreign-hat','foreign','Other','#64748b',0);").unwrap();
        let req = PutHatSessionDefaults {
            revision: 0,
            config: config(),
        };
        assert!(f.hosts.session_defaults("foreign-hat", "custom").unwrap().is_none());
        assert_eq!(
            f.hosts
                .put_session_defaults("foreign-hat", "custom", &req, &f.auth)
                .unwrap(),
            DefaultsChange::NotFound
        );
        assert_eq!(
            f.hosts
                .put_session_defaults("missing", "custom", &req, &f.auth)
                .unwrap(),
            DefaultsChange::NotFound
        );
        f.hosts
            .conn()
            .execute(
                "INSERT INTO purged_hats(hat_id,owner_id,purged_at) VALUES (?1,?2,0)",
                params![f.hat, f.hosts.owner_id()],
            )
            .unwrap();
        assert_eq!(f.put(0, config()), DefaultsChange::Purging);
        f.hosts
            .conn()
            .execute("DELETE FROM purged_hats WHERE hat_id = ?1", [&f.hat])
            .unwrap();
        f.hosts
            .conn()
            .execute(
                "UPDATE auth_sessions SET last_step_up_at = ?1",
                [unix_now() - STEP_UP_SECS],
            )
            .unwrap();
        assert_eq!(f.put(0, config()), DefaultsChange::StepUpRequired);
        f.hosts
            .conn()
            .execute(
                "UPDATE auth_sessions SET last_step_up_at = ?1, expires_at = ?1",
                [unix_now()],
            )
            .unwrap();
        assert_eq!(f.put(0, config()), DefaultsChange::StepUpRequired);
        f.hosts
            .conn()
            .execute("UPDATE auth_sessions SET expires_at = ?1", [unix_now() + 3600])
            .unwrap();
        assert!(matches!(f.put(0, config()), DefaultsChange::Done(_)));
        f.hosts
            .conn()
            .execute_batch("DELETE FROM settings; DELETE FROM hats;")
            .unwrap();
        let count: i64 = f
            .hosts
            .conn()
            .query_row("SELECT count(*) FROM hat_session_defaults", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert!(f.hosts.session_defaults(&f.hat, "custom").unwrap().is_none());
    }
    #[test]
    fn only_one_revision_racer_replaces_the_record() {
        let f = Fixture::new();
        let barrier = Arc::new(Barrier::new(2));
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let barrier = barrier.clone();
                    let f = &f;
                    scope.spawn(move || {
                        let hosts = Hosts::open(&f.path).unwrap();
                        barrier.wait();
                        hosts
                            .put_session_defaults(
                                &f.hat,
                                "custom",
                                &PutHatSessionDefaults {
                                    revision: 0,
                                    config: config(),
                                },
                                &f.auth,
                            )
                            .unwrap()
                    })
                })
                .collect();
            let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(
                results.iter().filter(|r| matches!(r, DefaultsChange::Done(_))).count(),
                1
            );
            assert_eq!(
                results.iter().filter(|r| matches!(r, DefaultsChange::Changed)).count(),
                1
            );
        });
    }
    #[test]
    fn validation_bounds_and_strict_wire_shape() {
        let mut request = PutHatSessionDefaults {
            revision: 0,
            config: config(),
        };
        assert!(valid_defaults("custom", &request));
        for agent in ["".into(), "a".repeat(129), "a\n".into()] {
            assert!(!valid_defaults(&agent, &request));
        }
        for revision in [-1, MAX_REVISION + 1] {
            request.revision = revision;
            assert!(!valid_defaults("custom", &request));
        }
        request.revision = 0;
        for model in ["".into(), "m".repeat(257), "bad\0".into()] {
            request.config.model = Some(model);
            assert!(!valid_defaults("custom", &request));
        }
        request.config = config();
        for mode in ["".into(), "m".repeat(257), "bad\n".into()] {
            let mut bad = request.clone();
            bad.config.mode = Some(mode);
            assert!(!valid_defaults("custom", &bad));
        }
        for id in ["".into(), "a".repeat(257), "bad\n".into(), "model".into()] {
            let mut bad = request.clone();
            bad.config.axes.insert(id, ConfigValue::Bool(false));
            assert!(!valid_defaults("custom", &bad));
        }
        for value in ["".into(), "v".repeat(257), "bad\n".into()] {
            let mut bad = request.clone();
            bad.config.axes.insert("custom".into(), ConfigValue::Id(value));
            assert!(!valid_defaults("custom", &bad));
        }
        let f = Fixture::new();
        assert_eq!(f.put(-1, config()), DefaultsChange::Invalid);
        request.config.axes.insert("mode".into(), ConfigValue::Bool(false));
        assert!(!valid_defaults("custom", &request));
        request.config.axes = (0..65).map(|i| (format!("a{i}"), ConfigValue::Bool(false))).collect();
        assert!(!valid_defaults("custom", &request));
        for json in [
            r#"{"revision":0,"config":{"other":true}}"#,
            r#"{"revision":0,"config":{},"other":true}"#,
            r#"{"revision":0,"config":{"axes":{"x":1}}}"#,
            r#"{"revision":0.5,"config":{}}"#,
        ] {
            assert!(serde_json::from_str::<PutHatSessionDefaults>(json).is_err());
        }
        assert!(
            serde_json::from_str::<PutHatSessionDefaults>(r#"{"revision":0,"config":{"model":null,"mode":null}}"#)
                .unwrap()
                .config
                .is_empty()
        );
    }
}
