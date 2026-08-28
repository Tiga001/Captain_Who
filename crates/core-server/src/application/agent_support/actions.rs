use super::*;
use std::path::{Component, Path};
#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
static DIRECT_FILE_CHANGE_OUTCOME_UNKNOWN_CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(test)]
static DIRECT_FILE_CHANGE_POST_COMMIT_BINDING_FAILURE_CALLS: Mutex<Vec<String>> =
    Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn inject_direct_file_change_outcome_unknown(call_id: &str) {
    DIRECT_FILE_CHANGE_OUTCOME_UNKNOWN_CALLS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(call_id.to_string());
}

#[cfg(test)]
pub(crate) fn inject_direct_file_change_post_commit_binding_failure(call_id: &str) {
    DIRECT_FILE_CHANGE_POST_COMMIT_BINDING_FAILURE_CALLS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(call_id.to_string());
}

#[cfg(test)]
fn take_direct_file_change_outcome_unknown(call_id: &str) -> bool {
    let mut calls = DIRECT_FILE_CHANGE_OUTCOME_UNKNOWN_CALLS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(index) = calls.iter().position(|candidate| candidate == call_id) else {
        return false;
    };
    calls.swap_remove(index);
    true
}

#[cfg(test)]
fn take_direct_file_change_post_commit_binding_failure(call_id: &str) -> bool {
    let mut failures = DIRECT_FILE_CHANGE_POST_COMMIT_BINDING_FAILURE_CALLS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(index) = failures.iter().position(|candidate| candidate == call_id) else {
        return false;
    };
    failures.swap_remove(index);
    true
}

pub(crate) fn initialize_turn_diff_best_effort(
    storage: &StorageService,
    agent_input: &AgentChatInput,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) {
    let Some(identity) =
        turn_diff_identity(agent_input, run_id, conversation_id, assistant_message_id)
    else {
        return;
    };
    if let Err(error) = storage.initialize_agent_turn_diff(&identity) {
        eprintln!("failed to initialize agent turn diff ledger: {error}");
    }
}

pub(crate) fn record_turn_file_change_best_effort(
    storage: &StorageService,
    agent_input: &AgentChatInput,
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
    action_id: &str,
    change: Option<&AgentTurnFileChange>,
) {
    let (Some(conversation_id), Some(assistant_message_id), Some(change)) =
        (conversation_id, assistant_message_id, change)
    else {
        return;
    };
    let Some(identity) =
        turn_diff_identity(agent_input, run_id, conversation_id, assistant_message_id)
    else {
        return;
    };
    let Some(change) = workspace_relative_turn_change(&identity, change) else {
        return;
    };
    if let Err(error) = storage.record_agent_turn_file_change(&identity, action_id, &change) {
        eprintln!("failed to record agent turn file change: {error}");
    }
}

fn turn_diff_identity(
    agent_input: &AgentChatInput,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> Option<AgentTurnDiffIdentity> {
    let context = agent_input.context.as_ref()?;
    if context
        .conversation_id
        .as_deref()
        .is_some_and(|context_id| context_id != conversation_id)
    {
        return None;
    }
    let project_id = context.project_id.as_deref()?.trim();
    let workspace = context.workspace.as_ref()?;
    if workspace
        .project_id
        .as_deref()
        .is_some_and(|workspace_project_id| workspace_project_id != project_id)
    {
        return None;
    }
    let workspace_root = Path::new(workspace.root_path.as_deref()?.trim())
        .canonicalize()
        .ok()?;
    if !workspace_root.is_dir() {
        return None;
    }

    Some(AgentTurnDiffIdentity {
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        project_id: project_id.to_string(),
        workspace_root: workspace_root.to_string_lossy().into_owned(),
    })
}

fn workspace_relative_turn_change(
    identity: &AgentTurnDiffIdentity,
    change: &AgentTurnFileChange,
) -> Option<AgentTurnFileChange> {
    let path = Path::new(&change.path);
    let relative = if path.is_absolute() {
        path.strip_prefix(Path::new(&identity.workspace_root))
            .ok()?
    } else {
        path
    };
    let components = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(AgentTurnFileChange {
        path: components.join("/"),
        before: change.before.clone(),
        after: change.after.clone(),
    })
}

pub(crate) fn action_id_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.id.clone(),
        AgentProposedAction::McpToolCall { approval } => approval.identity.action_id.clone(),
        AgentProposedAction::BuiltinCapabilityActivation { approval } => approval.action_id.clone(),
        AgentProposedAction::BuiltinMcpToolApproval { approval } => {
            approval.identity.action_id.clone()
        }
        AgentProposedAction::BrowserRiskApproval { approval } => approval.action_id.clone(),
        AgentProposedAction::Diff { diff } => diff.id.clone(),
        AgentProposedAction::FileWrite { file_write } => file_write.id.clone(),
        AgentProposedAction::Command { command } => command.id.clone(),
        AgentProposedAction::SkillMaterialization { materialization } => materialization.id.clone(),
        AgentProposedAction::SkillScript { script } => script.id.clone(),
        AgentProposedAction::OfficeOperation { office_operation } => office_operation.id.clone(),
        AgentProposedAction::SkillInstallation { installation } => installation.id.clone(),
    }
}

pub(crate) use crate::pending_action_identity::pending_action_storage_id;

