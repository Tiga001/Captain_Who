use super::*;

pub(super) fn authorize_structured_file_write(
    agent_input: &AgentChatInput,
    action: &AgentProposedAction,
    source: FileWriteAuthorizationSource,
) -> AgentResult<()> {
    if !proposed_action_uses_file_write_policy(action) {
        return Ok(());
    }

    if source == FileWriteAuthorizationSource::Automatic
        && file_write_action_approval_status(action) != Some(AgentApprovalStatus::Approved)
    {
        return Err(AgentError::structured(
            "agent.file_write_authorization_denied",
            "The file change does not carry an approved action snapshot.",
            serde_json::json!({
                "type": "file_write_policy",
                "code": "actionNotApproved",
                "recovery": "retry",
            }),
        ));
    }

    let permissions = permissions_from_input(agent_input);
    if file_write_authorized(permissions, source) {
        return Ok(());
    }

    let (code, recovery, message) = match file_write_approval_route(permissions) {
        FileWriteApprovalRoute::Denied => (
            "writePermissionDenied",
            "changePermissions",
            "The current permission policy does not allow file changes.",
        ),
        FileWriteApprovalRoute::RequireExplicitApproval => (
            "explicitApprovalRequired",
            "requestApproval",
            "This file change requires explicit user approval.",
        ),
        FileWriteApprovalRoute::AutoApprove => (
            "authorizationSourceInvalid",
            "retry",
            "The file change was routed through an invalid authorization source.",
        ),
    };
    Err(AgentError::structured(
        "agent.file_write_authorization_denied",
        message,
        serde_json::json!({
            "type": "file_write_policy",
            "code": code,
            "recovery": recovery,
        }),
    ))
}

#[derive(Clone)]
pub(super) struct AutoApprovedActionContext {
    agent_input: AgentChatInput,
    run_id: String,
    conversation_id: Option<String>,
    assistant_message_id: Option<String>,
    skill_resources: Option<Arc<SkillResourceSession>>,
}

impl AutoApprovedActionContext {
    pub(super) fn new(
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        skill_resources: Option<Arc<SkillResourceSession>>,
    ) -> Self {
        Self {
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            skill_resources,
        }
    }
}

fn proposed_action_failure_result(
    action: &AgentProposedAction,
    error: &AgentError,
) -> AgentToolResult {
    AgentToolResult {
        call_id: action_id_for_action(action),
        tool: tool_name_for_action(action),
        ok: false,
        result: error.details().cloned().or_else(|| {
            Some(serde_json::json!({
                "type": "host_action",
                "code": error.code().unwrap_or("executionRejected"),
                "recovery": "retry",
            }))
        }),
        error: Some(error.to_string()),
    }
}

fn agent_file_input_execution_context(
    input: &AgentChatInput,
    skill_resources: Option<Arc<SkillResourceSession>>,
    storage: Arc<StorageService>,
) -> AgentFileInputExecutionContext {
    AgentFileInputExecutionContext::new(
        input
            .context
            .as_ref()
            .and_then(|context| context.attachment_library.clone()),
        skill_resources,
    )
    .with_storage(Some(storage))
}

fn bounded_audit_error(error: &str) -> String {
    const MAX_AUDIT_ERROR_CHARS: usize = 2_048;
    error.chars().take(MAX_AUDIT_ERROR_CHARS).collect()
}

fn office_audit_persistence_failure(
    office_operation: &mycopilot_core::AgentOfficeOperationRequest,
    phase: &str,
    audit_error: &str,
    execution_result: Option<&AgentToolResult>,
) -> AgentToolResult {
    let tool = office_tool_name(office_operation.prepared.request.document_kind);
    let execution_attempted = execution_result.is_some();
    let commit_may_have_succeeded = execution_result.is_some_and(|result| {
        result.ok
            || result
                .result
                .as_ref()
                .and_then(|value| value.get("errorCode"))
                .and_then(Value::as_str)
                == Some("office.commit_indeterminate")
    });
    let message = if execution_attempted {
        "The Office operation finished, but its final action audit could not be persisted. Inspect the target state before retrying."
    } else {
        "The Office operation was not started because its executing action audit could not be persisted."
    };
    AgentToolResult {
        call_id: office_operation.id.clone(),
        tool: tool.to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "office_operation",
            "code": "auditPersistenceFailed",
            "recovery": if execution_attempted { "inspectState" } else { "retry" },
            "phase": phase,
            "executionAttempted": execution_attempted,
            "commitMayHaveSucceeded": commit_may_have_succeeded,
            "auditError": bounded_audit_error(audit_error),
            // The provider result is already output-bounded by the Office engine. Preserve it
            // verbatim so a post-execution audit failure never hides exit/stdout/stderr,
            // timeout, cancellation, truncation, or provider revision evidence.
            "execution": execution_result.map(|result| serde_json::json!({
                "ok": result.ok,
                "result": result.result,
                "error": result.error,
            })),
        })),
        error: Some(message.to_string()),
    }
}

fn office_skill_resource_restore_failure(
    office_operation: &mycopilot_core::AgentOfficeOperationRequest,
    tool: &str,
    error: &str,
) -> AgentToolResult {
    AgentToolResult {
        call_id: office_operation.id.clone(),
        tool: tool.to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "office_operation_policy",
            "code": "skillResourceSnapshotUnavailable",
            "recovery": "retry",
        })),
        error: Some(format!(
            "The approved Office input resources could not be restored: {error}"
        )),
    }
}

fn command_audit_persistence_failure(
    command: &mycopilot_core::AgentCommandRequest,
    phase: &str,
    audit_error: &str,
    execution_result: Option<&AgentCommandExecutionResult>,
) -> AgentToolResult {
    let execution_attempted = execution_result.is_some();
    let message = if execution_attempted {
        "The command finished, but its final action audit could not be persisted. Inspect the observed artifacts before retrying."
    } else {
        "The command was not started because its executing action audit could not be persisted."
    };
    AgentToolResult {
        call_id: command.id.clone(),
        tool: "run_command".to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "command_execution",
            "code": "auditPersistenceFailed",
            "recovery": if execution_attempted { "inspectArtifacts" } else { "retry" },
            "phase": phase,
            "executionAttempted": execution_attempted,
            // An approved command can have changed files even when its process or audit failed.
            // Keep the complete bounded command result, including artifactObservation, so the
            // model-facing continuation never mistakes an audit failure for a clean rollback.
            "effectsMayHaveOccurred": execution_attempted,
            "auditError": bounded_audit_error(audit_error),
            "execution": execution_result,
        })),
        error: Some(message.to_string()),
    }
}

fn command_audit_finalization_indeterminate(
    command: &mycopilot_core::AgentCommandRequest,
    audit_error: &str,
    reconciliation_error: &str,
    execution_result: &AgentCommandExecutionResult,
) -> AgentToolResult {
    AgentToolResult {
        call_id: command.id.clone(),
        tool: "run_command".to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "command_execution",
            "code": "finalizationIndeterminate",
            "recovery": "inspectArtifacts",
            "phase": "afterExecution",
            "executionAttempted": true,
            "effectsMayHaveOccurred": true,
            "auditError": bounded_audit_error(audit_error),
            "reconciliationError": bounded_audit_error(reconciliation_error),
            // The command already ran. Preserve every bounded process and artifact field even
            // when durable state cannot prove whether the terminal receipt committed.
            "execution": execution_result,
        })),
        error: Some(
            "The command finished, but the terminal audit commit could not be reconciled. Inspect the observed artifacts and durable action state before retrying."
                .to_string(),
        ),
    }
}

struct ManualCommandSettlementError<'a> {
    code: &'a str,
    message: String,
    attempt_error: &'a str,
    inspection_error: Option<&'a str>,
    command_result: &'a AgentCommandExecutionResult,
    tool_result: &'a AgentToolResult,
}

fn emit_manual_command_settlement_error(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    error: ManualCommandSettlementError<'_>,
) {
    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
        run_id: Some(run_id.to_string()),
        message: error.message,
        recoverable: true,
        code: Some(error.code.to_string()),
        details: Some(serde_json::json!({
            "type": "command_execution",
            "code": if error.code == "approval_result_commit_indeterminate" {
                "finalizationIndeterminate"
            } else {
                "auditPersistenceFailed"
            },
            "recovery": "inspectArtifacts",
            "effectsMayHaveOccurred": true,
            "attemptError": bounded_audit_error(error.attempt_error),
            "inspectionError": error.inspection_error.map(bounded_audit_error),
            "execution": error.command_result,
            "toolResult": error.tool_result,
        })),
    }));
}

pub(super) enum ManualFileEffectSettlement {
    Committed {
        agent_input: Box<AgentChatInput>,
        tool_result: AgentToolResult,
        pending_status: PendingActionStatus,
    },
    CommittedAndAdvanced,
    Unsettled,
}

struct ManualFileEffectSettlementError<'a> {
    effect_type: &'a str,
    code: &'a str,
    message: String,
    attempt_error: &'a str,
    inspection_error: Option<&'a str>,
    execution_result: &'a AgentToolResult,
}

fn emit_manual_file_effect_settlement_error(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    error: ManualFileEffectSettlementError<'_>,
) {
    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
        run_id: Some(run_id.to_string()),
        message: error.message,
        recoverable: true,
        code: Some(error.code.to_string()),
        details: Some(serde_json::json!({
            "type": error.effect_type,
            "code": if error.code == "approval_result_commit_indeterminate" {
                "finalizationIndeterminate"
            } else {
                "auditPersistenceFailed"
            },
            "recovery": "inspectState",
            "effectsMayHaveOccurred": true,
            "attemptError": bounded_audit_error(error.attempt_error),
            "inspectionError": error.inspection_error.map(bounded_audit_error),
            "execution": error.execution_result,
        })),
    }));
}

enum CommandAuditReconciliation {
    Executing,
    Terminal(AgentToolResult),
}

fn office_execution_claim_error(existing_status: &str, identity_conflict: bool) -> AgentError {
    let prior_execution_may_have_started = matches!(
        existing_status,
        "executing" | "completed" | "failed" | "cancelled"
    );
    let (code, message) = if identity_conflict {
        (
            "actionIdentityConflict",
            "An Office action with the same identity already exists but its frozen contents differ. The operation was not replayed; inspect the target and start a newly prepared action.",
        )
    } else {
        (
            "executionAlreadyClaimed",
            "This Office action has already been claimed for execution. It was not replayed; inspect the authoritative action and target state before continuing.",
        )
    };
    AgentError::structured(
        "agent.office_execution_claim_rejected",
        message,
        serde_json::json!({
            "type": "office_operation",
            "code": code,
            "recovery": "inspectState",
            "phase": "beforeExecution",
            "existingStatus": existing_status,
            "executionAttempted": false,
            "priorExecutionMayHaveStarted": prior_execution_may_have_started,
            "commitMayHaveSucceeded": prior_execution_may_have_started,
        }),
    )
}

