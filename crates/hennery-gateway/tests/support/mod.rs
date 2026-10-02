//! A fresh `hennery.db` with the gateway's stores, hosts, hats and
//! connections, and session tokens minted and revoked through the
//! primitives plan 8e will call, each in a transaction of its own as the
//! sessions store's would be (plan 8d).

#![allow(dead_code)]

pub mod upstream;

use ed25519_dalek::SigningKey;
use hennery_gateway::api::GatewayState;
use hennery_gateway::key::MasterKey;
use hennery_gateway::revocation::Revocations;
use hennery_gateway::model::{Change, CredKind, CredentialChange, NewConnection};
use hennery_gateway::scope::ProxyStore;
use hennery_gateway::store::GatewayStore;
use hennery_gateway::tokens;
use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use std::path::PathBuf;
use std::sync::Arc;

pub struct World {
    pub dir: tempfile::TempDir,
    pub db: PathBuf,
    pub store: Arc<GatewayStore>,
    pub proxy_store: Arc<ProxyStore>,
    pub key: Arc<MasterKey>,
    pub hosts: Hosts,
    /// Shared by the proxy (`Harness`) and `gateway()` (plan 8e).
    pub revocations: Revocations,
}

impl World {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let store = Arc::new(GatewayStore::open(&db).unwrap());
        let proxy_store = Arc::new(ProxyStore::open(&db).unwrap());
        Self {
            dir,
            db,
            store,
            proxy_store,
            key: Arc::new(MasterKey::from_bytes([7; 32])),
            hosts,
            revocations: Revocations::new(),
        }
    }

    pub fn hat(&self) -> String {
        self.hosts.default_hat_for_new_hosts().unwrap()
    }

    pub fn other_hat(&self) -> String {
        match self.hosts.create_hat("Work", None, unix_now()).unwrap() {
            HatChange::Done(hat) => hat.id,
            other => panic!("{other:?}"),
        }
    }

    pub fn host(&self, id: &str, seed: u8) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let enrollment = Enrollment {
            public_key: hex::encode(key.verifying_key().as_bytes()),
            name: format!("host {seed}"),
            host_version: "0.0.0".into(),
            platform: "macos-aarch64".into(),
        };
        self.hosts.register(id, &enrollment, unix_now()).unwrap();
    }

    /// A connection to `url` in `hat`, on the internal network (lane L7:
    /// test upstreams are on loopback).
    pub fn connection_in(
        &self,
        slug: &str,
        url: &str,
        kind: CredKind,
        hat: &str,
        allowlist: Option<&[&str]>,
    ) -> String {
        self.connection_with(NewConnection {
            slug: slug.into(),
            label: format!("Label {slug}"),
            url: url.into(),
            hat_id: hat.into(),
            cred_kind: kind,
            static_header: None,
            static_prefix: None,
            tool_allowlist: allowlist.map(|tools| tools.iter().map(|t| t.to_string()).collect()),
            internal_network: true,
        })
    }

    /// A connection in `hat` to a loopback upstream, not yet created.
    pub fn new_connection(&self, slug: &str, hat: &str) -> NewConnection {
        new_connection(slug, hat)
    }

    pub fn connection_with(&self, new: NewConnection) -> String {
        match self.store.create(&new, unix_now()).unwrap() {
            Change::Done(record) => record.id,
            other => panic!("{other:?}"),
        }
    }

    pub fn connection(&self, slug: &str, url: &str, kind: CredKind) -> String {
        self.connection_in(slug, url, kind, &self.hat(), None)
    }

    pub fn mount(&self, id: &str, hosts: &[&str]) {
        let hosts: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
        assert!(matches!(
            self.store.replace_mounts(id, &hosts).unwrap(),
            Change::Done(_)
        ));
    }

    pub fn set_token(&self, id: &str, token: &str) {
        assert_eq!(
            self.store
                .set_static_credential(id, token, &self.key, unix_now())
                .unwrap(),
            CredentialChange::Done
        );
    }

    /// A raw connection to `hennery.db`, as the sessions store's would be.
    pub fn raw(&self) -> rusqlite::Connection {
        hennery_kernel::db::open(&self.db).unwrap()
    }

    pub fn mint(&self, session: &str, host: &str, hat: &str) -> String {
        let mut conn = self.raw();
        let tx = conn.transaction().unwrap();
        let token = tokens::mint_in(&tx, self.store.owner_id(), session, host, hat, unix_now()).unwrap();
        tx.commit().unwrap();
        token.expose().to_string()
    }

    pub fn revoke(&self, session: &str) -> bool {
        let mut conn = self.raw();
        let tx = conn.transaction().unwrap();
        let revoked = tokens::revoke_in(&tx, self.store.owner_id(), session, unix_now()).unwrap();
        tx.commit().unwrap();
        revoked
    }

    pub fn revoke_host_tokens(&self, host: &str) -> usize {
        let mut conn = self.raw();
        let tx = conn.transaction().unwrap();
        let revoked = tokens::revoke_host_in(&tx, self.store.owner_id(), host, unix_now()).unwrap();
        tx.commit().unwrap();
        revoked
    }

    pub fn status(&self, id: &str) -> String {
        self.store.connection(id).unwrap().unwrap().status
    }

    /// The gateway on this database, as the collector builds it: its
    /// store, key and `revocations`, the owner opened on the same file.
    pub fn gateway(&self) -> GatewayState {
        GatewayState {
            store: self.store.clone(),
            key: self.key.clone(),
            operator: Arc::new(Operator::open(&self.db).unwrap()),
            revocations: self.revocations.clone(),
        }
    }

    /// What the proxy would resolve `token` to now.
    pub fn proxy_store_resolve(&self, token: &str) -> Option<hennery_gateway::scope::Principal> {
        use hennery_gateway::scope::ClientIdentity;
        self.proxy_store.resolve(token, unix_now()).unwrap()
    }

    /// How many session token rows there are, live or not.
    pub fn token_rows(&self) -> i64 {
        self.raw()
            .query_row("SELECT count(*) FROM gw_session_tokens", [], |r| r.get(0))
            .unwrap()
    }

    /// Whether `token`'s row is revoked (not whether it resolves: a revoked
    /// host's tokens stop resolving by the host's join alone).
    pub fn token_revoked(&self, token: &str) -> bool {
        self.raw()
            .query_row(
                "SELECT revoked_at IS NOT NULL FROM gw_session_tokens WHERE token_hash = ?1",
                [hennery_kernel::secret::sha256_hex(token.as_bytes())],
                |r| r.get(0),
            )
            .unwrap()
    }
}

/// A `none` connection in `hat` to a loopback upstream, not yet created.
pub fn new_connection(slug: &str, hat: &str) -> NewConnection {
    NewConnection {
        slug: slug.into(),
        label: format!("Label {slug}"),
        url: "http://127.0.0.1:9/mcp".into(),
        hat_id: hat.into(),
        cred_kind: CredKind::None,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: true,
    }
}