pub(crate) fn action_type_for_action(action: &AgentProposedAction) -> &'static str {
    match action {
        AgentProposedAction::ToolCall { .. } => "tool_call",
        AgentProposedAction::McpToolCall { .. } => "mcp_tool_call",
        AgentProposedAction::BuiltinCapabilityActivation { .. } => "builtin_capability_activation",
        AgentProposedAction::BuiltinMcpToolApproval { .. } => "builtin_mcp_tool_approval",
        AgentProposedAction::BrowserRiskApproval { .. } => "browser_risk_approval",
        AgentProposedAction::Diff { .. } => "diff",
        AgentProposedAction::FileWrite { .. } => "file_write",
        AgentProposedAction::Command { .. } => "command",
        AgentProposedAction::SkillMaterialization { .. } => "skill_materialization",
        AgentProposedAction::SkillScript { .. } => "skill_script",
        AgentProposedAction::OfficeOperation { .. } => "office_operation",
        AgentProposedAction::SkillInstallation { .. } => "skill_installation",
    }
}

pub(crate) fn tool_name_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.tool.clone(),
        AgentProposedAction::McpToolCall { approval } => {
            approval.identity.provenance.model_tool_name.clone()
        }
        AgentProposedAction::BuiltinCapabilityActivation { .. } => {
            "activate_capability".to_string()
        }
        AgentProposedAction::BuiltinMcpToolApproval { approval } => {
            approval.identity.model_name.clone()
        }
        AgentProposedAction::BrowserRiskApproval { approval } => approval.trigger_tool_name.clone(),
        AgentProposedAction::Diff { diff } => diff.execution.source_tool_name.clone(),
        AgentProposedAction::FileWrite { file_write } => {
            file_write.execution.source_tool_name.clone()
        }
        AgentProposedAction::Command { .. } => "run_command".to_string(),
        AgentProposedAction::SkillMaterialization { .. } => {
            "skills_materialize_resource".to_string()
        }
        AgentProposedAction::SkillScript { .. } => "skills_run_script".to_string(),
        AgentProposedAction::OfficeOperation { office_operation } => {
            office_tool_name(office_operation.prepared.request.document_kind).to_string()
        }
        AgentProposedAction::SkillInstallation { .. } => "skills_commit_install".to_string(),
    }
}

fn frozen_action_call_metadata(
    action: &AgentProposedAction,
) -> (
    String,
    String,
    mycopilot_core::AgentApprovalStatus,
    Option<String>,
) {
    match action {
        AgentProposedAction::ToolCall { call } => (
            call.id.clone(),
            call.tool.clone(),
            call.approval_status,
            call.reason.clone(),
        ),
        AgentProposedAction::McpToolCall { approval } => (
            approval.call.id.clone(),
            approval.call.tool.clone(),
            approval.call.approval_status,
            approval.call.reason.clone(),
        ),
        AgentProposedAction::BuiltinCapabilityActivation { approval } => (
            approval.call_id.clone(),
            "activate_capability".to_string(),
            approval.approval_status,
            Some(approval.reason.clone()),
        ),
        AgentProposedAction::BuiltinMcpToolApproval { approval } => (
            approval.identity.call_id.clone(),
            approval.identity.model_name.clone(),
            approval.approval_status,
            Some(approval.call_reason.clone()),
        ),
        AgentProposedAction::BrowserRiskApproval { approval } => (
            approval.call_id.clone(),
            approval.trigger_tool_name.clone(),
            approval.approval_status,
            Some(approval.reason.clone()),
        ),
        AgentProposedAction::Diff { diff } => (
            diff.id.clone(),
            diff.execution.source_tool_name.clone(),
            diff.approval_status,
            diff.summary.clone(),
        ),
        AgentProposedAction::FileWrite { file_write } => (
            file_write.id.clone(),
            file_write.execution.source_tool_name.clone(),
            file_write.approval_status,
            file_write.summary.clone(),
        ),
        AgentProposedAction::Command { command } => (
            command.id.clone(),
            "run_command".to_string(),
            command.approval_status,
            command.reason.clone(),
        ),
        AgentProposedAction::SkillMaterialization { materialization } => (
            materialization.id.clone(),
            "skills_materialize_resource".to_string(),
            materialization.approval_status,
            materialization.reason.clone(),
        ),
        AgentProposedAction::SkillScript { script } => (
            script.id.clone(),
            "skills_run_script".to_string(),
            script.approval_status,
            script.reason.clone(),
        ),
        AgentProposedAction::OfficeOperation { office_operation } => (
            office_operation.id.clone(),
            office_tool_name(office_operation.prepared.request.document_kind).to_string(),
            office_operation.approval_status,
            Some(office_operation.reason.clone()),
        ),
        AgentProposedAction::SkillInstallation { installation } => (
            installation.id.clone(),
            "skills_commit_install".to_string(),
            installation.approval_status,
            Some("Install the frozen inspected Skill package.".to_string()),
        ),
    }
}

pub(crate) fn office_tool_name(kind: mycopilot_core::office::OfficeDocumentKind) -> &'static str {
    match kind {
        mycopilot_core::office::OfficeDocumentKind::Document => "office_document",
        mycopilot_core::office::OfficeDocumentKind::Spreadsheet => "office_spreadsheet",
        mycopilot_core::office::OfficeDocumentKind::Presentation => "office_presentation",
    }
}

