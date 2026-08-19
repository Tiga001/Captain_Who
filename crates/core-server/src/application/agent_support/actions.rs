use super::*;
use std::path::{Component, Path};

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
        AgentProposedAction::ToolCall { call } if call.tool == "apply_patch" => {
            ActionExecutionDecision {
                status: "failed".to_string(),
                final_pending_status: PendingActionStatus::Failed,
                patch_result: None,
                file_write_result: None,
                file_change: None,
                tool_result: AgentToolResult {
                    exact_archive_file: None,
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    result: Some(json!({
                        "status": "failed",
                        "message": "apply_patch approval action must be represented as a diff proposal before execution."
                    })),
                    error: Some("apply_patch 审批缺少 diff proposal，不能执行。".to_string()),
                },
            }
        }
        _ => ActionExecutionDecision {
            status: "failed".to_string(),
            final_pending_status: PendingActionStatus::Failed,
            patch_result: None,
            file_write_result: None,
            file_change: None,
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
            tool_result: file_write_tool_result(&record.snapshot.action_id, true, &result),
        };
    }

    ActionExecutionDecision {
        status: "rejected".to_string(),
        final_pending_status: PendingActionStatus::Rejected,
        patch_result: None,
        file_write_result: None,
        file_change: None,
        tool_result: tool_result_for_decision(call, AgentApprovalDecisionStatus::Rejected, message),
    }
}

pub(crate) fn approved_patch_execution(
    record: &PendingActionRecord,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    approved_patch_execution_for_input(&record.agent_input, &record.snapshot.action_id, diff)
}

pub(crate) fn approved_patch_execution_for_input(
    agent_input: &AgentChatInput,
    action_id: &str,
    diff: &mycopilot_core::AgentDiffProposal,
) -> ActionExecutionDecision {
    let workspace_root = workspace_root_optional(agent_input);
    let permissions = permissions_from_input(agent_input);
    let (patch_result, file_change) = match apply_unified_diff_in_workspace(
        workspace_root.as_deref(),
        diff.operation,
        &diff.file_path,
        &diff.patch,
        diff.base_revision.as_deref(),
        permissions,
    ) {
        Ok(apply_result) => (
            AgentPatchResult {
                status: AgentPatchResultStatus::Applied,
                operation: diff.operation,
                file_path: diff.file_path.clone(),
                applied_file_paths: apply_result.file_paths,
                git_diff: None,
                git_diff_error: None,
                error: None,
                message: diff.summary.clone(),
            },
            Some(apply_result.file_change),
        ),
        Err(error) => (
            AgentPatchResult {
                status: patch_status_for_error(&error),
                operation: diff.operation,
                file_path: diff.file_path.clone(),
                applied_file_paths: Vec::new(),
                git_diff: None,
                git_diff_error: None,
                error: Some(error),
                message: None,
            },
            None,
        ),
    };

    let applied = patch_result.status == AgentPatchResultStatus::Applied;
    let conflict = patch_result.status == AgentPatchResultStatus::Conflict;
    let tool_result = patch_tool_result(action_id, applied, &patch_result);
    ActionExecutionDecision {
        status: if applied {
            "applied".to_string()
        } else if conflict {
            "conflict".to_string()
        } else {
            "failed".to_string()
        },
        final_pending_status: if applied {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        },
        patch_result: Some(patch_result),
        file_write_result: None,
        file_change,
        tool_result,
    }
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

pub(crate) fn patch_status_for_error(error: &str) -> AgentPatchResultStatus {
    if error.contains("审批前已发生变化")
        || error.contains("baseRevision")
        || error.contains("git apply --check")
        || error.contains("patch does not apply")
        || error.contains("patch failed")
    {
        AgentPatchResultStatus::Conflict
    } else {
        AgentPatchResultStatus::Failed
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
