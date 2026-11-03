//! Bounded image staging. Chunks live in SQLite until ordinary prompt admission
//! writes canonical attachments. Staging never grants access to global hashes.
use crate::{
    AppState,
    external_clients::{ClientRecord, ExternalIdentity},
    external_control::Command,
};
use anyhow::{Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use hennery_kernel::secret::{sha256_hex, unix_now};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
pub(crate) const CHUNK: usize = 24576;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Begin {
    request_key: String,
    session_id: String,
    mime_type: String,
    size: usize,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Upload {
    request_key: String,
    session_id: String,
    upload_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    session_id: String,
    upload_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    session_id: String,
    upload_id: String,
    offset: usize,
    data_base64: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Get {
    session_id: String,
    sha256: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "chunk_size")]
    limit: usize,
}
fn chunk_size() -> usize {
    CHUNK
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| anyhow::anyhow!("invalid_arguments"))
}
pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "attachment_upload_begin"
            | "attachment_upload_chunk"
            | "attachment_upload_finish"
            | "attachment_upload_abort"
            | "attachment_upload_status"
            | "attachment_get"
    )
}
pub(crate) fn tools(identity: &ExternalIdentity) -> Vec<Value> {
    let id = json!({"type":"string","minLength":1,"maxLength":128});
    let mut rows = Vec::new();
    for (name, description, props, required, read, transcript) in [
        (
            "attachment_upload_begin",
            "Reserve an image upload for one authorized session. PNG/JPEG/GIF/WebP, up to 5 MiB each; 20 MiB staged per client, 64 MiB per owner. Returns upload_id, expiry and chunk_bytes.",
            json!({"request_key":id,"session_id":id,"mime_type":{"enum":crate::content::IMAGE_TYPES},"size":{"type":"integer","minimum":1,"maximum":crate::content::MAX_IMAGE_BYTES},"sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"}}),
            vec!["request_key", "session_id", "mime_type", "size", "sha256"],
            false,
            false,
        ),
        (
            "attachment_upload_chunk",
            "Upload a contiguous base64 chunk, at most chunk_bytes decoded. Exact offset/content retries are idempotent, including after finalization. Conflicting or out-of-order chunks fail.",
            json!({"session_id":id,"upload_id":id,"offset":{"type":"integer","minimum":0},"data_base64":{"type":"string","maxLength":32768}}),
            vec!["session_id", "upload_id", "offset", "data_base64"],
            false,
            false,
        ),
        (
            "attachment_upload_finish",
            "Validate the complete image and its declared SHA-256. Returns a sealed upload_id usable in session_prompt attachments.",
            json!({"request_key":id,"session_id":id,"upload_id":id}),
            vec!["request_key", "session_id", "upload_id"],
            false,
            false,
        ),
        (
            "attachment_upload_abort",
            "Remove this client's staged upload. Refuses an upload referenced by an active prompt; never removes transcript attachments.",
            json!({"request_key":id,"session_id":id,"upload_id":id}),
            vec!["request_key", "session_id", "upload_id"],
            false,
            false,
        ),
        (
            "attachment_upload_status",
            "Recover this client's upload progress, expected offset, expiry and sealed state.",
            json!({"session_id":id,"upload_id":id}),
            vec!["session_id", "upload_id"],
            true,
            false,
        ),
        (
            "attachment_get",
            "Read a bounded base64 image chunk by session and hash, with transcript consent. Only images referenced by this session's canonical turns are readable.",
            json!({"session_id":id,"sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":CHUNK}}),
            vec!["session_id", "sha256"],
            true,
            true,
        ),
    ] {
        if (transcript && identity.client.read_transcript) || (!transcript && identity.client.control_sessions) {
            rows.push(json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":props,"required":required,"additionalProperties":false},"outputSchema":{"type":"object","properties":{"data":{}},"required":["data"],"additionalProperties":false},"annotations":{"readOnlyHint":read,"destructiveHint":!read,"idempotentHint":true,"openWorldHint":false}}));
        }
    }
    rows
}
fn status_in(conn: &Connection, owner: &str, client: &str, session: &str, upload: &str) -> Result<Value> {
    conn.query_row("SELECT mime,sha256,size,received,sealed,expires_at FROM external_uploads WHERE owner_id = ?1 AND client_id=?2 AND session_id=?3 AND id=?4 AND expires_at>?5",params![owner,client,session,upload,unix_now()],|r|Ok(json!({"upload_id":upload,"mime_type":r.get::<_,String>(0)?,"sha256":r.get::<_,String>(1)?,"size":r.get::<_,i64>(2)?,"received":r.get::<_,i64>(3)?,"sealed":r.get::<_,bool>(4)?,"expires_at":r.get::<_,i64>(5)?,"chunk_bytes":CHUNK}))).optional()?.ok_or_else(||anyhow::anyhow!("not_found"))
}
pub(crate) async fn invoke(state: &AppState, identity: &ExternalIdentity, name: &str, args: Value) -> Result<Value> {
    state.store.external_roots(identity)?;
    ensure!(
        if name == "attachment_get" {
            identity.client.read_transcript
        } else {
            identity.client.control_sessions
        },
        "{}",
        if name == "attachment_get" {
            "external_transcript_forbidden"
        } else {
            "external_control_forbidden"
        }
    );
    let value = match name {
        "attachment_upload_chunk" => {
            let q: Chunk = decode(args)?;
            ensure!(q.data_base64.len() <= CHUNK * 4 / 3, "invalid_arguments");
            let bytes = STANDARD
                .decode(q.data_base64)
                .map_err(|_| anyhow::anyhow!("invalid_arguments"))?;
            let digest = sha256_hex(&bytes);
            let mut conn = state.store.conn();
            let tx = conn.transaction()?;
            state.store.external_read_in(&tx, identity, &q.session_id, false)?;
            crate::external_control_store::session_scope(
                &tx,
                state.store.owner_id(),
                &identity.client,
                &q.session_id,
                true,
            )?;
            let status = status_in(
                &tx,
                state.store.owner_id(),
                &identity.client.client_id,
                &q.session_id,
                &q.upload_id,
            )?;
            let size = status["size"].as_u64().unwrap() as usize;
            let received = status["received"].as_u64().unwrap() as usize;
            ensure!(
                q.offset < size && q.offset.is_multiple_of(CHUNK) && bytes.len() == CHUNK.min(size - q.offset),
                "invalid_arguments"
            );
            let old: Option<String> = tx
                .query_row(
                    "SELECT digest FROM external_upload_chunks WHERE owner_id = ?1 AND upload_id=?2 AND offset=?3",
                    params![state.store.owner_id(), q.upload_id, q.offset as i64],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(old) = old {
                ensure!(old == digest, "chunk_conflict");
            } else {
                ensure!(status["sealed"] == false && q.offset == received, "invalid_offset");
                tx.execute(
                    "INSERT INTO external_upload_chunks(owner_id,upload_id,offset,digest,data)VALUES(?1,?2,?3,?4,?5)",
                    params![state.store.owner_id(), q.upload_id, q.offset as i64, digest, bytes],
                )?;
                tx.execute(
                    "UPDATE external_uploads SET received=received+?3 WHERE owner_id = ?1 AND id=?2",
                    params![state.store.owner_id(), q.upload_id, bytes.len() as i64],
                )?;
            }
            let result = status_in(
                &tx,
                state.store.owner_id(),
                &identity.client.client_id,
                &q.session_id,
                &q.upload_id,
            )?;
            tx.commit()?;
            result
        }
        "attachment_upload_status" => {
            let q: Status = decode(args)?;
            let conn = state.store.conn();
            state.store.external_read_in(&conn, identity, &q.session_id, false)?;
            status_in(
                &conn,
                state.store.owner_id(),
                &identity.client.client_id,
                &q.session_id,
                &q.upload_id,
            )?
        }
        "attachment_get" => {
            let q: Get = decode(args)?;
            ensure!(
                crate::attachments::is_sha256(&q.sha256) && (1..=CHUNK).contains(&q.limit),
                "invalid_arguments"
            );
            let state = state.clone();
            let identity = identity.clone();
            let permit = state
                .external_commands
                .reads
                .clone()
                .try_acquire_owned()
                .map_err(|_| anyhow::anyhow!("operation_limit"))?;
            tokio::task::spawn_blocking(move||->Result<Value>{let _permit=permit;let conn=state.store.conn();state.store.external_read_in(&conn,&identity,&q.session_id,true)?;
                let mime:Option<String>=conn.query_row("SELECT a.mime FROM attachments a WHERE a.owner_id = ?1 AND a.sha256=?3 AND EXISTS(SELECT 1 FROM turn_attachments ta JOIN turns t ON t.owner_id=ta.owner_id AND t.turn_id=ta.turn_id WHERE ta.owner_id = ?1 AND ta.sha256=a.sha256 AND t.session_id=?2)",params![state.store.owner_id(),q.session_id,q.sha256],|r|r.get(0)).optional()?;
                let mime=mime.ok_or_else(||anyhow::anyhow!("not_found"))?;let dir=state.store.attachments.as_deref().ok_or_else(||anyhow::anyhow!("not_found"))?;let bytes=crate::attachments::read(dir,&q.sha256)?.ok_or_else(||anyhow::anyhow!("not_found"))?;
                ensure!(bytes.len()<=crate::content::MAX_IMAGE_BYTES&&q.offset<=bytes.len(),"invalid_arguments");let end=q.offset.saturating_add(q.limit).min(bytes.len());Ok(json!({"sha256":q.sha256,"mime_type":mime,"size":bytes.len(),"offset":q.offset,"data_base64":STANDARD.encode(&bytes[q.offset..end]),"next_offset":if end<bytes.len(){Some(end)}else{None}}))}).await??
        }
        _ => {
            let key = match name {
                "attachment_upload_begin" => {
                    let q: Begin = decode(args.clone())?;
                    ensure!(
                        crate::content::IMAGE_TYPES.contains(&q.mime_type.as_str())
                            && (1..=crate::content::MAX_IMAGE_BYTES).contains(&q.size)
                            && crate::attachments::is_sha256(&q.sha256),
                        "invalid_arguments"
                    );
                    q.request_key
                }
                "attachment_upload_finish" | "attachment_upload_abort" => {
                    let q: Upload = decode(args.clone())?;
                    q.request_key
                }
                _ => bail!("unknown_tool"),
            };
            crate::external_control::after_startup(state);
            serde_json::to_value(state.store.external_admit(identity, name, &key, &args)?)?
        }
    };
    state.store.external_roots(identity)?;
    Ok(json!({"data":value}))
}
fn assemble(conn: &Connection, owner: &str, upload: &str) -> Result<Vec<u8>> {
    let mut stmt =
        conn.prepare("SELECT data FROM external_upload_chunks WHERE owner_id = ?1 AND upload_id=?2 ORDER BY offset")?;
    let mut bytes = Vec::new();
    for data in stmt.query_map(params![owner, upload], |r| r.get::<_, Vec<u8>>(0))? {
        bytes.extend(data?);
        ensure!(bytes.len() <= crate::content::MAX_IMAGE_BYTES, "invalid_image");
    }
    Ok(bytes)
}
pub(crate) async fn execute(state: &AppState, command: &Command, args: Value) -> Result<Value> {
    let state = state.clone();
    let command = command.clone();
    tokio::task::spawn_blocking(move || execute_blocking(&state, &command, args)).await?
}
fn execute_blocking(state: &AppState, command: &Command, args: Value) -> Result<Value> {
    let mut conn = state.store.conn();
    if command.tool == "attachment_upload_begin" {
        let q: Begin = decode(args)?;
        let tx = conn.transaction()?;
        state.store.external_guard_in(&tx, command, None)?;
        let (client,owner,count):(i64,i64,i64)=tx.query_row("SELECT COALESCE(sum(CASE WHEN client_id=?2 THEN size ELSE 0 END),0),COALESCE(sum(size),0),count(*) FROM external_uploads WHERE owner_id = ?1",params![state.store.owner_id(),command.client_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        ensure!(
            client + q.size as i64 <= 20 * 1024 * 1024 && owner + q.size as i64 <= 64 * 1024 * 1024 && count < 256,
            "upload_limit"
        );
        tx.execute("INSERT INTO external_uploads(owner_id,id,client_id,session_id,mime,sha256,size,expires_at)VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![state.store.owner_id(),command.operation_id,command.client_id,q.session_id,q.mime_type,q.sha256,q.size as i64,unix_now()+86400])?;
        let value = status_in(
            &tx,
            state.store.owner_id(),
            &command.client_id,
            &command.session_id,
            &command.operation_id,
        )?;
        finish_in(&tx, state.store.owner_id(), command, &value)?;
        tx.commit()?;
        return Ok(value);
    }
    let q: Upload = decode(args)?;
    state.store.external_guard_in(&conn, command, None)?;
    let status = status_in(
        &conn,
        state.store.owner_id(),
        &command.client_id,
        &q.session_id,
        &q.upload_id,
    )?;
    if command.tool == "attachment_upload_abort" {
        let tx = conn.transaction()?;
        state.store.external_guard_in(&tx, command, None)?;
        let used:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM external_operation_uploads u JOIN external_operations o ON o.owner_id=u.owner_id AND o.id=u.operation_id WHERE u.owner_id = ?1 AND u.upload_id=?2 AND o.state IN ('queued','running','uncertain'))",params![state.store.owner_id(),q.upload_id],|r|r.get(0))?;
        ensure!(!used, "upload_in_use");
        tx.execute(
            "DELETE FROM external_uploads WHERE owner_id = ?1 AND id=?2",
            params![state.store.owner_id(), q.upload_id],
        )?;
        let value = json!({"upload_id":q.upload_id,"state":"aborted"});
        finish_in(&tx, state.store.owner_id(), command, &value)?;
        tx.commit()?;
        return Ok(value);
    }
    ensure!(status["received"] == status["size"], "upload_incomplete");
    let bytes = assemble(&conn, state.store.owner_id(), &q.upload_id)?;
    drop(conn);
    ensure!(
        sha256_hex(&bytes) == status["sha256"].as_str().unwrap(),
        "hash_mismatch"
    );
    crate::content::check(vec![
        json!({"type":"image","mimeType":status["mime_type"],"data":STANDARD.encode(&bytes)}),
    ])
    .map_err(|_| anyhow::anyhow!("invalid_image"))?;
    let mut conn = state.store.conn();
    let tx = conn.transaction()?;
    state.store.external_guard_in(&tx, command, None)?;
    ensure!(
        status_in(
            &tx,
            state.store.owner_id(),
            &command.client_id,
            &q.session_id,
            &q.upload_id
        )? == status,
        "upload_changed"
    );
    tx.execute(
        "UPDATE external_uploads SET sealed=1 WHERE owner_id = ?1 AND id=?2",
        params![state.store.owner_id(), q.upload_id],
    )?;
    let mut value = status;
    value["sealed"] = json!(true);
    finish_in(&tx, state.store.owner_id(), command, &value)?;
    tx.commit()?;
    Ok(value)
}
fn finish_in(conn: &Connection, owner: &str, command: &Command, value: &Value) -> Result<()> {
    conn.execute(
        "UPDATE external_operations SET state='completed',args=json_object(),result=?3 WHERE owner_id = ?1 AND id=?2",
        params![owner, command.operation_id, serde_json::to_string(value)?],
    )?;
    Ok(())
}
pub(crate) fn bind_in(
    conn: &Connection,
    owner: &str,
    client: &ClientRecord,
    session: &str,
    operation: &str,
    args: &Value,
) -> Result<()> {
    let ids: Vec<String> = serde_json::from_value(args.get("attachments").cloned().unwrap_or_else(|| json!([])))
        .map_err(|_| anyhow::anyhow!("invalid_arguments"))?;
    ensure!(ids.len() <= 20, "invalid_arguments");
    let mut total = 0;
    for id in ids {
        let status = status_in(conn, owner, &client.client_id, session, &id)?;
        ensure!(status["sealed"] == true, "upload_incomplete");
        total += status["size"].as_u64().unwrap();
        ensure!(total <= crate::content::MAX_IMAGES_TOTAL as u64, "invalid_arguments");
        conn.execute("INSERT INTO external_operation_uploads(owner_id,operation_id,upload_id)VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",params![owner,operation,id])?;
    }
    Ok(())
}
pub(crate) fn prompt_images(state: &AppState, command: &Command, ids: &[String]) -> Result<Vec<Value>> {
    let conn = state.store.conn();
    state.store.external_guard_in(&conn, command, None)?;
    let mut images = Vec::new();
    for id in ids {
        let mime:Option<String>=conn.query_row("SELECT u.mime FROM external_uploads u JOIN external_operation_uploads b ON b.owner_id=u.owner_id AND b.upload_id=u.id WHERE u.owner_id = ?1 AND u.id=?2 AND b.operation_id=?3 AND u.client_id=?4 AND u.session_id=?5 AND u.sealed=1",params![state.store.owner_id(),id,command.operation_id,command.client_id,command.session_id],|r|r.get(0)).optional()?;
        let mime = mime.ok_or_else(|| anyhow::anyhow!("not_found"))?;
        images.push(
            json!({"type":"image","mimeType":mime,"data":STANDARD.encode(assemble(&conn,state.store.owner_id(),id)?)}),
        );
    }
    Ok(images)
}
pub(crate) fn cleanup(state: &AppState) -> Result<()> {
    let conn = state.store.conn();
    let mut stmt=conn.prepare("SELECT u.id FROM external_uploads u JOIN external_clients c ON c.owner_id=u.owner_id AND c.id=u.client_id WHERE u.owner_id = ?1 AND (c.expires_at<=?2 OR (u.expires_at<=?2 AND NOT EXISTS(SELECT 1 FROM external_operation_uploads b JOIN external_operations o ON o.owner_id=b.owner_id AND o.id=b.operation_id WHERE b.owner_id = ?1 AND b.upload_id=u.id AND o.state IN ('queued','running','uncertain')))) LIMIT 256")?;
    let ids = stmt
        .query_map(params![state.store.owner_id(), unix_now()], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in ids {
        conn.execute(
            "DELETE FROM external_uploads WHERE owner_id = ?1 AND id=?2",
            params![state.store.owner_id(), id],
        )?;
    }
    Ok(())
}