/// Restores the model-authored ToolCall from the approval checkpoint.
///
/// A prepared action may contain host-derived fields (for example a frozen managed runtime and
/// Office observation) that were never part of the model call. Continuation must close the exact
/// assistant/tool protocol pair from the checkpoint instead of synthesizing different arguments
/// from the prepared action.
pub(crate) fn tool_call_for_pending_record(
    record: &PendingActionRecord,
) -> Result<AgentToolCall, String> {
    let (action_call_id, action_tool_name, approval_status, reason) =
        frozen_action_call_metadata(&record.snapshot.action);
    let checkpoint = record
        .agent_input
        .resume_checkpoint
        .as_ref()
        .ok_or_else(|| "待审批操作缺少冻结运行检查点，无法安全恢复。".to_string())?;
    if checkpoint.context_items.is_empty() {
        return Err("待审批运行检查点缺少精确模型上下文，无法安全恢复。".to_string());
    }
    let mut matching_calls = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .filter(|call| call.id == checkpoint.pending_tool_call_id);
    let call = matching_calls.next().ok_or_else(|| {
        "待审批运行检查点缺少原始模型 ToolCall，无法安全构造续跑调用。".to_string()
    })?;
    if matching_calls.next().is_some() {
        return Err("待审批运行检查点包含重复的原始模型 ToolCall，无法安全续跑。".to_string());
    }
    if record.snapshot.tool_call_id.as_deref() != Some(call.id.as_str())
        || call.name != record.snapshot.tool_name
        || call.id != action_call_id
        || call.name != action_tool_name
    {
        return Err("待审批运行检查点中的原始 ToolCall 与冻结 action 不一致。".to_string());
    }
    if let AgentProposedAction::Command { command } = &record.snapshot.action {
        mycopilot_core::validate_frozen_agent_command_args(command, &call.args).map_err(
            |error| {
                format!("待审批运行检查点中的原始 run_command 参数与冻结 action 不一致：{error}")
            },
        )?;
    }
    if let AgentProposedAction::Diff { diff } = &record.snapshot.action {
        let digest = mycopilot_core::file_change::proposal_digest(&call.args)
            .map_err(|_| "待审批 apply_patch ToolCall 无法校验。".to_string())?;
        if digest != diff.execution.source_args_digest {
            return Err("待审批 apply_patch ToolCall 与冻结 FileChange 事务不一致。".to_string());
        }
    }
    if let AgentProposedAction::FileWrite { file_write } = &record.snapshot.action {
        let digest = mycopilot_core::file_change::proposal_digest(&call.args)
            .map_err(|_| "待审批文件事务 ToolCall 无法校验。".to_string())?;
        if digest != file_write.execution.source_args_digest {
            return Err("待审批 ToolCall 与冻结 FileChange 事务不一致。".to_string());
        }
    }
    if let AgentProposedAction::BuiltinCapabilityActivation { approval } = &record.snapshot.action {
        mycopilot_core::validate_frozen_builtin_capability_activation_args(approval, &call.args)
            .map_err(|_| {
                "待审批运行检查点中的内置能力激活参数与冻结 action 不一致。".to_string()
            })?;
    }
    Ok(AgentToolCall {
        id: call.id.clone(),
        tool: call.name.clone(),
        args: call.args.clone(),
        approval_status,
        reason,
    })
}

pub(crate) fn tool_result_for_decision(
    call: &AgentToolCall,
    decision_status: AgentApprovalDecisionStatus,
    message: Option<&str>,
) -> AgentToolResult {
    match decision_status {
        AgentApprovalDecisionStatus::Approved => AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "status": "unsupported",
                "message": "Generic approved tool calls cannot be executed. Only structured diff, file-write, and command actions are supported."
            })),
            error: Some("不支持执行通用审批工具调用；未修改文件或运行命令。".to_string()),
        },
        AgentApprovalDecisionStatus::Rejected => AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({
                "status": "rejected",
                "message": message
            })),
            error: None,
        },
    }
}

pub(crate) fn action_execution_for_decision(
    storage: &StorageService,
    record: &PendingActionRecord,
    call: &AgentToolCall,
    decision_status: AgentApprovalDecisionStatus,
    message: Option<&str>,
) -> ActionExecutionDecision {
    if decision_status == AgentApprovalDecisionStatus::Rejected {
        return rejected_action_execution(storage, record, call, message);
    }

    let execution = match &record.snapshot.action {
        AgentProposedAction::Diff { diff } => approved_patch_execution(record, diff),
        AgentProposedAction::FileWrite { file_write } => approved_file_write_execution(
            storage,
            &record.agent_input,
            &record.snapshot.run_id,
            file_write,
        ),
        _ => ActionExecutionDecision {
            status: "failed".to_string(),
            final_pending_status: PendingActionStatus::Failed,
            patch_result: None,
            file_write_result: None,
            file_change: None,
            committed_file_change_action: None,
            direct_file_change_finalization: None,
            tool_result: tool_result_for_decision(call, decision_status, message),
        },
    };
    record_turn_file_change_best_effort(
        storage,
        &record.agent_input,
        &record.snapshot.run_id,
        record.snapshot.conversation_id.as_deref(),
        record.snapshot.assistant_message_id.as_deref(),
        &record.snapshot.action_id,
        execution.file_change.as_ref(),
    );
    execution
}

