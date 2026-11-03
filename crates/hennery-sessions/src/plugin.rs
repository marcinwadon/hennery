//! Completed-activity projection of the existing committed event log. The
//! optional control store holds cursors/bindings/source ids, never bodies.

use crate::store::Store;
use anyhow::{Context, Result, ensure};
use hennery_gateway::{plugin::Grant, revocation::Watch};
use hennery_kernel::{redact::Assembly, secret::unix_now};
use hennery_proto::{
    plugin::{
        ActivityBody, Excerpt, InputOrigin, PluginActivityItem, PluginActivityPage, PluginSourceInventoryPage,
        Provenance, SourceRef, TextRole,
    },
    rest::{EventDto, StoredBlock},
};
use hennery_view::{Body, Fold};
use rusqlite::{Transaction, params};
use serde::Deserialize;

const PAGE_BYTES: usize = 128 * 1024;
const ASSEMBLY_BYTES: usize = 64 * 1024;
const MAX_EVENTS: usize = 20_000;

// Scope is checked before LIMIT and before a body is read. Resets leaving a
// hat are observable in their former scope; ordinary content is not.
const CANDIDATES: &str = "SELECT e.event_id,e.session_id,e.kind,e.ts,s.host_id,s.cwd,
    json_extract(e.body,'$.turn_id'),u.event_id
 FROM events e JOIN sessions s ON s.owner_id = ?1 AND s.id = e.session_id
 LEFT JOIN turns t ON t.owner_id = ?1 AND t.session_id = s.id AND t.turn_id = json_extract(e.body,'$.turn_id')
 LEFT JOIN events u ON u.owner_id = ?1 AND u.session_id = s.id AND u.kind = 'user_turn' AND u.applied = 1
    AND json_extract(u.body,'$.turn_id') = t.turn_id
 WHERE e.owner_id = ?1 AND e.applied = 1 AND e.event_id > ?4 AND e.event_id <= ?5
 AND EXISTS(SELECT 1 FROM gw_connections c WHERE c.owner_id = ?1 AND c.id = ?3 AND c.hat_id = ?2)
 AND EXISTS(SELECT 1 FROM gw_mounts m JOIN hosts h ON h.owner_id = ?1 AND h.id = m.host_id
    WHERE m.owner_id = ?1 AND m.connection_id = ?3 AND m.host_id = s.host_id AND h.revoked_at IS NULL)
 AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id = ?2)
 AND ((s.hat_id = ?2 AND e.kind = 'session_deleted' AND s.lifecycle = 'deleted')
 OR (e.kind = 'hat_reassigned' AND (json_extract(e.body,'$.from') = ?2 OR json_extract(e.body,'$.to') = ?2))
 OR (s.hat_id = ?2 AND s.lifecycle <> 'deleted' AND e.kind = 'turn_ended'
    AND json_extract(e.body,'$.outcome') = 'completed' AND t.state = 'ended' AND t.outcome = 'completed'
    AND u.event_id > max(?6,COALESCE((SELECT max(a.event_id) FROM events a WHERE a.owner_id = ?1
       AND a.session_id = s.id AND a.kind = 'hat_reassigned' AND a.applied = 1),0))))
 ORDER BY e.event_id LIMIT ?7";

const SOURCES: &str = "SELECT s.id FROM sessions s WHERE s.owner_id = ?1 AND s.hat_id = ?2 AND s.lifecycle <> 'deleted'
 AND EXISTS(SELECT 1 FROM events e WHERE e.owner_id = ?1 AND e.session_id = s.id AND e.applied = 1 AND e.event_id <= ?4)
 AND EXISTS(SELECT 1 FROM gw_mounts m JOIN hosts h ON h.owner_id = ?1 AND h.id = m.host_id
    WHERE m.owner_id = ?1 AND m.connection_id = ?3 AND m.host_id = s.host_id AND h.revoked_at IS NULL)
 ORDER BY s.id";

struct Candidate {
    event_id: i64,
    session_id: String,
    kind: String,
    ts: String,
    host: String,
    cwd: String,
    turn: Option<String>,
    start: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorInput {
    turn_id: String,
    content: Vec<StoredBlock>,
    origin: InputOrigin,
}

fn page_limit(limit: usize) -> Result<usize> {
    ensure!((1..=20).contains(&limit), "invalid plug-in page limit");
    Ok(limit)
}

fn watermark(core: &Transaction<'_>, owner: &str) -> Result<i64> {
    Ok(core.query_row(
        "SELECT COALESCE(MAX(event_id),0) FROM events WHERE owner_id = ?1",
        [owner],
        |r| r.get(0),
    )?)
}

impl Store {
    fn plugin_grant_in(&self, core: &Transaction<'_>, token: &str, activity: bool) -> Result<Grant> {
        let grant = self
            .plugins()
            .authenticate_in(core, token, unix_now())?
            .context("plug-in access denied")?;
        ensure!(!activity || grant.activity, "plug-in access denied");
        Ok(grant)
    }

