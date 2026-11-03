//! External command admission is durable, scoped and separate from operator identity.
use crate::{
    external_clients::{self, ClientRecord, ExternalIdentity},
    external_control::{Command, Receipt},
    store::Store,
};
use anyhow::{Result, ensure};
use hennery_kernel::{
    hats::{SessionHat, is_canonical},
    secret::{sha256_hex, unix_now},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

fn contains(root: &str, path: &str) -> bool {
    path == root
        || root == "/" && path.starts_with('/')
        || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}
pub(crate) fn root_for<'a>(client: &'a ClientRecord, host: &str, cwd: &str) -> Option<&'a str> {
    client
        .roots
        .iter()
        .filter(|r| r.host_id == host && contains(&r.root, cwd))
        .max_by_key(|r| r.root.len())
        .map(|r| r.root.as_str())
}
fn placement(conn: &Connection, owner: &str, client: &ClientRecord, host: &str, cwd: &str, hat: &str) -> Result<()> {
    ensure!(
        is_canonical(cwd) && root_for(client, host, cwd).is_some() && client.hat_id == hat,
        "not_found"
    );
    let allowed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM hosts h WHERE h.owner_id = ?1 AND h.id=?2 AND h.revoked_at IS NULL)
        AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=?3)",
        params![owner, host, hat],
        |r| r.get(0),
    )?;
    ensure!(allowed, "not_found");
    ensure!(
        matches!(hennery_kernel::hats::session_hat_in(conn,owner,host,cwd)?,Some(SessionHat::Decided(r)) if r.hat_id==hat),
        "not_found"
    );
    Ok(())
}
fn session_scope(conn: &Connection, owner: &str, client: &ClientRecord, id: &str, execution: bool) -> Result<()> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT host_id,cwd,hat_id FROM sessions WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted'",
            params![owner, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (host, cwd, hat) = row.ok_or_else(|| anyhow::anyhow!("not_found"))?;
    let owned: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM external_client_sessions WHERE owner_id = ?1 AND client_id=?2 AND session_id=?3)",
        params![owner, client.client_id, id],
        |r| r.get(0),
    )?;
    ensure!(client.existing_sessions || owned, "not_found");
    if execution {
        placement(conn, owner, client, &host, &cwd, &hat)
    } else {
        ensure!(
            client.hat_id == hat && root_for(client, &host, &cwd).is_some(),
            "not_found"
        );
        let allowed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM hosts h WHERE h.owner_id = ?1 AND h.id=?2 AND h.revoked_at IS NULL) AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=?3)",params![owner,host,hat],|r|r.get(0))?;
        ensure!(allowed, "not_found");
        Ok(())
    }
}
fn current_client(conn: &Connection, owner: &str, id: &str, revision: i64) -> Result<ClientRecord> {
    let client =
        external_clients::record(conn, owner, id)?.ok_or_else(|| anyhow::anyhow!("external_unauthenticated"))?;
    ensure!(
        !client.revoked && client.expires_at > unix_now() && client.revision == revision,
        "external_unauthenticated"
    );
    ensure!(
        client.control_sessions && client.read_metadata,
        "external_control_forbidden"
    );
    Ok(client)
}
fn command_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Command> {
    Ok(Command {
        operation_id: row.get(0)?,
        client_id: row.get(1)?,
        revision: row.get(2)?,
        tool: row.get(3)?,
        session_id: row.get(4)?,
        turn_id: row.get(5)?,
    })
}
impl Store {
    pub(crate) fn external_attach_in(&self, conn: &Connection, command: &Command) -> Result<()> {
        conn.execute(
            "UPDATE sessions SET external_attach_operation=?3 WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), command.session_id, command.operation_id],
        )?;
        Ok(())
    }
    pub(crate) fn external_intent_in(&self, conn: &Connection, command: &Command, event: i64) -> Result<()> {
        conn.execute(
            "UPDATE external_operations SET intent_event=?3 WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), command.operation_id, event],
        )?;
        Ok(())
    }
    /// Recheck the exact transition, not just the session's current authority.
    pub(crate) fn external_handoff_in(&self, conn: &Connection, command: &Command, transition: &str) -> Result<()> {
        self.external_guard_in(conn, command, None)?;
        let valid:bool = match transition {
            "start" | "resume" => conn.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id=?2 AND lifecycle='starting' AND external_attach_operation=?3)",params![self.owner_id(),command.session_id,command.operation_id],|r|r.get(0))?,
            "prompt" | "cancel" => conn.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id=?2 AND lifecycle='active' AND open_turn_id=?3)",params![self.owner_id(),command.session_id,command.turn_id],|r|r.get(0))?,
            "park" | "close" => {
                let kind=if transition=="park" {"operator_parked"} else {"operator_closed"};
                conn.query_row("SELECT EXISTS(SELECT 1 FROM sessions s JOIN external_operations o ON o.owner_id=s.owner_id AND o.session_id=s.id WHERE s.owner_id = ?1 AND s.id=?2 AND o.id=?3 AND s.lifecycle='active' AND o.intent_event=(SELECT MAX(e.event_id) FROM events e WHERE e.owner_id = ?1 AND e.session_id=?2 AND e.kind IN (?4,'operator_resumed')) AND (?4<>'operator_closed' OR s.close_requested=1))",params![self.owner_id(),command.session_id,command.operation_id,kind],|r|r.get(0))?
            }
            _ => false,
        };
        ensure!(valid, "not_running");
        Ok(())
    }
    /// Undo only this command's proven-unsent transition. Newer operator
    /// attachments/intents must survive a failed external handoff.
    pub(crate) fn external_unsent(&self, command: &Command, transition: &str) -> Result<()> {
        if transition == "prompt" {
            if let Some(turn) = &command.turn_id {
                self.abandon_turn(&command.session_id, turn)?;
            }
            return Ok(());
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if matches!(transition, "start" | "resume") {
            let changed=tx.execute("UPDATE sessions SET lifecycle='failed',failure_reason='external_authority_changed' WHERE owner_id = ?1 AND id=?2 AND lifecycle='starting' AND external_attach_operation=?3",params![self.owner_id(),command.session_id,command.operation_id])?;
            if changed == 1 {
                self.record_delegation_failure_in(&tx, &command.session_id, "external_authority_changed")?;
                let mcp = self.mcp();
                let cut = mcp.revoke_in(&tx, &command.session_id)?;
                crate::store::commit_then_cut(tx, mcp.as_ref(), cut)?;
                return Ok(());
            }
        } else if transition == "close" {
            tx.execute("UPDATE sessions SET close_requested=0 WHERE owner_id = ?1 AND id=?2 AND close_requested=1 AND (SELECT intent_event FROM external_operations WHERE owner_id = ?1 AND id=?3)=(SELECT MAX(event_id) FROM events WHERE owner_id = ?1 AND session_id=?2 AND kind='operator_closed')",params![self.owner_id(),command.session_id,command.operation_id])?;
        } else if transition == "park" {
            tx.execute("UPDATE sessions SET prompt_queue_held=0 WHERE owner_id = ?1 AND id=?2 AND lifecycle='active' AND (SELECT intent_event FROM external_operations WHERE owner_id = ?1 AND id=?3)=(SELECT MAX(event_id) FROM events WHERE owner_id = ?1 AND session_id=?2 AND kind='operator_parked')",params![self.owner_id(),command.session_id,command.operation_id])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn external_execution_client(&self, command: &Command) -> Result<ClientRecord> {
        current_client(&self.conn(), self.owner_id(), &command.client_id, command.revision)
    }
    pub(crate) fn external_guard_in(
        &self,
        conn: &Connection,
        command: &Command,
        create: Option<(&str, &str, &str)>,
    ) -> Result<()> {
        let client = current_client(conn, self.owner_id(), &command.client_id, command.revision)?;
        let known:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM external_operations WHERE owner_id = ?1 AND id=?2 AND client_id=?3 AND revision=?4 AND session_id=?5 AND state IN ('running','uncertain','queued'))",params![self.owner_id(),command.operation_id,command.client_id,command.revision,command.session_id],|r|r.get(0))?;
        ensure!(known, "external_unauthenticated");
        if let Some((host, cwd, hat)) = create {
            let live:i64=conn.query_row("SELECT count(*) FROM external_client_sessions x JOIN sessions s ON s.owner_id=x.owner_id AND s.id=x.session_id WHERE x.owner_id = ?1 AND x.client_id=?2 AND s.lifecycle NOT IN ('closed','deleted','failed')",params![self.owner_id(),client.client_id],|r|r.get(0))?;
            ensure!(live < 32, "session_limit");
            placement(conn, self.owner_id(), &client, host, cwd, hat)
        } else {
            session_scope(conn, self.owner_id(), &client, &command.session_id, true)
        }
    }
    pub(crate) fn external_request_in(&self, conn: &Connection, command: &Command, request: &str) -> Result<()> {
        conn.execute(
            "UPDATE external_operations SET request_id=?3 WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), command.operation_id, request],
        )?;
        Ok(())
    }
    pub(crate) fn external_admit(
        &self,
        identity: &ExternalIdentity,
        tool: &str,
        key: &str,
        args: &Value,
    ) -> Result<Receipt> {
        ensure!(
            !key.is_empty() && key.len() <= 128 && key.bytes().all(|b| b.is_ascii_graphic()),
            "invalid_request_key"
        );
        let raw = serde_json::to_string(args)?;
        ensure!(raw.len() <= 48 * 1024, "invalid_arguments");
        let digest = sha256_hex(serde_json::to_string(&json!([tool, args]))?.as_bytes());
        let key_hash = sha256_hex(key.as_bytes());
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            external_clients::live(&tx, self.owner_id(), identity)?,
            "external_unauthenticated"
        );
        let client = &identity.client;
        ensure!(client.control_sessions, "external_control_forbidden");
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT id,digest FROM external_operations WHERE owner_id = ?1 AND client_id=?2 AND request_key_hash=?3",
                params![self.owner_id(), client.client_id, key_hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, stored)) = old {
            ensure!(stored == digest, "request_key_conflict");
            return self.external_receipt_in(&tx, identity, &id);
        }
        let session = if tool == "session_create" {
            let host = args["host_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid_arguments"))?;
            let cwd = args["cwd"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid_arguments"))?;
            placement(&tx, self.owner_id(), client, host, cwd, &client.hat_id)?;
            let live:i64=tx.query_row("SELECT count(*) FROM external_client_sessions x JOIN sessions s ON s.owner_id=x.owner_id AND s.id=x.session_id WHERE x.owner_id = ?1 AND x.client_id=?2 AND s.lifecycle NOT IN ('closed','deleted','failed')",params![self.owner_id(),client.client_id],|r|r.get(0))?;
            ensure!(live < 32, "session_limit");
            uuid::Uuid::now_v7().to_string()
        } else {
            let id = args["session_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid_arguments"))?;
            session_scope(&tx, self.owner_id(), client, id, true)?;
            id.to_owned()
        };
        let (pending,bytes,total):(i64,i64,i64)=tx.query_row("SELECT COALESCE(SUM(state IN ('queued','running','uncertain')),0),COALESCE(SUM(length(CAST(args AS BLOB))),0),count(*) FROM external_operations WHERE owner_id = ?1 AND client_id=?2",params![self.owner_id(),client.client_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        ensure!(
            pending < 32 && bytes + raw.len() as i64 <= 1024 * 1024 && total < 4096,
            "operation_limit"
        );
        let id = uuid::Uuid::now_v7().to_string();
        let turn = if tool == "session_prompt" {
            Some(uuid::Uuid::now_v7().to_string())
        } else {
            args["turn_id"].as_str().map(str::to_owned)
        };
        tx.execute("INSERT INTO external_operations(owner_id,id,client_id,revision,request_key_hash,digest,tool,session_id,turn_id,state,args,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'queued',?10,?11)",params![self.owner_id(),id,client.client_id,client.revision,key_hash,digest,tool,session,turn,raw,unix_now()])?;
        let receipt = self.external_receipt_in(&tx, identity, &id)?;
        tx.commit()?;
        Ok(receipt)
    }
    fn external_receipt_in(&self, conn: &Connection, identity: &ExternalIdentity, id: &str) -> Result<Receipt> {
        ensure!(
            external_clients::live(conn, self.owner_id(), identity)?,
            "external_unauthenticated"
        );
        ensure!(identity.client.control_sessions, "external_control_forbidden");
        let mut receipt:Receipt=conn.query_row("SELECT id,request_key_hash,session_id,turn_id,state,code,result FROM external_operations WHERE owner_id = ?1 AND client_id=?2 AND id=?3",params![self.owner_id(),identity.client.client_id,id],|r|Ok(Receipt{operation_id:r.get(0)?,request_key_hash:r.get(1)?,session_id:r.get(2)?,turn_id:r.get(3)?,state:r.get(4)?,code:r.get(5)?,result:r.get::<_,Option<String>>(6)?.and_then(|s|serde_json::from_str(&s).ok()),outcome:None})).optional()?.ok_or_else(||anyhow::anyhow!("not_found"))?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted')",
            params![self.owner_id(), receipt.session_id],
            |r| r.get(0),
        )?;
        if exists {
            session_scope(conn, self.owner_id(), &identity.client, &receipt.session_id, false)?;
        }
        let (tool, request): (String, Option<String>) = conn.query_row(
            "SELECT tool,request_id FROM external_operations WHERE owner_id = ?1 AND id=?2",
            params![self.owner_id(), id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if !matches!(receipt.state.as_str(), "failed" | "cancelled") {
            if tool == "session_prompt" || tool == "session_cancel" {
                if let Some(turn) = receipt.turn_id.as_deref() {
                    let row: Option<(String, Option<String>)> = conn
                        .query_row(
                            "SELECT state,outcome FROM turns WHERE owner_id = ?1 AND session_id=?2 AND turn_id=?3",
                            params![self.owner_id(), receipt.session_id, turn],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    match row {
                        Some((state, outcome)) if state == "ended" => {
                            receipt.state = "completed".into();
                            receipt.outcome = outcome;
                        }
                        Some((state, _)) if state == "started" && tool == "session_prompt" => {
                            receipt.state = "running".into()
                        }
                        Some((state, _)) if state == "not_delivered" => {
                            receipt.state = "not_delivered".into();
                            receipt.code = Some("not_delivered".into());
                        }
                        _ => {}
                    }
                }
            } else if matches!(tool.as_str(), "session_create" | "session_resume") {
                let started:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE owner_id = ?1 AND session_id=?2 AND kind='session_started' AND applied=1 AND json_extract(body,'$.request_id')=?3)",params![self.owner_id(),receipt.session_id,request],|r|r.get(0))?;
                if started {
                    receipt.state = "completed".into();
                    receipt.code = None;
                }
            }
        }
        if receipt.state == "uncertain" {
            receipt.state = "delivery_unknown".into();
        }
        Ok(receipt)
    }
    pub(crate) fn external_operation(
        &self,
        identity: &ExternalIdentity,
        id: Option<&str>,
        key: Option<&str>,
    ) -> Result<Receipt> {
        ensure!(id.is_some() ^ key.is_some(), "invalid_arguments");
        let conn = self.conn();
        let id = match id {
            Some(id) => id.to_owned(),
            None => conn
                .query_row(
                    "SELECT id FROM external_operations WHERE owner_id = ?1 AND client_id=?2 AND request_key_hash=?3",
                    params![
                        self.owner_id(),
                        identity.client.client_id,
                        key.map(|k| sha256_hex(k.as_bytes()))
                    ],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| anyhow::anyhow!("not_found"))?,
        };
        self.external_receipt_in(&conn, identity, &id)
    }
    pub(crate) fn external_pending(&self) -> Result<Vec<(Command, Value)>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT id,client_id,revision,tool,session_id,turn_id,args FROM external_operations o WHERE o.owner_id = ?1 AND o.state='queued'
            AND (o.tool<>'session_prompt' OR (
                NOT EXISTS(SELECT 1 FROM external_operations p WHERE p.owner_id = ?1 AND p.session_id=o.session_id AND p.tool='session_prompt' AND p.position<o.position AND p.state IN ('queued','running','uncertain'))
                AND EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id = ?1 AND s.id=o.session_id AND s.lifecycle IN ('active','parked') AND s.open_turn_id IS NULL AND COALESCE(s.activity,'')<>'blocked'
                    AND NOT EXISTS(SELECT 1 FROM pending p WHERE p.owner_id = ?1 AND p.session_id=s.id AND p.state='open')
                    AND NOT EXISTS(SELECT 1 FROM session_dispatch_queue q WHERE q.owner_id = ?1 AND q.target_id=s.id AND q.state IN ('queued','waiting_config','awaiting_reconciliation'))))) ORDER BY position LIMIT 32")?;
        stmt.query_map([self.owner_id()], |r| Ok((command_row(r)?, r.get::<_, String>(6)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(c, s)| Ok((c, serde_json::from_str(&s)?)))
            .collect::<Result<Vec<_>>>()
    }
    pub(crate) fn external_begin(&self, command: &Command) -> Result<bool> {
        let conn = self.conn();
        Ok(conn.execute(
            "UPDATE external_operations SET state='running' WHERE owner_id = ?1 AND id=?2 AND state='queued'",
            params![self.owner_id(), command.operation_id],
        )? == 1)
    }
    pub(crate) fn external_finish(
        &self,
        command: &Command,
        state: &str,
        code: Option<&str>,
        result: Option<&Value>,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute("UPDATE external_operations SET state=?3,code=?4,result=?5,args=CASE WHEN ?3='queued' THEN args ELSE json_object() END WHERE owner_id = ?1 AND id=?2 AND state IN ('queued','running','uncertain')",params![self.owner_id(),command.operation_id,state,code,result.map(serde_json::to_string).transpose()?])?;
        Ok(())
    }
    pub(crate) fn external_recover_operations(&self) -> Result<()> {
        {
            let conn = self.conn();
            let pending:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM external_operations WHERE owner_id = ?1 AND state IN ('running','uncertain'))",[self.owner_id()],|r|r.get(0))?;
            if !pending {
                return Ok(());
            }
            conn.execute("UPDATE external_operations SET state='uncertain',args=json_object(),code='delivery_unknown' WHERE owner_id = ?1 AND state='running'",[self.owner_id()])?;
        }
        let unsent = {
            let conn = self.conn();
            let mut stmt=conn.prepare("SELECT id,client_id,revision,tool,session_id,turn_id FROM external_operations WHERE owner_id = ?1 AND state='uncertain' AND request_id IS NULL")?;
            stmt.query_map([self.owner_id()], command_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for command in unsent {
            let transition = if command.tool == "session_create" {
                "start"
            } else {
                command.tool.strip_prefix("session_").unwrap_or_default()
            };
            self.external_unsent(&command, transition)?;
            self.external_finish(&command, "failed", Some("not_delivered"), None)?;
        }
        Ok(())
    }
    pub(crate) fn external_settle_operations(&self) -> Result<()> {
        let conn = self.conn();
        // An idle worker must not contend for a SQLite writer or block the
        // collector's runtime on the busy timeout, including when its ledger
        // contains only terminal receipts.
        let pending:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM external_operations WHERE owner_id = ?1 AND state IN ('queued','running','uncertain'))",[self.owner_id()],|r|r.get(0))?;
        if !pending {
            return Ok(());
        }
        conn.execute("UPDATE external_operations SET state='failed',code='not_attached',args=json_object() WHERE owner_id = ?1 AND tool='session_prompt' AND state='queued' AND NOT EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id = ?1 AND s.id=external_operations.session_id AND s.lifecycle IN ('active','parked','starting'))",[self.owner_id()])?;
        conn.execute("UPDATE external_operations SET state='failed',code='not_delivered',args=json_object() WHERE owner_id = ?1 AND tool='session_prompt' AND state IN ('running','uncertain') AND EXISTS(SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.session_id=external_operations.session_id AND t.turn_id=external_operations.turn_id AND t.state='not_delivered')",[self.owner_id()])?;
        conn.execute("UPDATE external_operations SET state='cancelled',code='grant_changed',args=json_object() WHERE owner_id = ?1 AND state='queued'
            AND NOT EXISTS(SELECT 1 FROM external_clients c WHERE c.owner_id = ?1 AND c.id=external_operations.client_id AND c.revision=external_operations.revision AND c.revoked_at IS NULL AND c.expires_at>?2
              AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=c.hat_id))",params![self.owner_id(),unix_now()])?;
        conn.execute("UPDATE external_operations SET state='failed',code='not_delivered',args=json_object() WHERE owner_id = ?1 AND state='uncertain' AND request_id IS NULL AND NOT EXISTS(SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.session_id=external_operations.session_id AND t.turn_id=external_operations.turn_id)", [self.owner_id()])?;
        conn.execute("UPDATE external_operations SET state='completed',args=json_object() WHERE owner_id = ?1 AND state IN ('running','uncertain') AND
            ((tool='session_prompt' AND EXISTS(SELECT 1 FROM turns t WHERE t.owner_id = ?1 AND t.session_id=external_operations.session_id AND t.turn_id=external_operations.turn_id AND t.state='ended'))
            OR (tool IN ('session_create','session_resume') AND EXISTS(SELECT 1 FROM events e WHERE e.owner_id = ?1 AND e.session_id=external_operations.session_id AND e.kind='session_started' AND e.applied=1 AND json_extract(e.body,'$.request_id')=external_operations.request_id)))",[self.owner_id()])?;
        Ok(())
    }
    pub(crate) fn external_authorized_read(
        &self,
        identity: &ExternalIdentity,
        id: &str,
        transcript: bool,
    ) -> Result<()> {
        let conn = self.conn();
        ensure!(
            external_clients::live(&conn, self.owner_id(), identity)?,
            "external_unauthenticated"
        );
        ensure!(identity.client.read_metadata, "external_forbidden");
        ensure!(
            !transcript || identity.client.read_transcript,
            "external_transcript_forbidden"
        );
        session_scope(&conn, self.owner_id(), &identity.client, id, false)
    }
    pub(crate) fn external_turn_outcome(
        &self,
        command: &Command,
        turn: &str,
    ) -> Result<Option<hennery_proto::frames::TurnOutcome>> {
        let conn = self.conn();
        self.external_guard_in(&conn, command, None)?;
        let outcome: Option<String> = conn
            .query_row(
                "SELECT outcome FROM turns WHERE owner_id = ?1 AND session_id=?2 AND turn_id=?3 AND state='ended'",
                params![self.owner_id(), command.session_id, turn],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        outcome
            .map(|s| serde_json::from_value(json!(s)).map_err(Into::into))
            .transpose()
    }
}