pub(crate) fn rejected_action_execution(
    storage: &StorageService,
    record: &PendingActionRecord,
    call: &AgentToolCall,
    message: Option<&str>,
) -> ActionExecutionDecision {
    if let AgentProposedAction::Diff { diff } = &record.snapshot.action {
        let patch_result = AgentPatchResult {
            status: AgentPatchResultStatus::Rejected,
            operation: diff.operation,
            file_path: diff.file_path.clone(),
            applied_file_paths: Vec::new(),
            git_diff: None,
            git_diff_error: None,
            error_code: None,
            error: None,
            message: message.map(ToString::to_string),
        };
        let tool_result = patch_tool_result(&record.snapshot.action_id, true, &patch_result);
        return ActionExecutionDecision {
            status: "rejected".to_string(),
            final_pending_status: PendingActionStatus::Rejected,
            patch_result: Some(patch_result),
            file_write_result: None,
            file_change: None,
            committed_file_change_action: None,
            direct_file_change_finalization: None,
            tool_result,
        };
    }

    if let AgentProposedAction::FileWrite { file_write } = &record.snapshot.action {
        let source_tool_name = file_write.execution.source_tool_name.as_str();
        let owner = record.agent_input.context.as_ref();
        let conversation_id = owner.and_then(|context| context.conversation_id.as_deref());
        let project_id = owner.and_then(|context| context.project_id.as_deref());
        let settled = conversation_id
            .and_then(|conversation_id| {
                storage
                    .get_agent_file_change_for_owner(
                        &file_write.draft_id,
                        conversation_id,
                        project_id,
                        &record.snapshot.run_id,
                        source_tool_name,
                    )
                    .ok()
                    .flatten()
            })
            .is_some_and(|mut transaction| {
                if transaction.status != "waiting_approval"
                    || transaction.final_action_id.as_deref() != Some(file_write.id.as_str())
                    || transaction.draft_revision
                        != file_write
                            .execution
                            .staged_transaction_revision
                            .unwrap_or(u64::MAX)
                {
                    return false;
                }
                let expected_revision = transaction.draft_revision;
                let expected_index = transaction.next_mutation_index;
                transaction.status = "rejected".to_string();
                transaction.stats_final = true;
                transaction.updated_at = now_ms();
                storage
                    .transition_agent_file_change(
                        "waiting_approval",
                        expected_revision,
                        expected_index,
                        &transaction,
                    )
                    .ok()
                    == Some(true)
            });
        if !settled {
            return failed_staged_file_change_decision(
                file_write,
                source_tool_name,
                mycopilot_core::file_change::FileChangeError::new(
                    mycopilot_core::file_change::FileChangeErrorCode::OutcomeUnknown,
                ),
            );
        }
        let result = failed_file_write_result(
            file_write,
            AgentFileWriteResultStatus::Rejected,
            message.unwrap_or("用户拒绝了文件写入。"),
        );
        return ActionExecutionDecision {
            status: "rejected".to_string(),
            final_pending_status: PendingActionStatus::Rejected,
            patch_result: None,
            file_write_result: Some(result.clone()),
            file_change: None,
            committed_file_change_action: None,
            direct_file_change_finalization: None,
            tool_result: file_write_tool_result(
                &record.snapshot.action_id,
                source_tool_name,
                true,
                &result,
            ),
        };
    }

    ActionExecutionDecision {
        status: "rejected".to_string(),
        final_pending_status: PendingActionStatus::Rejected,
        patch_result: None,
        file_write_result: None,
        file_change: None,
        committed_file_change_action: None,
        direct_file_change_finalization: None,
        tool_result: tool_result_for_decision(call, AgentApprovalDecisionStatus::Rejected, message),
    }
}

pub(crate) fn approved_patch_execution(
    record: &PendingActionRecord,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    prepare_approved_patch_execution_for_input(
        &record.agent_input,
        &record.snapshot.run_id,
        &record.snapshot.action_id,
        diff,
    )
}

#[cfg(test)]
pub(crate) fn approved_patch_execution_for_input(
    agent_input: &AgentChatInput,
    run_id: &str,
    action_id: &str,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    let mut decision =
        prepare_approved_patch_execution_for_input(agent_input, run_id, action_id, diff);
    if let Some(finalization) = decision.direct_file_change_finalization.take() {
        if let Err(error) = finalization.finalize() {
            let failure = mycopilot_core::file_change::FileChangeError::with_diagnostic(
                mycopilot_core::file_change::FileChangeErrorCode::OutcomeUnknown,
                error,
            );
            decision = failed_direct_patch_decision(diff, action_id, failure);
        }
    }
    decision.committed_file_change_action = None;
    decision
}

pub(crate) fn direct_file_change_outcome_unknown(execution: &ActionExecutionDecision) -> bool {
    execution.status == "outcome_unknown"
        || execution.patch_result.as_ref().is_some_and(|result| {
            result.error_code.as_deref() == Some("agent.apply_patch.outcome_unknown")
        })
}

pub(crate) fn finalize_direct_file_change_action(
    action: &AgentProposedAction,
    finalization: DirectFileChangeFinalization,
) -> Result<AgentProposedAction, String> {
    let AgentProposedAction::Diff { diff } = action else {
        return Err("Direct FileChange delete finalization requires a Diff action".to_string());
    };
    let finalized_journal = finalization.finalize()?;
    let mut finalized_diff = diff.clone();
    *finalized_diff.execution = diff
        .execution
        .with_finalized_delete_journal(finalized_journal)
        .map_err(|error| error.to_string())?;
    Ok(AgentProposedAction::Diff {
        diff: finalized_diff,
    })
}

pub(crate) fn prepare_approved_patch_execution_for_input(
    agent_input: &AgentChatInput,
    run_id: &str,
    action_id: &str,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    let execution = execute_direct_file_change(agent_input, run_id, action_id, diff);
    let (patch_result, file_change, committed_action, finalization) = match execution {
        Ok(execution) => (
            AgentPatchResult {
                status: AgentPatchResultStatus::Applied,
                operation: diff.operation,
                file_path: diff.file_path.clone(),
                applied_file_paths: vec![diff.file_path.clone()],
                git_diff: None,
                git_diff_error: None,
                error_code: None,
                error: None,
                message: diff.summary.clone(),
            },
            Some(execution.file_change),
            Some(AgentProposedAction::Diff {
                diff: execution.committed_diff,
            }),
            execution.finalization,
        ),
        Err(error) => return failed_direct_patch_decision(diff, action_id, error),
    };

    let tool_result = patch_tool_result(action_id, true, &patch_result);
    ActionExecutionDecision {
        status: "applied".to_string(),
        final_pending_status: PendingActionStatus::Completed,
        patch_result: Some(patch_result),
        file_write_result: None,
        file_change,
        committed_file_change_action: committed_action,
        direct_file_change_finalization: finalization,
        tool_result,
    }
}

