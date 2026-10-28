//! Bounded fair draining of accepted parent commands and child notices.
use crate::{
    AppState,
    delegation_runtime::{checked_cwd, config_offered},
};
use std::time::Duration;
#[derive(Default)]
struct Cursors {
    prompts: String,
    answers: String,
    notices: String,
}
#[derive(Default)]
struct Reconciliation {
    attention: String,
    notifications: String,
}
pub fn after_startup(state: &AppState) {
    let store = state.store.clone();
    state.push.set_guard(std::sync::Arc::new(move |notice| {
        store.delegation_push_current(notice).unwrap_or(false)
    }));
    // Host probes can take seconds each. Deadlines and push reconciliation
    // must keep their own bounded cadence while a dispatch page waits on I/O.
    let reconciliation_state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut cursors = Reconciliation::default();
        loop {
            tokio::select! {biased; ()=reconciliation_state.shutdown.cancelled()=>break,_=tick.tick()=>{}}
            if let Err(error) = reconcile_page(&reconciliation_state, &mut cursors) {
                tracing::warn!(code=%crate::session_tools::error_code(&error),"delegation reconciliation deferred");
            }
        }
    });
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut cursors = Cursors::default();
        loop {
            tokio::select! {biased; ()=state.shutdown.cancelled()=>break,_=tick.tick()=>{}}
            if let Err(error) = drain_page(&state, &mut cursors).await {
                tracing::warn!(code=%crate::session_tools::error_code(&error),"delegation dispatch deferred");
            }
        }
    });
}
pub async fn drain(state: &AppState) -> anyhow::Result<()> {
    reconcile_page(state, &mut Reconciliation::default())?;
    drain_page(state, &mut Cursors::default()).await
}
fn reconcile_page(state: &AppState, cursors: &mut Reconciliation) -> anyhow::Result<()> {
    state.store.flush_idle_holds(&state.hub)?;
    let now = hennery_kernel::secret::unix_now();
    state.store.expire_operator_prompts(now)?;
    cursors.attention = state
        .store
        .refresh_attention_after(&state.hub, now, &cursors.attention)?;
    cursors.notifications =
        state
            .store
            .enqueue_delegation_attention(&state.push, &state.delegation_run, &cursors.notifications, now)?;
    state.store.flush_family_changes(&state.hub)?;
    state.store.flush_prompt_queue_changes(&state.hub)?;
    Ok(())
}
async fn drain_page(state: &AppState, cursors: &mut Cursors) -> anyhow::Result<()> {
    cursors.answers = state
        .store
        .dispatch_delegation_answers_after(&state.hub, &cursors.answers)?
        .unwrap_or_default();
    let mut targets = state.store.delegation_targets_after(&cursors.prompts)?;
    cursors.prompts = targets.last().cloned().unwrap_or_default();
    let parents = state.store.notice_parents_after(&cursors.notices)?;
    cursors.notices = parents.last().cloned().unwrap_or_default();
    targets.extend(parents);
    targets.sort();
    targets.dedup();
    for target in targets {
        tokio::select! {biased; ()=state.shutdown.cancelled()=>return Ok(()), result=drain_target(state,&target)=>{
            if let Err(error)=result {tracing::warn!(session_id=%target,code=%crate::session_tools::error_code(&error),"session dispatch deferred");}
        }}
    }
    state.store.flush_prompt_queue_changes(&state.hub)?;
    Ok(())
}