    /// HTTP acquires this before projection and holds/checks it through the
    /// last response byte. A page alone is not a revocation-safe transport.
    pub fn plugin_watch(&self, token: &str) -> Result<Watch> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let watch = self.plugins().watch_grant_in(&tx, token, unix_now())?;
        tx.commit()?;
        Ok(watch)
    }

    pub fn lease(&self, token: &str) -> Result<i64> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let grant = self.plugin_grant_in(&tx, token, false)?;
        self.plugins().charge(&grant, 128)?;
        tx.commit()?;
        Ok(unix_now().saturating_add(60))
    }

    pub fn activity_page(&self, token: &str, after: Option<&str>, limit: usize) -> Result<PluginActivityPage> {
        let limit = page_limit(limit)?;
        let plugins = self.plugins();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let grant = self.plugin_grant_in(&tx, token, true)?;
        plugins.charge(&grant, PAGE_BYTES)?;
        let high = watermark(&tx, self.owner_id())?;
        let after = match after {
            Some(cursor) => plugins
                .decode_cursor(&grant, cursor)
                .map_err(|_| anyhow::anyhow!("reset_required"))?,
            None => grant.start_event,
        };
        ensure!(after <= high, "reset_required");
        let candidates = {
            let mut stmt = tx.prepare(CANDIDATES)?;
            stmt.query_map(
                params![
                    self.owner_id(),
                    grant.hat_id,
                    grant.connection_id,
                    after,
                    high,
                    grant.start_event,
                    limit as i64 + 1
                ],
                |r| {
                    Ok(Candidate {
                        event_id: r.get(0)?,
                        session_id: r.get(1)?,
                        kind: r.get(2)?,
                        ts: r.get(3)?,
                        host: r.get(4)?,
                        cwd: r.get(5)?,
                        turn: r.get(6)?,
                        start: r.get(7)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut items = Vec::new();
        let mut next = after;
        let mut bytes = 0usize;
        let mut has_more = candidates.len() > limit;
        for candidate in candidates.into_iter().take(limit) {
            let item = self.project_in(&tx, &grant, &candidate)?;
            let weight = serde_json::to_vec(&item)?.len();
            if bytes.saturating_add(weight) > PAGE_BYTES - 2048 {
                has_more = true;
                break;
            }
            if candidate.kind == "turn_ended" {
                let _watch = plugins.admit_in(&tx, &grant.connection_id, &candidate.session_id, &grant.hat_id)?;
            }
            bytes += weight;
            next = candidate.event_id;
            items.push(item);
        }
        if !has_more {
            next = high;
        }
        let page = PluginActivityPage {
            version: 1,
            items,
            next_cursor: plugins.encode_cursor(&grant, next)?,
            has_more,
            lease_until: unix_now().saturating_add(60),
        };
        tx.commit()?;
        Ok(page)
    }

    fn project_in(&self, core: &Transaction<'_>, grant: &Grant, candidate: &Candidate) -> Result<PluginActivityItem> {
        let mut provenance = Vec::new();
        let body = match candidate.kind.as_str() {
            "session_deleted" => ActivityBody::SessionRemoved,
            "hat_reassigned" => ActivityBody::SourceReset,
            "turn_ended" => {
                let start = candidate.start.context("unsupported input origin")?;
                let turn = candidate.turn.as_ref().context("unsupported input origin")?;
                let raw:String=core.query_row("SELECT body FROM events WHERE owner_id = ?1 AND session_id = ?2 AND event_id = ?3 AND kind = 'user_turn' AND applied = 1",params![self.owner_id(),candidate.session_id,start],|r|r.get(0))?;
                // Decode the original bytes, before Value could erase duplicate
                // authority keys. Historical/missing or delegated input fails closed.
                let input: OperatorInput =
                    serde_json::from_str(&raw).map_err(|_| anyhow::anyhow!("unsupported input origin"))?;
                ensure!(
                    self.verified_input_in(core, &input.origin, &candidate.session_id, turn)? && input.turn_id == *turn,
                    "unsupported input origin"
                );
                let mut user = Assembly::new(ASSEMBLY_BYTES);
                let mut assistant = Assembly::new(ASSEMBLY_BYTES);
                // User text is sourced from the typed collector fact; the view
                // supplies assistant message folding without a second ACP parser.
                for block in input.content {
                    if let StoredBlock::Text { text } = block {
                        user.push(&text);
                    }
                }
                let mut fold = Fold::new();
                let mut stmt = core.prepare(
                    "SELECT event_id,host_seq,kind,body,ts FROM events WHERE owner_id = ?1 AND session_id = ?2
                    AND event_id >= ?3 AND event_id <= ?4 AND applied = 1 ORDER BY event_id LIMIT ?5",
                )?;
                let mut rows = stmt.query(params![
                    self.owner_id(),
                    candidate.session_id,
                    start,
                    candidate.event_id,
                    MAX_EVENTS as i64 + 1
                ])?;
                let mut count = 0;
                let mut gaps = false;
                while let Some(row) = rows.next()? {
                    if count == MAX_EVENTS {
                        gaps = true;
                        break;
                    }
                    count += 1;
                    let raw: String = row.get(3)?;
                    let event = EventDto {
                        event_id: row.get(0)?,
                        session_id: candidate.session_id.clone(),
                        host_seq: row.get::<_, Option<i64>>(1)?.map(|n| n as u64),
                        kind: row.get(2)?,
                        body: serde_json::from_str(&raw)?,
                        ts: row.get(4)?,
                    };
                    fold.apply(&event, &|_: &str| false);
                }
                for item in fold.take() {
                    match item.body {
                        Body::Message(text) => {
                            assistant.push(&text.text);
                            gaps |= text.truncated;
                        }
                        Body::Unrecognised(_) => gaps = true,
                        Body::Marker(marker) if marker.marker == hennery_view::item::MarkerKind::Elided => gaps = true,
                        _ => {}
                    }
                }
                let user = user.finish(4096);
                let assistant = assistant.finish(4096);
                let source = SourceRef {
                    session_id: candidate.session_id.clone(),
                    turn_id: Some(turn.clone()),
                    event_id: start,
                };
                provenance.push(Provenance::Source {
                    origin: input.origin,
                    source: source.clone(),
                });
                let project = serde_json::to_string(&(&candidate.host, &candidate.cwd))?;
                ActivityBody::CompletedTurn {
                    turn_id: turn.clone(),
                    project_key: hennery_kernel::redact::text(&project).unwrap_or(project),
                    completed_at: candidate.ts.clone(),
                    excerpts: vec![
                        Excerpt {
                            role: TextRole::User,
                            text: user.text,
                        },
                        Excerpt {
                            role: TextRole::Assistant,
                            text: assistant.text,
                        },
                    ],
                    source_refs: vec![source],
                    truncated: user.truncated || assistant.truncated,
                    gaps,
                }
            }
            _ => anyhow::bail!("unsupported activity event"),
        };
        Ok(PluginActivityItem {
            event_key: format!("{}:{}", grant.epoch, candidate.event_id),
            session_id: candidate.session_id.clone(),
            namespace_epoch: grant.epoch.clone(),
            provenance,
            body,
        })
    }

    pub fn source_inventory(
        &self,
        token: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<PluginSourceInventoryPage> {
        ensure!(
            (1..=hennery_gateway::plugin::feed::INVENTORY_MAX_IDS).contains(&limit),
            "invalid plug-in inventory limit"
        );
        let plugins = self.plugins();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let grant = self.plugin_grant_in(&tx, token, true)?;
        plugins.charge(&grant, PAGE_BYTES)?;
        let first;
        let cursor = match cursor {
            Some(cursor) => cursor,
            None => {
                let high = watermark(&tx, self.owner_id())?;
                let mut stmt = tx.prepare(SOURCES)?;
                for id in stmt.query_map(params![self.owner_id(), grant.hat_id, grant.connection_id, high], |r| {
                    r.get::<_, String>(0)
                })? {
                    let _watch = plugins.admit_in(&tx, &grant.connection_id, &id?, &grant.hat_id)?;
                }
                let ids = stmt.query_map(params![self.owner_id(), grant.hat_id, grant.connection_id, high], |r| {
                    r.get::<_, String>(0)
                })?;
                first = plugins.begin_inventory_in(
                    &tx,
                    &grant,
                    high,
                    ids.map(|r| r.map_err(anyhow::Error::from)),
                    unix_now(),
                )?;
                &first
            }
        };
        let slice = plugins.inventory_slice_in(&tx, &grant, cursor, limit, unix_now())?;
        // Every member was bound before the first snapshot page, so unread
        // members cannot be reassigned. Snapshot ids remain stable if a source
        // is deleted between pages; resume delivers its deletion tombstone.
        let resume = slice.resume_cursor;
        let page = PluginSourceInventoryPage {
            version: 1,
            source_ids: slice.source_ids,
            next_cursor: slice.next_cursor,
            resume_cursor: resume.clone(),
            watermark: resume,
            lease_until: slice.expires.min(unix_now().saturating_add(60)),
        };
        tx.commit()?;
        Ok(page)
    }
}

impl Store {
    fn verified_input_in(&self, tx: &Transaction<'_>, origin: &InputOrigin, target: &str, turn: &str) -> Result<bool> {
        match origin {
            InputOrigin::Operator {} => Ok(true),
            InputOrigin::ExternalClient { client_id, operation_id } => Ok(tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM external_operations WHERE owner_id = ?1 AND id=?2 AND client_id=?3 AND session_id=?4 AND turn_id=?5 AND tool='session_prompt')",
                params![self.owner_id(),operation_id,client_id,target,turn], |r|r.get(0),
            )?),
            InputOrigin::Parent { .. } | InputOrigin::ChildReport { .. } => {
                self.verified_delegated_input_in(tx, origin, target, turn)
            }
            InputOrigin::PluginContext { .. } => Ok(false),
        }
    }
}