fn failed_direct_patch_decision(
    diff: &mycopilot_core::AgentDiffProposal,
    action_id: &str,
    error: mycopilot_core::file_change::FileChangeError,
) -> ActionExecutionDecision {
    let patch_result = AgentPatchResult {
        status: patch_status_for_file_change_error(&error),
        operation: diff.operation,
        file_path: diff.file_path.clone(),
        applied_file_paths: Vec::new(),
        git_diff: None,
        git_diff_error: None,
        error_code: Some(file_change_error_code(&error)),
        error: Some(error.to_string()),
        message: None,
    };
    let conflict = patch_result.status == AgentPatchResultStatus::Conflict;
    let tool_result = patch_tool_result(action_id, false, &patch_result);
    ActionExecutionDecision {
        status: if conflict {
            "conflict".to_string()
        } else {
            "failed".to_string()
        },
        final_pending_status: PendingActionStatus::Failed,
        patch_result: Some(patch_result),
        file_write_result: None,
        file_change: None,
        committed_file_change_action: None,
        direct_file_change_finalization: None,
        tool_result,
    }
}

struct FileChangeBindingExecution {
    file_change: AgentTurnFileChange,
    committed_binding: mycopilot_core::file_change::FileChangeDirectBinding,
    finalization: Option<DirectFileChangeFinalization>,
}

struct FileChangeBindingExecutionRequest<'a> {
    run_id: &'a str,
    action_id: &'a str,
    file_path: &'a str,
    operation: mycopilot_core::AgentPatchOperation,
    base_revision: Option<&'a str>,
    presentation_patch: Option<String>,
}

fn execute_direct_file_change(
    agent_input: &AgentChatInput,
    run_id: &str,
    action_id: &str,
    diff: &mycopilot_core::AgentDiffProposal,
) -> Result<DirectFileChangeExecution, mycopilot_core::file_change::FileChangeError> {
    let execution = execute_file_change_binding(
        agent_input,
        &diff.execution,
        FileChangeBindingExecutionRequest {
            run_id,
            action_id,
            file_path: &diff.file_path,
            operation: patch_operation_for_file_change(diff.execution.transaction.operation),
            base_revision: diff.base_revision.as_deref(),
            presentation_patch: Some(diff.patch.clone()),
        },
    )?;
    let mut committed_diff = diff.clone();
    committed_diff.execution = Box::new(execution.committed_binding);
    Ok(DirectFileChangeExecution {
        file_change: execution.file_change,
        committed_diff,
        finalization: execution.finalization,
    })
}

struct DirectFileChangeExecution {
    file_change: AgentTurnFileChange,
    committed_diff: mycopilot_core::AgentDiffProposal,
    finalization: Option<DirectFileChangeFinalization>,
}

fn execute_file_change_binding(
    agent_input: &AgentChatInput,
    binding: &mycopilot_core::file_change::FileChangeDirectBinding,
    request: FileChangeBindingExecutionRequest<'_>,
) -> Result<FileChangeBindingExecution, mycopilot_core::file_change::FileChangeError> {
    use mycopilot_core::file_change::{
        FileChangeBase, FileChangeCommitter, FileChangeError, FileChangeErrorCode,
        FileChangeMutation, FileChangeOperation, FileChangePathPolicy, FileChangePlan,
        FileChangePlanRequest, FileChangePlanner,
    };

    binding.validate()?;
    let context = agent_input
        .context
        .as_ref()
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::PermissionDenied))?;
    let conversation_id = context
        .conversation_id
        .as_deref()
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::PermissionDenied))?;
    let permissions = permissions_from_input(agent_input);
    let permission_revision =
        mycopilot_core::file_change::proposal_digest(&permissions).map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
        })?;
    let project_id = context.project_id.as_deref();
    let provider_wire_revision = agent_input
        .provider_configuration_revision
        .clone()
        .or_else(|| {
            agent_input
                .provider_protocol_key
                .as_ref()
                .and_then(|protocol| mycopilot_core::file_change::proposal_digest(protocol).ok())
        })
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination))?;
    if binding.source_call_id != request.action_id
        || binding.run_id != request.run_id
        || binding.conversation_id != conversation_id
        || binding.project_id.as_deref() != project_id
        || binding.permission_revision != permission_revision
        || binding.provider_wire_revision != provider_wire_revision
        || patch_operation_for_file_change(binding.transaction.operation) != request.operation
        || binding.transaction.file_path != request.file_path
        || binding.transaction.base.revision() != request.base_revision
    {
        return Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ));
    }
    if let Some(checkpoint) = agent_input.resume_checkpoint.as_ref() {
        if checkpoint.tool_set.effective_revision != binding.tool_set_revision {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
    }
    let workspace_root = workspace_root_optional(agent_input);
    let target = FileChangePathPolicy::new(
        workspace_root.as_deref(),
        permissions.write == mycopilot_core::AgentWritePermission::All,
    )
    .resolve(request.file_path)?;
    if target.absolute_path().to_string_lossy() != binding.canonical_target {
        return Err(FileChangeError::new(
            FileChangeErrorCode::ObservationPathMismatch,
        ));
    }
    let plan = if let Some(patch) = request.presentation_patch {
        FileChangePlan::from_direct_binding(binding, patch)?
    } else {
        let base = match (&binding.transaction.base, binding.base_content.as_deref()) {
            (mycopilot_core::file_change::FileChangeContentState::Missing, None) => {
                FileChangeBase::Missing
            }
            (
                mycopilot_core::file_change::FileChangeContentState::Present { revision, .. },
                Some(content),
            ) => FileChangeBase::Existing { content, revision },
            _ => return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments)),
        };
        let target = binding
            .target_content
            .clone()
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination))?;
        let plan = FileChangePlanner.plan(FileChangePlanRequest {
            operation: binding.transaction.operation,
            file_path: request.file_path,
            base,
            mutation: FileChangeMutation::Complete(target),
        })?;
        if plan.base != binding.transaction.base
            || plan.target != binding.transaction.target
            || plan.diff_digest != binding.proposal.diff_digest
            || plan.proposal_digest != binding.proposal.proposal_digest
            || plan.additions != binding.proposal.additions
            || plan.deletions != binding.proposal.deletions
        {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        plan
    };
    let committer = FileChangeCommitter;
    binding
        .observation
        .revalidate_current_identity(target.absolute_path())?;
    #[cfg(test)]
    if take_direct_file_change_outcome_unknown(request.action_id) {
        return Err(FileChangeError::new(FileChangeErrorCode::OutcomeUnknown));
    }
    let committed_at = u64::try_from(now_ms()).unwrap_or(u64::MAX);
    let commit = match plan.operation {
        FileChangeOperation::Delete => {
            let mut journal = binding.delete_journal.clone().ok_or_else(|| {
                FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination)
            })?;
            committer.commit_fresh(
                &binding.transaction.id,
                &target,
                &plan,
                committed_at,
                Some(&mut journal),
            )?
        }
        FileChangeOperation::Create | FileChangeOperation::Update => {
            committer.commit_fresh(&binding.transaction.id, &target, &plan, committed_at, None)?
        }
    };
    #[cfg(test)]
    let commit = {
        let mut commit = commit;
        if take_direct_file_change_post_commit_binding_failure(request.action_id) {
            commit
                .receipt
                .file_path
                .push_str("#injected-binding-failure");
        }
        commit
    };
    if commit.receipt.proposal_digest != binding.proposal.proposal_digest {
        return Err(FileChangeError::with_diagnostic(
            FileChangeErrorCode::OutcomeUnknown,
            "post-commit receipt proposal digest mismatch",
        ));
    }
    let committed_binding = binding.with_commit(&commit).map_err(|error| {
        FileChangeError::with_diagnostic(
            FileChangeErrorCode::OutcomeUnknown,
            format!(
                "post-commit binding validation failed: code={:?}, diagnostic={:?}",
                error.code(),
                error.diagnostic()
            ),
        )
    })?;
    let finalization = commit
        .delete_journal
        .map(|journal| DirectFileChangeFinalization {
            target,
            journal,
            finalized_at: committed_at,
        });
    Ok(FileChangeBindingExecution {
        committed_binding,
        finalization,
        file_change: AgentTurnFileChange {
            path: request.file_path.to_string(),
            before: binding
                .base_content
                .clone()
                .map(AgentTurnFileContent::Text)
                .unwrap_or(AgentTurnFileContent::Missing),
            after: binding
                .target_content
                .clone()
                .map(AgentTurnFileContent::Text)
                .unwrap_or(AgentTurnFileContent::Missing),
        },
    })
}

