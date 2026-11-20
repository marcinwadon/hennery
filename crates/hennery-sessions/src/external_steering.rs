//! External prompts remain in their authenticated operation ledger through handoff.
//! Only the request transport, attachment history and canonical admission are shared.
use crate::{
    external_control::{Command, Prompt},
    hub::Hub,
    store::Store,
};
use anyhow::{Result, ensure};
use hennery_proto::{
    frames::SteeringOutcome,
    mcp_source::{McpSourcePolicy, McpSourceSnapshot},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::json;

impl Store {
    pub(super) fn check_external_prompt_placement_in(
        &self,
        conn: &Connection,
        command: &Command,
        client: &crate::external_clients::ClientRecord,
    ) -> Result<()> {
        let (host,cwd,hat,admitted):(String,String,String,Option<String>) = conn.query_row("SELECT s.host_id,s.cwd,s.hat_id,o.target_hat_id FROM external_operations o JOIN sessions s ON s.owner_id=o.owner_id AND s.id=o.session_id
            WHERE o.owner_id = ?1 AND o.id=?2",params![self.owner_id(),command.operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        ensure!(admitted.as_ref().is_none_or(|h| h == &hat), "not_found");
        if let Some(paths) = &command.resolved_prompt {
            ensure!(
                host == paths.host
                    && cwd == paths.cwd
                    && hat == paths.hat
                    && crate::external_control_store::root_for(client, &host, &cwd) == Some(paths.root.as_str()),
                "not_found"
            );
        }
        Ok(())
    }

    pub(super) fn check_attached_source_in(&self, conn: &Connection, session: &str) -> Result<()> {
        let (hat, agent, applied, invalid): (String, String, Option<String>, bool) = conn.query_row(
            "SELECT hat_id,agent,mcp_source_applied,mcp_source_requested IS NOT mcp_source_applied OR COALESCE(json_extract(mcp_source_requested,'$.invalidated'),0)
             FROM sessions WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let selected = crate::mcp_source::selected_in(conn, self.owner_id(), &hat, &agent)?;
        let applied = applied
            .map(|s| serde_json::from_str::<McpSourceSnapshot>(&s))
            .transpose()?;
        ensure!(
            !invalid
                && match applied {
                    Some(snapshot) => selected == (snapshot.policy, snapshot.revision),
                    None => selected.0 == McpSourcePolicy::Legacy,
                },
            "source_policy_changed"
        );
        Ok(())
    }

    pub(super) fn defer_external_steering_in(&self, conn: &Connection, command: &Command) -> Result<()> {
        conn.execute(
            "UPDATE external_operations SET steering_turn=NULL WHERE owner_id = ?1 AND id=?2 AND state='queued'",
            params![self.owner_id(), command.operation_id],
        )?;
        Ok(())
    }

    /// Avoid a realpath RPC once the busy route is known to be unavailable.
    pub(super) fn external_steering_candidate(&self, command: &Command, hub: &Hub) -> Result<bool> {
        let conn = self.conn();
        let eligible = self.steering_target_in(&conn, &command.session_id, hub)?.is_ok();
        if !eligible {
            self.defer_external_steering_in(&conn, command)?;
        }
        Ok(eligible)
    }

    pub(super) fn dispatch_external_steering(&self, command: &Command, hub: &Hub) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let row:Option<(String,String)>=tx.query_row("SELECT o.steering_turn,o.args FROM external_operations o JOIN external_prompt_heads h ON h.owner_id=o.owner_id AND h.id=o.id
             WHERE o.owner_id = ?1 AND o.id=?2 AND o.steering_turn IS NOT NULL",params![self.owner_id(),command.operation_id],|r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((intended, args)) = row else { return Ok(()) };
        self.external_guard_in(&tx, command, None)?;
        self.check_attached_source_in(&tx, &command.session_id)?;
        let prompt: Prompt = serde_json::from_str(&args)?;
        let target = self
            .steering_target_in(&tx, &command.session_id, hub)?
            .ok()
            .filter(|target| target.turn == intended);
        if let Some(target) = target {
            let checked = crate::content::check(prompt.content(&tx, self, command)?)
                .map_err(|_| anyhow::anyhow!("invalid_content"))?;
            if crate::hub::supports_steering_content(&target.capabilities, &checked.sent) {
                for image in &checked.images {
                    self.store_queued_attachment_in(&tx, image)?;
                }
                let handoff = self.steering_attempt_in(
                    &tx,
                    &command.session_id,
                    target,
                    &checked.stored,
                    checked.sent,
                    command.origin(),
                    true,
                )?;
                tx.execute("UPDATE external_operations SET state='running',request_id=?3,turn_id=NULL,steering_deadline=unixepoch()+30,result=?4
                    WHERE owner_id = ?1 AND id=?2", params![self.owner_id(),command.operation_id,handoff.request_id,json!({"delivery":"steering","admission":"pending","turn_id":intended}).to_string()])?;
                // Reading and persisting a sealed upload can take time. Recheck expiry at handoff.
                self.external_guard_in(&tx, command, None)?;
                tx.commit()?;
                if let Err(error) = handoff.deliver(self, &mut conn, &command.session_id, hub) {
                    // The attempt is durable. Its deadline retains uncertain delivery if
                    // recording a proven mailbox miss fails after commit.
                    tracing::warn!(operation_id = %command.operation_id, %error, "external steering settlement deferred");
                }
                return Ok(());
            }
        }
        self.defer_external_steering_in(&tx, command)?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn settle_external_steering_in(
        &self,
        tx: &Transaction<'_>,
        session: &str,
        request: &str,
        outcome: &SteeringOutcome,
    ) -> Result<()> {
        let row=tx.query_row("SELECT id,client_id,revision,tool,session_id,turn_id,steering_failure FROM external_operations
            WHERE owner_id = ?1 AND session_id=?2 AND request_id=?3 AND steering_turn IS NOT NULL AND state IN ('running','uncertain') AND json_extract(result,'$.admission')='pending'",
            params![self.owner_id(),session,request],|r|Ok((crate::external_control_store::command_row(r)?,r.get::<_,Option<String>>(6)?))).optional()?;
        let Some((command, prior_failure)) = row else {
            return Ok(());
        };
        let id = &command.operation_id;
        let (state, code) = match outcome {
            SteeringOutcome::Injected => {
                tx.execute("UPDATE external_operations SET state='running',args=json_object(),code=NULL,steering_deadline=NULL,result=json_set(result,'$.admission','confirmed') WHERE owner_id = ?1 AND id=?2",params![self.owner_id(),id])?;
                return Ok(());
            }
            SteeringOutcome::PromptRequired
            | SteeringOutcome::Rejected {
                steering_unavailable: true,
                ..
            } => {
                if let Some(failure) = prior_failure {
                    ("failed", failure)
                } else {
                    match self
                        .external_guard_in(tx, &command, None)
                        .and_then(|()| self.check_attached_source_in(tx, session))
                    {
                        Ok(()) => {
                            tx.execute("UPDATE external_operations SET state='queued',steering_turn=NULL,steering_deadline=NULL,request_id=NULL,result=NULL,code=NULL WHERE owner_id = ?1 AND id=?2",params![self.owner_id(),id])?;
                            return Ok(());
                        }
                        Err(e)
                            if matches!(
                                e.to_string().as_str(),
                                "external_unauthenticated"
                                    | "not_found"
                                    | "external_control_forbidden"
                                    | "source_policy_changed"
                            ) =>
                        {
                            ("cancelled", e.to_string())
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            SteeringOutcome::Rejected { reason, .. } => {
                let reason = crate::store::one_line(reason, 512, 2048).unwrap_or_else(|| "Rejected input".into());
                tx.execute(
                    "UPDATE external_operations SET result=json_set(result,'$.reason',?3) WHERE owner_id = ?1 AND id=?2",
                    params![self.owner_id(),id,reason])?;
                ("failed", "steering_rejected".into())
            }
            SteeringOutcome::Uncertain => ("uncertain", "delivery_unknown".into()),
        };
        tx.execute("UPDATE external_operations SET state=?3,code=?4,steering_deadline=NULL,args=CASE WHEN ?3='uncertain' THEN args ELSE json_object() END,
            result=CASE WHEN ?3='uncertain' THEN result ELSE json_set(result,'$.admission','rejected') END WHERE owner_id = ?1 AND id=?2",params![self.owner_id(),id,state,code])?;
        Ok(())
    }
}
