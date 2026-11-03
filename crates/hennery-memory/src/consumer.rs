//! Pull only from explicit C1 grants. Every late response is fenced against
//! the same namespace epoch/config/cursor revision inside its write transaction.
use crate::store::{Entry, Inner, Namespace, Observation, Store, bounded, put};
use anyhow::{Context, Result, ensure};
use hennery_gateway::{crypto, key::MasterKey};
use hennery_kernel::{
    egress::{self, Allowance, Egress, Timeouts},
    redact,
    secret::{sha256_hex, unix_now},
};
use hennery_proto::{plugin::*, plugin_management::PluginLease};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use url::Url;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedConfig {
    pub collector_url: String,
    pub namespace_epoch: String,
    pub activity_token: String,
    pub activity: bool,
    #[serde(default)]
    pub internal_network: bool,
}
#[derive(Clone)]
struct Job {
    id: String,
    epoch: String,
    revision: i64,
    cursor: Option<String>,
    config: FeedConfig,
}
struct InventoryPlan {
    absent: Vec<String>,
    note_ids: Vec<String>,
    replacements: Vec<(String, String)>,
}
pub(crate) struct NamespacePermit {
    inner: Arc<std::sync::Mutex<Inner>>,
    id: String,
    armed: bool,
}
impl Drop for NamespacePermit {
    fn drop(&mut self) {
        if self.armed {
            self.inner.lock().expect("store").writing.remove(&self.id);
        }
    }
}
impl NamespacePermit {
    fn publish(mut self, job: &Job, start: Instant, until: i64) -> Result<()> {
        let mut inner = self.inner.lock().expect("store");
        inner.live(job)?;
        check_lease(start, until)?;
        inner.writing.remove(&self.id);
        self.armed = false;
        inner.lease(job, start, until)
    }
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResetPhase {
    Commit,
    Checkpoint,
    Apply,
    ManagedReady,
    ImportVerified,
    ManagedCommit,
    ManagedCheckpoint,
    CleanupCheckpoint,
    CleanupConfirmed,
}
#[cfg(test)]
pub(crate) type ResetHook = Arc<dyn Fn(ResetPhase) + Send + Sync>;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forget {
    pub request_id: String,
    pub namespace_epoch: String,
    pub target: Target,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Scope,
    Session { session_id: String },
}
impl Drop for FeedConfig {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.activity_token.zeroize();
    }
}
pub const BACKLOG_DELAY: Duration = Duration::from_millis(2200);
pub struct Consumer {
    egress: Egress,
    http: reqwest::Client,
}
impl Consumer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            egress: Egress::new(Timeouts {
                connect: Duration::from_secs(3),
                request: Duration::from_secs(5),
            })?,
            http: reqwest::Client::new(),
        })
    }
    pub async fn poll(&self, store: &Store, key: &MasterKey, id: &str) -> Result<bool> {
        let job = store.job(key, id)?;
        let result = tokio::time::timeout(Duration::from_secs(20 * 60), self.run(store, &job)).await;
        match result {
            Ok(Ok(more)) => Ok(more),
            _ => {
                store.failed(&job, true);
                anyhow::bail!("activity unavailable")
            }
        }
    }
    async fn request(&self, job: &Job, path: &str, after: Option<&str>, post: bool) -> Result<Vec<u8>> {
        let mut url = Url::parse(&job.config.collector_url)?;
        url.set_path(path);
        if !post {
            let mut q = url.query_pairs_mut();
            q.append_pair("limit", if path == "/api/plugins/inventory" { "500" } else { "20" });
            if let Some(c) = after {
                q.append_pair("after", c);
            }
        }
        let request = self
            .http
            .request(
                if post {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::GET
                },
                url,
            )
            .bearer_auth(&job.config.activity_token)
            .build()?;
        let response = self
            .egress
            .client(if job.config.internal_network {
                Allowance::InternalNetwork
            } else {
                Allowance::PublicOnly
            })
            .send(request)
            .await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("grant denied");
        }
        if status == reqwest::StatusCode::CONFLICT {
            anyhow::bail!("reset required");
        }
        ensure!(
            status.is_success()
                && response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.split(';').next() == Some("application/json")),
            "invalid activity response"
        );
        Ok(egress::read_capped(response, 128 * 1024).await?)
    }
    async fn run(&self, store: &Store, job: &Job) -> Result<bool> {
        let start = Instant::now();
        let began = unix_now();
        // Lease has an explicit epoch even for an empty feed. Never infer an epoch
        // from a caller's hints or accept an empty foreign-token response.
        let lease: PluginLease = match self.request(job, "/api/plugins/lease", None, true).await {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) => {
                if e.to_string() == "grant denied" {
                    store.failed(job, true);
                }
                return Err(e);
            }
        };
        ensure!(lease.namespace_epoch == job.epoch, "epoch mismatch");
        let mut until = lease.lease_until.min(began + 60);
        ensure!(until > unix_now(), "expired lease");
        if !job.config.activity {
            store.renew(job, start, until)?;
            return Ok(false);
        }
        match self
            .request(job, "/api/plugins/activity", job.cursor.as_deref(), false)
            .await
        {
            Ok(bytes) => {
                let page: PluginActivityPage = serde_json::from_slice(&bytes)?;
                until = until.min(page.lease_until);
                let more = page.has_more;
                store.apply(job, page, start, until)?;
                Ok(more)
            }
            Err(e) => {
                if e.to_string() == "grant denied" {
                    store.failed(job, true);
                    return Err(e);
                }
                if e.to_string() != "reset required" {
                    return Err(e);
                }
                store.failed(job, true);
                let mut after = None;
                let known = store.known_sources(job)?;
                let mut latest_start;
                let mut ids = std::collections::BTreeSet::new();
                let mut marker = None;
                let mut resume = None;
                loop {
                    if after.is_some() {
                        tokio::time::sleep(Duration::from_millis(1100)).await;
                    }
                    store.ensure_current(job)?;
                    let page_start = Instant::now();
                    let page_began = unix_now();
                    let bytes = self
                        .request(job, "/api/plugins/inventory", after.as_deref(), false)
                        .await?;
                    store.ensure_current(job)?;
                    let page: PluginSourceInventoryPage = serde_json::from_slice(&bytes)?;
                    ensure!(
                        page.version == 1
                            && page.source_ids.len() <= 500
                            && page.watermark.len() <= 2048
                            && page.resume_cursor.len() <= 2048,
                        "invalid inventory"
                    );
                    if let Some(m) = &marker {
                        ensure!(
                            m == &page.watermark && resume.as_ref() == Some(&page.resume_cursor),
                            "inventory changed"
                        );
                    }
                    marker = Some(page.watermark);
                    resume = Some(page.resume_cursor);
                    until = page.lease_until.min(page_began.saturating_add(60));
                    check_lease(page_start, until)?;
                    latest_start = page_start;
                    for id in page.source_ids {
                        ensure!(!id.is_empty() && id.len() <= 256, "invalid source");
                        if known.contains(&id) {
                            ids.insert(id);
                        }
                    }

                    let Some(next) = page.next_cursor else {
                        break;
                    };
                    ensure!(
                        next.len() <= 2048 && after.as_ref() != Some(&next),
                        "invalid inventory cursor"
                    );
                    after = Some(next);
                }
                store
                    .inventory(job, ids, resume.context("missing inventory")?, latest_start, until)
                    .await?;
                Ok(false)
            }
        }
    }
}
impl Store {
    pub fn attach(&self, id: &str, mut config: FeedConfig, key: &MasterKey) -> Result<()> {
        let url = Url::parse(&config.collector_url)?;
        ensure!(
            url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none(),
            "collector origin required"
        );
        egress::check_url(
            &url,
            if config.internal_network {
                Allowance::InternalNetwork
            } else {
                Allowance::PublicOnly
            },
        )
        .map_err(|_| anyhow::anyhow!("collector origin refused"))?;
        ensure!(
            hex64(&config.namespace_epoch) && config.activity_token.strip_prefix("hnry_activity_").is_some_and(hex64),
            "invalid grant"
        );
        config.collector_url = url.to_string();
        let mut inner = self.inner.lock().expect("store");
        let tx = inner.db.transaction()?;
        let (registered, epoch, state): (bool, String, String) =
            tx.query_row("SELECT registered,epoch,state FROM namespaces WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        ensure!(state == "active", "namespace terminal");
        if registered {
            ensure!(epoch == config.namespace_epoch, "cannot replace epoch");
            let previous: Vec<u8> = tx.query_row("SELECT config FROM feeds WHERE namespace=?1", [id], |r| r.get(0))?;
            let previous = crypto::open(key, &format!("{id}:{epoch}"), "memory.feed", 1, &previous)?;
            let previous: FeedConfig = serde_json::from_slice(&previous)?;
            ensure!(
                Url::parse(&previous.collector_url)? == url,
                "cannot change collector origin"
            );
        } else {
            let records: i64 = tx.query_row("SELECT (SELECT count(*) FROM entries WHERE namespace=?1)+(SELECT count(*) FROM requests WHERE namespace=?1)+(SELECT count(*) FROM imports WHERE namespace=?1)", [id], |r| r.get(0))?;
            ensure!(records == 0, "attach requires empty namespace, including retry history");
        }
        let feeds: i64 = tx.query_row("SELECT count(*) FROM feeds", [], |r| r.get(0))?;
        ensure!(registered || feeds < 128, "too many feeds");
        let blob = crypto::seal(
            key,
            &format!("{id}:{}", config.namespace_epoch),
            "memory.feed",
            &serde_json::to_vec(&config)?,
        );
        tx.execute(
            "UPDATE namespaces SET registered=1,epoch=?2 WHERE id=?1",
            params![id, config.namespace_epoch],
        )?;
        tx.execute("INSERT INTO feeds(namespace,config) VALUES(?1,?2) ON CONFLICT(namespace) DO UPDATE SET config=excluded.config,revision=revision+1",params![id,blob])?;
        tx.commit()?;
        inner.invalidate(id);
        inner.leases.remove(id);
        inner.stale.insert(id.into());
        Ok(())
    }
    pub fn feed_ids(&self) -> Result<Vec<String>> {
        let inner = self.inner.lock().expect("store");
        let mut stmt=inner.db.prepare("SELECT f.namespace FROM feeds f JOIN namespaces n ON n.id=f.namespace WHERE n.state='active' ORDER BY f.namespace LIMIT 129")?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(ids.len() <= 128, "too many feeds");
        Ok(ids)
    }
    pub fn has_feed_credentials(&self) -> Result<bool> {
        let inner = self.inner.lock().expect("store");
        Ok(inner
            .db
            .query_row("SELECT EXISTS(SELECT 1 FROM feeds)", [], |r| r.get(0))?)
    }
    /// Local configuration validation only; never expose grant credentials.
    pub(crate) fn collector_origins(&self, key: &MasterKey) -> Result<Vec<String>> {
        self.feed_ids()?
            .iter()
            .map(|id| Ok(self.job(key, id)?.config.collector_url.clone()))
            .collect()
    }
    fn job(&self, key: &MasterKey, id: &str) -> Result<Job> {
        let mut inner = self.inner.lock().expect("store");
        let (epoch,revision,cursor,blob):(String,i64,Option<String>,Vec<u8>)=inner.db.query_row("SELECT n.epoch,f.revision,f.cursor,f.config FROM feeds f JOIN namespaces n ON n.id=f.namespace WHERE f.namespace=?1 AND n.state='active'",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let decoded = (|| -> Result<FeedConfig> {
            let bytes = zeroize::Zeroizing::new(crypto::open(key, &format!("{id}:{epoch}"), "memory.feed", 1, &blob)?);
            let config: FeedConfig = serde_json::from_slice(&bytes)?;
            ensure!(config.namespace_epoch == epoch, "epoch mismatch");
            Ok(config)
        })();
        let config = match decoded {
            Ok(config) => config,
            Err(e) => {
                inner.leases.remove(id);
                inner.stale.insert(id.into());
                return Err(e);
            }
        };
        Ok(Job {
            id: id.into(),
            epoch,
            revision,
            cursor,
            config,
        })
    }
    fn failed(&self, job: &Job, denied: bool) {
        let mut inner = self.inner.lock().expect("store");
        if inner.live(job).is_ok() {
            if denied {
                inner.leases.remove(&job.id);
            }
            inner.stale.insert(job.id.clone());
        }
    }
    fn renew(&self, job: &Job, start: Instant, until: i64) -> Result<()> {
        let mut inner = self.inner.lock().expect("store");
        inner.live(job)?;
        inner.lease(job, start, until)
    }
    fn apply(&self, job: &Job, page: PluginActivityPage, start: Instant, until: i64) -> Result<()> {
        ensure!(
            page.version == 1
                && page.items.len() <= 20
                && page.next_cursor.len() <= 2048
                && serde_json::to_vec(&page)?.len() <= 128 * 1024,
            "invalid page"
        );
        for item in &page.items {
            validate(item, &job.epoch)?;
        }
        let digests = page
            .items
            .iter()
            .map(|item| serde_json::to_vec(item).map(|bytes| sha256_hex(&bytes)))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut value = serde_json::to_value(page)?;
        redact::value(&mut value);
        let page: PluginActivityPage = serde_json::from_value(value)?;
        let mut inner = self.inner.lock().expect("store");
        inner.live(job)?;
        check_lease(start, until)?;
        #[cfg(test)]
        self.reset_phase(ResetPhase::Apply);
        let tx = inner.db.transaction()?;
        // The separate reset writer can commit after the preliminary check.
        // Fence again under SQLite's write lock before admitting any evidence.
        check_job(&tx, job)?;
        let erased = page
            .items
            .iter()
            .any(|item| !matches!(item.body, ActivityBody::CompletedTurn { .. }));
        for (item, digest) in page.items.into_iter().zip(digests) {
            let old: Option<String> = tx
                .query_row(
                    "SELECT digest FROM seen WHERE namespace=?1 AND event_key=?2",
                    params![job.id, item.event_key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(old) = &old {
                ensure!(*old == digest, "source event changed");
            } else {
                // A committed cursor may only replay already admitted evidence.
                // Pruned history cannot be reintroduced by a non-advancing page.
                ensure!(
                    job.cursor.as_ref() != Some(&page.next_cursor),
                    "non-advancing new event"
                );
            }
            // Refresh current-page evidence before pruning, including duplicates.
            tx.execute(
                "DELETE FROM seen WHERE namespace=?1 AND event_key=?2",
                params![job.id, item.event_key],
            )?;
            tx.execute(
                "INSERT INTO seen(namespace,event_key,digest) VALUES(?1,?2,?3)",
                params![job.id, item.event_key, digest],
            )?;
            if old.is_some() {
                continue;
            }
            match &item.body {
                ActivityBody::SessionRemoved | ActivityBody::SourceReset => {
                    remove_source(&tx, &job.id, &item.session_id)?;
                    if matches!(item.body, ActivityBody::SessionRemoved) {
                        tx.execute(
                            "INSERT OR IGNORE INTO suppressed(namespace,session) VALUES(?1,?2)",
                            params![job.id, item.session_id],
                        )?;
                    }
                }
                ActivityBody::CompletedTurn { project_key, .. } => {
                    let suppressed: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM suppressed WHERE namespace=?1 AND session=?2)",
                        params![job.id, item.session_id],
                        |r| r.get(0),
                    )?;
                    if !suppressed {
                        let id = sha256_hex(format!("native-note:{}", item.session_id).as_bytes());
                        let old: Option<String> = tx
                            .query_row(
                                "SELECT data FROM entries WHERE namespace=?1 AND id=?2",
                                params![job.id, id],
                                |r| r.get(0),
                            )
                            .optional()?;
                        let mut entry = if let Some(old) = old {
                            serde_json::from_str::<Entry>(&old)?
                        } else {
                            Entry {
                                id,
                                kind: "note".into(),
                                title: "Completed-session observation".into(),
                                body: String::new(),
                                project_key: Some(project_key.clone()),
                                author: "observation".into(),
                                created_at: unix_now(),
                                updated_at: 0,
                                covered_through: None,
                                project_key_omitted: false,
                                project_key_digest: None,
                                revision: 0,
                                memory_kind: None,
                                tags: vec![],
                                provenance: vec![],
                                source_refs: vec![],
                                truncated: false,
                                stale: false,
                                turns: vec![],
                                older_turns: 0,
                                pinned: false,
                                owner_annotation: String::new(),
                                legacy: None,
                            }
                        };
                        if let ActivityBody::CompletedTurn { completed_at, .. } = &item.body {
                            entry.updated_at = time::OffsetDateTime::parse(
                                completed_at,
                                &time::format_description::well_known::Rfc3339,
                            )?
                            .unix_timestamp();
                            if entry.revision == 0 {
                                entry.created_at = entry.updated_at;
                            }
                        }
                        entry.covered_through = Some(item.event_key.clone());
                        entry.turns.push(observation(&item)?);
                        entry.revision += 1;
                        entry.project_key = Some(project_key.clone());
                        while entry.turns.len() > 3 {
                            entry.turns.remove(0);
                            entry.older_turns += 1;
                            entry.truncated = true;
                        }
                        render_note(&mut entry)?;
                        put(&tx, &job.id, &entry)?;
                        tx.execute("INSERT INTO sources(namespace,session,event_key) VALUES(?1,?2,?3) ON CONFLICT(namespace,session) DO UPDATE SET event_key=excluded.event_key",params![job.id,item.session_id,item.event_key])?;
                    }
                }
            }
        }
        tx.execute(
            "UPDATE feeds SET cursor=?2,revision=revision+1 WHERE namespace=?1",
            params![job.id, page.next_cursor],
        )?;
        // Only evidence behind this cursor is evicted, atomically with its
        // projection. A stale job cannot commit, and the <=20 current items
        // were refreshed above. Capacity never prevents a deletion page.
        tx.execute(
            "DELETE FROM seen WHERE namespace=?1 AND rowid IN (SELECT rowid FROM seen WHERE namespace=?1 ORDER BY rowid DESC LIMIT -1 OFFSET 100000)",
            [&job.id],
        )?;
        budget(&tx, &job.id)?;
        check_lease(start, until)?;
        tx.commit()?;
        if erased {
            inner.invalidate(&job.id);
        }
        inner.leases.remove(&job.id);
        inner.stale.insert(job.id.clone());
        checkpoint(&inner)?;
        inner.lease(job, start, until)
    }
    fn ensure_current(&self, job: &Job) -> Result<()> {
        self.inner.lock().expect("store").live(job)
    }
    fn known_sources(&self, job: &Job) -> Result<std::collections::BTreeSet<String>> {
        let inner = self.inner.lock().expect("store");
        inner.live(job)?;
        let mut stmt = inner.db.prepare("SELECT session FROM sources WHERE namespace=?1")?;
        let ids = stmt
            .query_map([&job.id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
        ensure!(ids.len() <= 10000, "local source budget exceeded");
        Ok(ids)
    }
    async fn inventory(
        &self,
        job: &Job,
        ids: std::collections::BTreeSet<String>,
        resume: String,
        start: Instant,
        until: i64,
    ) -> Result<()> {
        let permit = self.begin_reset(job)?;
        let plan = self.prepare_inventory(job, ids, start, until).await?;
        let store = self.clone();
        let job = job.clone();
        tokio::task::spawn_blocking(move || store.commit_inventory(job, plan, resume, start, until, permit)).await?
    }
    fn begin_reset(&self, job: &Job) -> Result<NamespacePermit> {
        let mut inner = self.inner.lock().expect("store");
        inner.live(job)?;
        let permit = self.reserve_namespace(&mut inner, &job.id)?;
        inner.leases.remove(&job.id);
        inner.stale.insert(job.id.clone());
        Ok(permit)
    }
    pub(crate) fn reserve_namespace(&self, inner: &mut Inner, id: &str) -> Result<NamespacePermit> {
        ensure!(inner.writing.insert(id.into()), "namespace busy");
        inner.invalidate(id);
        Ok(NamespacePermit {
            inner: self.inner.clone(),
            id: id.into(),
            armed: true,
        })
    }
    async fn prepare_inventory(
        &self,
        job: &Job,
        ids: std::collections::BTreeSet<String>,
        start: Instant,
        until: i64,
    ) -> Result<InventoryPlan> {
        let absent = Arc::new(
            self.known_sources(job)?
                .difference(&ids)
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
        );
        let note_ids = Arc::new(
            absent
                .iter()
                .map(|s| sha256_hex(format!("native-note:{s}").as_bytes()))
                .collect::<std::collections::BTreeSet<_>>(),
        );
        let high_water: i64 = {
            let inner = self.inner.lock().expect("store");
            inner.live(job)?;
            check_lease(start, until)?;
            inner.db.query_row(
                "SELECT coalesce(max(seq),0) FROM entries WHERE namespace=?1",
                [&job.id],
                |r| r.get(0),
            )?
        };
        let mut replacements = Vec::new();
        let mut scanned = 0;
        // Both R1 entry kinds use scoped_entries(namespace,kind,seq). Each
        // bounded keyset read releases the mutex before JSON/CPU work. The
        // high water excludes later inserts; another feed/lifecycle mutation
        // changes the job revision and cancels the entire prepared result.
        for kind in ["note", "memory"] {
            let mut after = 0;
            loop {
                let rows = {
                    let inner = self.inner.lock().expect("store");
                    inner.live(job)?;
                    check_lease(start, until)?;
                    let mut stmt = inner.db.prepare("SELECT seq,id,data FROM entries WHERE namespace=?1 AND kind=?2 AND seq>?3 AND seq<=?4 ORDER BY seq LIMIT 32")?;
                    stmt.query_map(params![job.id, kind, after, high_water], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                };
                let Some(last) = rows.last() else {
                    break;
                };
                after = last.0;
                scanned += rows.len();
                ensure!(scanned <= 10000, "namespace full");
                let absent = absent.clone();
                let note_ids = note_ids.clone();
                replacements.extend(
                    tokio::task::spawn_blocking(move || prepare_entries(kind, rows, &absent, &note_ids)).await??,
                );
            }
        }
        Ok(InventoryPlan {
            absent: absent.iter().cloned().collect(),
            note_ids: note_ids.iter().cloned().collect(),
            replacements,
        })
    }
    fn commit_inventory(
        &self,
        mut job: Job,
        plan: InventoryPlan,
        resume: String,
        start: Instant,
        until: i64,
        permit: NamespacePermit,
    ) -> Result<()> {
        let absent = serde_json::to_string(&plan.absent)?;
        let note_ids = serde_json::to_string(&plan.note_ids)?;
        let mut db = self.lock_projection_writer()?;
        check_lease(start, until)?;
        // SQLite serializes writes, but other namespaces can read through
        // Inner's WAL connection throughout this transaction and checkpoint.
        let tx = db.transaction()?;
        check_job(&tx, &job)?;
        tx.execute(
            "DELETE FROM entries WHERE namespace=?1 AND kind='note' AND id IN (SELECT value FROM json_each(?2))",
            params![job.id, note_ids],
        )?;
        tx.execute(
            "DELETE FROM sources WHERE namespace=?1 AND session IN (SELECT value FROM json_each(?2))",
            params![job.id, absent],
        )?;
        for (id, data) in plan.replacements {
            ensure!(
                tx.execute(
                    "UPDATE entries SET data=?3 WHERE namespace=?1 AND id=?2",
                    params![job.id, id, data]
                )? == 1,
                "reset snapshot changed"
            );
        }
        tx.execute(
            "UPDATE feeds SET cursor=?2,revision=revision+1 WHERE namespace=?1",
            params![job.id, resume],
        )?;
        #[cfg(test)]
        self.reset_phase(ResetPhase::Commit);
        check_lease(start, until)?;
        tx.commit()?;
        #[cfg(test)]
        self.reset_phase(ResetPhase::Checkpoint);
        checkpoint_db(&db)?;
        job.revision += 1;
        job.cursor = Some(resume);
        permit.publish(&job, start, until)
    }
    #[cfg(test)]
    pub(crate) fn reset_phase(&self, phase: ResetPhase) {
        let hook = self.reset_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(phase);
        }
    }
    pub fn suppress(&self, id: &str, session: &str) -> Result<()> {
        ensure!(
            !session.is_empty() && session.len() <= 256 && redact::text(session).is_none(),
            "invalid session"
        );
        let mut inner = self.inner.lock().expect("store");
        let tx = inner.db.transaction()?;
        remove_source(&tx, id, session)?;
        tx.execute("UPDATE feeds SET revision=revision+1 WHERE namespace=?1", [id])?;
        tx.execute(
            "INSERT OR IGNORE INTO suppressed(namespace,session) VALUES(?1,?2)",
            params![id, session],
        )?;
        budget(&tx, id)?;
        tx.commit()?;
        inner.invalidate(id);
        checkpoint(&inner)?;
        Ok(())
    }
    pub fn forget(&self, token: &str, request: Forget) -> Result<ForgetReceipt> {
        ensure!(
            !request.request_id.is_empty()
                && request.request_id.len() <= 128
                && redact::text(&request.request_id).is_none(),
            "invalid request"
        );
        let mut inner = self.inner.lock().expect("store");
        let ns = inner.auth(token, true, false)?;
        Self::forget_inner(&mut inner, ns, request)
    }
    fn forget_inner(inner: &mut Inner, ns: Namespace, request: Forget) -> Result<ForgetReceipt> {
        ensure!(request.namespace_epoch == ns.epoch, "epoch mismatch");
        let target = serde_json::to_string(&request.target)?;
        let tx = inner.db.transaction()?;
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT epoch,target FROM receipts WHERE namespace=?1 AND request=?2",
                params![ns.id, request.request_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((epoch, old)) = old {
            ensure!(epoch == request.namespace_epoch && old == target, "request conflict");
        } else {
            let active: bool = tx.query_row("SELECT state='active' FROM namespaces WHERE id=?1", [&ns.id], |r| {
                r.get(0)
            })?;
            ensure!(active, "namespace terminal");
            match request.target {
                Target::Scope => {
                    tx.execute("DELETE FROM receipts WHERE namespace=?1", [&ns.id])?;
                    tx.execute(
                        "UPDATE namespaces SET state='purged',tool_hash=id,cursor_key='',writes=0 WHERE id=?1",
                        [&ns.id],
                    )?;
                    for table in [
                        "entries",
                        "sources",
                        "requests",
                        "suppressed",
                        "seen",
                        "feeds",
                        "imports",
                    ] {
                        tx.execute(&format!("DELETE FROM {table} WHERE namespace=?1"), [&ns.id])?;
                    }
                }
                Target::Session { ref session_id } => {
                    ensure!(
                        !session_id.is_empty() && session_id.len() <= 256 && redact::text(session_id).is_none(),
                        "invalid session"
                    );
                    remove_source(&tx, &ns.id, session_id)?;
                    tx.execute(
                        "INSERT OR IGNORE INTO suppressed(namespace,session) VALUES(?1,?2)",
                        params![ns.id, session_id],
                    )?;
                    tx.execute("UPDATE feeds SET revision=revision+1 WHERE namespace=?1", [&ns.id])?;
                }
            }
            tx.execute(
                "INSERT INTO receipts(namespace,request,epoch,target) VALUES(?1,?2,?3,?4)",
                params![ns.id, request.request_id, request.namespace_epoch, target],
            )?;
            budget(&tx, &ns.id)?;
        }
        tx.commit()?;
        inner.invalidate(&ns.id);
        inner.leases.remove(&ns.id);
        let previously: bool =
            inner
                .db
                .query_row("SELECT erasure_confirmed FROM namespaces WHERE id=?1", [&ns.id], |r| {
                    r.get(0)
                })?;
        let (busy, _, _): (i64, i64, i64) = inner.db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        let busy = if previously { 0 } else { busy };
        if busy == 0 && matches!(request.target, Target::Scope) {
            inner
                .db
                .execute("UPDATE namespaces SET erasure_confirmed=1 WHERE id=?1", [&ns.id])?;
        }
        Ok(ForgetReceipt {
            request_id: request.request_id,
            status: if busy == 0 {
                ForgetStatus::Confirmed
            } else {
                ForgetStatus::Unconfirmed
            },
            remaining_kinds: if busy == 0 {
                vec![]
            } else {
                vec!["storage_checkpoint".into()]
            },
        })
    }
    pub fn owner_purge(&self, id: &str) -> Result<ForgetReceipt> {
        let mut inner = self.inner.lock().expect("store");
        let ns = inner
            .db
            .query_row("SELECT id,epoch,cursor_key FROM namespaces WHERE id=?1", [id], |r| {
                Ok(Namespace {
                    id: r.get(0)?,
                    epoch: r.get(1)?,
                    cursor_key: r.get(2)?,
                })
            })?;
        let active: bool = inner
            .db
            .query_row("SELECT state='active' FROM namespaces WHERE id=?1", [id], |r| r.get(0))?;
        let request_id = if active {
            format!("owner-{}", crate::store::random())
        } else {
            inner.db.query_row(
                "SELECT request FROM receipts WHERE namespace=?1 AND target=?2 ORDER BY request LIMIT 1",
                params![id, serde_json::to_string(&Target::Scope)?],
                |r| r.get(0),
            )?
        };
        let epoch = ns.epoch.clone();
        Self::forget_inner(
            &mut inner,
            ns,
            Forget {
                request_id,
                namespace_epoch: epoch,
                target: Target::Scope,
            },
        )
    }
    pub fn set_writes(&self, id: &str, enabled: bool) -> Result<()> {
        let inner = self.inner.lock().expect("store");
        ensure!(
            inner.db.execute(
                "UPDATE namespaces SET writes=?2 WHERE id=?1 AND state='active'",
                params![id, enabled]
            )? == 1,
            "unavailable"
        );
        Ok(())
    }
    pub fn status(&self, id: &str) -> Result<serde_json::Value> {
        let inner = self.inner.lock().expect("store");
        let (state, registered, writes, confirmed): (String, bool, bool, bool) = inner.db.query_row(
            "SELECT state,registered,writes,erasure_confirmed FROM namespaces WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let cleanup_pending = inner.cleanup_pending(id)?;
        let lease = inner
            .leases
            .get(id)
            .filter(|(end, unix)| *end > Instant::now() && *unix > unix_now())
            .map(|(_, unix)| *unix);
        Ok(
            serde_json::json!({"state":state,"registered":registered,"writes":writes,"lease_until":lease,"stale":cleanup_pending||inner.stale(id)||registered&&lease.is_none(),"erasure_confirmed":confirmed,"cleanup_pending":cleanup_pending}),
        )
    }
    pub fn lifecycle_active(&self, token: &str) -> Result<()> {
        let inner = self.inner.lock().expect("store");
        let ns = inner.auth(token, true, false)?;
        let active: bool = inner
            .db
            .query_row("SELECT state='active' FROM namespaces WHERE id=?1", [ns.id], |r| {
                r.get(0)
            })?;
        ensure!(active, "unavailable");
        Ok(())
    }
}
impl Inner {
    fn live(&self, job: &Job) -> Result<()> {
        check_job(&self.db, job)
    }
    fn lease(&mut self, job: &Job, start: Instant, until: i64) -> Result<()> {
        ensure!(!self.writing.contains(&job.id), "reset in progress");
        ensure!(!self.cleanup_pending(&job.id)?, "storage cleanup pending");
        let end = check_lease(start, until)?;
        self.leases.insert(job.id.clone(), (end, until));
        self.stale.remove(&job.id);
        Ok(())
    }
}
fn check_job(db: &rusqlite::Connection, job: &Job) -> Result<()> {
    let current:Option<(String,i64,Option<String>)>=db.query_row("SELECT n.epoch,f.revision,f.cursor FROM feeds f JOIN namespaces n ON n.id=f.namespace WHERE f.namespace=?1 AND n.state='active'",[&job.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    ensure!(
        current == Some((job.epoch.clone(), job.revision, job.cursor.clone())),
        "stale feed result"
    );
    Ok(())
}
fn check_lease(start: Instant, until: i64) -> Result<Instant> {
    let remaining = until.checked_sub(unix_now()).context("invalid lease")?;
    ensure!(
        remaining > 0 && start.elapsed() < Duration::from_secs(60),
        "expired lease"
    );
    Ok((Instant::now() + Duration::from_secs(remaining.min(60) as u64)).min(start + Duration::from_secs(60)))
}
fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate(item: &PluginActivityItem, epoch: &str) -> Result<()> {
    ensure!(
        item.namespace_epoch == epoch
            && !item.session_id.is_empty()
            && item.session_id.len() <= 256
            && item.event_key.len() <= 512
            && !item.event_key.is_empty()
            && item.provenance.len() <= 8
            && serde_json::to_vec(&item.provenance)?.len() <= 2048,
        "invalid source"
    );
    if let ActivityBody::CompletedTurn {
        turn_id,
        project_key,
        completed_at,
        excerpts,
        source_refs,
        ..
    } = &item.body
    {
        ensure!(
            !turn_id.is_empty()
                && turn_id.len() <= 256
                && project_key.len() <= 128 * 1024
                && completed_at.len() <= 64
                && excerpts.len() <= 2
                && excerpts.iter().map(|e| e.text.len()).sum::<usize>() <= 8192
                && source_refs.len() <= 8
                && !item.provenance.is_empty(),
            "invalid completed turn"
        );
        let _ = time::OffsetDateTime::parse(completed_at, &time::format_description::well_known::Rfc3339)?;
        for source in source_refs {
            ensure!(
                source.session_id == item.session_id
                    && source.turn_id.as_deref() == Some(turn_id.as_str())
                    && source.event_id >= 0,
                "invalid reference"
            );
        }
        for provenance in &item.provenance {
            let Provenance::Source { source, origin } = provenance else {
                anyhow::bail!("unsupported source evidence");
            };
            ensure!(source_refs.contains(source), "invalid provenance");
            match origin {
                InputOrigin::Operator {} => {}
                InputOrigin::ExternalClient {
                    client_id,
                    operation_id,
                } => ensure!(
                    !client_id.is_empty()
                        && client_id.len() <= 256
                        && !operation_id.is_empty()
                        && operation_id.len() <= 256,
                    "invalid external client evidence"
                ),
                InputOrigin::PluginContext { .. } => anyhow::bail!("injected context is not source material"),
                InputOrigin::Parent {
                    session_id,
                    operation_id,
                } => ensure!(
                    !session_id.is_empty() && session_id.len() <= 256 && operation_id.len() <= 256,
                    "invalid parent evidence"
                ),
                InputOrigin::ChildReport {
                    child_id,
                    source_event_id,
                    delivery_id,
                } => ensure!(
                    child_id.len() <= 256 && source_event_id.len() <= 256 && delivery_id.len() <= 256,
                    "invalid child evidence"
                ),
            }
        }
    }
    Ok(())
}
pub(crate) fn checkpoint(inner: &Inner) -> Result<()> {
    checkpoint_db(&inner.db)
}
pub(crate) fn checkpoint_db(db: &rusqlite::Connection) -> Result<()> {
    let (busy, _, _): (i64, i64, i64) = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    ensure!(busy == 0, "storage checkpoint unconfirmed");
    Ok(())
}
fn observation(item: &PluginActivityItem) -> Result<Observation> {
    let ActivityBody::CompletedTurn {
        turn_id,
        completed_at,
        excerpts,
        source_refs,
        truncated,
        gaps,
        ..
    } = &item.body
    else {
        anyhow::bail!("not a completed turn");
    };
    Ok(Observation {
        event_key: item.event_key.clone(),
        turn_id: turn_id.clone(),
        completed_at: completed_at.clone(),
        excerpts: excerpts.clone(),
        source_refs: source_refs.clone(),
        provenance: item.provenance.clone(),
        truncated: *truncated,
        gaps: *gaps,
    })
}
fn render_note(entry: &mut Entry) -> Result<()> {
    loop {
        entry.body.clear();
        entry.provenance.clear();
        entry.source_refs.clear();
        for turn in &entry.turns {
            entry.body.push_str(&format!("[{}]\n", turn.completed_at));
            entry.provenance.extend(turn.provenance.clone());
            entry.source_refs.extend(turn.source_refs.clone());
            entry.truncated |= turn.truncated;
            entry.stale |= turn.gaps;
            for excerpt in &turn.excerpts {
                entry.body.push_str(&format!("{:?}: {}\n", excerpt.role, excerpt.text));
            }
        }
        if entry.body.len() <= 8192 {
            return Ok(());
        }
        entry.truncated = true;
        for turn in &mut entry.turns {
            for excerpt in &mut turn.excerpts {
                excerpt.text = bounded(&excerpt.text, excerpt.text.len() / 2);
            }
            turn.truncated = true;
        }
    }
}
fn prepare_entries(
    kind: &str,
    rows: Vec<(i64, String, String)>,
    absent: &std::collections::BTreeSet<String>,
    note_ids: &std::collections::BTreeSet<String>,
) -> Result<Vec<(String, String)>> {
    let mut changes = Vec::new();
    for (_, id, data) in rows {
        if kind == "note" && note_ids.contains(&id) {
            continue;
        }
        let mut entry: Entry = serde_json::from_str(&data)?;
        let changed = if kind == "note" {
            entry.stale = true;
            for turn in &mut entry.turns {
                turn.gaps = true;
            }
            true
        } else {
            let refs = entry.source_refs.len();
            let provenance = entry.provenance.len();
            entry.source_refs.retain(|r| !absent.contains(&r.session_id));
            entry
                .provenance
                .retain(|p| !matches!(p, Provenance::Source { source, .. } if absent.contains(&source.session_id)));
            let changed = refs != entry.source_refs.len() || provenance != entry.provenance.len();
            if changed && entry.provenance.is_empty() {
                entry.provenance.push(Provenance::Unverified {});
            }
            changed
        };
        if changed {
            changes.push((id, serde_json::to_string(&entry)?));
        }
    }
    Ok(changes)
}
pub(crate) fn remove_source(tx: &rusqlite::Transaction<'_>, ns: &str, session: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM entries WHERE namespace=?1 AND id=?2 AND kind='note'",
        params![ns, sha256_hex(format!("native-note:{session}").as_bytes())],
    )?;
    tx.execute(
        "DELETE FROM sources WHERE namespace=?1 AND session=?2",
        params![ns, session],
    )?;
    let entries = {
        let mut stmt = tx.prepare("SELECT data FROM entries WHERE namespace=?1 AND kind!='note'")?;
        stmt.query_map([ns], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for data in entries {
        let mut e: Entry = serde_json::from_str(&data)?;
        let old = e.source_refs.len();
        e.source_refs.retain(|r| r.session_id != session);
        e.provenance
            .retain(|p| !matches!(p,Provenance::Source{source,..} if source.session_id==session));
        if old != e.source_refs.len() {
            if e.provenance.is_empty() {
                e.provenance.push(Provenance::Unverified {});
            }
            put(tx, ns, &e)?;
        }
    }
    Ok(())
}
pub(crate) fn budget(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<()> {
    let (count, bytes): (i64, i64) = tx.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(data AS BLOB))),0) FROM entries WHERE namespace=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(count <= 10000 && bytes <= 64 * 1024 * 1024, "namespace full");
    for table in ["sources", "seen", "suppressed", "receipts"] {
        let count: i64 = tx.query_row(&format!("SELECT count(*) FROM {table} WHERE namespace=?1"), [id], |r| {
            r.get(0)
        })?;
        ensure!(count <= 100000, "namespace full");
    }
    Ok(())
}
pub async fn worker(store: Arc<Store>, key: Arc<MasterKey>) {
    let mut running = std::collections::HashSet::new();
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        while let Some(result) = tasks.try_join_next() {
            if let Ok(id) = result {
                running.remove(&id);
            }
        }
        if let Ok(ids) = store.feed_ids() {
            for id in ids {
                if running.insert(id.clone()) {
                    let store = store.clone();
                    let key = key.clone();
                    tasks.spawn(async move {
                        let Ok(consumer) = Consumer::new() else {
                            return id;
                        };
                        let mut backoff = 1;
                        loop {
                            if !store.feed_ids().is_ok_and(|ids| ids.contains(&id)) {
                                break;
                            }
                            let delay = match consumer.poll(&store, &key, &id).await {
                                Ok(more) => {
                                    backoff = 1;
                                    if more { BACKLOG_DELAY } else { Duration::from_secs(15) }
                                }
                                Err(_) => {
                                    let delay = Duration::from_secs(backoff);
                                    backoff = (backoff * 2).min(300);
                                    delay
                                }
                            };
                            tokio::time::sleep(delay).await;
                        }
                        id
                    });
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod reset_tests {
    use super::*;

    #[test]
    fn cleanup_recovery_io_keeps_its_namespace_fenced_and_foreign_reads_live() {
        use crate::management::NewMemory;
        use std::sync::{Mutex, mpsc};
        for phase in [ResetPhase::CleanupCheckpoint, ResetPhase::CleanupConfirmed] {
            let (dir, store, _attached, _key) = fixture();
            let c = store.create("owner", "standalone").unwrap();
            let foreign = store.create("other", "hat").unwrap();
            let manager = store.filesystem_manager("owner").unwrap();
            let entry = store
                .managed_create(
                    &manager,
                    &c.namespace,
                    NewMemory {
                        title: "reference".into(),
                        body: "recovery I/O marker".into(),
                    },
                )
                .unwrap();
            let reader = hennery_kernel::db::open(&dir.path().join("memory.db")).unwrap();
            reader.execute_batch("BEGIN").unwrap();
            let _: i64 = reader
                .query_row("SELECT count(*) FROM entries WHERE namespace=?1", [&c.namespace], |r| {
                    r.get(0)
                })
                .unwrap();
            assert!(
                store
                    .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
                    .is_err()
            );
            reader.execute_batch("ROLLBACK").unwrap();
            let (arrived_tx, arrived_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            *store.reset_hook.lock().unwrap() = Some(Arc::new(move |at| {
                if at == phase {
                    arrived_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(20))
                        .unwrap();
                }
            }));
            let recovering = store.clone();
            let recovery = std::thread::spawn(move || recovering.recover_cleanup());
            arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            let checking = store.clone();
            let manager_check = manager.clone();
            let namespace = c.namespace.clone();
            let own = c.tool.clone();
            let (read_tx, read_rx) = mpsc::channel();
            let reading = std::thread::spawn(move || {
                checking.available(&foreign.tool, false).unwrap();
                assert!(checking.available(&own, false).is_err());
                assert!(checking.managed_snapshot(&manager_check, &namespace).is_err());
                read_tx.send(()).unwrap();
            });
            let interleaved = read_rx.recv_timeout(Duration::from_secs(5));
            release_tx.send(()).unwrap();
            recovery.join().unwrap().unwrap();
            reading.join().unwrap();
            assert!(interleaved.is_ok(), "recovery {phase:?} held the shared mutex");
            assert_cleanup_recovered(&dir, &store, &c, &manager, "recovery I/O marker");
        }
    }

    fn assert_cleanup_recovered(
        dir: &tempfile::TempDir,
        store: &Store,
        c: &crate::store::Credentials,
        manager: &crate::management::Manager,
        marker: &str,
    ) {
        assert_eq!(store.status(&c.namespace).unwrap()["cleanup_pending"], false);
        let snapshot = store.managed_snapshot(manager, &c.namespace).unwrap();
        assert!(snapshot.entries.is_empty());
        store
            .managed_deliverable(manager, &c.namespace, snapshot.generation)
            .unwrap();
        store.available(&c.tool, false).unwrap();
        for name in ["memory.db", "memory.db-wal"] {
            if let Ok(bytes) = std::fs::read(dir.path().join(name)) {
                assert!(
                    !bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
                    "erased marker remains in {name}"
                );
            }
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_worker_retries_panics_and_busy_checkpoints_without_a_feed() {
        use crate::management::NewMemory;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("memory.db")).unwrap();
        assert!(!store.has_feed_credentials().unwrap());
        let c = store.create("owner", "standalone").unwrap();
        let manager = store.filesystem_manager("owner").unwrap();
        let entry = store
            .managed_create(
                &manager,
                &c.namespace,
                NewMemory {
                    title: "reference".into(),
                    body: "worker cleanup marker".into(),
                },
            )
            .unwrap();
        let reader = hennery_kernel::db::open(&dir.path().join("memory.db")).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT count(*) FROM entries WHERE namespace=?1", [&c.namespace], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            store
                .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
                .is_err()
        );
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let attempts = AtomicUsize::new(0);
        *store.reset_hook.lock().unwrap() = Some(Arc::new(move |at| {
            if at == ResetPhase::CleanupCheckpoint {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                let _ = attempt_tx.send(attempt);
                if attempt == 0 {
                    panic!("synthetic recovery unwind");
                }
            }
        }));
        let worker = tokio::spawn(crate::management::cleanup_worker(store.clone()));
        tokio::task::spawn_blocking(move || {
            assert_eq!(attempt_rx.recv_timeout(Duration::from_secs(10)).unwrap(), 0);
            assert_eq!(attempt_rx.recv_timeout(Duration::from_secs(10)).unwrap(), 1);
            // A third attempt proves the second returned busy while the
            // reader was still pinned, rather than succeeding after release.
            assert_eq!(attempt_rx.recv_timeout(Duration::from_secs(10)).unwrap(), 2);
        })
        .await
        .unwrap();
        assert!(store.available(&c.tool, false).is_err());
        assert!(store.managed_snapshot(&manager, &c.namespace).is_err());
        reader.execute_batch("ROLLBACK").unwrap();
        *store.reset_hook.lock().unwrap() = None;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if store.status(&c.namespace).unwrap()["cleanup_pending"] == false {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        worker.abort();
        let _ = worker.await;
        assert!(!store.projection_writer.is_poisoned());
        assert_cleanup_recovered(&dir, &store, &c, &manager, "worker cleanup marker");
    }
    #[test]
    fn precommit_unwind_rolls_back_cleanup_fence_and_keeps_the_old_entry() {
        use crate::management::NewMemory;
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let (_dir, store, c, key) = fixture();
        let manager = store.filesystem_manager("owner").unwrap();
        let job = store.job(&key, &c.namespace).unwrap();
        store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
        let entry = store
            .managed_create(
                &manager,
                &c.namespace,
                NewMemory {
                    title: "reference".into(),
                    body: "still retained".into(),
                },
            )
            .unwrap();
        *store.reset_hook.lock().unwrap() = Some(Arc::new(|at| {
            if at == ResetPhase::ManagedCommit {
                panic!("before deletion commit");
            }
        }));
        assert!(
            catch_unwind(AssertUnwindSafe(|| store.managed_delete(
                &manager,
                &c.namespace,
                &entry.id,
                entry.revision
            )))
            .is_err()
        );
        *store.reset_hook.lock().unwrap() = None;
        assert_eq!(store.status(&c.namespace).unwrap()["cleanup_pending"], false);
        assert_eq!(
            store.managed_entry(&manager, &c.namespace, &entry.id).unwrap().body,
            "still retained"
        );
        store.available(&c.tool, false).unwrap();
        store
            .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
            .unwrap();
        assert!(!store.projection_writer.is_poisoned());
    }
    #[test]
    fn cleanup_confirmation_unwind_stays_fenced_until_a_retry() {
        use crate::management::NewMemory;
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let (dir, store, _attached, _key) = fixture();
        let c = store.create("owner", "standalone").unwrap();
        let manager = store.filesystem_manager("owner").unwrap();
        let entry = store
            .managed_create(
                &manager,
                &c.namespace,
                NewMemory {
                    title: "reference".into(),
                    body: "confirmation cleanup marker".into(),
                },
            )
            .unwrap();
        *store.reset_hook.lock().unwrap() = Some(Arc::new(|at| {
            if at == ResetPhase::CleanupConfirmed {
                panic!("after physical checkpoint before fence clear");
            }
        }));
        assert!(
            catch_unwind(AssertUnwindSafe(|| store.managed_delete(
                &manager,
                &c.namespace,
                &entry.id,
                entry.revision
            )))
            .is_err()
        );
        *store.reset_hook.lock().unwrap() = None;
        assert!(store.available(&c.tool, false).is_err());
        assert!(store.managed_snapshot(&manager, &c.namespace).is_err());
        store.recover_cleanup().unwrap();
        assert_cleanup_recovered(&dir, &store, &c, &manager, "confirmation cleanup marker");
    }

    #[tokio::test]
    async fn pending_managed_cleanup_survives_reopen_for_standalone_and_registered_namespaces() {
        use crate::management::NewMemory;
        for registered in [false, true] {
            let (dir, store, attached, key) = fixture();
            let c = if registered {
                attached
            } else {
                store.create("owner", "standalone").unwrap()
            };
            let manager = store.filesystem_manager("owner").unwrap();
            if registered {
                let job = store.job(&key, &c.namespace).unwrap();
                store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
            }
            let entry = store
                .managed_create(
                    &manager,
                    &c.namespace,
                    NewMemory {
                        title: "reference".into(),
                        body: "durable cleanup marker".into(),
                    },
                )
                .unwrap();
            let stamp = store.managed_snapshot(&manager, &c.namespace).unwrap().generation;
            let reader = hennery_kernel::db::open(&dir.path().join("memory.db")).unwrap();
            reader.execute_batch("BEGIN").unwrap();
            let _: i64 = reader
                .query_row("SELECT count(*) FROM entries WHERE namespace=?1", [&c.namespace], |r| {
                    r.get(0)
                })
                .unwrap();
            assert!(
                store
                    .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
                    .is_err()
            );
            drop(store);
            let reopened = Store::open(&dir.path().join("memory.db")).unwrap();
            if registered {
                let job = reopened.job(&key, &c.namespace).unwrap();
                assert!(reopened.renew(&job, Instant::now(), unix_now() + 60).is_err());
            }
            let denied = reopened.managed_snapshot(&manager, &c.namespace).is_err()
                && reopened.managed_deliverable(&manager, &c.namespace, stamp).is_err()
                && reopened.available(&c.tool, false).is_err();
            assert!(reopened.recover_cleanup().is_err());
            reader.execute_batch("ROLLBACK").unwrap();
            assert!(
                denied,
                "restart reopened reads before cleanup confirmation: registered={registered}"
            );
            reopened.recover_cleanup().unwrap();
            if registered {
                let job = reopened.job(&key, &c.namespace).unwrap();
                reopened.renew(&job, Instant::now(), unix_now() + 60).unwrap();
            }
            assert_cleanup_recovered(&dir, &reopened, &c, &manager, "durable cleanup marker");
        }
    }

    #[test]
    fn managed_checkpoint_error_retains_namespace_fence() {
        use crate::management::NewMemory;
        let (dir, store, c, key) = fixture();
        let manager = store.filesystem_manager("owner").unwrap();
        let job = store.job(&key, &c.namespace).unwrap();
        store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
        let entry = store
            .managed_create(
                &manager,
                &c.namespace,
                NewMemory {
                    title: "reference".into(),
                    body: "pending erasure marker".into(),
                },
            )
            .unwrap();
        let reader = hennery_kernel::db::open(&dir.path().join("memory.db")).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let _: i64 = reader
            .query_row("SELECT count(*) FROM entries WHERE namespace=?1", [&c.namespace], |r| {
                r.get(0)
            })
            .unwrap();
        let error = store
            .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
            .unwrap_err();
        assert!(error.to_string().contains("checkpoint unconfirmed"));
        let snapshot = store.managed_snapshot(&manager, &c.namespace);
        let export_allowed = snapshot
            .as_ref()
            .is_ok_and(|s| store.managed_deliverable(&manager, &c.namespace, s.generation).is_ok());
        let tool_allowed = store.available(&c.tool, false).is_ok();
        let current = *store.inner.lock().unwrap().generations.get(&c.namespace).unwrap();
        assert!(store.managed_deliverable(&manager, &c.namespace, current).is_err());
        let marker_remains = std::fs::read(dir.path().join("memory.db-wal"))
            .unwrap()
            .windows(b"pending erasure marker".len())
            .any(|v| v == b"pending erasure marker");
        assert!(store.recover_cleanup().is_err());
        assert_eq!(store.status(&c.namespace).unwrap()["cleanup_pending"], true);
        reader.execute_batch("ROLLBACK").unwrap();
        eprintln!(
            "checkpoint error: {error}; snapshot={}, export={export_allowed}, MCP={tool_allowed}, old WAL marker={marker_remains}",
            snapshot.is_ok()
        );
        assert!(
            snapshot.is_err() && !export_allowed && !tool_allowed,
            "unconfirmed management checkpoint reopened namespace reads"
        );
        assert!(marker_remains);
        store.recover_cleanup().unwrap();
        assert_cleanup_recovered(&dir, &store, &c, &manager, "pending erasure marker");
    }

    #[test]
    fn managed_checkpoint_panic_retains_namespace_fence() {
        use crate::management::NewMemory;
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let (dir, store, c, key) = fixture();
        let manager = store.filesystem_manager("owner").unwrap();
        let job = store.job(&key, &c.namespace).unwrap();
        store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
        let entry = store
            .managed_create(
                &manager,
                &c.namespace,
                NewMemory {
                    title: "reference".into(),
                    body: "pending erasure marker".into(),
                },
            )
            .unwrap();
        *store.reset_hook.lock().unwrap() = Some(Arc::new(|at| {
            if at == ResetPhase::ManagedCheckpoint {
                panic!("synthetic checkpoint unwind");
            }
        }));
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                store
                    .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
                    .unwrap();
            }))
            .is_err()
        );
        *store.reset_hook.lock().unwrap() = None;
        assert!(store.projection_writer.is_poisoned());
        let snapshot = store.managed_snapshot(&manager, &c.namespace);
        let export_allowed = snapshot
            .as_ref()
            .is_ok_and(|s| store.managed_deliverable(&manager, &c.namespace, s.generation).is_ok());
        let tool_allowed = store.available(&c.tool, false).is_ok();
        let current = *store.inner.lock().unwrap().generations.get(&c.namespace).unwrap();
        assert!(store.managed_deliverable(&manager, &c.namespace, current).is_err());
        let marker_remains = std::fs::read(dir.path().join("memory.db-wal"))
            .unwrap()
            .windows(b"pending erasure marker".len())
            .any(|v| v == b"pending erasure marker");
        eprintln!(
            "checkpoint panic: snapshot={}, export={export_allowed}, MCP={tool_allowed}, old WAL marker={marker_remains}",
            snapshot.is_ok()
        );
        assert!(
            snapshot.is_err() && !export_allowed && !tool_allowed,
            "unwound management checkpoint reopened namespace reads"
        );
        assert!(marker_remains);
        store.recover_cleanup().unwrap();
        assert!(!store.projection_writer.is_poisoned());
        assert_cleanup_recovered(&dir, &store, &c, &manager, "pending erasure marker");
    }

    #[tokio::test]
    async fn import_and_curated_writes_are_fenced_against_reset_and_late_purge() {
        use crate::management::NewMemory;
        use std::sync::{Mutex, mpsc};
        for import in [true, false] {
            let (_dir, store, c, key) = fixture();
            let manager = store.filesystem_manager("owner").unwrap();
            let job = store.job(&key, &c.namespace).unwrap();
            store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
            let preview = import_preview(&store, &manager, &c.namespace);
            let namespace = c.namespace.clone();
            let (arrived_tx, arrived_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            *store.reset_hook.lock().unwrap() = Some(Arc::new(move |at| {
                if at == ResetPhase::ManagedReady {
                    arrived_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(20))
                        .unwrap();
                }
            }));
            let writing = store.clone();
            let worker = std::thread::spawn(move || {
                if import {
                    let result = writing.apply_import(&manager, &preview, &preview.digest).unwrap();
                    result[0].status == "untouched"
                } else {
                    writing
                        .managed_create(
                            &manager,
                            &namespace,
                            NewMemory {
                                title: "reference".into(),
                                body: "late mutation".into(),
                            },
                        )
                        .is_err()
                }
            });
            arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert!(
                store.begin_reset(&job).is_err(),
                "reset must not overlap a reserved management write"
            );
            assert!(store.renew(&job, Instant::now(), unix_now() + 60).is_err());
            assert!(store.available(&c.tool, false).is_err());
            let receipt = store.owner_purge(&c.namespace).unwrap();
            release_tx.send(()).unwrap();
            assert!(
                worker.join().unwrap(),
                "a captured management write committed after scope purge"
            );
            assert_eq!(receipt.status, ForgetStatus::Confirmed);
            let remaining: i64 = store
                .inner
                .lock()
                .unwrap()
                .db
                .query_row("SELECT count(*) FROM entries WHERE namespace=?1", [&c.namespace], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(
                remaining, 0,
                "a confirmed purge must not be followed by stale import data"
            );
            assert!(store.available(&c.tool, false).is_err());
            assert!(!store.inner.lock().unwrap().writing.contains(&c.namespace));
        }
    }
    #[tokio::test]
    async fn import_and_curated_writes_recheck_the_feed_after_acquiring_the_transaction() {
        use crate::management::NewMemory;
        use std::sync::{Mutex, mpsc};
        for import in [true, false] {
            let (_dir, store, c, key) = fixture();
            let manager = store.filesystem_manager("owner").unwrap();
            let job = store.job(&key, &c.namespace).unwrap();
            store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
            let preview = import_preview(&store, &manager, &c.namespace);
            let namespace = c.namespace.clone();
            let (arrived_tx, arrived_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            *store.reset_hook.lock().unwrap() = Some(Arc::new(move |at| {
                if at == ResetPhase::ManagedReady {
                    arrived_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(20))
                        .unwrap();
                }
            }));
            let writing = store.clone();
            let worker = std::thread::spawn(move || {
                if import {
                    writing.apply_import(&manager, &preview, &preview.digest).unwrap()[0].status == "untouched"
                } else {
                    writing
                        .managed_create(
                            &manager,
                            &namespace,
                            NewMemory {
                                title: "reference".into(),
                                body: "stale write".into(),
                            },
                        )
                        .is_err()
                }
            });
            arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            // An explicit owner reattachment can supersede the captured feed
            // while its management writer is still waiting to start SQL.
            store.attach(&c.namespace, job.config.clone(), &key).unwrap();
            release_tx.send(()).unwrap();
            assert!(
                worker.join().unwrap(),
                "stale feed revision admitted a management write"
            );
            assert!(
                store
                    .managed_snapshot(&store.filesystem_manager("owner").unwrap(), &c.namespace)
                    .is_err()
            );
            let fresh = store.job(&key, &c.namespace).unwrap();
            store.renew(&fresh, Instant::now(), unix_now() + 60).unwrap();
            assert!(
                store
                    .managed_snapshot(&store.filesystem_manager("owner").unwrap(), &c.namespace)
                    .unwrap()
                    .entries
                    .is_empty()
            );
        }
    }
    #[tokio::test]
    async fn managed_lease_expiry_before_commit_rolls_back_import_and_curated_sql() {
        use crate::management::NewMemory;
        for import in [true, false] {
            let (_dir, store, c, key) = fixture();
            let manager = store.filesystem_manager("owner").unwrap();
            let job = store.job(&key, &c.namespace).unwrap();
            store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
            let preview = import_preview(&store, &manager, &c.namespace);
            store.inner.lock().unwrap().leases.insert(
                c.namespace.clone(),
                (Instant::now() + Duration::from_secs(1), unix_now() + 60),
            );
            *store.reset_hook.lock().unwrap() = Some(Arc::new(|at| {
                if at == ResetPhase::ManagedCommit {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }));
            if import {
                assert_eq!(
                    store.apply_import(&manager, &preview, &preview.digest).unwrap()[0].status,
                    "untouched"
                );
            } else {
                assert!(
                    store
                        .managed_create(
                            &manager,
                            &c.namespace,
                            NewMemory {
                                title: "reference".into(),
                                body: "expired write".into()
                            }
                        )
                        .is_err()
                );
            }
            *store.reset_hook.lock().unwrap() = None;
            let fresh = store.job(&key, &c.namespace).unwrap();
            assert_eq!(
                fresh.revision, job.revision,
                "expired SQL must not advance the feed revision"
            );
            store.renew(&fresh, Instant::now(), unix_now() + 60).unwrap();
            assert!(
                store
                    .managed_snapshot(&manager, &c.namespace)
                    .unwrap()
                    .entries
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn managed_lease_expiry_while_waiting_does_not_run_the_mutation() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (_dir, store, c, key) = fixture();
        let manager = store.filesystem_manager("owner").unwrap();
        let job = store.job(&key, &c.namespace).unwrap();
        store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
        store.inner.lock().unwrap().leases.insert(
            c.namespace.clone(),
            (Instant::now() + Duration::from_secs(1), unix_now() + 60),
        );
        *store.reset_hook.lock().unwrap() = Some(Arc::new(|at| {
            if at == ResetPhase::ManagedReady {
                std::thread::sleep(Duration::from_secs(2));
            }
        }));
        let invoked = AtomicBool::new(false);
        let result = store.managed_transaction(&manager, &c.namespace, true, false, None, |_| {
            invoked.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(result.is_err());
        assert!(
            !invoked.load(Ordering::SeqCst),
            "expired admission must fail before running the mutation"
        );
        assert!(!store.inner.lock().unwrap().writing.contains(&c.namespace));
    }

    fn activity_page(event: &str, cursor: &str) -> PluginActivityPage {
        use serde_json::json;
        let source = json!({"session_id":"removed", "turn_id":event, "event_id":7});
        serde_json::from_value(json!({
            "version":1, "next_cursor":cursor, "has_more":false,
            "lease_until":unix_now()+60,
            "items":[{
                "kind":"completed_turn", "event_key":event, "session_id":"removed",
                "namespace_epoch":"a".repeat(64),
                "provenance":[{"kind":"source", "origin":{"kind":"operator"}, "source":source}],
                "turn_id":event, "project_key":"host:/scratch/project",
                "completed_at":"2026-10-15T10:00:00Z",
                "excerpts":[{"role":"assistant", "text":"resurrection marker"}],
                "source_refs":[source], "truncated":false, "gaps":false
            }]
        }))
        .unwrap()
    }

    fn import_preview(store: &Store, manager: &crate::management::Manager, namespace: &str) -> crate::import::Preview {
        use crate::import::{Bundle, Mapping, Target, VaultFile};
        store
            .preview_import(
                manager,
                &Bundle {
                    version: 1,
                    vault: "synthetic".into(),
                    rejected: vec![],
                    files: vec![VaultFile {
                        path: "memories/garden/reference.md".into(),
                        body: "import marker".into(),
                    }],
                },
                &Mapping {
                    scopes: std::collections::BTreeMap::from([(
                        "garden".into(),
                        Target::Namespace {
                            namespace: namespace.into(),
                            project_key: None,
                        },
                    )]),
                    files: Default::default(),
                    mixed: Default::default(),
                },
            )
            .unwrap()
    }
    async fn stalled_management(import: bool) {
        use crate::management::{Edit, NewMemory};
        use std::sync::{Mutex, mpsc};
        let phases = if import {
            vec![ResetPhase::ManagedCommit]
        } else {
            vec![ResetPhase::ManagedCommit, ResetPhase::ManagedCheckpoint]
        };
        for phase in phases {
            for delete in if import { vec![false] } else { vec![false, true] } {
                let (_dir, store, c, key) = fixture();
                let manager = store.filesystem_manager("owner").unwrap();
                let target = store.create("owner", "garden").unwrap();
                let foreign = store.create("other", "garden").unwrap();
                let entry = store
                    .managed_create(
                        &manager,
                        &target.namespace,
                        NewMemory {
                            title: "reference".into(),
                            body: "before edit".into(),
                        },
                    )
                    .unwrap();
                let preview = import_preview(&store, &manager, &target.namespace);
                let job = store.job(&key, &c.namespace).unwrap();
                let (arrived_tx, arrived_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let release_rx = Mutex::new(release_rx);
                *store.reset_hook.lock().unwrap() = Some(Arc::new(move |at| {
                    if at == phase {
                        arrived_tx.send(()).unwrap();
                        release_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(20))
                            .unwrap();
                    }
                }));
                let checking_manager = manager.clone();
                let checking_namespace = target.namespace.clone();
                let checking_tool = target.tool.clone();
                let writing = store.clone();
                let writer = std::thread::spawn(move || {
                    if import {
                        let results = writing.apply_import(&manager, &preview, &preview.digest).unwrap();
                        assert_eq!(results[0].status, "committed");
                    } else if delete {
                        writing
                            .managed_delete(&manager, &target.namespace, &entry.id, entry.revision)
                            .unwrap();
                    } else {
                        writing
                            .managed_edit(
                                &manager,
                                &target.namespace,
                                &entry.id,
                                Edit {
                                    revision: entry.revision,
                                    title: entry.title,
                                    body: "after edit".into(),
                                },
                            )
                            .unwrap();
                    }
                });
                arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap();
                let (read_tx, read_rx) = mpsc::channel();
                let reading = store.clone();
                let reader = std::thread::spawn(move || {
                    let permit = reading.begin_reset(&job).unwrap();
                    reading.available(&foreign.tool, false).unwrap();
                    assert!(reading.available(&checking_tool, false).is_err());
                    assert!(
                        reading
                            .managed_snapshot(&checking_manager, &checking_namespace)
                            .is_err()
                    );
                    drop(permit);
                    read_tx.send(()).unwrap();
                });
                let interleaved = read_rx.recv_timeout(Duration::from_secs(5));
                release_tx.send(()).unwrap();
                writer.join().unwrap();
                reader.join().unwrap();
                assert!(
                    interleaved.is_ok(),
                    "management I/O held the shared mutex: import={import}, delete={delete}, phase={phase:?}"
                );
            }
        }
    }
    #[tokio::test]
    async fn stalled_import_apply_allows_reset_and_foreign_reads() {
        stalled_management(true).await;
    }
    #[tokio::test]
    async fn stalled_curated_writes_allow_reset_and_foreign_reads() {
        stalled_management(false).await;
    }
    #[tokio::test]
    async fn managed_reads_obey_the_reset_namespace_fence() {
        let (_dir, store, c, key) = fixture();
        let manager = store.filesystem_manager("owner").unwrap();
        let job = store.job(&key, &c.namespace).unwrap();
        let start = Instant::now();
        let until = unix_now() + 60;
        store.renew(&job, start, until).unwrap();
        let permit = store.begin_reset(&job).unwrap();
        // Retaining an earlier lease must never bypass the explicit reset fence.
        store
            .inner
            .lock()
            .unwrap()
            .leases
            .insert(c.namespace.clone(), (start + Duration::from_secs(60), until));
        assert!(
            store.managed_snapshot(&manager, &c.namespace).is_err(),
            "management bypassed reset fence with old lease"
        );
        drop(permit);
    }
    #[tokio::test]
    async fn curated_and_import_writes_invalidate_prepared_reset_jobs() {
        use crate::management::{Edit, NewMemory};
        for operation in ["create", "edit", "pin", "delete", "import", "annotate", "retain"] {
            let (_dir, store, c, key) = fixture();
            let manager = store.filesystem_manager("owner").unwrap();
            let job = store.job(&key, &c.namespace).unwrap();
            store.renew(&job, Instant::now(), unix_now() + 60).unwrap();
            let entry = if ["annotate", "retain"].contains(&operation) {
                store
                    .apply(&job, activity_page("seed", "source"), Instant::now(), unix_now() + 60)
                    .unwrap();
                store
                    .search(&c.tool, &crate::store::Query::search("resurrection"))
                    .unwrap()
                    .entries
                    .remove(0)
            } else {
                store
                    .managed_create(
                        &manager,
                        &c.namespace,
                        NewMemory {
                            title: "reference".into(),
                            body: "initial".into(),
                        },
                    )
                    .unwrap()
            };
            let preview = import_preview(&store, &manager, &c.namespace);
            let job = store.job(&key, &c.namespace).unwrap();
            // Preparation itself is read-only; reserve after this competing
            // mutation to prove its captured durable job is invalidated.
            let _plan = store
                .prepare_inventory(&job, Default::default(), Instant::now(), unix_now() + 60)
                .await
                .unwrap();
            match operation {
                "create" => {
                    store
                        .managed_create(
                            &manager,
                            &c.namespace,
                            NewMemory {
                                title: "new".into(),
                                body: "new".into(),
                            },
                        )
                        .unwrap();
                }
                "edit" => {
                    store
                        .managed_edit(
                            &manager,
                            &c.namespace,
                            &entry.id,
                            Edit {
                                revision: entry.revision,
                                title: entry.title,
                                body: "edited".into(),
                            },
                        )
                        .unwrap();
                }
                "pin" => {
                    store
                        .managed_pin(&manager, &c.namespace, &entry.id, entry.revision, true)
                        .unwrap();
                }
                "delete" => {
                    store
                        .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
                        .unwrap();
                }
                "import" => {
                    assert_eq!(
                        store.apply_import(&manager, &preview, &preview.digest).unwrap()[0].status,
                        "committed"
                    );
                }
                "annotate" => {
                    store
                        .managed_annotate(&manager, &c.namespace, &entry.id, entry.revision, "owner annotation")
                        .unwrap();
                }
                "retain" => {
                    store
                        .managed_retain(
                            &manager,
                            &c.namespace,
                            &entry.id,
                            entry.revision,
                            "retained",
                            &entry.body,
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            assert!(
                store.begin_reset(&job).is_err(),
                "{operation} did not invalidate an older prepared reset"
            );
        }
    }

    #[tokio::test]
    async fn poisoned_reset_with_unconfirmed_rollback_stays_fenced() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let (_dir, store, c, key) = fixture();
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let db = store.projection_writer.lock().unwrap();
                // Model an incomplete rollback: recovery must not silently discard
                // an unresolved transaction or clear the poisoned state.
                db.execute_batch("BEGIN IMMEDIATE").unwrap();
                panic!("injected incomplete unwind");
            }))
            .is_err()
        );
        let job = store.job(&key, &c.namespace).unwrap();
        let start = Instant::now();
        let until = unix_now() + 60;
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            store.commit_inventory(job, plan, "unconfirmed".into(), start, until, permit)
        }));
        assert!(result.is_ok(), "uncertain recovery must return an error, not panic");
        assert!(result.unwrap().is_err());
        assert!(store.projection_writer.is_poisoned());
        assert!(store.available(&c.tool, false).is_err());
        assert!(store.job(&key, &c.namespace).unwrap().cursor.is_none());
        store
            .projection_writer
            .lock()
            .unwrap_err()
            .into_inner()
            .execute_batch("ROLLBACK")
            .unwrap();
        let job = store.job(&key, &c.namespace).unwrap();
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        store
            .commit_inventory(job, plan, "retry".into(), start, until, permit)
            .unwrap();
        store.available(&c.tool, false).unwrap();
    }

    #[tokio::test]
    async fn stale_activity_cannot_overwrite_a_committed_inventory() {
        use std::sync::{Mutex, mpsc};
        let (_dir, store, c, key) = fixture();
        let store = Arc::new(store);
        let page = activity_page;
        let job = store.job(&key, &c.namespace).unwrap();
        store
            .apply(&job, page("seed", "first"), Instant::now(), unix_now() + 60)
            .unwrap();
        let job = store.job(&key, &c.namespace).unwrap();
        let start = Instant::now();
        let until = unix_now() + 60;
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        let (arrived_tx, arrived_rx) = mpsc::channel();
        let (commit_tx, commit_rx) = mpsc::channel();
        let (apply_tx, apply_rx) = mpsc::channel();
        let (checkpoint_tx, checkpoint_rx) = mpsc::channel();
        let commit_rx = Mutex::new(commit_rx);
        let apply_rx = Mutex::new(apply_rx);
        let checkpoint_rx = Mutex::new(checkpoint_rx);
        *store.reset_hook.lock().unwrap() = Some(Arc::new(move |phase| {
            arrived_tx.send(phase).unwrap();
            let rx = match phase {
                ResetPhase::Commit => &commit_rx,
                ResetPhase::Apply => &apply_rx,
                ResetPhase::Checkpoint => &checkpoint_rx,
                _ => return,
            };
            rx.lock().unwrap().recv_timeout(Duration::from_secs(20)).unwrap();
        }));
        let writing = store.clone();
        let reset_job = job.clone();
        let writer = std::thread::spawn(move || {
            writing.commit_inventory(reset_job, plan, "reset-watermark".into(), start, until, permit)
        });
        assert_eq!(
            arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap(),
            ResetPhase::Commit
        );
        let applying = store.clone();
        let late = page("late", "old-feed-cursor");
        let applier = std::thread::spawn(move || applying.apply(&job, late, start, until));
        assert_eq!(
            arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap(),
            ResetPhase::Apply
        );
        commit_tx.send(()).unwrap();
        assert_eq!(
            arrived_rx.recv_timeout(Duration::from_secs(20)).unwrap(),
            ResetPhase::Checkpoint
        );
        apply_tx.send(()).unwrap();
        let applied = applier.join().unwrap();
        checkpoint_tx.send(()).unwrap();
        let committed = writer.join().unwrap();
        *store.reset_hook.lock().unwrap() = None;
        assert!(applied.is_err(), "stale page must be rejected before mutation");
        committed.unwrap();
        let fresh = store.job(&key, &c.namespace).unwrap();
        store.renew(&fresh, Instant::now(), unix_now() + 60).unwrap();
        let returned = store
            .search(&c.tool, &crate::store::Query::search("resurrection"))
            .unwrap();
        eprintln!(
            "apply={applied:?}; cursor={:?}; visible_notes={}",
            fresh.cursor,
            returned.entries.len()
        );
        assert_eq!(
            fresh.cursor.as_deref(),
            Some("reset-watermark"),
            "stale activity overwrote the reset cursor"
        );
        assert!(
            returned.entries.is_empty(),
            "stale activity resurrected an absent source"
        );
    }

    #[tokio::test]
    async fn panicked_reset_rolls_back_reopens_and_allows_a_retry() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        for phase in [ResetPhase::Commit, ResetPhase::Checkpoint] {
            let (_dir, store, c, key) = fixture();
            let job = store.job(&key, &c.namespace).unwrap();
            let start = Instant::now();
            let until = unix_now() + 60;
            let permit = store.begin_reset(&job).unwrap();
            let plan = store
                .prepare_inventory(&job, Default::default(), start, until)
                .await
                .unwrap();
            // A connection-local setting proves retry reopens, rather than just
            // ignoring poison and reusing the unwound connection.
            store
                .projection_writer
                .lock()
                .unwrap()
                .pragma_update(None, "cache_size", 1234)
                .unwrap();
            *store.reset_hook.lock().unwrap() = Some(Arc::new(move |at| {
                if at == phase {
                    panic!("injected reset panic");
                }
            }));
            let failed = catch_unwind(AssertUnwindSafe(|| {
                store.commit_inventory(job, plan, "reset".into(), start, until, permit)
            }));
            assert!(failed.is_err());
            *store.reset_hook.lock().unwrap() = None;
            assert!(!store.inner.lock().unwrap().writing.contains(&c.namespace));
            assert!(store.available(&c.tool, false).is_err());
            assert!(store.projection_writer.is_poisoned());
            {
                let connection = store.projection_writer.lock().unwrap_err().into_inner();
                assert!(connection.is_autocommit(), "unwinding must roll back the transaction");
            }
            let retry = store.job(&key, &c.namespace).unwrap();
            assert_eq!(
                retry.cursor.as_deref(),
                if phase == ResetPhase::Commit {
                    None
                } else {
                    Some("reset")
                }
            );
            let permit = store.begin_reset(&retry).unwrap();
            let plan = store
                .prepare_inventory(&retry, Default::default(), start, until)
                .await
                .unwrap();
            let retried = catch_unwind(AssertUnwindSafe(|| {
                store.commit_inventory(retry, plan, "retry".into(), start, until, permit)
            }));
            assert!(retried.is_ok(), "a poisoned reset writer must recover without restart");
            retried.unwrap().unwrap();
            assert!(!store.projection_writer.is_poisoned());
            let cache_size: i64 = store
                .projection_writer
                .lock()
                .unwrap()
                .pragma_query_value(None, "cache_size", |r| r.get(0))
                .unwrap();
            assert_ne!(cache_size, 1234, "retry must use a reopened connection");
            let busy_ms: i64 = store
                .projection_writer
                .lock()
                .unwrap()
                .pragma_query_value(None, "busy_timeout", |r| r.get(0))
                .unwrap();
            assert_eq!(busy_ms, 100, "reopened writer must retain its bounded wait");
            assert_eq!(store.job(&key, &c.namespace).unwrap().cursor.as_deref(), Some("retry"));
            store.available(&c.tool, false).unwrap();
        }
    }

    fn fixture() -> (tempfile::TempDir, Store, crate::store::Credentials, MasterKey) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("memory.db")).unwrap();
        let credentials = store.create("owner", "hat").unwrap();
        let key = MasterKey::from_bytes([9; 32]);
        store
            .attach(
                &credentials.namespace,
                FeedConfig {
                    collector_url: "http://localhost/".into(),
                    namespace_epoch: "a".repeat(64),
                    activity_token: format!("hnry_activity_{}", "b".repeat(64)),
                    activity: true,
                    internal_network: true,
                },
                &key,
            )
            .unwrap();
        (dir, store, credentials, key)
    }
    #[tokio::test]
    async fn stalled_reset_commit_and_checkpoint_do_not_hold_up_other_namespaces() {
        use crate::store::{Query, Remember};
        use std::sync::{Mutex, mpsc};
        for phase in [ResetPhase::Commit, ResetPhase::Checkpoint] {
            let (_dir, store, c, key) = fixture();
            let store = Arc::new(store);
            let other = store.create("owner", "other").unwrap();
            store
                .remember(
                    &other.tool,
                    Remember {
                        title: "foreign marker".into(),
                        body: "foreign marker".into(),
                        kind: None,
                        tags: vec![],
                        idempotency_key: "fixture".into(),
                    },
                )
                .unwrap();
            let job = store.job(&key, &c.namespace).unwrap();
            let start = Instant::now();
            let until = unix_now() + 60;
            store.renew(&job, start, until).unwrap();
            let stamp = store.delivery_stamp(&c.tool).unwrap();
            let permit = store.begin_reset(&job).unwrap();
            let plan = store
                .prepare_inventory(&job, Default::default(), start, until)
                .await
                .unwrap();
            let (paused_tx, paused_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let release_rx = Mutex::new(release_rx);
            *store.reset_hook.lock().unwrap() = Some(Arc::new(move |at| {
                if at == phase {
                    paused_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(20))
                        .unwrap();
                }
            }));
            let committing = store.clone();
            let writer = std::thread::spawn(move || {
                committing.commit_inventory(job, plan, "reset".into(), start, until, permit)
            });
            paused_rx.recv_timeout(Duration::from_secs(20)).unwrap();
            let (read_tx, read_rx) = mpsc::sync_channel(1);
            let reading = store.clone();
            let token = c.tool.clone();
            let namespace = c.namespace.clone();
            let prior = stamp.clone();
            let reader = std::thread::spawn(move || {
                let page = reading.search(&other.tool, &Query::search("foreign")).unwrap();
                assert_eq!(page.entries.len(), 1);
                let denied = reading.available(&token, false).is_err();
                let invalidated = reading.deliverable(&token, &prior).is_err();
                let fresh = reading.job(&MasterKey::from_bytes([9; 32]), &namespace).unwrap();
                let renewal_denied = reading.renew(&fresh, Instant::now(), unix_now() + 60).is_err();
                let _ = read_tx.send((denied, invalidated, renewal_denied));
            });
            let observed = read_rx.recv_timeout(Duration::from_secs(5));
            // Release before asserting, including on RED, so no blocked thread
            // or database owner leaks into another case.
            release_tx.send(()).unwrap();
            writer.join().unwrap().unwrap();
            reader.join().unwrap();
            assert!(
                observed.is_ok(),
                "foreign reads waited for the stalled {phase:?}; shared mutex held during reset I/O"
            );
            assert_eq!(
                observed.unwrap(),
                (true, true, true),
                "reset scope and prior deliveries stay denied"
            );
            assert!(store.available(&c.tool, false).is_ok());
            assert!(
                store.deliverable(&c.tool, &stamp).is_err(),
                "old delivery remains invalid after renewal"
            );
            *store.reset_hook.lock().unwrap() = None;
        }
    }
    #[tokio::test]
    async fn abandoning_a_prepared_reset_denies_old_leases_and_allows_a_retry() {
        let (_dir, store, c, key) = fixture();
        let job = store.job(&key, &c.namespace).unwrap();
        let start = Instant::now();
        let until = unix_now() + 60;
        store.renew(&job, start, until).unwrap();
        let stamp = store.delivery_stamp(&c.tool).unwrap();
        let permit = store.begin_reset(&job).unwrap();
        assert!(store.begin_reset(&job).is_err(), "one reset per namespace");
        assert!(
            store.renew(&job, start, until).is_err(),
            "renewal cannot bypass an unfinished reset"
        );
        drop(permit);
        assert!(store.available(&c.tool, false).is_err());
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        store
            .commit_inventory(job, plan, "retry".into(), start, until, permit)
            .unwrap();
        assert!(store.available(&c.tool, false).is_ok());
        assert!(store.deliverable(&c.tool, &stamp).is_err());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_the_waiter_keeps_the_running_reset_fenced() {
        use std::sync::{Mutex, mpsc};
        let (_dir, store, c, key) = fixture();
        let store = Arc::new(store);
        let job = store.job(&key, &c.namespace).unwrap();
        let (paused_tx, paused_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        *store.reset_hook.lock().unwrap() = Some(Arc::new(move |phase| {
            if phase == ResetPhase::Commit {
                paused_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap();
            }
        }));
        let running = store.clone();
        let waiter = tokio::spawn(async move {
            running
                .inventory(
                    &job,
                    Default::default(),
                    "cancelled-waiter".into(),
                    Instant::now(),
                    unix_now() + 60,
                )
                .await
        });
        paused_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let fresh = store.job(&key, &c.namespace).unwrap();
        let denied = store.available(&c.tool, false).is_err()
            && store.begin_reset(&fresh).is_err()
            && store.renew(&fresh, Instant::now(), unix_now() + 60).is_err();
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while store.available(&c.tool, false).is_err() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        *store.reset_hook.lock().unwrap() = None;
        assert!(denied, "cancelling an awaiter cannot release an in-flight reset fence");
    }
    #[tokio::test]
    async fn a_forget_after_reset_commit_prevents_late_lease_publication() {
        for scope in [false, true] {
            let (_dir, store, c, key) = fixture();
            let store = Arc::new(store);
            let job = store.job(&key, &c.namespace).unwrap();
            let start = Instant::now();
            let until = unix_now() + 60;
            let permit = store.begin_reset(&job).unwrap();
            let plan = store
                .prepare_inventory(&job, Default::default(), start, until)
                .await
                .unwrap();
            let weak = Arc::downgrade(&store);
            let token = c.lifecycle.clone();
            let epoch = job.epoch.clone();
            *store.reset_hook.lock().unwrap() = Some(Arc::new(move |phase| {
                if phase == ResetPhase::Checkpoint {
                    weak.upgrade()
                        .unwrap()
                        .forget(
                            &token,
                            Forget {
                                request_id: "late-forget".into(),
                                namespace_epoch: epoch.clone(),
                                target: if scope {
                                    Target::Scope
                                } else {
                                    Target::Session {
                                        session_id: "late-source".into(),
                                    }
                                },
                            },
                        )
                        .unwrap();
                }
            }));
            let result = store.commit_inventory(job, plan, "reset".into(), start, until, permit);
            *store.reset_hook.lock().unwrap() = None;
            assert!(result.is_err(), "late lifecycle mutation must fence lease publication");
            assert!(store.available(&c.tool, false).is_err());
        }
    }
    #[tokio::test]
    async fn expiry_during_reset_sql_rolls_back_the_cursor_and_releases_the_reservation() {
        let (_dir, store, c, key) = fixture();
        let job = store.job(&key, &c.namespace).unwrap();
        let start = Instant::now() - Duration::from_secs(59);
        let until = unix_now() + 60;
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        *store.reset_hook.lock().unwrap() = Some(Arc::new(|phase| {
            if phase == ResetPhase::Commit {
                std::thread::sleep(Duration::from_secs(2));
            }
        }));
        let result = store.commit_inventory(job, plan, "expired".into(), start, until, permit);
        *store.reset_hook.lock().unwrap() = None;
        assert!(result.is_err());
        let retry = store.job(&key, &c.namespace).unwrap();
        assert!(retry.cursor.is_none(), "expiry cannot commit partial reconciliation");
        assert!(store.available(&c.tool, false).is_err());
        drop(store.begin_reset(&retry).unwrap());
    }
    #[tokio::test]
    async fn a_busy_reset_checkpoint_keeps_reads_denied_until_a_successful_retry() {
        let (dir, store, c, key) = fixture();
        let reader = Arc::new(std::sync::Mutex::new(
            hennery_kernel::db::open(&dir.path().join("memory.db")).unwrap(),
        ));
        let snapshot = reader.clone();
        let namespace = c.namespace.clone();
        *store.reset_hook.lock().unwrap() = Some(Arc::new(move |phase| {
            if phase == ResetPhase::Commit {
                let db = snapshot.lock().unwrap();
                db.execute_batch("BEGIN").unwrap();
                let _: Option<String> = db
                    .query_row("SELECT cursor FROM feeds WHERE namespace=?1", [&namespace], |r| {
                        r.get(0)
                    })
                    .unwrap();
            }
        }));
        let job = store.job(&key, &c.namespace).unwrap();
        let start = Instant::now();
        let until = unix_now() + 60;
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        let result = store.commit_inventory(job, plan, "committed".into(), start, until, permit);
        *store.reset_hook.lock().unwrap() = None;
        reader.lock().unwrap().execute_batch("ROLLBACK").unwrap();
        assert!(result.unwrap_err().to_string().contains("checkpoint unconfirmed"));
        assert!(store.available(&c.tool, false).is_err());
        let job = store.job(&key, &c.namespace).unwrap();
        assert_eq!(job.cursor.as_deref(), Some("committed"));
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        store
            .commit_inventory(job, plan, "retried".into(), start, until, permit)
            .unwrap();
        assert!(store.available(&c.tool, false).is_ok());
    }
    #[tokio::test]
    async fn prepared_inventory_checks_revision_before_commit() {
        let (_dir, store, c, key) = fixture();
        let job = store.job(&key, &c.namespace).unwrap();
        let start = Instant::now();
        let until = unix_now() + 60;
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), start, until)
            .await
            .unwrap();
        store
            .forget(
                &c.lifecycle,
                Forget {
                    request_id: "fence".into(),
                    namespace_epoch: job.epoch.clone(),
                    target: Target::Session {
                        session_id: "cancelled".into(),
                    },
                },
            )
            .unwrap();
        assert!(
            store
                .commit_inventory(job, plan, "stale".into(), start, until, permit)
                .is_err()
        );
        assert!(
            store
                .inner
                .lock()
                .unwrap()
                .db
                .query_row("SELECT cursor FROM feeds WHERE namespace=?1", [&c.namespace], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .unwrap()
                .is_none()
        );
        assert!(store.available(&c.tool, false).is_err());
    }
    #[tokio::test]
    async fn prepared_inventory_checks_lease_before_commit() {
        let (_dir, store, c, key) = fixture();
        let job = store.job(&key, &c.namespace).unwrap();
        let permit = store.begin_reset(&job).unwrap();
        let plan = store
            .prepare_inventory(&job, Default::default(), Instant::now(), unix_now() + 60)
            .await
            .unwrap();
        assert!(
            store
                .commit_inventory(
                    job,
                    plan,
                    "expired".into(),
                    Instant::now() - Duration::from_secs(61),
                    unix_now() + 60,
                    permit,
                )
                .is_err()
        );
        assert!(
            store
                .inner
                .lock()
                .unwrap()
                .db
                .query_row("SELECT cursor FROM feeds WHERE namespace=?1", [&c.namespace], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .unwrap()
                .is_none()
        );
        assert!(store.available(&c.tool, false).is_err());
    }
}