fn patch_operation_for_file_change(
    operation: mycopilot_core::file_change::FileChangeOperation,
) -> mycopilot_core::AgentPatchOperation {
    match operation {
        mycopilot_core::file_change::FileChangeOperation::Create => {
            mycopilot_core::AgentPatchOperation::Create
        }
        mycopilot_core::file_change::FileChangeOperation::Update => {
            mycopilot_core::AgentPatchOperation::Update
        }
        mycopilot_core::file_change::FileChangeOperation::Delete => {
            mycopilot_core::AgentPatchOperation::Delete
        }
    }
}

fn patch_status_for_file_change_error(
    error: &mycopilot_core::file_change::FileChangeError,
) -> AgentPatchResultStatus {
    use mycopilot_core::file_change::FileChangeErrorCode;
    if matches!(
        error.code(),
        FileChangeErrorCode::FileExists
            | FileChangeErrorCode::FileMissing
            | FileChangeErrorCode::RevisionConflict
            | FileChangeErrorCode::Conflict
            | FileChangeErrorCode::ObservationStale
    ) {
        AgentPatchResultStatus::Conflict
    } else {
        AgentPatchResultStatus::Failed
    }
}

fn file_change_error_code(error: &mycopilot_core::file_change::FileChangeError) -> String {
    let code = serde_json::to_value(error.code())
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "failed".to_string());
    format!("agent.apply_patch.{code}")
}