fn replay_persisted_office_tool_result(
    office_operation: &mycopilot_core::AgentOfficeOperationRequest,
    existing_status: &str,
    tool_result_json: Option<&str>,
) -> AgentResult<AgentToolResult> {
    let expected_tool = office_tool_name(office_operation.prepared.request.document_kind);
    if let Some(json) = tool_result_json {
        if let Ok(result) = serde_json::from_str::<AgentToolResult>(json) {
            if result.call_id == office_operation.id && result.tool == expected_tool {
                return Ok(result);
            }
        }
        return Err(AgentError::structured(
            "agent.office_persisted_result_invalid",
            "The claimed Office action has an invalid persisted result. It was not replayed; inspect durable state before continuing.",
            serde_json::json!({
                "type": "office_operation",
                "code": "persistedResultInvalid",
                "recovery": "inspectState",
                "existingStatus": existing_status,
                "executionAttempted": false,
                "commitMayHaveSucceeded": true,
            }),
        ));
    }
    Err(office_execution_claim_error(existing_status, false))
}

fn resolve_office_claim_outcome(
    office_operation: &mycopilot_core::AgentOfficeOperationRequest,
    outcome: AgentActionAuditExecutionClaimOutcome,
) -> AgentResult<Option<AgentToolResult>> {
    match outcome {
        AgentActionAuditExecutionClaimOutcome::Claimed => Ok(None),
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json,
        } => replay_persisted_office_tool_result(
            office_operation,
            &status,
            tool_result_json.as_deref(),
        )
        .map(Some),
        AgentActionAuditExecutionClaimOutcome::IdentityConflict { status } => {
            Err(office_execution_claim_error(&status, true))
        }
    }
}

fn command_execution_claim_error(existing_status: &str, identity_conflict: bool) -> AgentError {
    let prior_execution_may_have_started = matches!(
        existing_status,
        "executing" | "completed" | "failed" | "cancelled"
    );
    let (code, message) = if identity_conflict {
        (
            "actionIdentityConflict",
            "A command action with the same identity already exists but its frozen contents differ. The command was not replayed; inspect its artifacts and start a new action.",
        )
    } else {
        (
            "executionAlreadyClaimed",
            "This command has already been claimed for execution. It was not replayed; inspect the authoritative result and observed artifacts before continuing.",
        )
    };
    AgentError::structured(
        "agent.command_execution_claim_rejected",
        message,
        serde_json::json!({
            "type": "command_execution",
            "code": code,
            "recovery": "inspectArtifacts",
            "phase": "beforeExecution",
            "existingStatus": existing_status,
            "executionAttempted": false,
            "priorExecutionMayHaveStarted": prior_execution_may_have_started,
            "effectsMayHaveOccurred": prior_execution_may_have_started,
        }),
    )
}

fn resolve_command_claim_outcome(
    command: &mycopilot_core::AgentCommandRequest,
    outcome: AgentActionAuditExecutionClaimOutcome,
) -> AgentResult<Option<AgentToolResult>> {
    match outcome {
        AgentActionAuditExecutionClaimOutcome::Claimed => Ok(None),
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json,
        } => {
            if let Some(json) = tool_result_json {
                if let Ok(result) = serde_json::from_str::<AgentToolResult>(&json) {
                    if result.call_id == command.id && result.tool == "run_command" {
                        return Ok(Some(result));
                    }
                }
                return Err(AgentError::structured(
                    "agent.command_persisted_result_invalid",
                    "The claimed command has an invalid persisted result. It was not replayed; inspect durable state before continuing.",
                    serde_json::json!({
                        "type": "command_execution",
                        "code": "persistedResultInvalid",
                        "recovery": "inspectArtifacts",
                        "existingStatus": status,
                        "executionAttempted": false,
                        "effectsMayHaveOccurred": true,
                    }),
                ));
            }
            Err(command_execution_claim_error(&status, false))
        }
        AgentActionAuditExecutionClaimOutcome::IdentityConflict { status } => {
            Err(command_execution_claim_error(&status, true))
        }
    }
}

fn resolve_file_effect_claim_outcome(
    call_id: &str,
    tool: &str,
    effect_type: &str,
    outcome: AgentActionAuditExecutionClaimOutcome,
) -> AgentResult<Option<AgentToolResult>> {
    match outcome {
        AgentActionAuditExecutionClaimOutcome::Claimed => Ok(None),
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json,
        } => {
            if let Some(json) = tool_result_json {
                if let Ok(result) = serde_json::from_str::<AgentToolResult>(&json) {
                    if result.call_id == call_id && result.tool == tool {
                        return Ok(Some(result));
                    }
                }
                return Err(AgentError::structured(
                    "agent.file_effect_persisted_result_invalid",
                    "The claimed file-producing action has an invalid persisted result and was not replayed.",
                    serde_json::json!({
                        "type": effect_type,
                        "code": "persistedResultInvalid",
                        "recovery": "inspectState",
                        "existingStatus": status,
                        "effectsMayHaveOccurred": true,
                    }),
                ));
            }
            Err(AgentError::structured(
                "agent.file_effect_execution_claim_rejected",
                "This file-producing action has already been claimed and was not replayed.",
                serde_json::json!({
                    "type": effect_type,
                    "code": "executionAlreadyClaimed",
                    "recovery": "inspectState",
                    "existingStatus": status,
                    "effectsMayHaveOccurred": matches!(status.as_str(), "executing" | "completed" | "failed" | "cancelled"),
                }),
            ))
        }
        AgentActionAuditExecutionClaimOutcome::IdentityConflict { status } => {
            Err(AgentError::structured(
                "agent.file_effect_execution_claim_rejected",
                "A file-producing action with this identity already has different frozen contents.",
                serde_json::json!({
                    "type": effect_type,
                    "code": "actionIdentityConflict",
                    "recovery": "inspectState",
                    "existingStatus": status,
                    "effectsMayHaveOccurred": matches!(status.as_str(), "executing" | "completed" | "failed" | "cancelled"),
                }),
            ))
        }
    }
}

fn file_effect_audit_persistence_failure(
    call_id: &str,
    tool: &str,
    effect_type: &str,
    phase: &str,
    audit_error: &str,
    execution_result: Option<&AgentToolResult>,
) -> AgentToolResult {
    let execution_attempted = execution_result.is_some();
    AgentToolResult {
        call_id: call_id.to_string(),
        tool: tool.to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": effect_type,
            "code": "auditPersistenceFailed",
            "recovery": if execution_attempted { "inspectState" } else { "retry" },
            "phase": phase,
            "executionAttempted": execution_attempted,
            "effectsMayHaveOccurred": execution_attempted,
            "auditError": bounded_audit_error(audit_error),
            "execution": execution_result,
        })),
        error: Some(if execution_attempted {
            "The file-producing action finished, but its terminal audit could not be persisted. Inspect durable state before retrying."
        } else {
            "The file-producing action was not started because its execution claim could not be persisted."
        }.to_string()),
    }
}

enum FileEffectAuditReconciliation {
    Executing,
    Terminal(AgentToolResult),
}

fn reconcile_file_effect_audit_outcome(
    call_id: &str,
    tool: &str,
    outcome: Option<AgentActionAuditExecutionClaimOutcome>,
) -> Result<FileEffectAuditReconciliation, String> {
    let Some(outcome) = outcome else {
        return Err("the durable execution claim is missing".to_string());
    };
    match outcome {
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json,
        } if status == "executing" && tool_result_json.is_none() => {
            Ok(FileEffectAuditReconciliation::Executing)
        }
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json,
        } if matches!(
            status.as_str(),
            "completed" | "failed" | "cancelled" | "rejected"
        ) =>
        {
            let json = tool_result_json.ok_or_else(|| {
                format!("terminal audit status `{status}` has no paired ToolResult")
            })?;
            let result = serde_json::from_str::<AgentToolResult>(&json)
                .map_err(|error| format!("terminal audit ToolResult is invalid: {error}"))?;
            if result.call_id != call_id || result.tool != tool {
                return Err(format!(
                    "terminal audit ToolResult identity differs: expected {tool}/{call_id}, found {}/{}",
                    result.tool, result.call_id
                ));
            }
            Ok(FileEffectAuditReconciliation::Terminal(result))
        }
        AgentActionAuditExecutionClaimOutcome::AlreadyClaimed { status, .. } => Err(format!(
            "the durable execution claim has unsupported status `{status}`"
        )),
        AgentActionAuditExecutionClaimOutcome::IdentityConflict { status } => Err(format!(
            "the durable execution claim identity differs (status `{status}`)"
        )),
        AgentActionAuditExecutionClaimOutcome::Claimed => {
            Err("inspection unexpectedly reported a newly claimed action".to_string())
        }
    }
}

fn reconcile_command_audit_outcome(
    command: &mycopilot_core::AgentCommandRequest,
    outcome: Option<AgentActionAuditExecutionClaimOutcome>,
) -> Result<CommandAuditReconciliation, String> {
    match outcome {
        Some(AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json: None,
        }) if status == "executing" => Ok(CommandAuditReconciliation::Executing),
        Some(AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json: Some(json),
        }) if matches!(status.as_str(), "completed" | "failed") => {
            let result = serde_json::from_str::<AgentToolResult>(&json).map_err(|error| {
                format!(
                    "terminal command audit contains invalid tool_result_json (status={status}): {error}"
                )
            })?;
            if result.call_id != command.id || result.tool != "run_command" {
                return Err(format!(
                    "terminal command audit result identity differs from call={} (status={status})",
                    command.id
                ));
            }
            Ok(CommandAuditReconciliation::Terminal(result))
        }
        Some(AgentActionAuditExecutionClaimOutcome::AlreadyClaimed {
            status,
            tool_result_json,
        }) => Err(format!(
            "command audit is in unexpected state status={status}, hasToolResult={}",
            tool_result_json.is_some()
        )),
        Some(AgentActionAuditExecutionClaimOutcome::IdentityConflict { status }) => Err(format!(
            "command audit identity conflict while reconciling terminal commit (status={status})"
        )),
        Some(AgentActionAuditExecutionClaimOutcome::Claimed) => {
            Err("command audit inspection unexpectedly returned a newly claimed state".to_string())
        }
        None => {
            Err("command audit claim disappeared while reconciling terminal commit".to_string())
        }
    }
}

fn materialization_failure(error: SkillMaterializationError) -> (String, Option<Value>) {
    let message = error.message();
    (
        message.clone(),
        Some(serde_json::json!({
            "type": "skill_materialization",
            "code": error.code().stable_name(),
            "recovery": error.recovery().stable_name(),
            "message": message,
        })),
    )
}

