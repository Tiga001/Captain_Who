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
        AgentProposedAction::Diff { .. } => "apply_patch".to_string(),
        AgentProposedAction::FileWrite { .. } => "write_file".to_string(),
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
            "apply_patch".to_string(),
            diff.approval_status,
            diff.summary.clone(),
        ),
        AgentProposedAction::FileWrite { file_write } => (
            file_write.id.clone(),
            "write_file".to_string(),
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
        AgentProposedAction::FileWrite { file_write } => {
            approved_file_write_execution(storage, &record.agent_input, file_write)
        }
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
        if let Ok(Some(mut draft)) = storage.get_agent_file_draft(&file_write.draft_id) {
            draft.status = "rejected".to_string();
            draft.updated_at = now_ms();
            let _ = storage.update_agent_file_draft(&draft);
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
            tool_result: file_write_tool_result(&record.snapshot.action_id, true, &result),
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
    execution.patch_result.as_ref().is_some_and(|result| {
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

struct DirectFileChangeExecution {
    file_change: AgentTurnFileChange,
    committed_diff: mycopilot_core::AgentDiffProposal,
    finalization: Option<DirectFileChangeFinalization>,
}

fn execute_direct_file_change(
    agent_input: &AgentChatInput,
    run_id: &str,
    action_id: &str,
    diff: &mycopilot_core::AgentDiffProposal,
) -> Result<DirectFileChangeExecution, mycopilot_core::file_change::FileChangeError> {
    use mycopilot_core::file_change::{
        FileChangeCommitter, FileChangeError, FileChangeErrorCode, FileChangeOperation,
        FileChangePathPolicy, FileChangePlan,
    };

    let binding = &diff.execution;
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
    if binding.source_call_id != action_id
        || binding.run_id != run_id
        || binding.conversation_id != conversation_id
        || binding.permission_revision != permission_revision
        || patch_operation_for_file_change(binding.transaction.operation) != diff.operation
        || binding.transaction.file_path != diff.file_path
        || binding.transaction.base.revision() != diff.base_revision.as_deref()
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
    .resolve(&diff.file_path)?;
    if target.absolute_path().to_string_lossy() != binding.canonical_target {
        return Err(FileChangeError::new(
            FileChangeErrorCode::ObservationPathMismatch,
        ));
    }
    let plan = FileChangePlan::from_direct_binding(binding, diff.patch.clone())?;
    let committer = FileChangeCommitter;
    binding
        .observation
        .revalidate_current_identity(target.absolute_path())?;
    #[cfg(test)]
    if take_direct_file_change_outcome_unknown(action_id) {
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
        if take_direct_file_change_post_commit_binding_failure(action_id) {
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
    let mut committed_diff = diff.clone();
    committed_diff.execution = Box::new(committed_binding);
    let finalization = commit
        .delete_journal
        .map(|journal| DirectFileChangeFinalization {
            target,
            journal,
            finalized_at: committed_at,
        });
    Ok(DirectFileChangeExecution {
        committed_diff,
        finalization,
        file_change: AgentTurnFileChange {
            path: diff.file_path.clone(),
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
    proposal: &AgentFileWriteProposal,
) -> ActionExecutionDecision {
    let Some(mut draft) = storage
        .get_agent_file_draft(&proposal.draft_id)
        .ok()
        .flatten()
    else {
        let result = failed_file_write_result(
            proposal,
            AgentFileWriteResultStatus::Failed,
            "未找到待应用的文件草稿。",
        );
        return file_write_decision(proposal, result, None);
    };
    if draft.status != "waiting_approval" {
        let result = failed_file_write_result(
            proposal,
            AgentFileWriteResultStatus::Failed,
            format!(
                "文件草稿当前状态为 {}，不再等待此审批，不能重复执行。",
                draft.status
            ),
        );
        return file_write_decision(proposal, result, None);
    }
    draft.status = "applying".to_string();
    draft.updated_at = now_ms();
    let _ = storage.update_agent_file_draft(&draft);
    let result = apply_file_write(
        workspace_root_optional(agent_input).as_deref(),
        proposal,
        &draft,
        permissions_from_input(agent_input),
    )
    .unwrap_or_else(|error| {
        let status = if error.contains("发生变化") || error.contains("已出现") {
            AgentFileWriteResultStatus::Conflict
        } else {
            AgentFileWriteResultStatus::Failed
        };
        failed_file_write_result(proposal, status, error)
    });
    draft.status = match result.status {
        AgentFileWriteResultStatus::Applied | AgentFileWriteResultStatus::AlreadyApplied => {
            "applied"
        }
        AgentFileWriteResultStatus::Conflict => "conflict",
        AgentFileWriteResultStatus::Rejected => "rejected",
        AgentFileWriteResultStatus::Failed => "failed",
    }
    .to_string();
    draft.updated_at = now_ms();
    let _ = storage.update_agent_file_draft(&draft);
    let file_change = matches!(
        result.status,
        AgentFileWriteResultStatus::Applied | AgentFileWriteResultStatus::AlreadyApplied
    )
    .then(|| AgentTurnFileChange {
        path: draft.file_path.clone(),
        before: if draft.base_revision.is_some() {
            AgentTurnFileContent::Text(draft.base_content.clone())
        } else {
            AgentTurnFileContent::Missing
        },
        after: AgentTurnFileContent::Text(draft.content.clone()),
    });
    file_write_decision(proposal, result, file_change)
}

fn file_write_decision(
    proposal: &AgentFileWriteProposal,
    result: AgentFileWriteResult,
    file_change: Option<AgentTurnFileChange>,
) -> ActionExecutionDecision {
    let applied = matches!(
        result.status,
        AgentFileWriteResultStatus::Applied | AgentFileWriteResultStatus::AlreadyApplied
    );
    let conflict = result.status == AgentFileWriteResultStatus::Conflict;
    ActionExecutionDecision {
        status: if applied {
            "applied"
        } else if conflict {
            "conflict"
        } else {
            "failed"
        }
        .to_string(),
        final_pending_status: if applied {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        },
        patch_result: None,
        file_write_result: Some(result.clone()),
        file_change,
        committed_file_change_action: None,
        direct_file_change_finalization: None,
        tool_result: file_write_tool_result(&proposal.id, applied, &result),
    }
}

pub(crate) fn file_write_tool_result(
    action_id: &str,
    ok: bool,
    result: &AgentFileWriteResult,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: action_id.to_string(),
        tool: "write_file".to_string(),
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
