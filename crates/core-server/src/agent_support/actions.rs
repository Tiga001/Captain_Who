use super::*;
use mycopilot_core::office::{OfficeOperationParameters, OfficeRequestParameters};
use serde_json::Map;

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
        args: office_operation_model_args(request, &office_operation.reason),
        approval_status: office_operation.approval_status,
        reason: Some(office_operation.reason.clone()),
    }
}

/// Reconstructs the provider-neutral model call from a frozen Office request.
///
/// Prepared actions intentionally contain both a typed request and host-generated argv. Only the
/// typed request is projected back into the conversation. The model contract keeps the
/// user-visible reason separate from the strict operation request:
/// `{ "request": { "operation": "...", ... }, "reason": "..." }`.
///
/// Exposing `argv` (or the frozen request's internal `parameters` representation) would
/// reintroduce the provider-specific surface the v4 contract removes. Schema-v3 requests remain
/// loadable for historical recovery, but are represented only by a marker and never disclose
/// their legacy argv.
fn office_operation_model_args(
    request: &mycopilot_core::office::OfficeExecutionRequest,
    reason: &str,
) -> Value {
    let mut model_request = Map::new();
    model_request.insert("operation".to_string(), json!(request.operation));
    if let Some(file_path) = request.document_path.as_ref() {
        model_request.insert("filePath".to_string(), json!(file_path));
    }

    match &request.parameters {
        OfficeRequestParameters::Typed(parameters) => {
            project_office_operation_parameters(&mut model_request, parameters);
        }
        OfficeRequestParameters::Legacy(_) => {
            // Old pending actions are rejected by their outer schema version before execution.
            // Keep historical rendering explicit without replaying or exposing raw OfficeCLI argv.
            model_request.insert("legacyRequest".to_string(), Value::Bool(true));
        }
    }

    insert_optional(
        &mut model_request,
        "outputPath",
        request.output_path.as_ref(),
    );
    insert_optional(
        &mut model_request,
        "destinationPath",
        request.destination_path.as_ref(),
    );
    insert_optional(&mut model_request, "timeoutMs", request.timeout_ms.as_ref());

    let mut args = Map::new();
    args.insert("request".to_string(), Value::Object(model_request));
    args.insert("reason".to_string(), Value::String(reason.to_string()));
    Value::Object(args)
}

fn project_office_operation_parameters(
    args: &mut Map<String, Value>,
    parameters: &OfficeOperationParameters,
) {
    match parameters {
        OfficeOperationParameters::Help { verb, element } => {
            insert_optional(args, "verb", verb.as_ref());
            insert_optional(args, "element", element.as_ref());
        }
        OfficeOperationParameters::Create {
            locale,
            minimal,
            overwrite,
        } => {
            insert_optional(args, "locale", locale.as_ref());
            insert_if_true(args, "minimal", *minimal);
            insert_if_true(args, "overwriteExisting", *overwrite);
        }
        OfficeOperationParameters::View {
            mode,
            start,
            end,
            max_lines,
            issue_type,
            limit,
            columns,
            pages,
            range,
            viewport,
            grid,
            render_mode,
            page_count,
        } => {
            args.insert("mode".to_string(), json!(mode));
            insert_optional(args, "start", start.as_ref());
            insert_optional(args, "end", end.as_ref());
            insert_optional(args, "maxLines", max_lines.as_ref());
            insert_optional(args, "issueType", issue_type.as_ref());
            insert_optional(args, "limit", limit.as_ref());
            insert_if_non_empty(args, "columns", columns);
            insert_if_non_empty(args, "pages", pages);
            insert_optional(args, "range", range.as_ref());
            insert_optional(args, "viewport", viewport.as_ref());
            insert_optional(args, "grid", grid.as_ref());
            insert_optional(args, "renderMode", render_mode.as_ref());
            insert_if_true(args, "includePageCount", *page_count);
        }
        OfficeOperationParameters::Get { target, depth } => {
            insert_optional(args, "target", target.as_ref());
            insert_optional(args, "depth", depth.as_ref());
        }
        OfficeOperationParameters::Query {
            selector,
            contains,
            compact,
            fields,
        } => {
            args.insert("selector".to_string(), Value::String(selector.clone()));
            insert_optional(args, "containsText", contains.as_ref());
            insert_if_true(args, "compact", *compact);
            insert_if_non_empty(args, "fields", fields);
        }
        OfficeOperationParameters::Validate => {}
        OfficeOperationParameters::Set {
            target,
            properties,
            replacement,
            force,
        } => {
            args.insert("target".to_string(), Value::String(target.clone()));
            insert_if_non_empty(args, "properties", properties);
            insert_optional(args, "textReplacement", replacement.as_ref());
            insert_if_true(args, "overrideProtection", *force);
        }
        OfficeOperationParameters::Add {
            parent,
            element_type,
            copy_from,
            position,
            properties,
            force,
        } => {
            args.insert("parent".to_string(), Value::String(parent.clone()));
            args.insert("element".to_string(), Value::String(element_type.clone()));
            insert_optional(args, "copyFrom", copy_from.as_ref());
            insert_optional(args, "placement", position.as_ref());
            insert_if_non_empty(args, "properties", properties);
            insert_if_true(args, "overrideProtection", *force);
        }
        OfficeOperationParameters::Remove {
            target,
            shift,
            properties,
        } => {
            args.insert("target".to_string(), Value::String(target.clone()));
            insert_optional(args, "shift", shift.as_ref());
            insert_if_non_empty(args, "properties", properties);
        }
        OfficeOperationParameters::Move {
            target,
            new_parent,
            position,
            properties,
        } => {
            args.insert("target".to_string(), Value::String(target.clone()));
            insert_optional(args, "toParent", new_parent.as_ref());
            insert_optional(args, "placement", position.as_ref());
            insert_if_non_empty(args, "properties", properties);
        }
        OfficeOperationParameters::Swap {
            first_target,
            second_target,
        } => {
            args.insert(
                "firstTarget".to_string(),
                Value::String(first_target.clone()),
            );
            args.insert(
                "secondTarget".to_string(),
                Value::String(second_target.clone()),
            );
        }
    }
}