pub(crate) fn approved_file_write_execution(
    storage: &StorageService,
    agent_input: &AgentChatInput,
    run_id: &str,
    proposal: &AgentFileWriteProposal,
) -> ActionExecutionDecision {
    let binding = &proposal.execution;
    let source_tool_name = binding.source_tool_name.as_str();
    let Some(conversation_id) = agent_input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
    else {
        return failed_staged_file_change_decision(
            proposal,
            source_tool_name,
            mycopilot_core::file_change::FileChangeError::new(
                mycopilot_core::file_change::FileChangeErrorCode::PermissionDenied,
            ),
        );
    };
    let project_id = agent_input
        .context
        .as_ref()
        .and_then(|context| context.project_id.as_deref());
    let Some(transaction_id) = binding.staged_transaction_id.as_deref() else {
        return failed_staged_file_change_decision(
            proposal,
            source_tool_name,
            mycopilot_core::file_change::FileChangeError::new(
                mycopilot_core::file_change::FileChangeErrorCode::IllegalFieldCombination,
            ),
        );
    };
    let mut record = match storage.get_agent_file_change_for_owner(
        transaction_id,
        conversation_id,
        project_id,
        run_id,
        source_tool_name,
    ) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return failed_staged_file_change_decision(
                proposal,
                source_tool_name,
                mycopilot_core::file_change::FileChangeError::new(
                    mycopilot_core::file_change::FileChangeErrorCode::TransactionOwnerMismatch,
                ),
            )
        }
        Err(error) => {
            return failed_staged_file_change_decision(
                proposal,
                source_tool_name,
                mycopilot_core::file_change::FileChangeError::with_diagnostic(
                    mycopilot_core::file_change::FileChangeErrorCode::Failed,
                    error,
                ),
            )
        }
    };
    if let Err(error) = validate_staged_execution_record(&record, proposal) {
        return failed_staged_file_change_decision(proposal, source_tool_name, error);
    }
    let expected_revision = record.draft_revision;
    let expected_index = record.next_mutation_index;
    record.status = "applying".to_string();
    record.updated_at = now_ms();
    match storage.transition_agent_file_change(
        "waiting_approval",
        expected_revision,
        expected_index,
        &record,
    ) {
        Ok(true) => {}
        Ok(false) => {
            return failed_staged_file_change_decision(
                proposal,
                source_tool_name,
                mycopilot_core::file_change::FileChangeError::new(
                    mycopilot_core::file_change::FileChangeErrorCode::DraftRevisionConflict,
                ),
            )
        }
        Err(error) => {
            return failed_staged_file_change_decision(
                proposal,
                source_tool_name,
                mycopilot_core::file_change::FileChangeError::with_diagnostic(
                    mycopilot_core::file_change::FileChangeErrorCode::Failed,
                    error,
                ),
            )
        }
    }

    let operation = patch_operation_for_file_change(binding.transaction.operation);
    let execution = execute_file_change_binding(
        agent_input,
        binding,
        FileChangeBindingExecutionRequest {
            run_id,
            action_id: &proposal.id,
            file_path: &proposal.file_path,
            operation,
            base_revision: proposal.base_revision.as_deref(),
            presentation_patch: None,
        },
    );
    let execution = match execution {
        Ok(execution) => execution,
        Err(error) => {
            if error.code() != mycopilot_core::file_change::FileChangeErrorCode::OutcomeUnknown {
                let terminal = if patch_status_for_file_change_error(&error)
                    == AgentPatchResultStatus::Conflict
                {
                    "conflict"
                } else {
                    "failed"
                };
                let _ = transition_staged_terminal(storage, &mut record, terminal);
            }
            return failed_staged_file_change_decision(proposal, source_tool_name, error);
        }
    };
    record.status = "applied".to_string();
    record.updated_at = now_ms();
    match storage.transition_agent_file_change(
        "applying",
        expected_revision,
        expected_index,
        &record,
    ) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            return failed_staged_file_change_decision(
                proposal,
                source_tool_name,
                mycopilot_core::file_change::FileChangeError::new(
                    mycopilot_core::file_change::FileChangeErrorCode::OutcomeUnknown,
                ),
            )
        }
    }
    let revision = execution
        .committed_binding
        .transaction
        .target
        .revision()
        .map(str::to_string);
    let result = AgentFileWriteResult {
        status: AgentFileWriteResultStatus::Applied,
        draft_id: proposal.draft_id.clone(),
        mode: proposal.mode,
        file_path: proposal.file_path.clone(),
        additions: proposal.additions,
        deletions: proposal.deletions,
        line_count: proposal.line_count,
        byte_count: proposal.byte_count,
        revision,
        error: None,
        message: Some("文件变更已原子应用。".to_string()),
    };
    let mut committed_proposal = proposal.clone();
    committed_proposal.execution = Box::new(execution.committed_binding);
    file_write_decision(
        proposal,
        source_tool_name,
        result,
        Some(execution.file_change),
        Some(AgentProposedAction::FileWrite {
            file_write: committed_proposal,
        }),
    )
}

fn validate_staged_execution_record(
    record: &mycopilot_core::storage::models::AgentFileChangeRecord,
    proposal: &AgentFileWriteProposal,
) -> Result<(), mycopilot_core::file_change::FileChangeError> {
    use mycopilot_core::file_change::{FileChangeError, FileChangeErrorCode};
    let binding = &proposal.execution;
    binding.validate()?;
    let expected_revision = binding
        .staged_transaction_revision
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination))?;
    let operation = match binding.transaction.operation {
        mycopilot_core::file_change::FileChangeOperation::Create => "create",
        mycopilot_core::file_change::FileChangeOperation::Update => "update",
        mycopilot_core::file_change::FileChangeOperation::Delete => {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ))
        }
    };
    let mode_matches = matches!(
        (
            proposal.mode,
            record.operation.as_str(),
            record.strategy.as_deref()
        ),
        (AgentFileWriteMode::Create, "create", None)
            | (AgentFileWriteMode::Modify, "update", Some("modify"))
            | (AgentFileWriteMode::Rewrite, "update", Some("rewrite"))
    );
    let observation_matches = serde_json::from_str::<
        mycopilot_core::file_change::FileObservationCheckpoint,
    >(&record.observation_json)
    .is_ok_and(|checkpoint| checkpoint == binding.observation);
    if record.schema_version
        != mycopilot_core::storage::file_change_repository::AGENT_FILE_CHANGE_SCHEMA_VERSION
        || record.id != proposal.draft_id
        || binding.staged_transaction_id.as_deref() != Some(record.id.as_str())
        || record.status != "waiting_approval"
        || record.final_action_id.as_deref() != Some(proposal.id.as_str())
        || record.draft_revision != expected_revision
        || record.operation != operation
        || !mode_matches
        || record.file_path != proposal.file_path
        || record.file_path != binding.transaction.file_path
        || record.base_revision.as_deref() != proposal.base_revision.as_deref()
        || record.base_content != binding.base_content.as_deref().unwrap_or_default()
        || binding.target_content.as_deref() != Some(record.content.as_str())
        || record.permission_revision != binding.permission_revision
        || record.tool_set_revision != binding.tool_set_revision
        || record.provider_wire_revision != binding.provider_wire_revision
        || record.observation_id != binding.observation_id
        || !observation_matches
    {
        return Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ));
    }
    Ok(())
}

