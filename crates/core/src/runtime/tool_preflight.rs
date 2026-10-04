//! Per-call policy checks, before announcing or dispatching a tool.
use super::*;

pub(super) struct ToolPreflightContext<'a> {
    pub(super) effective_tool_set: &'a EffectiveToolSet,
    pub(super) tool_registry: &'a ToolRegistry,
    pub(super) tool_context: &'a ToolExecutionContext,
    pub(super) transaction_storage: Option<&'a StorageService>,
    pub(super) run_id: &'a str,
    pub(super) trace_conversation_id: Option<&'a str>,
    pub(super) run_context: Option<&'a AgentRunContext>,
    pub(super) tool_failure_guard: &'a mut ToolFailureGuard,
    pub(super) command_permissions: AgentPermissions,
    pub(super) command_workspace_root: Option<&'a Path>,
    pub(super) command_auto_approve: bool,
    pub(super) patch_auto_approve: bool,
}

pub(super) struct PreparedToolCall {
    pub(super) call: AgentToolCall,
    pub(super) prepared_policy_action: Option<AgentProposedAction>,
    pub(super) file_change_run_grant_ref: Option<crate::file_change::FileChangeRunGrantRef>,
    pub(super) policy_preflight_failure: Option<AgentToolResult>,
    pub(super) terminate_after_repeat_guard_result: bool,
    pub(super) requires_approval: bool,
    pub(super) duplicate_in_batch: bool,
    pub(super) tool_identity: AgentToolIdentity,
    pub(super) is_mcp_tool: bool,
    pub(super) is_policy_process_tool: bool,
    pub(super) auto_execute_host_action: bool,
}