fn insert_optional<T: Serialize>(args: &mut Map<String, Value>, name: &str, value: Option<&T>) {
    if let Some(value) = value {
        args.insert(
            name.to_string(),
            serde_json::to_value(value).expect("Office request fields must be serializable"),
        );
    }
}

fn insert_if_true(args: &mut Map<String, Value>, name: &str, value: bool) {
    if value {
        args.insert(name.to_string(), Value::Bool(true));
    }
}

fn insert_if_non_empty<T>(args: &mut Map<String, Value>, name: &str, values: &T)
where
    T: IsEmpty + Serialize,
{
    if !values.is_empty() {
        args.insert(
            name.to_string(),
            serde_json::to_value(values).expect("Office request fields must be serializable"),
        );
    }
}

trait IsEmpty {
    fn is_empty(&self) -> bool;
}

impl<T> IsEmpty for Vec<T> {
    fn is_empty(&self) -> bool {
        Vec::is_empty(self)
    }
}

impl<K, V> IsEmpty for std::collections::BTreeMap<K, V> {
    fn is_empty(&self) -> bool {
        std::collections::BTreeMap::is_empty(self)
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
            "reason": command.reason.clone(),
            "observe": command.observe.clone(),
            "runtimeProfile": command.runtime_binding.as_ref().map(|binding| binding.profile),
            // Retained only when reconstructing a legacy pending action so startup recovery can
            // retire it explicitly. New actions never expose exact runtime authority to the model.
            "runtime": command.runtime_binding.is_none().then(|| command.runtime.clone()).flatten()
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

#[cfg(test)]
mod office_projection_tests {
    use super::*;
    use mycopilot_core::office::{
        OfficeCellShift, OfficeElementPosition, OfficeGridLayout, OfficeHelpVerb, OfficePageRange,
        OfficeTextReplacement, OfficeViewMode, OfficeViewRenderMode, OfficeViewport,
    };
    use std::collections::BTreeMap;

    #[test]
    fn typed_office_parameters_project_to_operation_request_fields() {
        let cases = [
            (
                OfficeOperationParameters::Help {
                    verb: Some(OfficeHelpVerb::Add),
                    element: Some("chart".to_string()),
                },
                json!({ "verb": "add", "element": "chart" }),
            ),
            (
                OfficeOperationParameters::Create {
                    locale: Some("zh-CN".to_string()),
                    minimal: true,
                    overwrite: true,
                },
                json!({
                    "locale": "zh-CN",
                    "minimal": true,
                    "overwriteExisting": true
                }),
            ),
            (
                OfficeOperationParameters::View {
                    mode: OfficeViewMode::Screenshot,
                    start: Some(1),
                    end: Some(10),
                    max_lines: Some(200),
                    issue_type: Some("overflow".to_string()),
                    limit: Some(5),
                    columns: vec!["A".to_string(), "B".to_string()],
                    pages: vec![OfficePageRange {
                        start: 1,
                        end: Some(5),
                    }],
                    range: Some("A1:D10".to_string()),
                    viewport: Some(OfficeViewport {
                        width: 1440,
                        height: 900,
                    }),
                    grid: Some(OfficeGridLayout::Columns { columns: 3 }),
                    render_mode: Some(OfficeViewRenderMode::Html),
                    page_count: true,
                },
                json!({
                    "mode": "screenshot",
                    "start": 1,
                    "end": 10,
                    "maxLines": 200,
                    "issueType": "overflow",
                    "limit": 5,
                    "columns": ["A", "B"],
                    "pages": [{ "start": 1, "end": 5 }],
                    "range": "A1:D10",
                    "viewport": { "width": 1440, "height": 900 },
                    "grid": { "mode": "columns", "columns": 3 },
                    "renderMode": "html",
                    "includePageCount": true
                }),
            ),
            (
                OfficeOperationParameters::Get {
                    target: Some("/body".to_string()),
                    depth: Some(3),
                },
                json!({ "target": "/body", "depth": 3 }),
            ),
            (
                OfficeOperationParameters::Query {
                    selector: "paragraph".to_string(),
                    contains: Some("安全".to_string()),
                    compact: true,
                    fields: vec!["text".to_string()],
                },
                json!({
                    "selector": "paragraph",
                    "containsText": "安全",
                    "compact": true,
                    "fields": ["text"]
                }),
            ),
            (OfficeOperationParameters::Validate, json!({})),
            (
                OfficeOperationParameters::Set {
                    target: "/body/p[1]".to_string(),
                    properties: BTreeMap::from([("fontSize".to_string(), json!(24))]),
                    replacement: Some(OfficeTextReplacement {
                        find: "old".to_string(),
                        replace: "new".to_string(),
                    }),
                    force: true,
                },
                json!({
                    "target": "/body/p[1]",
                    "properties": { "fontSize": 24 },
                    "textReplacement": { "find": "old", "replace": "new" },
                    "overrideProtection": true
                }),
            ),
            (
                OfficeOperationParameters::Add {
                    parent: "/slides".to_string(),
                    element_type: "slide".to_string(),
                    copy_from: Some("/slides/slide[1]".to_string()),
                    position: Some(OfficeElementPosition::After {
                        target: "/slides/slide[2]".to_string(),
                    }),
                    properties: BTreeMap::from([("title".to_string(), json!("Summary"))]),
                    force: true,
                },
                json!({
                    "parent": "/slides",
                    "element": "slide",
                    "copyFrom": "/slides/slide[1]",
                    "placement": { "type": "after", "target": "/slides/slide[2]" },
                    "properties": { "title": "Summary" },
                    "overrideProtection": true
                }),
            ),
            (
                OfficeOperationParameters::Remove {
                    target: "/Sheet1/A1".to_string(),
                    shift: Some(OfficeCellShift::Up),
                    properties: BTreeMap::from([("preserveStyle".to_string(), json!(true))]),
                },
                json!({
                    "target": "/Sheet1/A1",
                    "shift": "up",
                    "properties": { "preserveStyle": true }
                }),
            ),
            (
                OfficeOperationParameters::Move {
                    target: "/slides/slide[3]".to_string(),
                    new_parent: Some("/slides".to_string()),
                    position: Some(OfficeElementPosition::Index { index: 1 }),
                    properties: BTreeMap::new(),
                },
                json!({
                    "target": "/slides/slide[3]",
                    "toParent": "/slides",
                    "placement": { "type": "index", "index": 1 }
                }),
            ),
            (
                OfficeOperationParameters::Swap {
                    first_target: "/slides/slide[1]".to_string(),
                    second_target: "/slides/slide[2]".to_string(),
                },
                json!({
                    "firstTarget": "/slides/slide[1]",
                    "secondTarget": "/slides/slide[2]"
                }),
            ),
        ];

        for (parameters, expected) in cases {
            let mut actual = Map::new();
            project_office_operation_parameters(&mut actual, &parameters);
            assert_eq!(Value::Object(actual), expected);
        }
    }
}
