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
    result: OfficeExecutionResult,
) -> AgentToolResult {
    let succeeded = result.exit_code == Some(0)
        && !result.timed_out
        && !result.cancelled
        && result.error_code.is_none();
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
            service.execute_auto_approved_action(context.clone(), action, cancellation_token)
        })
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
        if self.is_agent_input_project_deleting(&agent_input) {
            return Err(AgentError::cancelled());
        }
        let created_at = now_ms();
        if let AgentProposedAction::OfficeOperation { office_operation } = &action {
            match self.inspect_auto_action_execution_audit(
                &run_id,
                conversation_id.as_deref(),
                assistant_message_id.as_deref(),
                &agent_input,
                &action,
                created_at,
            ) {
                Ok(Some(outcome)) => {
                    if let Some(result) = resolve_office_claim_outcome(office_operation, outcome)? {
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
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action_id = diff.id.clone();
                let execution = approved_patch_execution_for_input(&agent_input, &action_id, &diff);
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
                drop(deleting_projects);
                Ok(execution.tool_result)
            }
            AgentProposedAction::Command { command } => {
                let workspace_root = workspace_root_optional(&agent_input);
                let permissions = permissions_from_input(&agent_input);
                let command_for_error = command.clone();
                let command_result = run_authorized_command(
                    workspace_root.as_deref(),
                    &command,
                    permissions,
                    CommandAuthorizationSource::Automatic,
                    cancellation_token.clone(),
                    None,
                )
                .unwrap_or_else(|error| {
                    let policy_evaluation = error.policy_evaluation().cloned();
                    failed_command_result(&command_for_error, error.to_string(), policy_evaluation)
                });
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                let command_succeeded = command_result.error.is_none()
                    && !command_result.timed_out
                    && !command_result.cancelled
                    && command_result.exit_code == Some(0);
                let tool_result =
                    command_tool_result(&command.id, command_succeeded, &command_result);
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::Command { command },
                    if command_succeeded {
                        "completed"
                    } else {
                        "failed"
                    },
                    None,
                    Some(&command_result),
                    Some(&tool_result),
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deleting_projects);
                Ok(tool_result)
            }
            AgentProposedAction::FileWrite { file_write } => {
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action = AgentProposedAction::FileWrite {
                    file_write: file_write.clone(),
                };
                let execution =
                    approved_file_write_execution(&self.storage, &agent_input, &file_write);
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
                drop(deleting_projects);
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
                let tool_result = self.execute_skill_materialization(
                    &agent_input,
                    &materialization,
                    skill_resources.as_deref(),
                );
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::SkillMaterialization { materialization },
                    if tool_result.ok {
                        "completed"
                    } else {
                        "failed"
                    },
                    None,
                    None,
                    Some(&tool_result),
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                Ok(tool_result)
            }
            AgentProposedAction::SkillScript { script } => {
                let tool_result = self.execute_skill_script(
                    &agent_input,
                    &script,
                    skill_resources.as_deref(),
                    CommandAuthorizationSource::Automatic,
                    cancellation_token,
                    None,
                );
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::SkillScript { script },
                    if tool_result.ok {
                        "completed"
                    } else {
                        "failed"
                    },
                    None,
                    None,
                    Some(&tool_result),
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                Ok(tool_result)
            }
            AgentProposedAction::OfficeOperation { office_operation } => {
                let action = AgentProposedAction::OfficeOperation { office_operation };
                let AgentProposedAction::OfficeOperation { office_operation } = &action else {
                    unreachable!("the action was constructed as an Office operation")
                };
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
                let tool_result = self.execute_office_operation(
                    &agent_input,
                    office_operation,
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
                    &tool_result,
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                ) {
                    return Ok(office_audit_persistence_failure(
                        office_operation,
                        "afterExecution",
                        &error,
                        Some(&tool_result),
                    ));
                }
                Ok(tool_result)
            }
        }
    }

    pub(super) fn execute_office_operation(
        &self,
        agent_input: &AgentChatInput,
        office_operation: &mycopilot_core::AgentOfficeOperationRequest,
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
        let execution_context = mycopilot_core::office::OfficeExecutionContext::from_run_context(
            agent_input.context.as_ref(),
        );
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
        if self.is_agent_input_project_deleting(&record.agent_input) {
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
        let office_operation_for_audit = office_operation.clone();

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let run_cancellation_token = cancellation_token.clone();
        let cancel_flag = guard.cancel_flag();
        let service = self.clone();
        let agent_input = record.agent_input.clone();
        let task_result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            service.execute_office_operation(
                &agent_input,
                &office_operation,
                cancellation_token,
                Some(cancel_flag),
            )
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
        let mut final_pending_status = if result_cancelled {
            PendingActionStatus::Cancelled
        } else if tool_result.ok {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        if let Err(error) = self.persist_action_audit(
            &record,
            Some("approved"),
            pending_status_label(final_pending_status),
            None,
            None,
            Some(&tool_result),
            tool_result.error.as_deref(),
            None,
            Some(now_ms()),
        ) {
            tool_result = office_audit_persistence_failure(
                &office_operation_for_audit,
                "afterExecution",
                &error,
                Some(&tool_result),
            );
            final_pending_status = PendingActionStatus::Failed;
        }

        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: action_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call,
            result: tool_result.clone(),
        });
        if let Err(error) = self.commit_pending_result_trace_with_continuation(
            &record,
            &agent_input,
            final_pending_status,
            &notifications,
        ) {
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                run_id: run_id.clone(),
                result: tool_result.clone(),
            }));
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!(
                    "Office operation outcome and paired trace could not be committed atomically; continuation stopped: {error}"
                ),
                recoverable: true,
                code: Some("approval_result_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
            run_id: run_id.clone(),
            result: tool_result,
        }));
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
        if self.is_agent_input_project_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::SkillScript { mut script } = record.snapshot.action.clone() else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        script.approval_status = AgentApprovalStatus::Approved;

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
        let final_pending_status = if result_cancelled {
            PendingActionStatus::Cancelled
        } else if tool_result.ok {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        self.record_action_audit(
            &record,
            Some("approved"),
            pending_status_label(final_pending_status),
            None,
            None,
            Some(&tool_result),
            tool_result.error.as_deref(),
            None,
            Some(now_ms()),
        );

        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: action_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call,
            result: tool_result.clone(),
        });
        if let Err(error) = self.persist_pending_target_status(&record, final_pending_status) {
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!(
                    "Skill script target status could not be persisted; continuation stopped: {error}"
                ),
                recoverable: true,
                code: Some("pending_action_target_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        if let Err(error) =
            self.commit_trace_snapshot_with_continuation(&record, &agent_input, &notifications)
        {
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!("Skill script result could not be persisted: {error}"),
                recoverable: true,
                code: Some("conversation_trace_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
            run_id: run_id.clone(),
            result: tool_result,
        }));
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
        if self.is_agent_input_project_deleting(&record.agent_input) {
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

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_project_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let command_for_error = command.clone();
        let command_for_execution = command.clone();
        let execution_record = record.clone();
        let mut command_result = match tokio::task::spawn_blocking(move || {
            let _guard = guard;
            if cancel_flag.load(Ordering::SeqCst) || cancellation_token.is_cancelled() {
                Ok(cancelled_command_result(&command_for_execution))
            } else {
                run_explicitly_approved_command_from_snapshot(
                    &execution_record,
                    cancellation_token,
                    Some(cancel_flag),
                )
            }
        })
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let policy_evaluation = error.policy_evaluation().cloned();
                failed_command_result(&command_for_error, error.to_string(), policy_evaluation)
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
            let deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if agent_input_project_id(&record.agent_input)
                .is_some_and(|project_id| deleting_projects.contains(project_id))
            {
                self.discard_usage_context(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }

            let command_succeeded = !execution_was_cancelled
                && command_result.error.is_none()
                && !command_result.timed_out
                && !command_result.cancelled
                && command_result.exit_code == Some(0);
            let final_pending_status = if execution_was_cancelled {
                PendingActionStatus::Cancelled
            } else if command_succeeded {
                PendingActionStatus::Completed
            } else {
                PendingActionStatus::Failed
            };
            let tool_result = command_tool_result(&action_id, command_succeeded, &command_result);
            self.record_action_audit(
                &record,
                Some("approved"),
                pending_status_label(final_pending_status),
                None,
                Some(&command_result),
                Some(&tool_result),
                tool_result.error.as_deref(),
                None,
                Some(now_ms()),
            );

            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: action_id.clone(),
                status: AgentApprovalDecisionStatus::Approved,
                message: None,
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call,
                result: tool_result,
            });
            (agent_input, final_pending_status)
        };
        if let Err(error) = self.persist_pending_target_status(&record, final_pending_status) {
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!("命令目标终态无法持久化，已停止续跑：{error}"),
                recoverable: true,
                code: Some("pending_action_target_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        if let Err(error) =
            self.commit_trace_snapshot_with_continuation(&record, &agent_input, &notifications)
        {
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!("命令结果无法写入会话轨迹：{error}"),
                recoverable: true,
                code: Some("conversation_trace_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        if let Some(continuation) = agent_input.tool_continuation.as_ref() {
            let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                run_id: run_id.clone(),
                result: continuation.result.clone(),
            }));
        }

        if execution_was_cancelled {
            const REASON: &str =
                "Agent run was cancelled while the approved command was executing.";
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
                let output = AgentChatOutput {
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
                self.persist_final_assistant_output(conversation_id, assistant_message_id, &output)
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
                usage: None,
                finish_reason: Some(REASON.to_string()),
                proposed_actions: Vec::new(),
            }));
            return;
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
        if self.is_agent_input_project_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation(&run_id);
            return;
        }
        let cancellation_token = existing_cancellation_token.unwrap_or_default();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_project_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

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
                let agent_input = agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                match emitter_service.store_pending_action(
                    run_id,
                    emitter_conversation_id.as_deref().unwrap_or_default(),
                    emitter_assistant_message_id.as_deref().unwrap_or_default(),
                    action.clone(),
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
            if let Some(event) = emitter_terminal_event_gate.route(event) {
                let _ = emitter_notifications.send(agent_event_notification(event));
            }
        });

        let skill_resources = match self.restore_skill_resource_session(&agent_input) {
            Ok(resources) => resources,
            Err(error) => {
                let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
                self.discard_usage_context(&run_id);
                self.unregister_cancellation(&run_id);
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
            notifications.clone(),
        );
        let context_compaction_services = self.context_compaction_services(
            &run_id,
            trace_conversation_id,
            trace_assistant_message_id,
            record.agent_input.clone(),
            notifications.clone(),
        );
        let model_request_observer =
            self.model_request_observer(&run_id, trace_conversation_id, trace_assistant_message_id);
        let mut host_services = AgentRuntimeHostServices::new()
            .with_host_actions(host_executor, self.storage.clone())
            .with_office_engine(self.office_engine.clone())
            .with_trace_observer(trace_observer)
            .with_model_request_observer(model_request_observer)
            .with_context_compaction(context_compaction_services);
        if let Some(resources) = skill_resources {
            host_services = host_services.with_skill_resources(resources);
        }
        let result = send_chat_with_host_services(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token,
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
        let keep_trace_snapshot = matches!(
            &result,
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval
        );

        let deleting_projects = self
            .deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if agent_input_project_id(&record.agent_input)
            .is_some_and(|project_id| deleting_projects.contains(project_id))
        {
            drop(deleting_projects);
            self.discard_usage_context(&run_id);
            self.unregister_cancellation(&run_id);
            return;
        }

        match result {
            Ok(agent_output) => {
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
                            &agent_output,
                        ),
                    _ => Err("审批续跑缺少 assistant 持久化身份。".to_string()),
                };
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
                        usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
            }
        }

        drop(deleting_projects);
        if !keep_trace_snapshot {
            self.discard_trace_snapshot(&run_id);
        }
        self.unregister_cancellation(&run_id);
    }
}

/// Executes only the command frozen in the backend-owned pending-action snapshot.
///
/// The approval endpoint accepts an action id rather than a replacement command. Keeping snapshot
/// selection and the `ExplicitUser` authorization source together at this boundary prevents a
/// caller from turning approval of one command into execution of another.
pub(super) fn run_explicitly_approved_command_from_snapshot(
    record: &PendingActionRecord,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    let AgentProposedAction::Command { command } = &record.snapshot.action else {
        return Err("待审批操作不包含可执行命令。".to_string().into());
    };
    let workspace_root = workspace_root_optional(&record.agent_input);
    run_authorized_command(
        workspace_root.as_deref(),
        command,
        permissions_from_input(&record.agent_input),
        CommandAuthorizationSource::ExplicitUser,
        cancellation_token,
        action_cancel_flag,
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
