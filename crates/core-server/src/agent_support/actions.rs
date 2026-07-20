use super::*;

pub(crate) fn action_id_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.id.clone(),
        AgentProposedAction::Diff { diff } => diff.id.clone(),
        AgentProposedAction::FileWrite { file_write } => file_write.id.clone(),
        AgentProposedAction::Command { command } => command.id.clone(),
        AgentProposedAction::SkillMaterialization { materialization } => materialization.id.clone(),
        AgentProposedAction::SkillScript { script } => script.id.clone(),
        AgentProposedAction::OfficeOperation { office_operation } => office_operation.id.clone(),
    }
}

/// Stable backend identity for a provider-scoped action id.
pub(crate) fn pending_action_storage_id(run_id: &str, action_id: &str) -> String {
    format!("v2:{}:{run_id}:{action_id}", run_id.len())
}

pub(crate) fn action_type_for_action(action: &AgentProposedAction) -> &'static str {
    match action {
        AgentProposedAction::ToolCall { .. } => "tool_call",
        AgentProposedAction::Diff { .. } => "diff",
        AgentProposedAction::FileWrite { .. } => "file_write",
        AgentProposedAction::Command { .. } => "command",
        AgentProposedAction::SkillMaterialization { .. } => "skill_materialization",
        AgentProposedAction::SkillScript { .. } => "skill_script",
        AgentProposedAction::OfficeOperation { .. } => "office_operation",
    }
}

pub(crate) fn tool_name_for_action(action: &AgentProposedAction) -> String {
    match action {
        AgentProposedAction::ToolCall { call } => call.tool.clone(),
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
    }
}

pub(crate) fn tool_call_for_action(action: &AgentProposedAction) -> AgentToolCall {
    match action {
        AgentProposedAction::ToolCall { call } => call.clone(),
        AgentProposedAction::Diff { diff } => diff_tool_call(diff),
        AgentProposedAction::FileWrite { file_write } => file_write_tool_call(file_write),
        AgentProposedAction::Command { command } => command_tool_call(command),
        AgentProposedAction::SkillMaterialization { materialization } => {
            skill_materialization_tool_call(materialization)
        }
        AgentProposedAction::SkillScript { script } => skill_script_tool_call(script),
        AgentProposedAction::OfficeOperation { office_operation } => {
            office_operation_tool_call(office_operation)
        }
    }
}

pub(crate) fn office_tool_name(kind: mycopilot_core::office::OfficeDocumentKind) -> &'static str {
    match kind {
        mycopilot_core::office::OfficeDocumentKind::Document => "office_document",
        mycopilot_core::office::OfficeDocumentKind::Spreadsheet => "office_spreadsheet",
        mycopilot_core::office::OfficeDocumentKind::Presentation => "office_presentation",
    }
}

pub(crate) fn office_operation_tool_call(
    office_operation: &mycopilot_core::AgentOfficeOperationRequest,
) -> AgentToolCall {
    let request = &office_operation.prepared.request;
    AgentToolCall {
        id: office_operation.id.clone(),
        tool: office_tool_name(request.document_kind).to_string(),
        args: json!({
            "operation": request.operation,
            "path": request.document_path,
            "arguments": request.arguments,
            "outputPath": request.output_path,
            "destinationPath": request.destination_path,
            "timeoutMs": request.timeout_ms,
            "reason": office_operation.reason,
        }),
        approval_status: office_operation.approval_status,
        reason: office_operation.reason.clone(),
    }
}

pub(crate) fn skill_script_tool_call(script: &AgentSkillScriptRequest) -> AgentToolCall {
    AgentToolCall {
        id: script.id.clone(),
        tool: "skills_run_script".to_string(),
        args: json!({
            "scriptUri": script.script_uri,
            "interpreter": script.interpreter,
            "args": script.args,
            "requirements": script.requirements,
            "timeoutMs": script.timeout_ms,
            "reason": script.reason,
        }),
        approval_status: script.approval_status,
        reason: script.reason.clone(),
    }
}