fn materialization_result_status(
    status: SkillMaterializationStatus,
) -> AgentSkillMaterializationResultStatus {
    match status {
        SkillMaterializationStatus::Created => AgentSkillMaterializationResultStatus::Applied,
        SkillMaterializationStatus::AlreadyPresent => {
            AgentSkillMaterializationResultStatus::AlreadyApplied
        }
        _ => AgentSkillMaterializationResultStatus::Failed,
    }
}

fn materialization_success_message(status: SkillMaterializationStatus, tree: bool) -> String {
    match (status, tree) {
        (SkillMaterializationStatus::Created, false) => {
            "Skill resource was atomically created in the workspace."
        }
        (SkillMaterializationStatus::Created, true) => {
            "Skill template tree was atomically created in the workspace."
        }
        (SkillMaterializationStatus::AlreadyPresent, false) => {
            "The workspace already contains the exact verified resource."
        }
        (SkillMaterializationStatus::AlreadyPresent, true) => {
            "The workspace already contains the exact verified template tree."
        }
        _ => "Skill resource materialization completed.",
    }
    .to_string()
}

fn skill_script_runtime_failure(call_id: &str, error: SkillScriptRuntimeError) -> AgentToolResult {
    let message = error.message().to_string();
    AgentToolResult {
        call_id: call_id.to_string(),
        tool: "skills_run_script".to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "skill_script",
            "code": error.code().stable_name(),
            "recovery": error.recovery().stable_name(),
        })),
        error: Some(message),
    }
}

fn skill_script_tool_result(call_id: &str, result: AgentSkillScriptResult) -> AgentToolResult {
    let succeeded = result.exit_code == Some(0)
        && !result.timed_out
        && !result.cancelled
        && result.error.is_none();
    let error = if succeeded {
        None
    } else {
        result.error.clone().or_else(|| {
            if result.cancelled {
                Some("Skill script execution was cancelled.".to_string())
            } else if result.timed_out {
                Some("Skill script execution timed out.".to_string())
            } else {
                Some(format!(
                    "Skill script exited with code {}.",
                    result
                        .exit_code
                        .map_or_else(|| "unknown".to_string(), |code| code.to_string())
                ))
            }
        })
    };
    AgentToolResult {
        call_id: call_id.to_string(),
        tool: "skills_run_script".to_string(),
        ok: succeeded,
        result: serde_json::to_value(result).ok(),
        error,
    }
}

fn office_engine_failure(
    call_id: &str,
    tool: &str,
    prepared: &mycopilot_core::office::OfficePreparedExecution,
    error: OfficeEngineError,
) -> AgentToolResult {
    let error_code = error.code().stable_name();
    let message = error.message().to_string();
    AgentToolResult {
        call_id: call_id.to_string(),
        tool: tool.to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "office_engine",
            "code": error_code,
            "recovery": error.recovery().stable_name(),
            "providerId": prepared.provider_id,
            "engineRevision": prepared.engine_revision,
            "documentKind": prepared.request.document_kind,
            "operation": prepared.request.operation,
            "argv": prepared.argv,
            "cwd": ".",
            "exitCode": Value::Null,
            "stdout": "",
            "stderr": "",
            "timedOut": false,
            "cancelled": false,
            "durationMs": 0,
            "stdoutTruncated": false,
            "stderrTruncated": false,
            "errorCode": error_code,
            "error": message,
        })),
        error: Some(message),
    }
}

fn office_operation_tool_result(
    call_id: &str,
    tool: &str,
    mut result: OfficeExecutionResult,
) -> AgentToolResult {
    let succeeded = result.exit_code == Some(0)
        && !result.timed_out
        && !result.cancelled
        && result.error_code.is_none();
    if !succeeded {
        // A provider failure, timeout, or cancellation can preserve complete
        // process diagnostics, but it can never advertise a successful
        // published output to the model or UI.
        result.outputs.clear();
    }
    let error = if succeeded {
        None
    } else {
        result.error.clone().or_else(|| {
            if result.cancelled {
                Some("Office operation was cancelled.".to_string())
            } else if result.timed_out {
                Some("Office operation timed out.".to_string())
            } else {
                Some(format!(
                    "OfficeCLI exited with code {}.",
                    result
                        .exit_code
                        .map_or_else(|| "unknown".to_string(), |code| code.to_string())
                ))
            }
        })
    };
    AgentToolResult {
        call_id: call_id.to_string(),
        tool: tool.to_string(),
        ok: succeeded,
        // Preserve the complete provider result, including exitCode, stdout,
        // stderr, truncation markers, timeout, cancellation, and duration.
        result: serde_json::to_value(result).ok(),
        error,
    }
}

impl AgentService {
    pub(super) fn host_action_executor(
        &self,
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        skill_resources: Option<Arc<SkillResourceSession>>,
    ) -> AgentHostActionExecutor {
        let service = self.clone();
        let context = AutoApprovedActionContext::new(
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            skill_resources,
        );
        Arc::new(move |action, cancellation_token| {
            let mut refreshed = context.clone();
            service
                .refresh_agent_input_attachment_library(&mut refreshed.agent_input)
                .map_err(AgentError::new)?;
            service.execute_auto_approved_action(refreshed, action, cancellation_token)
        })
    }

