//! D2 handlers accept only the verified caller and one canonical typed request.
use crate::{
    AppState,
    delegation_runtime::{checked_cwd, config_offered},
    tool_args::{ToolCall, decode_tool},
};
use anyhow::{Result, ensure};
use hennery_gateway::identity::SessionIdentity;
use hennery_proto::{
    delegation_host::{CatalogFreshness, HostCatalog, SecurityClassification},
    delegation_status::{CapabilitiesResult, DelegationLimits},
    frames::Capability,
    rest::AnswerRequest,
};
use serde_json::Value;

pub(crate) fn unknown_catalog(agent: String) -> HostCatalog {
    HostCatalog {
        saved_at: None,
        agent,
        adapter_revision: String::new(),
        config_options: vec![],
        defaults: Default::default(),
        security: SecurityClassification {
            known: false,
            axes: Default::default(),
        },
        freshness: CatalogFreshness::Unknown,
    }
}
pub async fn execute(
    state: &AppState,
    identity: &SessionIdentity,
    revision: i64,
    name: &str,
    args: Value,
) -> Result<Value> {
    let call = decode_tool(name, &args)?;
    let digest = call.request_digest();
    let caller = state
        .store
        .session_tool_context(identity, revision, true)?
        .ok_or_else(|| anyhow::anyhow!("session_starting"))?;
    if let Some(key) = call.operation_key().filter(|_| !matches!(call, ToolCall::Status(_)))
        && let Some(receipt) = state.store.delegation_retry(identity, revision, key, &digest)?
    {
        return Ok(serde_json::to_value(receipt)?);
    }
    match call {
        ToolCall::Capabilities(args) => {
            let cwd = checked_cwd(
                state,
                caller.host_id(),
                caller.hat_id(),
                args.cwd.as_deref().unwrap_or(caller.canonical_cwd()),
            )
            .await?;
            let runtimes = state.hub.runtime_ids(caller.host_id());
            if let Some(agent) = &args.agent {
                ensure!(runtimes.contains(agent), "runtime_unavailable");
            }
            let mut catalogs = Vec::new();
            for agent in runtimes
                .iter()
                .filter(|agent| args.agent.as_ref().is_none_or(|a| a == *agent))
            {
                catalogs.push(
                    state
                        .catalogs
                        .get(state, caller.host_id(), caller.hat_id(), &cwd, agent)
                        .await
                        .unwrap_or_else(|_| unknown_catalog(agent.clone()))
                        .into(),
                );
            }
            let limits = state.store.delegation_limits()?;
            Ok(serde_json::to_value(CapabilitiesResult {
                runtimes,
                catalogs,
                cwd,
                limits: DelegationLimits {
                    children: limits.children,
                    starts_per_operator_epoch: limits.starts_per_operator_epoch,
                    automatic_wakes_per_operator_epoch: limits.automatic_wakes_per_operator_epoch,
                    queued_prompts: 32,
                    queued_bytes: 1024 * 1024,
                    message_bytes: 65536,
                },
            })?)
        }
        ToolCall::Spawn(mut args) => {
            state.store.precheck_spawn(identity, revision, None)?;
            ensure!(
                state.hub.has_capability(caller.host_id(), Capability::DelegationConfig),
                "session_tools_unavailable"
            );
            let agent = args.agent.as_deref().unwrap_or(caller.agent());
            ensure!(
                state.hub.runtime_ids(caller.host_id()).contains(&agent.to_string()),
                "runtime_unavailable"
            );
            let cwd = checked_cwd(state, caller.host_id(), caller.hat_id(), &args.cwd).await?;
            state.store.precheck_spawn(identity, revision, Some(&cwd))?;
            // Reserve admission before discovery so this fresh result can also
            // authorize the first attach without another disposable process.
            let operation = uuid::Uuid::now_v7().to_string();
            let catalog = state
                .catalogs
                .get_for_spawn(state, caller.host_id(), caller.hat_id(), &cwd, agent)
                .await?;
            let parent = state.store.find_session(caller.session_id())?;
            if let Some(parent) = parent {
                if args.config.model.is_none() && agent == caller.agent() {
                    args.config.model = parent.config.model;
                }
                if args.config.mode.is_none() {
                    let inherited = if agent == caller.agent() {
                        parent.config.mode
                    } else if matches!(
                        parent.config.mode.as_deref(),
                        Some("bypassPermissions" | "agent-full-access")
                    ) {
                        // Profiles are fixed by the authenticated connection's hello.
                        // Inheritance must not start another disposable adapter.
                        crate::delegation_runtime::cross_runtime_mode(
                            state.hub.runtime_profile(caller.host_id(), caller.agent()),
                            state.hub.runtime_profile(caller.host_id(), agent),
                            parent.config.mode.as_deref(),
                        )
                        .map(str::to_owned)
                    } else {
                        None
                    };
                    args.config.mode = inherited.filter(|mode| {
                        config_offered(
                            &catalog,
                            &hennery_proto::frames::SessionConfig {
                                mode: Some(mode.clone()),
                                ..Default::default()
                            },
                        )
                    });
                }
            }
            let required = crate::delegation_runtime::launch_configuration(&catalog, &args.config);
            ensure!(config_offered(&catalog, &required), "configuration_unavailable");
            let mcp = crate::api::mcp_context(state, caller.host_id(), agent)?;
            Ok(serde_json::to_value(state.store.queue_spawn_admitted(
                identity,
                revision,
                &digest,
                &crate::delegation_queue::DelegatedLaunch {
                    key: &args.operation_key,
                    agent,
                    cwd: &cwd,
                    brief: &args.brief,
                    name: args.name.as_deref(),
                    config: &required,
                    mcp,
                },
                &operation,
            )?)?)
        }
        ToolCall::Send(args) => Ok(serde_json::to_value(state.store.queue_followup(
            identity,
            revision,
            &args.operation_key,
            &digest,
            &args.child_id,
            &args.text,
        )?)?),
        ToolCall::Answer(args) => {
            let answer = AnswerRequest::Elicitation {
                action: args.action,
                content: args.content,
            };
            Ok(serde_json::to_value(
                state
                    .store
                    .queue_ordinary_answer_async(
                        identity,
                        revision,
                        &digest,
                        &crate::delegation_control::DelegatedAnswer {
                            key: &args.operation_key,
                            child: &args.child_id,
                            pending: &args.pending_id,
                            answer: &answer,
                        },
                        &state.hub,
                    )
                    .await?,
            )?)
        }
        ToolCall::Status(args) => Ok(serde_json::to_value(state.store.child_status(
            identity,
            revision,
            &args.child_id,
            args.operation_key.as_deref(),
        )?)?),
        ToolCall::List(args) => Ok(serde_json::to_value(state.store.child_list(
            identity,
            revision,
            args.cursor.as_deref(),
            args.limit.unwrap_or(25),
        )?)?),
        ToolCall::Close(args) => Ok(serde_json::to_value(state.store.queue_child_close(
            identity,
            revision,
            &args.operation_key,
            &digest,
            &args.child_id,
            &state.hub,
        )?)?),
    }
}
pub fn error_code(error: &anyhow::Error) -> &'static str {
    const CODES: &[&str] = &[
        crate::mcp_source::UNSUPPORTED,
        "not_found",
        "operation_conflict",
        "operator_required",
        "queue_full",
        "child_limit",
        "start_budget",
        "generation_limit",
        "session_starting",
        "runtime_unavailable",
        "agent_has_no_record",
        "configuration_unavailable",
        "session_tools_unavailable",
        "catalog_unavailable",
        "catalog_stale",
        "catalog_busy",
        "cwd_unavailable",
        "busy",
        "timeout",
        "adapter_failed",
        "auth_required",
        "host_offline",
        "invalid_cwd",
        "not_open",
        "already_answered",
        "invalid_arguments",
        "schema_changed",
        "invalid_cursor",
    ];
    let message = error.to_string();
    CODES
        .iter()
        .copied()
        .find(|code| *code == message)
        .unwrap_or("invalid_arguments")
}

