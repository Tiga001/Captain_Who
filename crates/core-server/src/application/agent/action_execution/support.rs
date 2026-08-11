use super::*;

pub(super) fn proposed_action_failure_result(
    action: &AgentProposedAction,
    error: &AgentError,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
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

pub(super) fn agent_file_input_execution_context(
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
    .with_conversation_id(
        input
            .context
            .as_ref()
            .and_then(|context| context.conversation_id.as_deref()),
    )
}

pub(super) fn bounded_audit_error(error: &str) -> String {
    const MAX_AUDIT_ERROR_CHARS: usize = 2_048;
    error.chars().take(MAX_AUDIT_ERROR_CHARS).collect()
}

pub(super) fn office_audit_persistence_failure(
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
        exact_archive_file: None,
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

pub(super) fn office_skill_resource_restore_failure(
    office_operation: &mycopilot_core::AgentOfficeOperationRequest,
    tool: &str,
    error: &str,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
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

pub(super) fn command_audit_persistence_failure(
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
        exact_archive_file: None,
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

pub(super) fn command_audit_finalization_indeterminate(
    command: &mycopilot_core::AgentCommandRequest,
    audit_error: &str,
    reconciliation_error: &str,
    execution_result: &AgentCommandExecutionResult,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
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

pub(super) struct ManualCommandSettlementError<'a> {
    pub(super) code: &'a str,
    pub(super) message: String,
    pub(super) attempt_error: &'a str,
    pub(super) inspection_error: Option<&'a str>,
    pub(super) command_result: &'a AgentCommandExecutionResult,
    pub(super) tool_result: &'a AgentToolResult,
}

pub(super) fn emit_manual_command_settlement_error(
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

pub(crate) enum ManualFileEffectSettlement {
    Committed {
        agent_input: Box<AgentChatInput>,
        tool_result: AgentToolResult,
        pending_status: PendingActionStatus,
    },
    CommittedAndAdvanced,
    Unsettled,
}

pub(super) struct ManualFileEffectSettlementError<'a> {
    pub(super) effect_type: &'a str,
    pub(super) code: &'a str,
    pub(super) message: String,
    pub(super) attempt_error: &'a str,
    pub(super) inspection_error: Option<&'a str>,
    pub(super) execution_result: &'a AgentToolResult,
}

pub(super) fn emit_manual_file_effect_settlement_error(
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

pub(super) enum CommandAuditReconciliation {
    Executing,
    Terminal(AgentToolResult),
}

/// Compares the complete protocol-visible ToolResult while deliberately ignoring the backend-only
/// exact-archive spool handle. A commit-unknown handoff may only be adopted when durable storage
/// contains the exact running receipt the caller attempted to publish.
pub(super) fn agent_tool_results_match(left: &AgentToolResult, right: &AgentToolResult) -> bool {
    left.call_id == right.call_id
        && left.tool == right.tool
        && left.ok == right.ok
        && left.result == right.result
        && left.error == right.error
}

pub(super) fn office_execution_claim_error(
    existing_status: &str,
    identity_conflict: bool,
) -> AgentError {
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

pub(super) fn replay_persisted_office_tool_result(
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

pub(super) fn resolve_office_claim_outcome(
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

pub(super) fn command_execution_claim_error(
    existing_status: &str,
    identity_conflict: bool,
) -> AgentError {
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

pub(super) fn resolve_command_claim_outcome(
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

pub(super) fn resolve_file_effect_claim_outcome(
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

pub(super) fn file_effect_audit_persistence_failure(
    call_id: &str,
    tool: &str,
    effect_type: &str,
    phase: &str,
    audit_error: &str,
    execution_result: Option<&AgentToolResult>,
) -> AgentToolResult {
    let execution_attempted = execution_result.is_some();
    AgentToolResult {
        exact_archive_file: None,
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

pub(super) enum FileEffectAuditReconciliation {
    Executing,
    Terminal(AgentToolResult),
}

pub(super) fn reconcile_file_effect_audit_outcome(
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

pub(super) fn reconcile_command_audit_outcome(
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

pub(super) fn materialization_failure(error: SkillMaterializationError) -> (String, Option<Value>) {
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

pub(super) fn materialization_result_status(
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

pub(super) fn materialization_success_message(
    status: SkillMaterializationStatus,
    tree: bool,
) -> String {
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

pub(super) fn skill_script_runtime_failure(
    call_id: &str,
    error: SkillScriptRuntimeError,
) -> AgentToolResult {
    let message = error.message().to_string();
    AgentToolResult {
        exact_archive_file: None,
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

pub(super) fn skill_script_tool_result(
    call_id: &str,
    result: AgentSkillScriptResult,
) -> AgentToolResult {
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
    let substitutions = result.output_spool_substitutions();
    let mut tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: "skills_run_script".to_string(),
        ok: succeeded,
        result: serde_json::to_value(&result).ok(),
        error,
    };
    if !substitutions.is_empty() {
        match mycopilot_core::command::materialize_process_tool_result_archive(
            &tool_result,
            &substitutions,
        ) {
            Ok(file) => tool_result.exact_archive_file = file,
            Err(error) => {
                eprintln!("failed to materialize exact skills_run_script output archive: {error}");
            }
        }
    }
    tool_result
}

pub(super) fn office_engine_failure(
    call_id: &str,
    tool: &str,
    prepared: &mycopilot_core::office::OfficePreparedExecution,
    error: OfficeEngineError,
) -> AgentToolResult {
    let error_code = error.code().stable_name();
    let message = error.message().to_string();
    AgentToolResult {
        exact_archive_file: None,
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

pub(super) fn office_operation_tool_result(
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
    let substitutions = result.output_spool_substitutions();
    let mut tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: tool.to_string(),
        ok: succeeded,
        // Preserve the complete provider result, including exitCode, stdout,
        // stderr, truncation markers, timeout, cancellation, and duration.
        result: serde_json::to_value(&result).ok(),
        error,
    };
    if !substitutions.is_empty() {
        match mycopilot_core::command::materialize_process_tool_result_archive(
            &tool_result,
            &substitutions,
        ) {
            Ok(file) => tool_result.exact_archive_file = file,
            Err(error) => {
                eprintln!("failed to materialize exact OfficeCLI output archive: {error}");
            }
        }
    }
    tool_result
}