    pub(super) fn settle_manual_file_effect(
        &self,
        record: &PendingActionRecord,
        call: &AgentToolCall,
        desired_pending_status: PendingActionStatus,
        execution_result: AgentToolResult,
        effect_type: &str,
        notifications: &CoreServerNotificationSender,
    ) -> ManualFileEffectSettlement {
        let run_id = &record.snapshot.run_id;
        let completed_at = now_ms();
        let build_input = |tool_result: &AgentToolResult| {
            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: record.snapshot.action_id.clone(),
                status: AgentApprovalDecisionStatus::Approved,
                message: None,
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });
            agent_input
        };

        let agent_input = build_input(&execution_result);
        let mut attempt_errors = Vec::new();
        for _ in 0..2 {
            match self.commit_audited_result_trace_with_continuation(
                record,
                &agent_input,
                desired_pending_status,
                None,
                completed_at,
                notifications,
            ) {
                Ok(()) => {
                    return ManualFileEffectSettlement::Committed {
                        agent_input: Box::new(agent_input),
                        tool_result: execution_result,
                        pending_status: desired_pending_status,
                    };
                }
                Err(error) => attempt_errors.push(error),
            }
        }

        match self.inspect_audited_result_trace_with_continuation(
            record,
            &agent_input,
            desired_pending_status,
            None,
            completed_at,
        ) {
            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                return ManualFileEffectSettlement::Committed {
                    agent_input: Box::new(agent_input),
                    tool_result: execution_result,
                    pending_status: desired_pending_status,
                };
            }
            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                return ManualFileEffectSettlement::CommittedAndAdvanced;
            }
            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {}
            Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its terminal receipt is divergent; continuation stopped: {component}: {reason}"
                        ),
                        attempt_error: &attempt_errors.join("; retry: "),
                        inspection_error: Some(&reason),
                        execution_result: &execution_result,
                    },
                );
                return ManualFileEffectSettlement::Unsettled;
            }
            Err(inspection_error) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its terminal receipt could not be inspected; continuation stopped: {inspection_error}"
                        ),
                        attempt_error: &attempt_errors.join("; retry: "),
                        inspection_error: Some(&inspection_error),
                        execution_result: &execution_result,
                    },
                );
                return ManualFileEffectSettlement::Unsettled;
            }
        }

        // The original result is definitely absent. Persist an explicit durability failure that
        // embeds the original bounded execution evidence, so the model never sees a clean
        // rollback or loses provider output after the side effect has already been attempted.
        let attempt_error = attempt_errors.join("; retry: ");
        let persistence_failure = file_effect_audit_persistence_failure(
            &execution_result.call_id,
            &execution_result.tool,
            effect_type,
            "afterExecution",
            &attempt_error,
            Some(&execution_result),
        );
        let failure_input = build_input(&persistence_failure);
        let failure_status = PendingActionStatus::Failed;
        let mut failure_errors = Vec::new();
        for _ in 0..2 {
            match self.commit_audited_result_trace_with_continuation(
                record,
                &failure_input,
                failure_status,
                None,
                completed_at,
                notifications,
            ) {
                Ok(()) => {
                    return ManualFileEffectSettlement::Committed {
                        agent_input: Box::new(failure_input),
                        tool_result: persistence_failure,
                        pending_status: failure_status,
                    };
                }
                Err(error) => failure_errors.push(error),
            }
        }

        let failure_error = failure_errors.join("; retry: ");
        match self.inspect_audited_result_trace_with_continuation(
            record,
            &failure_input,
            failure_status,
            None,
            completed_at,
        ) {
            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                ManualFileEffectSettlement::Committed {
                    agent_input: Box::new(failure_input),
                    tool_result: persistence_failure,
                    pending_status: failure_status,
                }
            }
            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                ManualFileEffectSettlement::CommittedAndAdvanced
            }
            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_persistence_failed",
                        message: format!(
                            "The file-producing action finished, but its audit, target state and paired ToolResult trace were not persisted; continuation stopped: {failure_error}"
                        ),
                        attempt_error: &failure_error,
                        inspection_error: None,
                        execution_result: &execution_result,
                    },
                );
                ManualFileEffectSettlement::Unsettled
            }
            Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its failure receipt is divergent; continuation stopped: {component}: {reason}"
                        ),
                        attempt_error: &failure_error,
                        inspection_error: Some(&reason),
                        execution_result: &execution_result,
                    },
                );
                ManualFileEffectSettlement::Unsettled
            }
            Err(inspection_error) => {
                emit_manual_file_effect_settlement_error(
                    notifications,
                    run_id,
                    ManualFileEffectSettlementError {
                        effect_type,
                        code: "approval_result_commit_indeterminate",
                        message: format!(
                            "The file-producing action finished, but its failure receipt could not be inspected; continuation stopped: {inspection_error}"
                        ),
                        attempt_error: &failure_error,
                        inspection_error: Some(&inspection_error),
                        execution_result: &execution_result,
                    },
                );
                ManualFileEffectSettlement::Unsettled
            }
        }
    }

    pub(super) fn execute_auto_approved_action(
        &self,
        context: AutoApprovedActionContext,
        action: AgentProposedAction,
        cancellation_token: AgentCancellationToken,
    ) -> AgentResult<AgentToolResult> {
        let AutoApprovedActionContext {
            agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            skill_resources,
        } = context;
        cancellation_token.check()?;
        if self.is_agent_input_scope_deleting(&agent_input) {
            return Err(AgentError::cancelled());
        }
        let created_at = now_ms();
        match &action {
            AgentProposedAction::OfficeOperation { office_operation } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) =
                            resolve_office_claim_outcome(office_operation, outcome)?
                        {
                            let effect_storage_id =
                                pending_action_storage_id(&run_id, &office_operation.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(office_audit_persistence_failure(
                            office_operation,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            AgentProposedAction::Command { command } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) = resolve_command_claim_outcome(command, outcome)? {
                            let effect_storage_id = pending_action_storage_id(&run_id, &command.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(command_audit_persistence_failure(
                            command,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            AgentProposedAction::SkillMaterialization { materialization } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            outcome,
                        )? {
                            let effect_storage_id =
                                pending_action_storage_id(&run_id, &materialization.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            AgentProposedAction::SkillScript { script } => {
                match self.inspect_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(Some(outcome)) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            outcome,
                        )? {
                            let effect_storage_id = pending_action_storage_id(&run_id, &script.id);
                            let mut file_effect_guard = self.register_file_effect(
                                &agent_input,
                                &run_id,
                                &effect_storage_id,
                            )?;
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
            }
            _ => {}
        }
        if let Err(error) = authorize_structured_file_write(
            &agent_input,
            &action,
            FileWriteAuthorizationSource::Automatic,
        ) {
            // A Host policy rejection is the result of this tool call, not a failure of the
            // agent transport. Office calls must stay paired with their original callId/tool so
            // the model loop and trace remain structurally valid.
            if matches!(&action, AgentProposedAction::OfficeOperation { .. }) {
                let tool_result = proposed_action_failure_result(&action, &error);
                if let Err(audit_error) = self.persist_auto_action_audit_if_absent(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    "rejected",
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    eprintln!(
                        "failed to write rejected Office auto action audit log: {audit_error}"
                    );
                }
                return Ok(tool_result);
            }
            return Err(error);
        }
        match action {
            AgentProposedAction::Diff { diff } => {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&agent_input) {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action_id = diff.id.clone();
                let execution = approved_patch_execution_for_input(&agent_input, &action_id, &diff);
                record_turn_file_change_best_effort(
                    &self.storage,
                    &agent_input,
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &action_id,
                    execution.file_change.as_ref(),
                );
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::Diff { diff },
                    &execution.status,
                    execution.patch_result.as_ref(),
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deletion_lifecycle);
                Ok(execution.tool_result)
            }
            AgentProposedAction::Command { command } => {
                let effect_storage_id = pending_action_storage_id(&run_id, &command.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
                let action = AgentProposedAction::Command {
                    command: command.clone(),
                };
                match self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(outcome) => {
                        if let Some(result) = resolve_command_claim_outcome(&command, outcome)? {
                            // A prior attempt already published the authoritative terminal
                            // receipt. Clear any in-process unresolved marker restored by an
                            // earlier commit-unknown path before returning the replayed result.
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(command_audit_persistence_failure(
                            &command,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                let workspace_root = workspace_root_optional(&agent_input);
                let permissions = permissions_from_input(&agent_input);
                let command_for_error = command.clone();
                let file_input_context = agent_file_input_execution_context(
                    &agent_input,
                    skill_resources.clone(),
                    Arc::clone(&self.storage),
                );
                file_effect_guard.mark_effects_started();
                let command_result = run_authorized_command_with_artifact_runtime_and_inputs(
                    workspace_root.as_deref(),
                    &command,
                    permissions,
                    CommandAuthorizationSource::Automatic,
                    cancellation_token.clone(),
                    None,
                    self.artifact_runtime.as_deref(),
                    Some(&file_input_context),
                )
                .unwrap_or_else(|error| {
                    let policy_evaluation = error.policy_evaluation().cloned();
                    let artifact_observation = error.artifact_observation().cloned();
                    let mut result = failed_command_result(
                        &command_for_error,
                        error.to_string(),
                        policy_evaluation,
                    );
                    result.artifact_observation = artifact_observation;
                    result
                });
                let command_succeeded = command_result.error.is_none()
                    && !command_result.timed_out
                    && !command_result.cancelled
                    && command_result.exit_code == Some(0);
                let tool_result = command_tool_result(&command.id, &command_result);
                let status = if command_succeeded {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    Some(&command_result),
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!("failed to inspect command audit: {inspect_error}")
                        })
                        .and_then(|outcome| reconcile_command_audit_outcome(&command, outcome));
                    match reconciliation {
                        Ok(CommandAuditReconciliation::Terminal(persisted)) => {
                            // SQLite committed the first terminal receipt even though the caller
                            // observed an error. Durable state is authoritative; never replace a
                            // successful receipt with a manufactured persistence failure.
                            file_effect_guard.mark_durably_settled();
                            return Ok(persisted);
                        }
                        Ok(CommandAuditReconciliation::Executing) => {}
                        Err(reconciliation_error) => {
                            let indeterminate = command_audit_finalization_indeterminate(
                                &command,
                                &error,
                                &reconciliation_error,
                                &command_result,
                            );
                            return Ok(indeterminate);
                        }
                    }

                    let persistence_failure = command_audit_persistence_failure(
                        &command,
                        "afterExecution",
                        &error,
                        Some(&command_result),
                    );
                    // A transient finalization failure gets one terminal retry. The immutable
                    // executing claim prevents a duplicate process execution, while persisting
                    // the original command result alongside the explicit durability failure.
                    if let Err(retry_error) = self.finalize_auto_action_execution_audit(
                        &run_id,
                        conversation_id.as_deref(),
                        assistant_message_id.as_deref(),
                        &agent_input,
                        &action,
                        "failed",
                        Some(&command_result),
                        &persistence_failure,
                        persistence_failure.error.as_deref(),
                        created_at,
                        now_ms(),
                    ) {
                        let reconciliation = self
                            .inspect_auto_action_execution_audit(
                                &run_id,
                                conversation_id.as_deref(),
                                assistant_message_id.as_deref(),
                                &agent_input,
                                &action,
                                created_at,
                            )
                            .map_err(|inspect_error| {
                                format!(
                                    "failed to inspect command audit after retry: {inspect_error}"
                                )
                            })
                            .and_then(|outcome| reconcile_command_audit_outcome(&command, outcome));
                        match reconciliation {
                            Ok(CommandAuditReconciliation::Terminal(persisted)) => {
                                file_effect_guard.mark_durably_settled();
                                return Ok(persisted);
                            }
                            Ok(CommandAuditReconciliation::Executing) => {
                                let indeterminate = command_audit_finalization_indeterminate(
                                    &command,
                                    &retry_error,
                                    "command audit remained executing after terminal retry",
                                    &command_result,
                                );
                                return Ok(indeterminate);
                            }
                            Err(reconciliation_error) => {
                                let indeterminate = command_audit_finalization_indeterminate(
                                    &command,
                                    &retry_error,
                                    &reconciliation_error,
                                    &command_result,
                                );
                                return Ok(indeterminate);
                            }
                        }
                    }
                    file_effect_guard.mark_durably_settled();
                    return Ok(persistence_failure);
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
            AgentProposedAction::FileWrite { file_write } => {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&agent_input) {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action = AgentProposedAction::FileWrite {
                    file_write: file_write.clone(),
                };
                let execution =
                    approved_file_write_execution(&self.storage, &agent_input, &file_write);
                record_turn_file_change_best_effort(
                    &self.storage,
                    &agent_input,
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &file_write.id,
                    execution.file_change.as_ref(),
                );
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    action,
                    &execution.status,
                    None,
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deletion_lifecycle);
                Ok(execution.tool_result)
            }
            AgentProposedAction::ToolCall { call } => Ok(AgentToolResult {
                call_id: call.id,
                tool: call.tool,
                ok: false,
                result: None,
                error: Some(
                    "自动批准执行器只支持结构化 apply_patch diff 和 run_command。".to_string(),
                ),
            }),
            AgentProposedAction::SkillMaterialization { materialization } => {
                let effect_storage_id = pending_action_storage_id(&run_id, &materialization.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
                let action = AgentProposedAction::SkillMaterialization {
                    materialization: materialization.clone(),
                };
                match self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(outcome) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            outcome,
                        )? {
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &materialization.id,
                            "skills_materialize_resource",
                            "skill_materialization",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                file_effect_guard.mark_effects_started();
                let tool_result = self.execute_skill_materialization(
                    &agent_input,
                    &materialization,
                    skill_resources.as_deref(),
                );
                let status = if tool_result.ok {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    None,
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!(
                                "failed to inspect Skill materialization audit: {inspect_error}"
                            )
                        })
                        .and_then(|outcome| {
                            reconcile_file_effect_audit_outcome(
                                &materialization.id,
                                "skills_materialize_resource",
                                outcome,
                            )
                        });
                    let reconciliation_error = match reconciliation {
                        Ok(FileEffectAuditReconciliation::Terminal(persisted)) => {
                            file_effect_guard.mark_durably_settled();
                            return Ok(persisted);
                        }
                        Ok(FileEffectAuditReconciliation::Executing) => {
                            "audit remained executing after terminal finalization".to_string()
                        }
                        Err(reconciliation_error) => reconciliation_error,
                    };
                    return Ok(file_effect_audit_persistence_failure(
                        &materialization.id,
                        "skills_materialize_resource",
                        "skill_materialization",
                        "afterExecution",
                        &format!("{error}; reconciliation: {reconciliation_error}"),
                        Some(&tool_result),
                    ));
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
            AgentProposedAction::SkillScript { script } => {
                let effect_storage_id = pending_action_storage_id(&run_id, &script.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
                let action = AgentProposedAction::SkillScript {
                    script: script.clone(),
                };
                match self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                ) {
                    Ok(outcome) => {
                        if let Some(result) = resolve_file_effect_claim_outcome(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            outcome,
                        )? {
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(file_effect_audit_persistence_failure(
                            &script.id,
                            "skills_run_script",
                            "skill_script",
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                file_effect_guard.mark_effects_started();
                let tool_result = self.execute_skill_script(
                    &agent_input,
                    &script,
                    skill_resources.as_deref(),
                    CommandAuthorizationSource::Automatic,
                    cancellation_token,
                    None,
                );
                let status = if tool_result.ok {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    None,
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!("failed to inspect Skill script audit: {inspect_error}")
                        })
                        .and_then(|outcome| {
                            reconcile_file_effect_audit_outcome(
                                &script.id,
                                "skills_run_script",
                                outcome,
                            )
                        });
                    let reconciliation_error = match reconciliation {
                        Ok(FileEffectAuditReconciliation::Terminal(persisted)) => {
                            file_effect_guard.mark_durably_settled();
                            return Ok(persisted);
                        }
                        Ok(FileEffectAuditReconciliation::Executing) => {
                            "audit remained executing after terminal finalization".to_string()
                        }
                        Err(reconciliation_error) => reconciliation_error,
                    };
                    return Ok(file_effect_audit_persistence_failure(
                        &script.id,
                        "skills_run_script",
                        "skill_script",
                        "afterExecution",
                        &format!("{error}; reconciliation: {reconciliation_error}"),
                        Some(&tool_result),
                    ));
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
            AgentProposedAction::OfficeOperation { office_operation } => {
                let action = AgentProposedAction::OfficeOperation { office_operation };
                let AgentProposedAction::OfficeOperation { office_operation } = &action else {
                    unreachable!("the action was constructed as an Office operation")
                };
                let tool = office_tool_name(office_operation.prepared.request.document_kind);
                let effect_storage_id = pending_action_storage_id(&run_id, &office_operation.id);
                let mut file_effect_guard =
                    self.register_file_effect(&agent_input, &run_id, &effect_storage_id)?;
                let claim = self.claim_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    created_at,
                );
                match claim {
                    Ok(outcome) => {
                        if let Some(result) =
                            resolve_office_claim_outcome(office_operation, outcome)?
                        {
                            file_effect_guard.mark_durably_settled();
                            return Ok(result);
                        }
                    }
                    Err(error) => {
                        return Ok(office_audit_persistence_failure(
                            office_operation,
                            "beforeExecution",
                            &error,
                            None,
                        ));
                    }
                }
                file_effect_guard.mark_effects_started();
                let tool_result = self.execute_office_operation(
                    &agent_input,
                    office_operation,
                    skill_resources.clone(),
                    cancellation_token,
                    None,
                );
                let status = if tool_result.ok {
                    "completed"
                } else {
                    "failed"
                };
                if let Err(error) = self.finalize_auto_action_execution_audit(
                    &run_id,
                    conversation_id.as_deref(),
                    assistant_message_id.as_deref(),
                    &agent_input,
                    &action,
                    status,
                    None,
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    let reconciliation = self
                        .inspect_auto_action_execution_audit(
                            &run_id,
                            conversation_id.as_deref(),
                            assistant_message_id.as_deref(),
                            &agent_input,
                            &action,
                            created_at,
                        )
                        .map_err(|inspect_error| {
                            format!("failed to inspect Office audit: {inspect_error}")
                        })
                        .and_then(|outcome| {
                            reconcile_file_effect_audit_outcome(&office_operation.id, tool, outcome)
                        });
                    if let Ok(FileEffectAuditReconciliation::Terminal(persisted)) = reconciliation {
                        file_effect_guard.mark_durably_settled();
                        return Ok(persisted);
                    }
                    return Ok(office_audit_persistence_failure(
                        office_operation,
                        "afterExecution",
                        &error,
                        Some(&tool_result),
                    ));
                }
                file_effect_guard.mark_durably_settled();
                Ok(tool_result)
            }
        }
    }

    pub(super) fn execute_office_operation(
        &self,
        agent_input: &AgentChatInput,
        office_operation: &mycopilot_core::AgentOfficeOperationRequest,
        skill_resources: Option<Arc<SkillResourceSession>>,
        cancellation_token: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> AgentToolResult {
        let tool = office_tool_name(office_operation.prepared.request.document_kind);
        if office_operation.approval_status != AgentApprovalStatus::Approved {
            return AgentToolResult {
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "notAuthorized",
                })),
                error: Some("Office operation has not been authorized.".to_string()),
            };
        }
        if office_operation.schema_version != mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION
            || !mycopilot_core::is_valid_agent_office_reason(&office_operation.reason)
            || mycopilot_core::validate_frozen_agent_office_semantic_args(office_operation).is_err()
            || office_operation.prepared.access
                != mycopilot_core::office::OfficeOperationAccess::FileWrite
            || office_operation.prepared.request.access()
                != mycopilot_core::office::OfficeOperationAccess::FileWrite
        {
            return AgentToolResult {
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "invalidApprovedSnapshot",
                    "recovery": "retry",
                })),
                error: Some(
                    "The approved Office action is not a supported file-write snapshot."
                        .to_string(),
                ),
            };
        }
        if permissions_from_input(agent_input).write == mycopilot_core::AgentWritePermission::Denied
        {
            return AgentToolResult {
                call_id: office_operation.id.clone(),
                tool: tool.to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation_policy",
                    "code": "writePermissionDenied",
                    "recovery": "changePermissions",
                })),
                error: Some(
                    "The current permission policy does not allow Office file changes.".to_string(),
                ),
            };
        }
        // Rebuild the Host-owned execution context at the last responsible moment. The Office
        // engine re-resolves every frozen path against these current run-scoped permissions and
        // attachment capabilities before it creates staging or invokes the provider.
        let skill_resources = match skill_resources {
            Some(resources) => Some(resources),
            None => match self.restore_skill_resource_session(agent_input) {
                Ok(resources) => resources,
                Err(error) => {
                    return office_skill_resource_restore_failure(
                        office_operation,
                        tool,
                        &error.to_string(),
                    );
                }
            },
        };
        let file_inputs = agent_file_input_execution_context(
            agent_input,
            skill_resources,
            Arc::clone(&self.storage),
        );
        let execution_context = mycopilot_core::office::OfficeExecutionContext::from_run_context(
            agent_input.context.as_ref(),
        )
        .with_file_inputs(file_inputs);
        match self.office_engine.execute_prepared(
            &execution_context,
            &office_operation.prepared,
            cancellation_token,
            action_cancel_flag,
        ) {
            Ok(result) => office_operation_tool_result(&office_operation.id, tool, result),
            Err(error) => office_engine_failure(
                &office_operation.id,
                tool,
                &office_operation.prepared,
                error,
            ),
        }
    }

    pub(super) fn execute_skill_script(
        &self,
        agent_input: &AgentChatInput,
        script: &AgentSkillScriptRequest,
        resources: Option<&SkillResourceSession>,
        authorization_source: CommandAuthorizationSource,
        cancellation_token: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> AgentToolResult {
        if script.approval_status != AgentApprovalStatus::Approved {
            return AgentToolResult {
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script_policy",
                    "code": "notAuthorized",
                })),
                error: Some("Skill script execution has not been authorized.".to_string()),
            };
        }
        let permissions = permissions_from_input(agent_input);
        let authorized = permissions.read == mycopilot_core::AgentReadPermission::All
            && permissions.write == mycopilot_core::AgentWritePermission::All
            && permissions.command_safety == mycopilot_core::AgentCommandSafetyPolicy::FullAccess
            && match authorization_source {
                CommandAuthorizationSource::Automatic => false,
                CommandAuthorizationSource::ExplicitUser => true,
            };
        if !authorized {
            return AgentToolResult {
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script_policy",
                    "code": "authorizationDenied",
                    "authorizationSource": authorization_source,
                })),
                error: Some(
                    "The current permission policy does not authorize this Skill script."
                        .to_string(),
                ),
            };
        }
        let Some(resources) = resources else {
            return AgentToolResult {
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script",
                    "code": "snapshotUnavailable",
                    "recovery": "reactivateSkill",
                })),
                error: Some("The activated Skill resource snapshot is unavailable.".to_string()),
            };
        };
        let Some(workspace_root) = workspace_root_optional(agent_input) else {
            return AgentToolResult {
                call_id: script.id.clone(),
                tool: "skills_run_script".to_string(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "skill_script",
                    "code": "workspaceUnavailable",
                    "recovery": "selectWorkspace",
                })),
                error: Some("Skill script execution requires a workspace.".to_string()),
            };
        };
        match execute_skill_python_script(
            resources,
            &workspace_root,
            script,
            cancellation_token,
            action_cancel_flag,
        ) {
            Ok(result) => skill_script_tool_result(&script.id, result),
            Err(error) => skill_script_runtime_failure(&script.id, error),
        }
    }

    pub(super) fn execute_skill_materialization(
        &self,
        agent_input: &AgentChatInput,
        materialization: &AgentSkillMaterializationRequest,
        resources: Option<&SkillResourceSession>,
    ) -> AgentToolResult {
        let execute = || -> Result<AgentSkillMaterializationResult, (String, Option<Value>)> {
            let plain_error = |message: &str| (message.to_string(), None);
            if materialization.approval_status != AgentApprovalStatus::Approved {
                return Err(plain_error(
                    "Skill resource materialization has not been authorized.",
                ));
            }
            if permissions_from_input(agent_input).write
                == mycopilot_core::AgentWritePermission::Denied
            {
                return Err(plain_error(
                    "Skill resource materialization requires workspace write permission.",
                ));
            }
            let resources = resources.ok_or_else(|| {
                plain_error("The activated Skill resource snapshot is unavailable.")
            })?;
            let workspace_root = workspace_root_optional(agent_input).ok_or_else(|| {
                plain_error("Skill resource materialization requires a workspace.")
            })?;
            let destination =
                SkillMaterializationDestination::parse(materialization.destination.clone())
                    .map_err(materialization_failure)?;
            let materializer = SkillResourceMaterializer::new();
            let result = if let Some(source_prefix) = materialization.source_prefix.as_deref() {
                let source = SkillPackageUri::parse(&materialization.source_uri)
                    .map_err(|error| plain_error(&error.to_string()))?;
                let source_prefix = SkillResourcePath::parse(source_prefix.to_string())
                    .map_err(|error| plain_error(&error.to_string()))?;
                let request = SkillTemplateTreeMaterializationRequest::new(
                    source,
                    source_prefix,
                    workspace_root,
                    destination,
                )
                .map_err(materialization_failure)?;
                let outcome = materializer
                    .materialize_template_tree(resources, &request)
                    .map_err(materialization_failure)?;
                AgentSkillMaterializationResult {
                    status: materialization_result_status(outcome.status()),
                    source_uri: outcome.source().to_string(),
                    source_prefix: Some(outcome.source_prefix().to_string()),
                    destination: outcome.destination().to_string(),
                    source_revision: outcome.source().revision().as_str().to_string(),
                    file_count: u64::try_from(outcome.file_count()).unwrap_or(u64::MAX),
                    byte_count: outcome.byte_length(),
                    plan_digest: Some(outcome.plan_digest().to_string()),
                    error: None,
                    message: Some(materialization_success_message(outcome.status(), true)),
                }
            } else {
                let source = SkillResourceUri::parse(&materialization.source_uri)
                    .map_err(|error| plain_error(&error.to_string()))?;
                let request = SkillMaterializationRequest::new(source, workspace_root, destination)
                    .map_err(materialization_failure)?;
                let outcome = materializer
                    .materialize(resources, &request)
                    .map_err(materialization_failure)?;
                AgentSkillMaterializationResult {
                    status: materialization_result_status(outcome.status()),
                    source_uri: outcome.source().to_string(),
                    source_prefix: None,
                    destination: outcome.destination().to_string(),
                    source_revision: outcome.source().package().revision().as_str().to_string(),
                    file_count: 1,
                    byte_count: outcome.byte_length(),
                    plan_digest: Some(outcome.content_digest().to_string()),
                    error: None,
                    message: Some(materialization_success_message(outcome.status(), false)),
                }
            };
            Ok(result)
        };

        match execute() {
            Ok(result) => AgentToolResult {
                call_id: materialization.id.clone(),
                tool: "skills_materialize_resource".to_string(),
                ok: true,
                result: serde_json::to_value(result).ok(),
                error: None,
            },
            Err((error, structured)) => AgentToolResult {
                call_id: materialization.id.clone(),
                tool: "skills_materialize_resource".to_string(),
                ok: false,
                result: structured,
                error: Some(error),
            },
        }
    }

    pub(super) fn queue_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let command_record = record.clone();
        tokio::spawn(async move {
            service
                .run_command_execution(command_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(super) fn queue_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_skill_script_execution(execution_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(super) fn queue_office_operation_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_office_operation_execution(execution_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(super) async fn run_office_operation_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::OfficeOperation {
            mut office_operation,
        } = record.snapshot.action.clone()
        else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        office_operation.approval_status = AgentApprovalStatus::Approved;

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => guard,
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let run_cancellation_token = cancellation_token.clone();
        let cancel_flag = guard.cancel_flag();
        let service = self.clone();
        let agent_input = record.agent_input.clone();
        file_effect_guard.mark_effects_started();
        let task_result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            match service.restore_skill_resource_session(&agent_input) {
                Ok(skill_resources) => service.execute_office_operation(
                    &agent_input,
                    &office_operation,
                    skill_resources,
                    cancellation_token,
                    Some(cancel_flag),
                ),
                Err(error) => office_skill_resource_restore_failure(
                    &office_operation,
                    office_tool_name(office_operation.prepared.request.document_kind),
                    &error.to_string(),
                ),
            }
        })
        .await;
        let mut tool_result = match task_result {
            Ok(result) => result,
            Err(error) => AgentToolResult {
                call_id: action_id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation",
                    "code": "executionTaskFailed",
                    "recovery": "retry",
                })),
                error: Some(format!("Office operation task failed: {error}")),
            },
        };
        // The Office engine owns the commit boundary. A cancellation observed before its
        // commit-ready point produces a cancelled result and discards staging; once the atomic
        // publish begins, a later cancellation must not rewrite a completed commit as cancelled.
        let result_cancelled = tool_result
            .result
            .as_ref()
            .and_then(|result| result.get("cancelled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if result_cancelled {
            tool_result.ok = false;
            let message = "Office operation was cancelled.".to_string();
            tool_result.error = Some(message.clone());
            if let Some(result) = tool_result.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("cancelled".to_string(), Value::Bool(true));
                result.insert(
                    "errorCode".to_string(),
                    Value::String("office.cancelled".to_string()),
                );
                result.insert("error".to_string(), Value::String(message));
            }
        }
        let desired_pending_status = if result_cancelled {
            PendingActionStatus::Cancelled
        } else if tool_result.ok {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        let settlement = self.settle_manual_file_effect(
            &record,
            &call,
            desired_pending_status,
            tool_result,
            "office_operation",
            &notifications,
        );
        let (agent_input, tool_result, final_pending_status) = match settlement {
            ManualFileEffectSettlement::Committed {
                agent_input,
                tool_result,
                pending_status,
            } => (*agent_input, tool_result, pending_status),
            ManualFileEffectSettlement::CommittedAndAdvanced => {
                file_effect_guard.mark_durably_settled();
                self.unregister_cancellation(&run_id);
                return;
            }
            ManualFileEffectSettlement::Unsettled => {
                self.unregister_cancellation(&run_id);
                return;
            }
        };
        file_effect_guard.mark_durably_settled();
        drop(file_effect_guard);
        // The receipt is already durable, but a concurrent message/conversation deletion may now
        // own the scope. Do not emit a transient result for an owner that is being removed; the
        // continuation below observes the same marker and terminates without recreating state.
        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: tool_result,
                }));
            }
        }
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }

    pub(super) async fn run_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::SkillScript { mut script } = record.snapshot.action.clone() else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        script.approval_status = AgentApprovalStatus::Approved;

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => guard,
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let mut tool_result = match self.restore_skill_resource_session(&record.agent_input) {
            Err(error) => {
                drop(guard);
                AgentToolResult {
                    call_id: action_id.clone(),
                    tool: "skills_run_script".to_string(),
                    ok: false,
                    result: Some(serde_json::json!({
                        "type": "skill_script",
                        "code": "snapshotUnavailable",
                        "recovery": "reactivateSkill",
                    })),
                    error: Some(error.to_string()),
                }
            }
            Ok(resources) => {
                let service = self.clone();
                let agent_input = record.agent_input.clone();
                file_effect_guard.mark_effects_started();
                match tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    service.execute_skill_script(
                        &agent_input,
                        &script,
                        resources.as_deref(),
                        CommandAuthorizationSource::ExplicitUser,
                        cancellation_token,
                        Some(cancel_flag),
                    )
                })
                .await
                {
                    Ok(result) => result,
                    Err(error) => AgentToolResult {
                        call_id: action_id.clone(),
                        tool: "skills_run_script".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "type": "skill_script",
                            "code": "executionTaskFailed",
                            "recovery": "retry",
                        })),
                        error: Some(format!("Skill script execution task failed: {error}")),
                    },
                }
            }
        };
        let result_cancelled = run_cancellation_token.is_cancelled()
            || post_execution_cancel_flag.load(Ordering::SeqCst)
            || tool_result
                .result
                .as_ref()
                .and_then(|result| result.get("cancelled"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
        if result_cancelled {
            tool_result.ok = false;
            let message = "Skill script execution was cancelled.".to_string();
            tool_result.error = Some(message.clone());
            if let Some(result) = tool_result.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("cancelled".to_string(), Value::Bool(true));
                result.insert(
                    "errorCode".to_string(),
                    Value::String("skill_script.cancelled".to_string()),
                );
                result.insert("error".to_string(), Value::String(message));
            }
        }
        let desired_pending_status = if result_cancelled {
            PendingActionStatus::Cancelled
        } else if tool_result.ok {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        let settlement = self.settle_manual_file_effect(
            &record,
            &call,
            desired_pending_status,
            tool_result,
            "skill_script",
            &notifications,
        );
        let (agent_input, tool_result, final_pending_status) = match settlement {
            ManualFileEffectSettlement::Committed {
                agent_input,
                tool_result,
                pending_status,
            } => (*agent_input, tool_result, pending_status),
            ManualFileEffectSettlement::CommittedAndAdvanced => {
                file_effect_guard.mark_durably_settled();
                self.unregister_cancellation(&run_id);
                return;
            }
            ManualFileEffectSettlement::Unsettled => {
                self.unregister_cancellation(&run_id);
                return;
            }
        };
        file_effect_guard.mark_durably_settled();
        drop(file_effect_guard);
        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: tool_result,
                }));
            }
        }
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }

    pub(super) async fn run_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::Command { command } = record.snapshot.action.clone() else {
            if let Err(error) =
                self.persist_pending_target_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &error,
                );
                return;
            }
            if let Err(error) = self.transition_pending_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &error,
                );
            }
            return;
        };

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => guard,
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let command_for_error = command.clone();
        let execution_record = record.clone();
        let artifact_runtime = self.artifact_runtime.clone();
        let skill_resources = self
            .restore_skill_resource_session(&record.agent_input)
            .ok()
            .flatten();
        let file_input_context = agent_file_input_execution_context(
            &record.agent_input,
            skill_resources,
            Arc::clone(&self.storage),
        );
        file_effect_guard.mark_effects_started();
        let mut command_result = match tokio::task::spawn_blocking(move || {
            let _guard = guard;
            // Keep pre-cancelled manual approvals on the same executor path as every other
            // command. The executor short-circuits before spawning a process, while still
            // producing the frozen runtime and artifact-observation evidence expected by the
            // durable command-result contract.
            run_explicitly_approved_command_from_snapshot_with_artifact_runtime(
                &execution_record,
                cancellation_token,
                Some(cancel_flag),
                artifact_runtime.as_deref(),
                Some(&file_input_context),
            )
        })
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let policy_evaluation = error.policy_evaluation().cloned();
                let artifact_observation = error.artifact_observation().cloned();
                let mut result =
                    failed_command_result(&command_for_error, error.to_string(), policy_evaluation);
                result.artifact_observation = artifact_observation;
                result
            }
            Err(error) => failed_command_result(
                &command_for_error,
                format!("命令执行任务失败：{error}"),
                None,
            ),
        };
        let execution_was_cancelled = run_cancellation_token.is_cancelled()
            || post_execution_cancel_flag.load(Ordering::SeqCst)
            || command_result.cancelled;
        if execution_was_cancelled {
            command_result.cancelled = true;
        }

        let (agent_input, final_pending_status) = {
            let command_succeeded = !execution_was_cancelled
                && command_result.error.is_none()
                && !command_result.timed_out
                && !command_result.cancelled
                && command_result.exit_code == Some(0);
            let desired_pending_status = if execution_was_cancelled {
                PendingActionStatus::Cancelled
            } else if command_succeeded {
                PendingActionStatus::Completed
            } else {
                PendingActionStatus::Failed
            };
            let mut final_pending_status = desired_pending_status;
            let mut tool_result = command_tool_result(&action_id, &command_result);
            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: action_id.clone(),
                status: AgentApprovalDecisionStatus::Approved,
                message: None,
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });

            // Capture one immutable terminal timestamp and retry the exact settlement. The
            // storage transaction is idempotent, so a SQLite commit that succeeded but returned
            // an error is recovered without replacing the real result with a synthetic failure.
            let completed_at = now_ms();
            let mut settlement_errors = Vec::new();
            let mut settled = false;
            for _ in 0..2 {
                match self.commit_audited_command_result_trace_with_continuation(
                    &record,
                    &agent_input,
                    desired_pending_status,
                    &command_result,
                    completed_at,
                    &notifications,
                ) {
                    Ok(()) => {
                        settled = true;
                        break;
                    }
                    Err(error) => settlement_errors.push(error),
                }
            }

            if !settled {
                match self.inspect_audited_command_result_trace_with_continuation(
                    &record,
                    &agent_input,
                    desired_pending_status,
                    &command_result,
                    completed_at,
                ) {
                    Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                        settled = true;
                    }
                    Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                        file_effect_guard.mark_durably_settled();
                        self.unregister_cancellation(&run_id);
                        return;
                    }
                    Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {}
                    Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => {
                        self.unregister_cancellation(&run_id);
                        emit_manual_command_settlement_error(
                            &notifications,
                            &run_id,
                            ManualCommandSettlementError {
                                code: "approval_result_commit_indeterminate",
                                message: format!(
                                    "命令已经结束，但审计事务处于冲突或部分提交状态；已停止续跑：{component}: {reason}"
                                ),
                                attempt_error: &settlement_errors.join("; retry: "),
                                inspection_error: Some(&reason),
                                command_result: &command_result,
                                tool_result: &tool_result,
                            },
                        );
                        return;
                    }
                    Err(inspection_error) => {
                        self.unregister_cancellation(&run_id);
                        emit_manual_command_settlement_error(
                            &notifications,
                            &run_id,
                            ManualCommandSettlementError {
                                code: "approval_result_commit_indeterminate",
                                message: format!(
                                    "命令已经结束，但无法权威核对审计事务是否提交；已停止续跑：{inspection_error}"
                                ),
                                attempt_error: &settlement_errors.join("; retry: "),
                                inspection_error: Some(&inspection_error),
                                command_result: &command_result,
                                tool_result: &tool_result,
                            },
                        );
                        return;
                    }
                }
            }

            if !settled {
                let audit_error = settlement_errors.join("; retry: ");
                tool_result = command_audit_persistence_failure(
                    &command_for_error,
                    "afterExecution",
                    &audit_error,
                    Some(&command_result),
                );
                final_pending_status = PendingActionStatus::Failed;
                agent_input.tool_continuation = Some(AgentToolContinuation {
                    call: call.clone(),
                    result: tool_result.clone(),
                });

                let mut failure_errors = Vec::new();
                for _ in 0..2 {
                    match self.commit_audited_command_result_trace_with_continuation(
                        &record,
                        &agent_input,
                        final_pending_status,
                        &command_result,
                        completed_at,
                        &notifications,
                    ) {
                        Ok(()) => {
                            settled = true;
                            break;
                        }
                        Err(error) => failure_errors.push(error),
                    }
                }

                if !settled {
                    let persistence_error = failure_errors.join("; retry: ");
                    match self.inspect_audited_command_result_trace_with_continuation(
                        &record,
                        &agent_input,
                        final_pending_status,
                        &command_result,
                        completed_at,
                    ) {
                        Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                            settled = true;
                        }
                        Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                            file_effect_guard.mark_durably_settled();
                            self.unregister_cancellation(&run_id);
                            return;
                        }
                        Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_persistence_failed",
                                    message: format!(
                                        "命令已经结束，但审计、目标终态和工具结果轨迹确认未提交；已停止续跑：{persistence_error}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: None,
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                        Ok(AgentPendingActionSettlementInspection::Diverged {
                            component,
                            reason,
                        }) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_commit_indeterminate",
                                    message: format!(
                                        "命令已经结束，但失败回执处于冲突或部分提交状态；已停止续跑：{component}: {reason}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: Some(&reason),
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                        Err(inspection_error) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_commit_indeterminate",
                                    message: format!(
                                        "命令已经结束，但无法权威核对失败回执是否提交；已停止续跑：{inspection_error}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: Some(&inspection_error),
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                    }
                }
            }
            debug_assert!(settled);
            file_effect_guard.mark_durably_settled();
            (agent_input, final_pending_status)
        };
        // The file-producing boundary is now durably paired with its ToolResult. Release the
        // project effect lease before model continuation; a concurrent deletion may proceed and
        // the continuation's deletion marker check will then stop any new action.
        drop(file_effect_guard);

        if execution_was_cancelled && final_pending_status == PendingActionStatus::Cancelled {
            const REASON: &str =
                "Agent run was cancelled while the approved command was executing.";
            // Order the final cancelled receipt, pending transition, and terminal events against
            // the same lifecycle marker used by destructive operations. If deletion won the
            // marker first, it owns cancellation and no state or event may be recreated here.
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if deletion_lifecycle.contains_input(&record.agent_input) {
                drop(deletion_lifecycle);
                self.discard_usage_context(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }
            if let Some(continuation) = agent_input.tool_continuation.as_ref() {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: continuation.result.clone(),
                }));
            }
            let mut cancelled_usage = None;
            let persisted = if let (
                Some(conversation_id),
                Some(assistant_message_id),
                Some(checkpoint),
                Some(continuation),
            ) = (
                record.snapshot.conversation_id.as_deref(),
                record.snapshot.assistant_message_id.as_deref(),
                record.agent_input.resume_checkpoint.as_ref(),
                agent_input.tool_continuation.as_ref(),
            ) {
                let trace = cancelled_conversation_trace_from_checkpoint(
                    checkpoint,
                    conversation_id,
                    assistant_message_id,
                    &continuation.call,
                    &continuation.result,
                    REASON,
                );
                let mut output = AgentChatOutput {
                    content: String::new(),
                    status: AgentRunStatus::Cancelled,
                    run_id: run_id.clone(),
                    events: Vec::new(),
                    tool_definitions: Vec::new(),
                    todo: None,
                    usage: None,
                    finish_reason: Some(REASON.to_string()),
                    proposed_actions: Vec::new(),
                    conversation_turn_trace: Some(trace),
                };
                let persisted = self.persist_final_assistant_output(
                    conversation_id,
                    assistant_message_id,
                    &mut output,
                );
                if persisted.is_ok() {
                    cancelled_usage = output.usage.clone();
                }
                persisted
            } else {
                Err("cancelled command is missing its conversation trace checkpoint".to_string())
            };
            if let Err(error) = persisted {
                self.unregister_cancellation(&run_id);
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    message: format!(
                        "无法原子持久化已取消命令的 assistant 终态与会话轨迹：{error}"
                    ),
                    recoverable: true,
                    code: Some("conversation_trace_persistence_failed".to_string()),
                    details: None,
                }));
                return;
            }
            if let Err(error) =
                self.transition_pending_status(&record, PendingActionStatus::Cancelled)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Cancelled,
                    &error,
                );
                self.unregister_cancellation(&run_id);
                return;
            }
            self.discard_trace_snapshot(&run_id);
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                run_id,
                success: false,
                status: Some(AgentRunStatus::Cancelled),
                content: None,
                usage: cancelled_usage,
                finish_reason: Some(REASON.to_string()),
                proposed_actions: Vec::new(),
            }));
            return;
        }

        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                if let Some(continuation) = agent_input.tool_continuation.as_ref() {
                    let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                        run_id: run_id.clone(),
                        result: continuation.result.clone(),
                    }));
                }
            }
        }

        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }

    pub(super) async fn run_action_continuation(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        existing_cancellation_token: Option<AgentCancellationToken>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let cancellation_token = existing_cancellation_token.unwrap_or_default();
        if cancellation_token.is_cancelled() {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return;
        }
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return;
        }
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            self.discard_usage_context(&run_id);
            return;
        }
        let steer_input = match (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
        ) {
            (Some(conversation_id), Some(assistant_message_id)) => Some(
                self.register_active_run_control(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    record
                        .agent_input
                        .context
                        .as_ref()
                        .and_then(|context| context.project_id.as_deref()),
                    record.agent_input.model_capabilities,
                ),
            ),
            _ => None,
        };

        let emitter_notifications = notifications.clone();
        let emitter_service = self.clone();
        let emitter_conversation_id = record.snapshot.conversation_id.clone();
        let emitter_assistant_message_id = record.snapshot.assistant_message_id.clone();
        let emitter_agent_input = record.agent_input.clone();
        let terminal_event_gate = Arc::new(AgentTerminalEventGate::default());
        let emitter_terminal_event_gate = terminal_event_gate.clone();
        let pending_store_failure = Arc::new(Mutex::new(None::<String>));
        let emitter_pending_store_failure = Arc::clone(&pending_store_failure);
        let emitter: AgentEventEmitter = Arc::new(move |event| {
            if let AgentEvent::ApprovalRequired {
                run_id,
                action,
                checkpoint,
            } = &event
            {
                if let Err(error) = emitter_service.close_active_run_steering(
                    run_id,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    "The agent run is waiting for approval and no longer accepts guidance.",
                    &emitter_notifications,
                ) {
                    emitter_terminal_event_gate.discard();
                    *emitter_pending_store_failure
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                    return;
                }
                let mut agent_input =
                    agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                if let Err(error) =
                    emitter_service.refresh_agent_input_attachment_library(&mut agent_input)
                {
                    emitter_terminal_event_gate.discard();
                    *emitter_pending_store_failure
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                    return;
                }
                match emitter_service.store_pending_action(
                    run_id,
                    emitter_conversation_id.as_deref().unwrap_or_default(),
                    emitter_assistant_message_id.as_deref().unwrap_or_default(),
                    action.as_ref().clone(),
                    agent_input,
                ) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => {
                        emitter_terminal_event_gate.discard();
                        *emitter_pending_store_failure
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                        return;
                    }
                }
            }
            if emitter_pending_store_failure
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some()
            {
                return;
            }
            let event = emitter_service.project_cumulative_usage_onto_event(event);
            if let Some(event) = emitter_terminal_event_gate.route(event) {
                let _ = emitter_notifications.send(agent_event_notification(event));
            }
        });

        let skill_resources = match self.restore_skill_resource_session(&agent_input) {
            Ok(resources) => resources,
            Err(error) => {
                let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
                self.discard_usage_context(&run_id);
                if let Some(steer_input) = steer_input.as_ref() {
                    let _ = self.unregister_active_run_control(
                        &run_id,
                        steer_input,
                        AgentSteerRunRejectionCode::RunNotSteerable,
                        "The agent run has finished and no longer accepts guidance.",
                        &notifications,
                    );
                }
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    message: error.to_string(),
                    recoverable: true,
                    code: Some("skill_resource_snapshot_unavailable".to_string()),
                    details: None,
                }));
                return;
            }
        };
        let initial_context_window_tool_projection = match self
            .context_window_tool_projection(&record.agent_input, skill_resources.clone())
        {
            Ok(projection) => projection,
            Err(error) => {
                let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
                self.discard_usage_context(&run_id);
                if let Some(steer_input) = steer_input.as_ref() {
                    let _ = self.unregister_active_run_control(
                        &run_id,
                        steer_input,
                        AgentSteerRunRejectionCode::RunNotSteerable,
                        "The agent run has finished and no longer accepts guidance.",
                        &notifications,
                    );
                }
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    message: error,
                    recoverable: true,
                    code: Some("context_window_tool_projection_unavailable".to_string()),
                    details: None,
                }));
                return;
            }
        };
        let context_window_tool_projection =
            RunContextToolProjection::new(initial_context_window_tool_projection);
        let host_executor = self.host_action_executor(
            agent_input.clone(),
            run_id.clone(),
            record.snapshot.conversation_id.clone(),
            record.snapshot.assistant_message_id.clone(),
            skill_resources.clone(),
        );
        let trace_conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .unwrap_or_default();
        let trace_assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .unwrap_or_default();
        let trace_observer = self.trace_observer(
            &run_id,
            trace_conversation_id,
            trace_assistant_message_id,
            record.snapshot.created_at,
            record.agent_input.clone(),
            context_window_tool_projection.clone(),
            notifications.clone(),
        );
        let context_compaction_services = self.context_compaction_services(
            &run_id,
            trace_conversation_id,
            trace_assistant_message_id,
            record.agent_input.clone(),
            context_window_tool_projection.clone(),
            notifications.clone(),
        );
        let model_request_observer =
            self.model_request_observer(&run_id, trace_conversation_id, trace_assistant_message_id);
        let context_window_observer =
            record
                .agent_input
                .context_window_indicator_enabled
                .then(|| {
                    self.context_window_observer(
                        &run_id,
                        trace_conversation_id,
                        &record.agent_input.model,
                        notifications.clone(),
                    )
                });
        let mut host_services = AgentRuntimeHostServices::new()
            .with_host_actions(host_executor, self.storage.clone())
            .with_office_engine(self.office_engine.clone())
            .with_trace_observer(trace_observer)
            .with_model_request_observer(model_request_observer)
            .with_context_compaction(context_compaction_services);
        if let Some(context_window_observer) = context_window_observer {
            host_services = host_services.with_context_window_observer(context_window_observer);
        }
        if let Some(image_generation_execution) = self.image_generation_execution.clone() {
            host_services =
                host_services.with_image_generation_execution(image_generation_execution);
        }
        host_services = host_services.with_skill_activation_resolver(
            model_skill_activation_resolver(self.storage.clone(), self.skills.clone()),
        );
        if let Some(resources) = skill_resources {
            host_services = host_services.with_skill_resources(resources);
        }
        if let Some(resolver) = self.artifact_runtime.clone() {
            host_services = host_services.with_command_runtime_profile_resolver(resolver);
        }
        if let Some(steer_input) = steer_input.as_ref() {
            host_services = host_services.with_steer_input(steer_input.clone());
        }
        let result = send_chat_with_host_services(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token.clone(),
            host_services,
        )
        .await;
        let result = match pending_store_failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            Some(error) => {
                terminal_event_gate.discard();
                Err(pending_action_persistence_error(error))
            }
            None => result,
        };
        let close_message = match &result {
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval => {
                "The agent run is waiting for approval and no longer accepts guidance."
            }
            _ => "The agent run has finished and no longer accepts guidance.",
        };
        let result = if let Some(steer_input) = steer_input.as_ref() {
            match self.unregister_active_run_control(
                &run_id,
                steer_input,
                AgentSteerRunRejectionCode::RunNotSteerable,
                close_message,
                &notifications,
            ) {
                Ok(()) => result,
                Err(error) => {
                    terminal_event_gate.discard();
                    Err(AgentError::new(format!(
                        "无法关闭审批续跑的用户引导通道并持久化剩余引导：{error}"
                    )))
                }
            }
        } else {
            result
        };
        let keep_trace_snapshot = matches!(
            &result,
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval
        );

        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&record.agent_input) {
            drop(deletion_lifecycle);
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return;
        }

        match result {
            Ok(mut agent_output) => {
                let committed_durable_context = is_terminal_run_status(agent_output.status);
                let owner_ids = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                );
                let persisted = match owner_ids {
                    (Some(conversation_id), Some(assistant_message_id)) => self
                        .persist_final_assistant_output(
                            conversation_id,
                            assistant_message_id,
                            &mut agent_output,
                        ),
                    _ => Err("审批续跑缺少 assistant 持久化身份。".to_string()),
                };
                if persisted.is_ok() {
                    if let (Some(conversation_id), Some(task_state)) = (
                        record.snapshot.conversation_id.as_deref(),
                        record.agent_input.task_state.as_ref(),
                    ) {
                        if let Err(error) = self.storage.settle_task_state_run(
                            conversation_id,
                            &task_state.control.task_id,
                            &run_id,
                            agent_output.status == AgentRunStatus::WaitingForApproval,
                            match agent_output.status {
                                AgentRunStatus::WaitingForApproval => Some("waiting_for_approval"),
                                AgentRunStatus::Cancelled => Some("run_cancelled"),
                                _ => None,
                            },
                            now_ms(),
                        ) {
                            terminal_event_gate.discard();
                            let _ =
                                notifications.send(agent_event_notification(AgentEvent::Error {
                                    run_id: Some(run_id.clone()),
                                    message: format!(
                                        "审批续跑已持久化，但 Task State 终态写入失败：{error}"
                                    ),
                                    recoverable: true,
                                    code: Some("task_state_settlement_failed".to_string()),
                                    details: None,
                                }));
                        }
                    }
                }
                let pending_transition = if persisted.is_ok() {
                    self.transition_pending_status(&record, final_pending_status)
                } else {
                    Err("assistant 终态未持久化，已保留非终态 pending 记录供启动对账。".to_string())
                }
                .inspect_err(|error| {
                    terminal_event_gate.discard();
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        error,
                    );
                });
                if let (Some(conversation_id), Some(assistant_message_id)) = owner_ids {
                    if pending_terminal_commit_is_publishable(
                        persisted.is_ok(),
                        pending_transition.is_ok(),
                        committed_durable_context,
                    ) {
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            if agent_output.status == AgentRunStatus::Cancelled {
                                ""
                            } else {
                                &agent_output.content
                            },
                        );
                    } else if let Err(error) = &persisted {
                        self.discard_usage_context(&run_id);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if pending_terminal_commit_is_publishable(
                        persisted.is_ok(),
                        pending_transition.is_ok(),
                        committed_durable_context,
                    ) {
                        emit_terminal_events_after_persistence(
                            &notifications,
                            &terminal_event_gate,
                            &agent_output,
                        );
                    }
                }
            }
            Err(error) => {
                terminal_event_gate.discard();
                let usage = error.usage().cloned();
                let code = error.code().map(ToString::to_string);
                let details = error.details().cloned();
                let message = error.to_string();
                let conversation_turn_trace = match (
                    error.conversation_turn_trace().cloned(),
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    (Some(trace), _, _) => Some(trace),
                    (None, Some(conversation_id), Some(assistant_message_id)) => {
                        Some(failed_conversation_trace_without_items(
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            &message,
                        ))
                    }
                    _ => None,
                };
                let persisted = if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let persisted = self.persist_assistant_error(
                        conversation_id,
                        assistant_message_id,
                        &message,
                        usage.clone(),
                        conversation_turn_trace
                            .as_ref()
                            .expect("trace exists when conversation and assistant ids exist"),
                    );
                    if let Err(error) = &persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: format!(
                                "无法原子持久化 assistant 失败终态与会话轨迹：{error}"
                            ),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    persisted
                } else {
                    Err("审批续跑缺少 assistant 持久化身份。".to_string())
                };
                let cumulative_usage = persisted.as_ref().ok().cloned().flatten();
                if persisted.is_ok() {
                    if let (Some(conversation_id), Some(task_state)) = (
                        record.snapshot.conversation_id.as_deref(),
                        record.agent_input.task_state.as_ref(),
                    ) {
                        if let Err(error) = self.storage.settle_task_state_run(
                            conversation_id,
                            &task_state.control.task_id,
                            &run_id,
                            false,
                            Some("run_failed"),
                            now_ms(),
                        ) {
                            let _ =
                                notifications.send(agent_event_notification(AgentEvent::Error {
                                    run_id: Some(run_id.clone()),
                                    message: format!(
                                        "审批续跑失败已持久化，但 Task State 写入失败：{error}"
                                    ),
                                    recoverable: true,
                                    code: Some("task_state_settlement_failed".to_string()),
                                    details: None,
                                }));
                        }
                    }
                }
                let pending_transition = if persisted.is_ok() {
                    self.transition_pending_status(&record, final_pending_status)
                } else {
                    Err(
                        "assistant 失败终态未持久化，已保留非终态 pending 记录供启动对账。"
                            .to_string(),
                    )
                }
                .inspect_err(|transition_error| {
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        transition_error,
                    );
                });
                let terminal_commit_published = pending_terminal_commit_is_publishable(
                    persisted.is_ok(),
                    pending_transition.is_ok(),
                    true,
                );
                if terminal_commit_published {
                    if let (Some(conversation_id), Some(assistant_message_id)) = (
                        record.snapshot.conversation_id.as_deref(),
                        record.snapshot.assistant_message_id.as_deref(),
                    ) {
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            &message,
                        );
                    }
                    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                        run_id: Some(run_id.clone()),
                        message: message.clone(),
                        recoverable: false,
                        code,
                        details,
                    }));
                    let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                        run_id: run_id.clone(),
                        success: false,
                        status: Some(AgentRunStatus::Failed),
                        content: Some(message),
                        usage: cumulative_usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
            }
        }

        drop(deletion_lifecycle);
        if !keep_trace_snapshot {
            self.discard_trace_snapshot(&run_id);
            self.discard_exact_running_context_window_snapshot(&run_id);
        }
        self.unregister_cancellation_if_current(&run_id, &cancellation_token);
    }
}