async fn drain_target(state: &AppState, target: &str) -> anyhow::Result<()> {
    let Some(session) = state.store.find_session(target)? else {
        return Ok(());
    };
    if state.hosts.is_revoked(&session.host_id)? {
        state.store.fail_unavailable_operator_queue(target, "host_revoked")?;
        return Ok(());
    }
    if session.lifecycle == "parked" && !state.hub.has_connection(&session.host_id) {
        state.store.fail_unavailable_operator_queue(target, "host_offline")?;
        return Ok(());
    }
    if !state.hub.is_ready(&session.host_id) || !matches!(session.lifecycle.as_str(), "starting" | "active" | "parked")
    {
        return Ok(());
    }
    if session.open_turn_id.is_some()
        || session.activity.as_deref() == Some("blocked")
        || !state.store.open_pending(target)?.is_empty()
    {
        return Ok(());
    }
    if session.lifecycle == "active" && state.store.operator_dispatch_ready(target)? {
        let Some(_admission) = state.store.reserve_prompt(target) else {
            return Ok(());
        };
        let Some(_operation) = state.store.operator_dispatch_id(target)? else {
            return Ok(());
        };
        let request = uuid::Uuid::now_v7().to_string();
        let mut startup = state
            .store
            .mcp()
            .startup_context(
                target,
                &request,
                state
                    .hub
                    .has_capability(&session.host_id, hennery_proto::frames::Capability::PluginContext),
            )
            .await;
        if state
            .store
            .dispatch_operator_prompt_with_request(target, &state.hub, &mut startup, None, &request)?
        {
            return Ok(());
        }
    }
    let candidate = if session.lifecycle == "active" {
        None
    } else {
        let head = match state.store.delegated_attach_candidate(target, &state.hub)? {
            Some(head) => Some(head),
            None => state.store.notice_attach_candidate(target, &state.hub)?,
        };
        let Some(head) = head else { return Ok(()) };
        if let Some(code) = head.refusal {
            if head.operator && code == "operator_resume_unsupported" {
                if state.hub.is_ready(&session.host_id)
                    && (!state
                        .hub
                        .has_capability(&session.host_id, hennery_proto::frames::Capability::OperatorResume)
                        || !state
                            .hub
                            .has_capability(&session.host_id, hennery_proto::frames::Capability::DelegationConfig))
                {
                    state.store.fail_unavailable_operator_queue(target, code)?;
                }
                return Ok(());
            }
            if crate::delegation_dispatch::attach_availability(
                &state.hub,
                &session.host_id,
                code != "agent_has_no_record",
            ) != Some(Some(code))
            {
                return Ok(());
            }
            let now = hennery_kernel::secret::unix_now();
            if code == "configuration_unavailable" {
                refuse_configuration(state, target, &head.operation, now)?;
            } else {
                state.store.fail_delegation_catalog(target, code, now)?;
                state.store.fail_notice_catalog(target, code, now)?;
            }
            return Ok(());
        }
        Some(head)
    };
    let work = async {
        // A hat edit applies only at a real attach. Already attached queues
        // continue under their original delivery/source snapshot.
        if session.lifecycle != "active" {
            state.store.check_mcp_source_policy(
                &session.hat_id,
                &session.agent,
                crate::api::mcp_context(state, &session.host_id, &session.agent)?,
            )?;
        }
        let cwd = checked_cwd(state, &session.host_id, &session.hat_id, &session.cwd).await?;
        anyhow::ensure!(cwd == session.cwd, "invalid_cwd");
        let catalog = if session.lifecycle == "active" {
            let read = if state.store.operator_dispatch_ready(target)? {
                let Some(operation) = state.store.operator_dispatch_id(target)? else {
                    return Ok(());
                };
                // Post-resume validation reuses the catalog captured after
                // queue admission, rather than requesting an explicit Refresh.
                state
                    .catalogs
                    .get_for_operator_attach(
                        state,
                        &session.host_id,
                        &session.hat_id,
                        &cwd,
                        &session.agent,
                        operation_not_before(&operation),
                    )
                    .await
            } else {
                state
                    .catalogs
                    .get(state, &session.host_id, &session.hat_id, &cwd, &session.agent)
                    .await
            };
            read.unwrap_or_else(|_| crate::session_tools::unknown_catalog(session.agent.clone()))
        } else {
            let not_before = operation_not_before(&candidate.as_ref().expect("nonactive attach candidate").operation);
            let catalog = if candidate.as_ref().is_some_and(|head| head.operator) {
                state
                    .catalogs
                    .get_for_operator_attach(
                        state,
                        &session.host_id,
                        &session.hat_id,
                        &cwd,
                        &session.agent,
                        not_before,
                    )
                    .await
            } else {
                state
                    .catalogs
                    .get_for_attach(
                        state,
                        &session.host_id,
                        &session.hat_id,
                        &cwd,
                        &session.agent,
                        not_before,
                    )
                    .await
            };
            match catalog.and_then(|catalog| {
                anyhow::ensure!(
                    catalog.freshness == hennery_proto::delegation_host::CatalogFreshness::Fresh
                        || candidate
                            .as_ref()
                            .is_some_and(|head| head.operator
                                && head.required == hennery_proto::frames::SessionConfig::default()),
                    "catalog_unavailable"
                );
                state.store.clear_delegation_catalog_failures(target)?;
                anyhow::ensure!(
                    candidate.as_ref().is_some_and(
                        |head| head.operator && head.required == hennery_proto::frames::SessionConfig::default()
                    ) || config_offered(
                        &catalog,
                        &candidate.as_ref().expect("nonactive attach candidate").required
                    ),
                    "configuration_unavailable"
                );
                Ok(catalog)
            }) {
                Ok(catalog) => catalog,
                Err(error) => {
                    let code = crate::session_tools::error_code(&error);
                    let now = hennery_kernel::secret::unix_now();
                    if code == "configuration_unavailable" {
                        refuse_configuration(
                            state,
                            target,
                            &candidate.as_ref().expect("attach candidate").operation,
                            now,
                        )?;
                    } else {
                        state.store.fail_delegation_catalog(target, code, now)?;
                        state.store.fail_notice_catalog(target, code, now)?;
                    }
                    return Ok(());
                }
            }
        };
        if session.lifecycle == "active" {
            let catalog = state.store.attached_delegation_catalog(target, &catalog)?;
            if state.store.operator_dispatch_ready(target)? {
                let Some(_admission) = state.store.reserve_prompt(target) else {
                    return Ok(());
                };
                state
                    .store
                    .dispatch_operator_prompt(target, &state.hub, &mut None, Some(&catalog))?;
            }
            state.store.dispatch_delegated(target, &catalog, &state.hub)?;
            state.store.dispatch_child_notice(target, &catalog, &state.hub)?;
        } else {
            let mcp = crate::api::mcp_context(state, &session.host_id, &session.agent)?;
            state.store.attach_delegated(target, mcp, &catalog, &state.hub)?;
            state.store.attach_notice_parent(target, mcp, &catalog, &state.hub)?;
        }
        Ok::<_, anyhow::Error>(())
    };
    match work.await {
        Err(error) if error.is::<crate::mcp_source::Unsupported>() => {
            let now = hennery_kernel::secret::unix_now();
            if let Some(head) = &candidate {
                state
                    .store
                    .fail_delegation_source_policy(target, &head.operation, now)?;
                state.store.fail_notice_source_policy(target, &head.operation, now)?;
            }
            state
                .store
                .fail_unavailable_operator_queue(target, crate::mcp_source::UNSUPPORTED)?;
            Ok(())
        }
        result => result,
    }
}

fn refuse_configuration(state: &AppState, target: &str, operation: &str, now: i64) -> anyhow::Result<()> {
    state.store.fail_delegation_configuration(target, operation, now)?;
    state.store.fail_notice_configuration(target, operation, now)
}

fn operation_not_before(operation: &str) -> std::time::SystemTime {
    uuid::Uuid::parse_str(operation)
        .ok()
        .and_then(|id| id.get_timestamp())
        .map(|at| {
            let (seconds, nanos) = at.to_unix();
            std::time::UNIX_EPOCH + Duration::new(seconds, nanos)
        })
        .unwrap_or_else(std::time::SystemTime::now)
}