/// Reload the transaction fence for each call and preserve policy precedence.
/// Preparing an action may mint authority; its existing rejection paths invalidate it here.
pub(super) async fn prepare_tool_call(
    tool_request: crate::llm::LlmToolCall,
    batch_claim: ToolCallBatchClaim,
    context: ToolPreflightContext<'_>,
) -> AgentResult<PreparedToolCall> {
    let ToolPreflightContext {
        effective_tool_set,
        tool_registry,
        tool_context,
        transaction_storage,
        run_id,
        trace_conversation_id,
        run_context,
        tool_failure_guard,
        command_permissions,
        command_workspace_root,
        command_auto_approve,
        patch_auto_approve,
    } = context;
    let reason = extract_reason_from_args(&tool_request.args);
    // The effective definitions are both the model contract and the execution
    // allowlist. The registry may retain tools hidden by the current permission
    // mode; a hallucinated or text-fallback call must not resurrect one.
    let tool_is_exposed = effective_tool_set.contains(&tool_request.name);
    let definition_requires_approval = tool_is_exposed
        && tool_registry.requires_approval_for_call(&tool_request.name, &tool_request.args);
    let mut call = AgentToolCall {
        id: tool_request.id,
        tool: tool_request.name,
        args: tool_request.args,
        approval_status: if definition_requires_approval {
            AgentApprovalStatus::Required
        } else {
            AgentApprovalStatus::NotRequired
        },
        reason,
    };
    // Re-load at dispatch time instead of relying on the state observed before
    // the model request. An earlier Tool Call in the same provider batch may have
    // opened a transaction, and no later call may cross that newly-active fence.
    let dispatch_file_transactions = FileTransactionState::load(
        transaction_storage,
        run_id,
        trace_conversation_id,
        run_context
            .as_ref()
            .and_then(|context| context.project_id.as_deref()),
    )?;
    let file_transaction_fence_blocked = dispatch_file_transactions.blocks_user_text()
        && !dispatch_file_transactions.allows_tool_call(&call.tool, &call.args);
    let is_policy_process_tool = call.tool == "run_command" || call.tool == "skills_run_script";
    let mut prepared_policy_action = None;
    let mut file_change_run_grant_ref = None;
    let mut policy_preflight_failure = None;
    let mut terminate_after_repeat_guard_result = false;
    let mut auto_execute_policy_action = false;
    let mut requires_approval = definition_requires_approval;
    let duplicate_in_batch = matches!(
        batch_claim,
        ToolCallBatchClaim::Duplicate { .. } | ToolCallBatchClaim::FileObservationReused
    );
    let tool_identity = tool_registry
        .identity(&call.tool)
        .cloned()
        .unwrap_or_else(|| AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        });
    let is_mcp_tool = matches!(&tool_identity, AgentToolIdentity::Mcp { .. });

    if let ToolCallBatchClaim::Duplicate {
        semantic_fingerprint,
    } = &batch_claim
    {
        let message = format!(
            "Tool `{}` repeated the same semantic operation in one model response. \
                     The duplicate was not executed; use the result from the earlier call.",
            call.tool
        );
        policy_preflight_failure = Some(AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "type": "runtime_guard",
                "code": "duplicateToolCallInBatch",
                "errorCode": "agent.duplicate_tool_call_in_batch",
                "recovery": "useEarlierCallResult",
                "tool": call.tool,
                "semanticFingerprint": semantic_fingerprint,
                "executed": false,
                "message": message,
            })),
            error: Some(message),
        });
        requires_approval = false;
        call.approval_status = AgentApprovalStatus::NotRequired;
    }

    if matches!(batch_claim, ToolCallBatchClaim::FileObservationReused) {
        let message = "同一模型响应中的较早文件修改已经使用了这个 observationId；当前调用未执行。请等待较早调用返回后，再使用其结果中续约后的同一 ID。".to_string();
        policy_preflight_failure = Some(AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "type": "runtime_guard",
                "code": "fileObservationReusedInBatch",
                "errorCode": "agent.file_observation_reused_in_batch",
                "recovery": "waitForEarlierCallResult",
                "executed": false,
                "message": message,
            })),
            error: Some(message),
        });
        requires_approval = false;
        call.approval_status = AgentApprovalStatus::NotRequired;
    }

    if policy_preflight_failure.is_none() && file_transaction_fence_blocked {
        let message = "FileChange transaction 尚未结算；当前调用已在副作用前拒绝。请使用 apply_patch 继续或结算返回的 exact transactionId。".to_string();
        policy_preflight_failure = Some(AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "type": "runtime_guard",
                "code": "fileChangeTransactionUnsettled",
                "errorCode": "agent.file_change_transaction_unsettled",
                "recovery": "continueExactApplyPatchTransaction",
                "executed": false,
                "message": message,
            })),
            error: Some(message),
        });
        requires_approval = false;
        call.approval_status = AgentApprovalStatus::NotRequired;
    }

    if policy_preflight_failure.is_none() {
        if let Some(block) = tool_failure_guard.before_call(&call) {
            terminate_after_repeat_guard_result = block.terminate_after_result;
            policy_preflight_failure = Some(block.result);
            requires_approval = false;
            call.approval_status = AgentApprovalStatus::NotRequired;
        }
    }

    if policy_preflight_failure.is_none() && !tool_is_exposed {
        let unavailable_error = unavailable_tool_error(effective_tool_set, &call.tool);
        policy_preflight_failure = Some(failed_tool_call_result(&call, unavailable_error));
    }

    if policy_preflight_failure.is_none() && is_policy_process_tool && tool_is_exposed {
        match tool_registry
            .proposed_action_async(tool_context, &call)
            .await
        {
            Ok(action) => match if call.tool == "run_command" {
                prepare_command_dispatch_in_workspace(
                    &call,
                    action,
                    command_permissions,
                    &crate::workspace::WorkspaceResolver::from_context(
                        run_context.and_then(|context| context.workspace.as_ref()),
                    ),
                    command_auto_approve,
                )
            } else {
                prepare_skill_script_dispatch(
                    &call,
                    action,
                    command_permissions,
                    command_workspace_root,
                    command_auto_approve,
                )
            } {
                CommandDispatch::ExecuteAutomatically(action) => {
                    prepared_policy_action = Some(action);
                    auto_execute_policy_action = true;
                    requires_approval = false;
                    call.approval_status = AgentApprovalStatus::Approved;
                }
                CommandDispatch::RequireApproval(action) => {
                    prepared_policy_action = Some(action);
                    requires_approval = true;
                    call.approval_status = AgentApprovalStatus::Required;
                }
                CommandDispatch::Reject(result) => {
                    policy_preflight_failure = Some(result);
                    requires_approval = false;
                    call.approval_status = AgentApprovalStatus::NotRequired;
                }
            },
            Err(error) => {
                policy_preflight_failure = Some(failed_tool_call_result(&call, error));
                requires_approval = false;
                call.approval_status = AgentApprovalStatus::NotRequired;
            }
        }
    }

    let uses_file_change_policy = tool_registry
        .permission_policy(&call.tool)
        .uses_file_change_approval();
    if !is_policy_process_tool
        && policy_preflight_failure.is_none()
        && uses_file_change_policy
        && definition_requires_approval
        && file_change_approval_route(command_permissions) == FileChangeApprovalRoute::Denied
    {
        policy_preflight_failure = Some(failed_tool_call_result(
            &call,
            AgentError::structured(
                "agent.file_change_permission_denied",
                "The current permission policy does not allow file changes.",
                json!({
                    "type": "file_change_policy",
                    "code": "writePermissionDenied",
                    "recovery": "changePermissions",
                }),
            ),
        ));
        requires_approval = false;
    }
    if policy_preflight_failure.is_none()
        && call.tool == "apply_patch"
        && uses_file_change_policy
        && definition_requires_approval
        && !patch_auto_approve
    {
        match tool_registry
            .proposed_action_async(tool_context, &call)
            .await
        {
            Ok(action) => {
                match tool_context.resolve_active_file_change_run_grant(&action) {
                    Ok(grant) => file_change_run_grant_ref = grant,
                    Err(error) => {
                        let _ = tool_registry.invalidate_proposed_action(&action);
                        policy_preflight_failure = Some(failed_tool_call_result(&call, error));
                    }
                }
                if policy_preflight_failure.is_none() {
                    prepared_policy_action = Some(action);
                }
            }
            Err(error) => {
                policy_preflight_failure = Some(failed_tool_call_result(&call, error));
            }
        }
    }
    let auto_execute_patch = policy_preflight_failure.is_none()
        && uses_file_change_policy
        && (patch_auto_approve || file_change_run_grant_ref.is_some())
        && definition_requires_approval;
    let auto_execute_mcp_action = policy_preflight_failure.is_none()
        && tool_registry.auto_executes_prepared_action(&call.tool);
    // Built-in capability actions still need their typed Host preparation even
    // when the effective permission skips the human prompt. In particular,
    // activation mints a run-bound capability grant and sensitive MCP calls mint
    // a target-bound one-shot grant. Only calls whose reviewed call-level policy
    // actually requires approval enter this route, so Dynamic tools with benign
    // arguments continue through the ordinary direct execution path.
    let auto_execute_builtin_action = policy_preflight_failure.is_none()
        && auto_executes_builtin_prepared_action(
            command_permissions.builtin_execution,
            definition_requires_approval,
            &tool_identity,
        );
    let auto_execute_host_action = auto_execute_policy_action
        || auto_execute_patch
        || auto_execute_mcp_action
        || auto_execute_builtin_action;
    if !is_policy_process_tool {
        if policy_preflight_failure.is_some() {
            // A rejected or unavailable call is terminal for this attempt. Never
            // turn a policy failure back into a pending approval merely because
            // the underlying tool normally writes files.
            requires_approval = false;
            call.approval_status = AgentApprovalStatus::NotRequired;
        } else {
            requires_approval = definition_requires_approval && !auto_execute_host_action;
            call.approval_status = if auto_execute_host_action {
                AgentApprovalStatus::Approved
            } else if requires_approval {
                AgentApprovalStatus::Required
            } else {
                AgentApprovalStatus::NotRequired
            };
        }
    }
    Ok(PreparedToolCall {
        call,
        prepared_policy_action,
        file_change_run_grant_ref,
        policy_preflight_failure,
        terminate_after_repeat_guard_result,
        requires_approval,
        duplicate_in_batch,
        tool_identity,
        is_mcp_tool,
        is_policy_process_tool,
        auto_execute_host_action,
    })
}