pub(crate) fn skill_materialization_tool_call(
    materialization: &AgentSkillMaterializationRequest,
) -> AgentToolCall {
    AgentToolCall {
        id: materialization.id.clone(),
        tool: "skills_materialize_resource".to_string(),
        args: json!({
            "sourceUri": materialization.source_uri,
            "sourcePrefix": materialization.source_prefix,
            "destination": materialization.destination,
            "reason": materialization.reason,
        }),
        approval_status: materialization.approval_status,
        reason: materialization.reason.clone(),
    }
}

pub(crate) fn file_write_tool_call(file_write: &AgentFileWriteProposal) -> AgentToolCall {
    AgentToolCall {
        id: file_write.id.clone(),
        tool: "write_file".to_string(),
        args: json!({
            "phase": "finish",
            "draftId": file_write.draft_id,
            "summary": file_write.summary
        }),
        approval_status: file_write.approval_status,
        reason: file_write.summary.clone(),
    }
}

pub(crate) fn diff_tool_call(diff: &AgentDiffProposal) -> AgentToolCall {
    AgentToolCall {
        id: diff.id.clone(),
        tool: "apply_patch".to_string(),
        args: json!({
            "operation": diff.operation,
            "filePath": diff.file_path.clone(),
            "patch": diff.patch.clone(),
            "baseRevision": diff.base_revision.clone(),
            "summary": diff.summary.clone()
        }),
        approval_status: diff.approval_status,
        reason: diff.summary.clone(),
    }
}

pub(crate) fn command_tool_call(command: &AgentCommandRequest) -> AgentToolCall {
    AgentToolCall {
        id: command.id.clone(),
        tool: "run_command".to_string(),
        args: json!({
            "command": command.command.clone(),
            "cwd": command.cwd.clone(),
            "timeoutMs": command.timeout_ms,
            "riskLevel": command.risk_level,
            "reason": command.reason.clone(),
            "observe": command.observe.clone(),
            "runtime": command.runtime.clone()
        }),
        approval_status: command.approval_status,
        reason: command.reason.clone(),
    }
}

pub(crate) fn tool_result_for_decision(
    call: &AgentToolCall,
    decision_status: AgentApprovalDecisionStatus,
    message: Option<&str>,
) -> AgentToolResult {
    match decision_status {
        AgentApprovalDecisionStatus::Approved => AgentToolResult {
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

    match &record.snapshot.action {
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
                tool_result: AgentToolResult {
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
            tool_result: tool_result_for_decision(call, decision_status, message),
        },
    }
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
            tool_result: file_write_tool_result(&record.snapshot.action_id, true, &result),
        };
    }

    ActionExecutionDecision {
        status: "rejected".to_string(),
        final_pending_status: PendingActionStatus::Rejected,
        patch_result: None,
        file_write_result: None,
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
    let patch_result = match apply_unified_diff_in_workspace(
        workspace_root.as_deref(),
        diff.operation,
        &diff.file_path,
        &diff.patch,
        diff.base_revision.as_deref(),
        permissions,
    ) {
        Ok(apply_result) => AgentPatchResult {
            status: AgentPatchResultStatus::Applied,
            operation: diff.operation,
            file_path: diff.file_path.clone(),
            applied_file_paths: apply_result.file_paths,
            git_diff: None,
            git_diff_error: None,
            error: None,
            message: diff.summary.clone(),
        },
        Err(error) => AgentPatchResult {
            status: patch_status_for_error(&error),
            operation: diff.operation,
            file_path: diff.file_path.clone(),
            applied_file_paths: Vec::new(),
            git_diff: None,
            git_diff_error: None,
            error: Some(error),
            message: None,
        },
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
        return file_write_decision(proposal, result);
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
        return file_write_decision(proposal, result);
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
    file_write_decision(proposal, result)
}

fn file_write_decision(
    proposal: &AgentFileWriteProposal,
    result: AgentFileWriteResult,
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
        tool_result: file_write_tool_result(&proposal.id, applied, &result),
    }
}

pub(crate) fn file_write_tool_result(
    action_id: &str,
    ok: bool,
    result: &AgentFileWriteResult,
) -> AgentToolResult {
    AgentToolResult {
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
        error: Some(error),
        policy_evaluation,
        artifact_observation: None,
        runtime: None,
    }
}
