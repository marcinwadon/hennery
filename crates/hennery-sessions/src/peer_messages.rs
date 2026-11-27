//! Peer input is accepted once and uses the existing durable independent FIFO.
use crate::{delegation::SessionToolContext, prompt_receipts::PromptReceipt, store::Store};
use anyhow::{Result, ensure};
use hennery_gateway::identity::SessionIdentity;
use hennery_proto::plugin::InputOrigin;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Send {
    pub session_id: String,
    pub request_key: String,
    pub text: String,
}
pub(crate) struct Admission<'a> {
    pub identity: &'a SessionIdentity,
    pub revision: i64,
    pub request: &'a Send,
    pub digest: String,
}
impl<'a> Admission<'a> {
    pub fn new(identity: &'a SessionIdentity, revision: i64, request: &'a Send) -> Result<Self> {
        let key = &request.request_key;
        ensure!(
            !request.session_id.is_empty()
                && request.session_id.len() <= 128
                && !request.session_id.chars().any(char::is_control),
            "invalid_arguments"
        );
        ensure!(
            !key.is_empty()
                && key.len() <= 128
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.')),
            "invalid_request_key"
        );
        ensure!(
            !request.text.is_empty() && request.text.len() <= 48 * 1024 && !request.text.contains('\0'),
            "invalid_arguments"
        );
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(
            &json!({"session_id":request.session_id,"text":request.text}),
        )?));
        Ok(Self {
            identity,
            revision,
            request,
            digest,
        })
    }
}
impl Store {
    pub(crate) fn peer_message_admission_in(
        &self,
        tx: &Transaction<'_>,
        admission: &Admission<'_>,
    ) -> Result<Option<PromptReceipt>> {
        let c = self.peer_context_in(tx, admission.identity, admission.revision, true, true)?;
        ensure!(c.session_id() != admission.request.session_id, "invalid_arguments");
        self.peer_target_in(tx, &c, &admission.request.session_id)?;
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT digest,operation_id FROM peer_messages WHERE owner_id = ?1 AND source_id=?2 AND request_key=?3",
                params![self.owner_id(), c.session_id(), admission.request.request_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((digest, id)) = old {
            ensure!(digest == admission.digest, "request_key_conflict");
            return Ok(Some(PromptReceipt::Queued { queue_id: id }));
        }
        let epoch = self.peer_epoch_in(tx, &c)?;
        let used: i64 = tx.query_row(
            "SELECT count(*) FROM peer_messages WHERE owner_id = ?1 AND source_id=?2 AND epoch=?3",
            params![self.owner_id(), c.session_id(), epoch],
            |r| r.get(0),
        )?;
        ensure!(used < 32, "peer_message_budget");
        Ok(None)
    }
    fn peer_epoch_in(&self, tx: &Transaction<'_>, c: &SessionToolContext) -> Result<i64> {
        Ok(tx.query_row("SELECT COALESCE(MAX(event_id),0) FROM events WHERE owner_id = ?1 AND session_id=?2 AND kind='user_turn' AND applied=1 AND COALESCE(json_extract(body,'$.origin.kind'),'operator')='operator'",
            params![self.owner_id(),c.session_id()],|r|r.get(0))?)
    }
    pub(crate) fn bind_peer_message_in(&self, tx: &Transaction<'_>, a: &Admission<'_>, id: &str) -> Result<()> {
        let c = self.peer_context_in(tx, a.identity, a.revision, true, true)?;
        self.peer_target_in(tx, &c, &a.request.session_id)?;
        let origin = InputOrigin::Peer {
            session_id: c.session_id().into(),
            operation_id: id.into(),
        };
        tx.execute("INSERT INTO peer_messages(owner_id,source_id,request_key,target_id,hat_id,revision,epoch,digest,operation_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![self.owner_id(),c.session_id(),a.request.request_key,a.request.session_id,c.hat_id(),a.revision,self.peer_epoch_in(tx,&c)?,a.digest,id])?;
        tx.execute(
            "UPDATE session_dispatch_queue SET input_origin=?3 WHERE owner_id = ?1 AND operation_id=?2",
            params![self.owner_id(), id, serde_json::to_string(&origin)?],
        )?;
        // Keep the legacy independent-dispatch class for queue reconciliation
        // and operator recovery. It grants no authority: input_origin and this
        // peer ledger identify the sender and the accepted policy separately.
        Ok(())
    }
    pub(crate) fn prune_peer_prompts(&self, target: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut statement=tx.prepare("SELECT operation_id FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND origin='operator' AND state IN ('queued','waiting_config') AND json_extract(input_origin,'$.kind')='peer' ORDER BY position LIMIT 32")?;
        let ids = statement
            .query_map(params![self.owner_id(), target], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for id in ids {
            self.independent_origin_in(&tx, target, &id)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Recheck accepted peer intent before either queue or steering handoff.
    /// A parked/closed sender's already accepted data may still be delivered;
    /// deletion, hat movement and any policy revision invalidate unstarted input.
    pub(crate) fn independent_origin_in(
        &self,
        tx: &Transaction<'_>,
        target: &str,
        id: &str,
    ) -> Result<Option<InputOrigin>> {
        let raw:Option<String>=tx.query_row("SELECT input_origin FROM session_dispatch_queue WHERE owner_id = ?1 AND target_id=?2 AND operation_id=?3",params![self.owner_id(),target,id],|r|r.get(0)).optional()?;
        let Some(raw) = raw else { return Ok(None) };
        let origin: InputOrigin = serde_json::from_str(&raw)?;
        if let InputOrigin::Peer {
            session_id,
            operation_id,
        } = &origin
        {
            let allowed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM peer_messages p
                JOIN sessions s ON s.owner_id = ?1 AND s.id=p.source_id JOIN sessions t ON t.owner_id = ?1 AND t.id=p.target_id
                JOIN hosts sh ON sh.owner_id = ?1 AND sh.id=s.host_id JOIN hosts th ON th.owner_id = ?1 AND th.id=t.host_id
                JOIN hat_peer_access h ON h.owner_id = ?1 AND h.hat_id=p.hat_id
                WHERE p.owner_id = ?1 AND p.operation_id=?2 AND p.source_id=?3 AND p.target_id=?4
                AND s.hat_id=p.hat_id AND t.hat_id=p.hat_id AND s.lifecycle<>'deleted' AND t.lifecycle IN ('active','parked') AND t.close_requested=0
                AND sh.revoked_at IS NULL AND th.revoked_at IS NULL AND h.access='read_write' AND h.revision=p.revision
                AND NOT EXISTS(SELECT 1 FROM purged_hats x WHERE x.owner_id = ?1 AND x.hat_id=p.hat_id))",
                params![self.owner_id(),operation_id,session_id,target],|r|r.get(0))?;
            if !allowed {
                self.fail_operator_item_in(tx, id, "peer_access_changed")?;
                return Ok(None);
            }
        }
        Ok(Some(origin))
    }
    pub(crate) fn peer_message_status(&self, identity: &SessionIdentity, revision: i64, key: &str) -> Result<Value> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let c = self.peer_context_in(&tx, identity, revision, false, true)?;
        let(target,id):(String,String)=tx.query_row("SELECT target_id,operation_id FROM peer_messages WHERE owner_id = ?1 AND source_id=?2 AND request_key=?3",params![self.owner_id(),c.session_id(),key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or_else(||anyhow::anyhow!("not_found"))?;
        self.peer_target_in(&tx, &c, &target)?;
        let row: Option<(String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT state,code,turn_id FROM session_operations WHERE owner_id = ?1 AND operation_id=?2",
                params![self.owner_id(), id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (state, code, turn) = row.unwrap_or(("expired".into(), None, None));
        Ok(json!({"operation_id":id,"session_id":target,"state":state,"code":code,"turn_id":turn}))
    }
}