fn transition_staged_terminal(
    storage: &StorageService,
    record: &mut mycopilot_core::storage::models::AgentFileChangeRecord,
    status: &str,
) -> Result<(), String> {
    record.status = status.to_string();
    record.stats_final = true;
    record.updated_at = now_ms();
    if storage.transition_agent_file_change(
        "applying",
        record.draft_revision,
        record.next_mutation_index,
        record,
    )? {
        Ok(())
    } else {
        Err("文件变更终态发生并发冲突。".to_string())
    }
}

fn failed_staged_file_change_decision(
    proposal: &AgentFileWriteProposal,
    source_tool_name: &str,
    error: mycopilot_core::file_change::FileChangeError,
) -> ActionExecutionDecision {
    let outcome_unknown =
        error.code() == mycopilot_core::file_change::FileChangeErrorCode::OutcomeUnknown;
    let conflict = patch_status_for_file_change_error(&error) == AgentPatchResultStatus::Conflict;
    let result = failed_file_write_result(
        proposal,
        if conflict {
            AgentFileWriteResultStatus::Conflict
        } else {
            AgentFileWriteResultStatus::Failed
        },
        error.to_string(),
    );
    file_write_decision_with_status(
        proposal,
        source_tool_name,
        result,
        None,
        None,
        if outcome_unknown {
            "outcome_unknown"
        } else if conflict {
            "conflict"
        } else {
            "failed"
        },
    )
}

fn file_write_decision(
    proposal: &AgentFileWriteProposal,
    source_tool_name: &str,
    result: AgentFileWriteResult,
    file_change: Option<AgentTurnFileChange>,
    committed_action: Option<AgentProposedAction>,
) -> ActionExecutionDecision {
    let applied = matches!(
        result.status,
        AgentFileWriteResultStatus::Applied | AgentFileWriteResultStatus::AlreadyApplied
    );
    let conflict = result.status == AgentFileWriteResultStatus::Conflict;
    file_write_decision_with_status(
        proposal,
        source_tool_name,
        result,
        file_change,
        committed_action,
        if applied {
            "applied"
        } else if conflict {
            "conflict"
        } else {
            "failed"
        },
    )
}

fn file_write_decision_with_status(
    proposal: &AgentFileWriteProposal,
    source_tool_name: &str,
    result: AgentFileWriteResult,
    file_change: Option<AgentTurnFileChange>,
    committed_action: Option<AgentProposedAction>,
    status: &str,
) -> ActionExecutionDecision {
    let applied = matches!(
        result.status,
        AgentFileWriteResultStatus::Applied | AgentFileWriteResultStatus::AlreadyApplied
    );
    ActionExecutionDecision {
        status: status.to_string(),
        final_pending_status: if applied {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        },
        patch_result: None,
        file_write_result: Some(result.clone()),
        file_change,
        committed_file_change_action: committed_action,
        direct_file_change_finalization: None,
        tool_result: file_write_tool_result(&proposal.id, source_tool_name, applied, &result),
    }
}

pub(crate) fn file_write_tool_result(
    action_id: &str,
    tool_name: &str,
    ok: bool,
    result: &AgentFileWriteResult,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: action_id.to_string(),
        tool: tool_name.to_string(),
        ok,
        result: Some(json!(result)),
        error: if ok { None } else { result.error.clone() },
    }
}

pub(crate) fn patch_tool_result(
    action_id: &str,
    observation_ok: bool,
    patch_result: &AgentPatchResult,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: action_id.to_string(),
        tool: "apply_patch".to_string(),
        ok: observation_ok,
        result: Some(json!(patch_result)),
        error: if observation_ok {
            None
        } else {
            patch_result
                .error
                .clone()
                .or_else(|| Some("应用 patch 失败。".to_string()))
        },
    }
}

pub(crate) fn failed_command_result(
    request: &AgentCommandRequest,
    error: String,
    policy_evaluation: Option<CommandPolicyEvaluation>,
) -> AgentCommandExecutionResult {
    AgentCommandExecutionResult {
        outputs: Vec::new(),
        command: request.command.clone(),
        cwd: request.cwd.clone().unwrap_or_else(|| ".".to_string()),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration_ms: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: Default::default(),
        stdout_spool: Default::default(),
        stderr_spool: Default::default(),
        error: Some(error),
        policy_evaluation,
        artifact_observation: None,
        input_files: request
            .inputs
            .iter()
            .map(|binding| mycopilot_core::AgentFileInputEvidence {
                mount_path: binding.mount_path.clone(),
                source_kind: match &binding.source {
                    mycopilot_core::AgentFileInputRef::Attachment { .. } => {
                        mycopilot_core::AgentFileInputSourceKind::Attachment
                    }
                    mycopilot_core::AgentFileInputRef::Workspace { .. } => {
                        mycopilot_core::AgentFileInputSourceKind::Workspace
                    }
                    mycopilot_core::AgentFileInputRef::External { .. } => {
                        mycopilot_core::AgentFileInputSourceKind::External
                    }
                    mycopilot_core::AgentFileInputRef::GeneratedArtifact { .. } => {
                        mycopilot_core::AgentFileInputSourceKind::GeneratedArtifact
                    }
                    mycopilot_core::AgentFileInputRef::SkillResource { .. } => {
                        mycopilot_core::AgentFileInputSourceKind::SkillResource
                    }
                    mycopilot_core::AgentFileInputRef::BrowserDownload { .. } => {
                        mycopilot_core::AgentFileInputSourceKind::BrowserDownload
                    }
                },
                size_bytes: binding.size_bytes,
                sha256: binding.sha256.clone(),
            })
            .collect(),
        runtime: None,
        managed_outputs: None,
        authoritative_archive_ref: None,
        history_open: None,
    }
}