/// Expose only deliberate, bounded argument guidance; never raw store errors or
/// submitted content. These remain MCP tool results so a parent can retry.
pub fn error_message(error: &anyhow::Error) -> String {
    let runtime_reason = match error_code(error) {
        "auth_required" => {
            Some("the selected runtime is not signed in on the parent’s host; sign in there and retry this child spawn")
        }
        "host_offline" => Some("the parent’s host is offline; reconnect it and retry this child spawn"),
        "runtime_unavailable" => Some("the selected runtime is unavailable on the parent’s host"),
        "agent_has_no_record" => Some("the runtime has no record of this parked session; start a new child session"),
        "adapter_failed" => Some(
            "the selected runtime’s adapter failed to provide a catalog; check the runtime on the parent’s host and retry",
        ),
        "catalog_unavailable" | "catalog_stale" => Some(
            "a fresh catalog is unavailable for the selected runtime on the parent’s host; retry when it is available",
        ),
        "configuration_unavailable" => Some(
            "the selected runtime’s fresh catalog does not offer the requested configuration; read session_capabilities and choose offered values",
        ),
        "timeout" => Some("the selected runtime’s catalog probe timed out on the parent’s host; retry"),
        "busy" => Some("the parent’s host is busy probing runtime catalogs; retry shortly"),
        "cwd_unavailable" => Some("the working directory is unavailable for the selected runtime on the parent’s host"),
        _ => None,
    };
    if let Some(reason) = runtime_reason {
        return reason.into();
    }
    if error.to_string() == "schema_changed" {
        return "the form changed during validation; read session_status and submit an answer matching the current requestedSchema".into();
    }
    if let Some(error) = error.downcast_ref::<crate::delegation_form::InvalidContent>() {
        return error.to_string();
    }
    if matches!(
        error.downcast_ref::<crate::tool_args::ToolArgsError>(),
        Some(crate::tool_args::ToolArgsError::InvalidAnswerContent)
    ) {
        return error.to_string();
    }
    "the session operation was refused".into()
}