/// Executes only the command frozen in the backend-owned pending-action snapshot.
///
/// The approval endpoint accepts an action id rather than a replacement command. Keeping snapshot
/// selection and the `ExplicitUser` authorization source together at this boundary prevents a
/// caller from turning approval of one command into execution of another.
#[cfg(test)]
pub(super) fn run_explicitly_approved_command_from_snapshot(
    record: &PendingActionRecord,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    run_explicitly_approved_command_from_snapshot_with_artifact_runtime(
        record,
        cancellation_token,
        action_cancel_flag,
        None,
        None,
    )
}

pub(super) fn run_explicitly_approved_command_from_snapshot_with_artifact_runtime(
    record: &PendingActionRecord,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    artifact_runtime: Option<&mycopilot_core::artifact_runtime::ArtifactRuntimeProvider>,
    file_inputs: Option<&AgentFileInputExecutionContext>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    let AgentProposedAction::Command { command } = &record.snapshot.action else {
        return Err("待审批操作不包含可执行命令。".to_string().into());
    };
    let workspace_root = workspace_root_optional(&record.agent_input);
    run_authorized_command_with_artifact_runtime_and_inputs(
        workspace_root.as_deref(),
        command,
        permissions_from_input(&record.agent_input),
        CommandAuthorizationSource::ExplicitUser,
        cancellation_token,
        action_cancel_flag,
        artifact_runtime,
        file_inputs,
    )
}

pub(super) fn cancelled_file_write_outcome_is_durable_or_unknown(
    storage: &StorageService,
    record: &PendingActionRecord,
) -> bool {
    let AgentProposedAction::FileWrite { file_write } = &record.snapshot.action else {
        return false;
    };
    match storage.get_agent_file_draft(&file_write.draft_id) {
        Ok(Some(draft)) => draft.status == "rejected",
        Ok(None) => false,
        // A storage read failure makes rollback unsafe: the rejection write may have committed.
        Err(_) => true,
    }
}
