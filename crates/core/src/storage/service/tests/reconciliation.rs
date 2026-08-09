use super::*;

fn in_progress_result_trace(
    conversation_id: &str,
    assistant_message_id: &str,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-1".to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "action-atomic-result".to_string(),
                tool: "office_spreadsheet".to_string(),
                provenance: None,
                operation: serde_json::json!({ "operation": "set" }),
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: "action-atomic-result".to_string(),
                tool: "office_spreadsheet".to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: serde_json::json!({ "exitCode": 0 }),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    }
}

fn manual_command_settlement(
    storage_id: &str,
    call_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> (
    AgentPendingActionRecord,
    AgentActionAuditRecord,
    AgentActionAuditRecord,
    ConversationTurnTrace,
) {
    let action = AgentProposedAction::Command {
        command: crate::AgentCommandRequest {
            id: call_id.to_string(),
            command: "node script.mjs".to_string(),
            cwd: None,
            timeout_ms: Some(5_000),
            approval_status: crate::AgentApprovalStatus::Required,
            risk_level: None,
            reason: Some("test atomic settlement".to_string()),
            observe: None,
            inputs: Vec::new(),
            runtime: None,
            runtime_binding: None,
        },
    };
    let action_json = serde_json::to_string(&action).unwrap();
    let pending = AgentPendingActionRecord {
        action_id: storage_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "command".to_string(),
        tool_name: "run_command".to_string(),
        tool_call_id: Some(call_id.to_string()),
        status: "approved".to_string(),
        target_status: None,
        action_json: action_json.clone(),
        agent_input_json: "{}".to_string(),
        created_at: 10,
        updated_at: 11,
    };
    let approved_audit = AgentActionAuditRecord {
        action_id: storage_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "command".to_string(),
        tool_name: "run_command".to_string(),
        decision: Some("approved".to_string()),
        status: "approved".to_string(),
        action_json,
        patch_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 10,
        decided_at: Some(11),
        completed_at: None,
        effective_permissions_json: Some("{}".to_string()),
        path_scope: Some("workspace".to_string()),
        command_cwd_scope: Some("workspace".to_string()),
        blocked_reason: None,
        decision_source: Some("manual".to_string()),
    };
    let command_result = AgentCommandExecutionResult {
        command: "node script.mjs".to_string(),
        cwd: "/workspace".to_string(),
        exit_code: Some(0),
        stdout: "created workbook".to_string(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration_ms: 12,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: Default::default(),
        stdout_spool: Default::default(),
        stderr_spool: Default::default(),
        error: None,
        policy_evaluation: None,
        artifact_observation: None,
        input_files: Vec::new(),
        runtime: None,
    };
    let tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(serde_json::to_value(&command_result).unwrap()),
        error: None,
    };
    let mut terminal_audit = approved_audit.clone();
    terminal_audit.status = "completed".to_string();
    terminal_audit.command_result_json = Some(serde_json::to_string(&command_result).unwrap());
    terminal_audit.tool_result_json = Some(serde_json::to_string(&tool_result).unwrap());
    terminal_audit.completed_at = Some(12);
    let trace_operation = serde_json::json!({
        "command": "node script.mjs",
        "timeoutMs": 5_000,
        "reason": "test atomic settlement",
    });
    let trace_call = AgentToolCall {
        id: call_id.to_string(),
        tool: "run_command".to_string(),
        args: trace_operation.clone(),
        approval_status: crate::AgentApprovalStatus::Approved,
        reason: Some("test atomic settlement".to_string()),
    };
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-1".to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                provenance: None,
                operation: trace_operation,
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            crate::conversation_trace::projected_tool_result_trace_item(
                1,
                &trace_call,
                &tool_result,
            ),
        ],
    };
    (pending, approved_audit, terminal_audit, trace)
}

#[derive(Debug, Clone, Copy)]
enum ManualNonCommandFileEffect {
    OfficeOperation,
    SkillScript,
    SkillMaterialization,
}

#[derive(Debug, Clone, Copy)]
enum LegacyManualAuditShape {
    Missing,
    Preterminal,
    TamperedTerminal,
}

impl LegacyManualAuditShape {
    const ALL: [Self; 3] = [Self::Missing, Self::Preterminal, Self::TamperedTerminal];

    fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing-audit",
            Self::Preterminal => "preterminal-audit",
            Self::TamperedTerminal => "tampered-terminal-audit",
        }
    }
}

type CommittedManualFileEffectState = (
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
);

type LegacyManualFileEffectSplitState = (
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
);

type UnverifiableLegacyManualFileEffectState = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl ManualNonCommandFileEffect {
    const ALL: [Self; 3] = [
        Self::OfficeOperation,
        Self::SkillScript,
        Self::SkillMaterialization,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::OfficeOperation => "office",
            Self::SkillScript => "skill-script",
            Self::SkillMaterialization => "skill-materialization",
        }
    }

    fn action_type(self) -> &'static str {
        match self {
            Self::OfficeOperation => "office_operation",
            Self::SkillScript => "skill_script",
            Self::SkillMaterialization => "skill_materialization",
        }
    }

    fn tool_name(self) -> &'static str {
        match self {
            Self::OfficeOperation => "office_spreadsheet",
            Self::SkillScript => "skills_run_script",
            Self::SkillMaterialization => "skills_materialize_resource",
        }
    }

    fn frozen_action(self, call_id: &str, alternate_payload: bool) -> AgentProposedAction {
        match self {
            Self::OfficeOperation => AgentProposedAction::OfficeOperation {
                office_operation: Box::new(crate::AgentOfficeOperationRequest {
                    schema_version: crate::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
                    id: call_id.to_string(),
                    semantic_args: serde_json::json!({
                        "operation": "create",
                        "filePath": if alternate_payload {
                            "alternate-budget.xlsx"
                        } else {
                            "budget.xlsx"
                        },
                        "reason": "create the reviewed workbook"
                    }),
                    prepared: crate::office::OfficePreparedExecution {
                        schema_version: crate::office::OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
                        provider_id: "officecli".to_string(),
                        engine_revision: "engine-revision-1".to_string(),
                        workspace_revision: Some("workspace-revision-1".to_string()),
                        access: crate::office::OfficeOperationAccess::FileWrite,
                        request: crate::office::OfficeExecutionRequest {
                            document_kind: crate::office::OfficeDocumentKind::Spreadsheet,
                            operation: crate::office::OfficeOperation::Create,
                            document_path: Some(if alternate_payload {
                                "alternate-budget.xlsx".to_string()
                            } else {
                                "budget.xlsx".to_string()
                            }),
                            parameters: crate::office::OfficeRequestParameters::Typed(
                                crate::office::OfficeOperationParameters::Create {
                                    locale: None,
                                    minimal: false,
                                    overwrite: false,
                                },
                            ),
                            output_path: None,
                            destination_path: None,
                            inputs: Vec::new(),
                            timeout_ms: None,
                        },
                        argv: vec![
                            "create".to_string(),
                            if alternate_payload {
                                "alternate-budget.xlsx".to_string()
                            } else {
                                "budget.xlsx".to_string()
                            },
                        ],
                        paths: Vec::new(),
                        input_bindings: Vec::new(),
                        document_precondition: None,
                        output_precondition: None,
                        destination_precondition: None,
                        resource_preconditions: Vec::new(),
                    },
                    approval_status: crate::AgentApprovalStatus::Required,
                    reason: "create the reviewed workbook".to_string(),
                }),
            },
            Self::SkillScript => AgentProposedAction::SkillScript {
                script: Box::new(crate::AgentSkillScriptRequest {
                    id: call_id.to_string(),
                    script_uri:
                        "skill://package/installed%3Auser%3Afixture/revision/scripts/build.py"
                            .to_string(),
                    skill_id: "installed:user:fixture".to_string(),
                    skill_revision: "revision-1".to_string(),
                    resource_path: "scripts/build.py".to_string(),
                    resource_digest: "sha256:fixture".to_string(),
                    interpreter: crate::AgentSkillScriptInterpreter::Python3,
                    args: if alternate_payload {
                        vec!["--alternate".to_string()]
                    } else {
                        Vec::new()
                    },
                    requirements: crate::AgentSkillScriptRequirements::default(),
                    preflight: crate::AgentSkillScriptPreflightReport {
                        status: crate::AgentSkillScriptPreflightStatus::Ready,
                        interpreter: crate::AgentSkillScriptInterpreter::Python3,
                        interpreter_version: Some("Python 3.12.13".to_string()),
                        dependencies: Vec::new(),
                        runtime_fingerprint: "runtime-fingerprint-1".to_string(),
                        error_code: None,
                        message: None,
                    },
                    timeout_ms: Some(crate::skills::DEFAULT_SKILL_SCRIPT_TIMEOUT_MS),
                    approval_status: crate::AgentApprovalStatus::Required,
                    reason: Some("run the reviewed Skill script".to_string()),
                }),
            },
            Self::SkillMaterialization => AgentProposedAction::SkillMaterialization {
                materialization: crate::AgentSkillMaterializationRequest {
                    id: call_id.to_string(),
                    source_uri: "skill://package/installed%3Auser%3Afixture/revision/template.xlsx"
                        .to_string(),
                    source_prefix: None,
                    destination: if alternate_payload {
                        "alternate-template.xlsx".to_string()
                    } else {
                        "template.xlsx".to_string()
                    },
                    approval_status: crate::AgentApprovalStatus::Required,
                    reason: Some("materialize the reviewed Skill resource".to_string()),
                },
            },
        }
    }

    fn tool_operation(self, action: &AgentProposedAction) -> serde_json::Value {
        match (self, action) {
            (Self::OfficeOperation, AgentProposedAction::OfficeOperation { office_operation }) => {
                office_operation.semantic_args.clone()
            }
            (Self::SkillScript, AgentProposedAction::SkillScript { script }) => {
                serde_json::json!({
                    "scriptUri": script.script_uri,
                })
            }
            (
                Self::SkillMaterialization,
                AgentProposedAction::SkillMaterialization { materialization },
            ) => serde_json::json!({
                "sourceUri": materialization.source_uri,
                "destination": materialization.destination,
            }),
            _ => panic!("manual file-effect test action does not match its effect kind"),
        }
    }
}

fn manual_non_command_file_effect_settlement(
    effect: ManualNonCommandFileEffect,
    storage_id: &str,
    call_id: &str,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> (
    AgentPendingActionRecord,
    AgentActionAuditRecord,
    AgentActionAuditRecord,
    ConversationTurnTrace,
) {
    let action = effect.frozen_action(call_id, false);
    let tool_operation = effect.tool_operation(&action);
    let action_json = serde_json::to_string(&action).unwrap();
    let pending = AgentPendingActionRecord {
        action_id: storage_id.to_string(),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: effect.action_type().to_string(),
        tool_name: effect.tool_name().to_string(),
        tool_call_id: Some(call_id.to_string()),
        status: "approved".to_string(),
        target_status: None,
        action_json: action_json.clone(),
        agent_input_json: "{}".to_string(),
        created_at: 10,
        updated_at: 11,
    };
    let approved_audit = AgentActionAuditRecord {
        action_id: storage_id.to_string(),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: effect.action_type().to_string(),
        tool_name: effect.tool_name().to_string(),
        decision: Some("approved".to_string()),
        status: "approved".to_string(),
        action_json,
        patch_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 10,
        decided_at: Some(11),
        completed_at: None,
        effective_permissions_json: Some("{}".to_string()),
        path_scope: Some("workspace".to_string()),
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some("manual".to_string()),
    };
    let tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: effect.tool_name().to_string(),
        ok: true,
        result: Some(serde_json::json!({
            "status": "applied",
            "effect": effect.label(),
        })),
        error: None,
    };
    let mut terminal_audit = approved_audit.clone();
    terminal_audit.status = "completed".to_string();
    terminal_audit.tool_result_json = Some(serde_json::to_string(&tool_result).unwrap());
    terminal_audit.completed_at = Some(12);
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: effect.tool_name().to_string(),
                provenance: None,
                operation: tool_operation,
                approval_status: crate::AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: call_id.to_string(),
                tool: effect.tool_name().to_string(),
                status: crate::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: tool_result.result.clone().unwrap(),
                approval_status: crate::AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    };
    (pending, approved_audit, terminal_audit, trace)
}

fn mcp_rejection_settlement(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> (
    AgentPendingActionRecord,
    AgentActionAuditRecord,
    AgentActionAuditRecord,
    ConversationTurnTrace,
    AgentToolCall,
) {
    let action_id = "31522e9e-0f12-4d7c-9a7d-2c6e91c1f0d1";
    let call_id =
        crate::llm::model_response_tool_call_id(run_id, 0, 0, "fixture-mcp-rejection-call");
    let model_tool_name = "mcp__fixture__list_directory";
    let provenance = crate::AgentMcpToolProvenance {
        server_id: "7f4a2d91-24ab-4d24-9eed-63daf26a6c15".to_string(),
        scope: crate::AgentMcpServerScope::User,
        raw_tool_name: "list_directory".to_string(),
        model_tool_name: model_tool_name.to_string(),
        config_epoch: "66dcbb6b-92a3-4d4e-9591-f0707e4ca3e3".to_string(),
        registry_revision: 3,
        config_digest: "a".repeat(64),
        catalog_generation: 4,
        catalog_digest: "b".repeat(64),
        catalog_schema_digest: "c".repeat(64),
        schema_digest: "d".repeat(64),
        schema_normalizer_version: crate::MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
    };
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: model_tool_name.to_string(),
        args: serde_json::json!({}),
        approval_status: crate::AgentApprovalStatus::Required,
        reason: None,
    };
    let approval = crate::AgentMcpToolApproval {
        identity: crate::AgentMcpToolInvocationIdentity {
            action_id: action_id.to_string(),
            invocation_id: "8e8272e7-a27b-4b82-a7bf-c90b98b2d76c".to_string(),
            run_id: run_id.to_string(),
            call_id: call_id.to_string(),
            provenance: provenance.clone(),
            arguments_digest: crate::mcp_tool_arguments_digest(&serde_json::json!({})).unwrap(),
        },
        call: call.clone(),
        summary: crate::AgentMcpToolApprovalSummary {
            server_id: provenance.server_id.clone(),
            server_display_name: "Fixture MCP".to_string(),
            scope: provenance.scope.clone(),
            raw_tool_name: provenance.raw_tool_name.clone(),
            model_tool_name: provenance.model_tool_name.clone(),
            display_reason: Some("List the allowed fixture directory.".to_string()),
            arguments: crate::AgentMcpArgumentSummary {
                encoded_bytes: 2,
                top_level_property_count: 0,
                string_value_count: 0,
                number_value_count: 0,
                boolean_value_count: 0,
                null_value_count: 0,
                object_value_count: 1,
                array_value_count: 0,
                max_depth: 0,
                truncated: false,
            },
            risk: crate::AgentMcpToolRisk::ReadOnlyClaimed,
            external: true,
        },
        approval_mode: crate::AgentMcpApprovalMode::Prompt,
        payload_persistence: crate::AgentMcpApprovalPayloadPersistence::ProcessOnly,
        created_at: 10,
        expires_at: 1_000,
    };
    let action = AgentProposedAction::McpToolCall {
        approval: Box::new(approval.clone()),
    };
    let action_json = serde_json::to_string(&action).unwrap();
    let storage_id = format!("v2:{}:{run_id}:{action_id}", run_id.len());
    let pending = AgentPendingActionRecord {
        action_id: storage_id.clone(),
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "mcp_tool_call".to_string(),
        tool_name: model_tool_name.to_string(),
        tool_call_id: Some(call_id.to_string()),
        status: "pending".to_string(),
        target_status: None,
        action_json: action_json.clone(),
        agent_input_json: "{}".to_string(),
        created_at: 10,
        updated_at: 11,
    };
    let pending_audit = AgentActionAuditRecord {
        action_id: storage_id,
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: "mcp_tool_call".to_string(),
        tool_name: model_tool_name.to_string(),
        decision: None,
        status: "pending".to_string(),
        action_json,
        patch_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 10,
        decided_at: None,
        completed_at: None,
        effective_permissions_json: Some("{}".to_string()),
        path_scope: None,
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some("manual_pending".to_string()),
    };
    let rejected_result = crate::mcp_tool_result_persistence_projection(
        &crate::mcp_tool_result_from_rejected_approval(&approval, None).unwrap(),
    );
    let mut terminal_audit = pending_audit.clone();
    terminal_audit.decision = Some("rejected".to_string());
    terminal_audit.status = "rejected".to_string();
    terminal_audit.tool_result_json = Some(serde_json::to_string(&rejected_result).unwrap());
    terminal_audit.decided_at = Some(12);
    terminal_audit.completed_at = Some(12);
    terminal_audit.decision_source = Some("manual".to_string());
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: call_id.to_string(),
                tool: model_tool_name.to_string(),
                provenance: Some(crate::AgentToolIdentity::Mcp { provenance }),
                operation: serde_json::json!({}),
                approval_status: crate::AgentApprovalStatus::Required,
                truncated: false,
            },
            crate::conversation_trace::projected_tool_result_trace_item(1, &call, &rejected_result),
        ],
    };
    (pending, pending_audit, terminal_audit, trace, call)
}

fn assert_manual_file_effect_is_uncommitted(
    service: &StorageService,
    storage_id: &str,
    assistant_message_id: &str,
    frozen_action_json: &str,
) {
    let connection = service.state.connection().unwrap();
    let state: (
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        String,
    ) = connection
        .query_row(
            "
            SELECT pending.target_status,
                   audit.status,
                   audit.command_result_json,
                   audit.tool_result_json,
                   audit.action_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [storage_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(state.0, None, "pending target must remain uncommitted");
    assert_eq!(state.1, "approved", "audit must remain preterminal");
    assert_eq!(state.2, None, "non-command result column must stay empty");
    assert_eq!(state.3, None, "terminal ToolResult must not leak");
    assert_eq!(state.4, frozen_action_json, "frozen action changed");
    drop(connection);
    assert!(service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .is_none());
}

fn assert_legacy_manual_file_effect_remains_split(
    service: &StorageService,
    storage_id: &str,
    assistant_message_id: &str,
    expected_pending_status: &str,
    terminal_tool_result_json: &str,
    frozen_action_json: &str,
) {
    let connection = service.state.connection().unwrap();
    let state: LegacyManualFileEffectSplitState = connection
        .query_row(
            "
            SELECT pending.status,
                   pending.target_status,
                   audit.status,
                   audit.command_result_json,
                   audit.tool_result_json,
                   audit.action_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [storage_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(state.0, expected_pending_status);
    assert_eq!(
        state.1, None,
        "legacy pending target was partially repaired"
    );
    assert_eq!(state.2, "completed");
    assert_eq!(state.3, None, "non-command result column was populated");
    assert_eq!(state.4.as_deref(), Some(terminal_tool_result_json));
    assert_eq!(state.5, frozen_action_json);
    drop(connection);
    assert!(service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .is_none());
}

fn save_assistant_conversation(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
) {
    save_assistant_conversation_in_scope(
        service,
        conversation_id,
        assistant_message_id,
        Some("project-1"),
    );
}

fn save_assistant_conversation_in_scope(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    project_id: Option<&str>,
) {
    let mut stored = conversation(conversation_id, project_id, assistant_message_id);
    stored.messages[0].role = "assistant".to_string();
    service.save_conversation(stored).unwrap();
}

fn file_effect_action_type(tool_name: &str) -> &'static str {
    match tool_name {
        "office_document" | "office_spreadsheet" | "office_presentation" => "office_operation",
        "skills_run_script" => "skill_script",
        "skills_materialize_resource" => "skill_materialization",
        other => panic!("unexpected file-producing tool in test: {other}"),
    }
}

fn file_effect_audit(
    storage_id: &str,
    run_id: &str,
    conversation_id: &str,
    tool_name: &str,
    status: &str,
    decision_source: &str,
    created_at: i64,
) -> AgentActionAuditRecord {
    let mut audit = action_audit(storage_id, conversation_id);
    audit.run_id = run_id.to_string();
    audit.action_type = file_effect_action_type(tool_name).to_string();
    audit.tool_name = tool_name.to_string();
    audit.status = status.to_string();
    audit.decision_source = Some(decision_source.to_string());
    audit.created_at = created_at;
    audit.decided_at = Some(created_at);
    audit.completed_at =
        matches!(status, "completed" | "failed" | "cancelled").then_some(created_at + 1);
    audit
}

fn interrupted_file_effect(
    storage_id: &str,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    tool_name: &str,
    created_at: i64,
) -> AgentPendingActionRecord {
    let mut pending = pending_action(storage_id, conversation_id);
    pending.run_id = run_id.to_string();
    pending.assistant_message_id = Some(assistant_message_id.to_string());
    pending.action_type = file_effect_action_type(tool_name).to_string();
    pending.tool_name = tool_name.to_string();
    pending.tool_call_id = Some(format!("{storage_id}:call"));
    pending.status = "approved".to_string();
    pending.created_at = created_at;
    pending.updated_at = created_at;
    pending
}

#[test]
fn startup_reconciliation_finishes_committed_mcp_rejection_without_replay() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-mcp-rejection-crash";
    let conversation_id = "conversation-mcp-rejection-crash";
    let assistant_message_id = "assistant-mcp-rejection-crash";
    let (pending, pending_audit, terminal_audit, expected_trace, _) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    let action_id = pending.action_id.clone();
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(pending_audit).unwrap();

    assert_eq!(
        service
            .commit_pending_agent_action_audited_result_trace(
                &terminal_audit,
                "pending",
                "rejected",
                &expected_trace,
                12,
            )
            .unwrap(),
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true,
        }
    );

    // Reproduce the exact crash boundary: the rejection receipt is fully committed, but the
    // following pending-status CAS never ran before the process exited.
    let connection = service.state.connection().unwrap();
    let committed_state: (
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(
            "
            SELECT pending.status,
                   pending.target_status,
                   audit.status,
                   audit.decision,
                   audit.error,
                   audit.blocked_reason
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [&action_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(committed_state.0, "pending");
    assert_eq!(committed_state.1.as_deref(), Some("rejected"));
    assert_eq!(committed_state.2, "rejected");
    assert_eq!(committed_state.3.as_deref(), Some("rejected"));
    assert_eq!(committed_state.4, None);
    assert_eq!(committed_state.5, None);
    drop(connection);
    assert_eq!(
        service
            .get_conversation_turn_trace(assistant_message_id)
            .unwrap()
            .as_ref(),
        Some(&expected_trace)
    );

    let reconciled = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0].action_id, action_id);
    assert_eq!(reconciled[0].status, "pending");
    assert_eq!(reconciled[0].target_status.as_deref(), Some("rejected"));

    let connection = service.state.connection().unwrap();
    let recovered_state: (
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        String,
    ) = connection
        .query_row(
            "
            SELECT pending.status,
                   pending.target_status,
                   audit.status,
                   audit.decision,
                   audit.error,
                   audit.tool_result_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            [&action_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(recovered_state.0, "rejected");
    assert_eq!(recovered_state.1.as_deref(), Some("rejected"));
    assert_eq!(recovered_state.2, "rejected");
    assert_eq!(recovered_state.3.as_deref(), Some("rejected"));
    assert_eq!(recovered_state.4, None);
    assert!(!recovered_state.5.contains("outcome_unknown"));
    assert!(!recovered_state.5.contains("mcp.tool_outcome_unknown"));
    drop(connection);

    let recovered_trace = service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(recovered_trace, expected_trace);
    assert_eq!(
        recovered_trace
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
            .count(),
        1,
        "startup reconciliation must not append or replay the rejected MCP call"
    );

    assert!(service
        .reconcile_interrupted_pending_agent_actions(43)
        .unwrap()
        .is_empty());
    assert_eq!(
        service
            .get_conversation_turn_trace(assistant_message_id)
            .unwrap()
            .as_ref(),
        Some(&expected_trace)
    );
}

#[test]
fn mcp_server_tool_error_is_a_completed_authoritative_receipt() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-mcp-tool-error";
    let conversation_id = "conversation-mcp-tool-error";
    let assistant_message_id = "assistant-mcp-tool-error";
    let (mut pending, mut preterminal_audit, _, mut trace, call) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    pending.status = "executing".to_string();
    preterminal_audit.status = "approved".to_string();
    preterminal_audit.decision = Some("approved".to_string());
    preterminal_audit.decided_at = Some(11);
    preterminal_audit.decision_source = Some("manual".to_string());

    let live_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: Some(serde_json::json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "external": true,
            "status": "completed",
            "outcome": "tool_error",
            "dispatchCertainty": "response_received",
            "isError": true,
            "content": [{ "type": "text", "text": "fixture error omitted durably" }]
        })),
        error: Some("fixture server returned isError".to_string()),
    };
    let durable_result = crate::mcp_tool_result_persistence_projection(&live_result);
    let mut terminal_audit = preterminal_audit.clone();
    terminal_audit.status = "completed".to_string();
    terminal_audit.tool_result_json = Some(serde_json::to_string(&durable_result).unwrap());
    terminal_audit.error = durable_result.error.clone();
    terminal_audit.blocked_reason = durable_result.error.clone();
    terminal_audit.completed_at = Some(12);
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &durable_result);

    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service
        .upsert_agent_action_audit(preterminal_audit)
        .unwrap();
    assert_eq!(
        service
            .commit_pending_agent_action_audited_result_trace(
                &terminal_audit,
                "executing",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true,
        }
    );

    let connection = service.state.connection().unwrap();
    let (pending_status, target_status, audit_status, persisted_result): (
        String,
        Option<String>,
        String,
        String,
    ) = connection
        .query_row(
            "
            SELECT pending.status, pending.target_status, audit.status, audit.tool_result_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.run_id = ?1
            ",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(pending_status, "executing");
    assert_eq!(target_status.as_deref(), Some("completed"));
    assert_eq!(audit_status, "completed");
    assert_eq!(
        serde_json::to_value(serde_json::from_str::<AgentToolResult>(&persisted_result).unwrap())
            .unwrap(),
        serde_json::to_value(durable_result).unwrap()
    );
}

#[test]
fn mcp_durable_receipt_rejects_unknown_canary_fields_without_partial_commit() {
    const CANARY: &str = "MCP_NEUTRAL_FIELD_CANARY_MUST_NOT_PERSIST";
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let run_id = "run-mcp-rejection-canary";
    let conversation_id = "conversation-mcp-rejection-canary";
    let assistant_message_id = "assistant-mcp-rejection-canary";
    let (pending, pending_audit, mut terminal_audit, mut trace, call) =
        mcp_rejection_settlement(run_id, conversation_id, assistant_message_id);
    let mut tampered_result = serde_json::from_str::<AgentToolResult>(
        terminal_audit.tool_result_json.as_deref().unwrap(),
    )
    .unwrap();
    tampered_result
        .result
        .as_mut()
        .and_then(serde_json::Value::as_object_mut)
        .unwrap()
        .insert(
            "neutralData".to_string(),
            serde_json::Value::String(CANARY.to_string()),
        );
    terminal_audit.tool_result_json = Some(serde_json::to_string(&tampered_result).unwrap());
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &tampered_result);

    let storage_id = pending.action_id.clone();
    save_assistant_conversation(&service, conversation_id, assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(pending_audit).unwrap();
    let error = service
        .commit_pending_agent_action_audited_result_trace(
            &terminal_audit,
            "pending",
            "rejected",
            &trace,
            12,
        )
        .unwrap_err();
    assert!(error.contains("unknown field"), "{error}");

    let connection = service.state.connection().unwrap();
    let (target_status, audit_status, tool_result_json): (Option<String>, String, Option<String>) =
        connection
            .query_row(
                "
            SELECT pending.target_status, audit.status, audit.tool_result_json
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
    assert_eq!(target_status, None);
    assert_eq!(audit_status, "pending");
    assert_eq!(tool_result_json, None);
    assert_eq!(
        connection
            .query_row(
                "SELECT instr(COALESCE(tool_result_json, ''), ?2)
                 FROM agent_action_audit
                 WHERE action_id = ?1",
                rusqlite::params![storage_id, CANARY],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    drop(connection);
    assert!(service
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .is_none());
}

#[test]
fn unsettled_effects_restore_every_auto_file_producer_but_exclude_terminal_receipts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let cases = [
        (
            "office_document",
            "run-auto-document",
            "run-auto-document:document-call",
            "conversation-auto-document",
        ),
        (
            "skills_run_script",
            "run-auto-script",
            "run-auto-script:script-call",
            "conversation-auto-script",
        ),
        (
            "skills_materialize_resource",
            "run-auto-materialize",
            "run-auto-materialize:materialize-call",
            "conversation-auto-materialize",
        ),
    ];

    for (index, (tool_name, run_id, storage_id, conversation_id)) in cases.iter().enumerate() {
        let assistant_message_id = format!("assistant-{run_id}");
        save_assistant_conversation(&service, conversation_id, &assistant_message_id);
        service
            .upsert_agent_action_audit(file_effect_audit(
                storage_id,
                run_id,
                conversation_id,
                tool_name,
                "executing",
                "auto",
                index as i64 + 1,
            ))
            .unwrap();

        let terminal_storage_id = format!("{storage_id}:terminal");
        service
            .upsert_agent_action_audit(file_effect_audit(
                &terminal_storage_id,
                &format!("{run_id}-terminal"),
                conversation_id,
                tool_name,
                if index % 2 == 0 {
                    "completed"
                } else {
                    "failed"
                },
                "auto",
                index as i64 + 10,
            ))
            .unwrap();
    }

    let mut restored = service
        .list_unsettled_file_effects()
        .unwrap()
        .into_iter()
        .map(|effect| {
            (
                effect.project_id,
                effect.conversation_id,
                effect.run_id,
                effect.action_id,
            )
        })
        .collect::<Vec<_>>();
    restored.sort();

    let mut expected = cases
        .iter()
        .map(|(_, run_id, storage_id, conversation_id)| {
            (
                Some("project-1".to_string()),
                (*conversation_id).to_string(),
                (*run_id).to_string(),
                (*storage_id).to_string(),
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(restored, expected);
}

#[test]
fn startup_reconciliation_conservatively_restores_interrupted_manual_file_effects() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let cases = [
        (
            "office_document",
            "run-manual-document",
            "run-manual-document:document-call",
            "conversation-manual-document",
            "assistant-manual-document",
        ),
        (
            "skills_run_script",
            "run-manual-script",
            "run-manual-script:script-call",
            "conversation-manual-script",
            "assistant-manual-script",
        ),
    ];

    for (index, (tool_name, run_id, storage_id, conversation_id, assistant_message_id)) in
        cases.iter().enumerate()
    {
        save_assistant_conversation(&service, conversation_id, assistant_message_id);
        service
            .store_pending_agent_action(interrupted_file_effect(
                storage_id,
                run_id,
                conversation_id,
                assistant_message_id,
                tool_name,
                index as i64 + 1,
            ))
            .unwrap();
        service
            .upsert_agent_action_audit(file_effect_audit(
                storage_id,
                run_id,
                conversation_id,
                tool_name,
                "approved",
                "manual",
                index as i64 + 1,
            ))
            .unwrap();
    }

    let interrupted = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(interrupted.len(), cases.len());

    let mut restored = service
        .list_unsettled_file_effects()
        .unwrap()
        .into_iter()
        .map(|effect| {
            (
                effect.project_id,
                effect.conversation_id,
                effect.run_id,
                effect.action_id,
            )
        })
        .collect::<Vec<_>>();
    restored.sort();
    let mut expected = cases
        .iter()
        .map(|(_, run_id, storage_id, conversation_id, _)| {
            (
                Some("project-1".to_string()),
                (*conversation_id).to_string(),
                (*run_id).to_string(),
                (*storage_id).to_string(),
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(restored, expected);

    let connection = service.state.connection().unwrap();
    for (_, _, storage_id, _, _) in cases {
        let state: (String, Option<String>) = connection
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, ("failed".to_string(), Some("failed".to_string())));
    }
}

#[test]
fn unsettled_file_effects_preserve_conversation_scope_without_a_project() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    save_assistant_conversation_in_scope(
        &service,
        "conversation-unscoped-auto",
        "assistant-unscoped-auto",
        None,
    );
    service
        .upsert_agent_action_audit(file_effect_audit(
            "run-unscoped-auto:auto-call",
            "run-unscoped-auto",
            "conversation-unscoped-auto",
            "office_spreadsheet",
            "executing",
            "auto",
            1,
        ))
        .unwrap();

    save_assistant_conversation_in_scope(
        &service,
        "conversation-unscoped-manual",
        "assistant-unscoped-manual",
        None,
    );
    service
        .store_pending_agent_action(interrupted_file_effect(
            "run-unscoped-manual:manual-call",
            "run-unscoped-manual",
            "conversation-unscoped-manual",
            "assistant-unscoped-manual",
            "skills_run_script",
            2,
        ))
        .unwrap();
    service
        .upsert_agent_action_audit(file_effect_audit(
            "run-unscoped-manual:manual-call",
            "run-unscoped-manual",
            "conversation-unscoped-manual",
            "skills_run_script",
            "approved",
            "manual",
            2,
        ))
        .unwrap();

    let interrupted = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(interrupted.len(), 1);

    let mut restored = service.list_unsettled_file_effects().unwrap();
    restored.sort_by(|left, right| left.action_id.cmp(&right.action_id));
    assert_eq!(
        restored,
        vec![
            AgentUnsettledFileEffect {
                project_id: None,
                conversation_id: "conversation-unscoped-auto".to_string(),
                run_id: "run-unscoped-auto".to_string(),
                action_id: "run-unscoped-auto:auto-call".to_string(),
            },
            AgentUnsettledFileEffect {
                project_id: None,
                conversation_id: "conversation-unscoped-manual".to_string(),
                run_id: "run-unscoped-manual".to_string(),
                action_id: "run-unscoped-manual:manual-call".to_string(),
            },
        ]
    );
}

#[test]
fn startup_reconciliation_excludes_authoritatively_settled_manual_file_effects() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (mut command_pending, command_approved, command_terminal, command_trace) =
        manual_command_settlement(
            "run-1:settled-command",
            "settled-command",
            "conversation-settled-command",
            "assistant-settled-command",
        );
    attach_manual_file_effect_recovery_checkpoint(&mut command_pending, &command_trace);
    save_assistant_conversation(
        &service,
        "conversation-settled-command",
        "assistant-settled-command",
    );
    service.store_pending_agent_action(command_pending).unwrap();
    service.upsert_agent_action_audit(command_approved).unwrap();
    let exact_model_items = vec![
        crate::ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "settled-command".to_string(),
                name: "run_command".to_string(),
                args: match &command_trace.items[0] {
                    ConversationTurnTraceItem::ToolCall { operation, .. } => operation.clone(),
                    _ => panic!("manual settlement must start with a ToolCall"),
                },
                provider_identity: None,
            }],
            is_error: false,
        },
        crate::ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: r#"{"ok":true,"result":{"stdout":"EXACT_APPROVAL_RESULT"}}"#.to_string(),
            tool_call_id: Some("settled-command".to_string()),
            tool_calls: Vec::new(),
            is_error: false,
        },
    ];
    service
        .commit_pending_agent_action_audited_result_trace_with_model_context(
            &command_terminal,
            "approved",
            "completed",
            &command_trace,
            &exact_model_items,
            12,
        )
        .unwrap();
    assert_eq!(
        service
            .get_conversation_model_context_log("assistant-settled-command")
            .unwrap()
            .unwrap()
            .items,
        exact_model_items
    );

    let mut expected_action_ids = vec!["run-1:settled-command".to_string()];
    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-settled-{label}:{label}-call");
        let call_id = format!("settled-{label}-call");
        let run_id = format!("run-settled-{label}");
        let conversation_id = format!("conversation-settled-{label}");
        let assistant_message_id = format!("assistant-settled-{label}");
        let (mut pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &storage_id,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        attach_manual_file_effect_recovery_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap();
        expected_action_ids.push(storage_id);
    }

    assert_eq!(
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        expected_action_ids.len()
    );
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());
    let connection = service.state.connection().unwrap();
    for action_id in expected_action_ids {
        let state: (String, Option<String>) = connection
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&action_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("completed".to_string(), Some("completed".to_string()))
        );
    }
    drop(connection);
    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert!(reopened.list_unsettled_file_effects().unwrap().is_empty());
    assert!(reopened
        .get_conversation_model_context_log("assistant-settled-command")
        .unwrap()
        .unwrap()
        .items[1]
        .content
        .contains("EXACT_APPROVAL_RESULT"));
}

fn attach_manual_file_effect_recovery_checkpoint(
    pending: &mut AgentPendingActionRecord,
    trace: &ConversationTurnTrace,
) {
    let (call_id, tool, operation) = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                ..
            } => Some((call_id.clone(), tool.clone(), operation.clone())),
            _ => None,
        })
        .expect("manual file-effect settlement trace has a ToolCall");
    let checkpoint_call = ConversationTurnTraceItem::ToolCall {
        sequence: 0,
        call_id: call_id.clone(),
        tool,
        provenance: None,
        operation,
        approval_status: crate::AgentApprovalStatus::Required,
        truncated: false,
    };
    pending.agent_input_json = serde_json::json!({
        "apiUrl": "https://not-used.invalid/v1",
        "apiToken": "redacted",
        "model": "reconciliation-test",
        "messages": [],
        "resumeCheckpoint": {
            // Startup reconciliation must inspect this legacy v5 projection without treating it
            // as a resumable v6 checkpoint. True approval restore remains strict in core-server.
            "version": 5,
            "runId": pending.run_id,
            "contextItems": [],
            "nextModelRequestIndex": 1,
            "queuedToolCalls": [],
            "suppressedNarration": false,
            "extensionSnapshots": [],
            "pendingToolCallId": call_id,
            "conversationTraceItems": [checkpoint_call],
            "nextConversationTraceSequence": 1,
            "conversationTraceTruncated": false,
            "modelVisibleTraceItemCount": 0,
            "toolSet": test_checkpoint_tool_set(),
            "runContext": null,
            "modelCapabilities": { "imageInput": false },
            "runWorldState": test_checkpoint_run_world_state()
        }
    })
    .to_string();
}

fn test_checkpoint_tool_set() -> serde_json::Value {
    serde_json::json!({
        "stableRevision": "stable-tool-set-test-v1",
        "dynamicRevision": "dynamic-tool-set-test-v1",
        "effectiveRevision": "effective-tool-set-test-v1",
        "exposedToolNames": []
    })
}

fn test_checkpoint_run_world_state() -> serde_json::Value {
    let snapshot = crate::WorldStateSnapshot::new(
        "reconciliation-test-run-world-state",
        0,
        vec![crate::WorldStateSectionEnvelope::host_only(
            crate::WorldStateSectionId::ModelCapabilities,
            crate::WorldStateLifetime::Run,
            serde_json::json!({ "imageInput": false }),
        )
        .unwrap()],
    )
    .unwrap();
    serde_json::to_value(snapshot).unwrap()
}

#[test]
fn every_manual_non_command_file_effect_settles_atomically_and_idempotently() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-{label}:{label}-call");
        let call_id = format!("{label}-call");
        let run_id = format!("run-{label}");
        let conversation_id = format!("conversation-{label}");
        let assistant_message_id = format!("assistant-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &storage_id,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        let terminal_tool_result_json = terminal.tool_result_json.clone().unwrap();
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let outcome = service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap();
        assert_eq!(
            outcome,
            AgentPendingActionResultCommitOutcome::Committed {
                trace_changed: true
            },
            "{label} did not commit all durable records"
        );

        let connection = service.state.connection().unwrap();
        let state: CommittedManualFileEffectState = connection
            .query_row(
                "
                SELECT pending.target_status,
                       audit.status,
                       audit.command_result_json,
                       audit.tool_result_json,
                       audit.action_json,
                       pending.tool_name,
                       pending.tool_call_id
                FROM agent_pending_actions pending
                JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                WHERE pending.action_id = ?1
                ",
                [&storage_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state.0.as_deref(), Some("completed"));
        assert_eq!(state.1, "completed");
        assert_eq!(
            state.2, None,
            "{label} must not populate command_result_json"
        );
        assert_eq!(state.3.as_deref(), Some(terminal_tool_result_json.as_str()));
        assert_eq!(state.4, frozen_action_json);
        assert_eq!(state.5, effect.tool_name());
        assert_eq!(state.6.as_deref(), Some(call_id.as_str()));
        drop(connection);
        assert_eq!(
            service
                .get_conversation_turn_trace(&assistant_message_id)
                .unwrap()
                .as_ref(),
            Some(&trace)
        );

        let retry = service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap();
        assert_eq!(
            retry,
            AgentPendingActionResultCommitOutcome::Idempotent,
            "{label} exact retry was not idempotent"
        );
    }
}

#[test]
fn every_manual_non_command_trace_failure_rolls_back_audit_and_pending_target() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-rollback-{label}:{label}-call");
        let call_id = format!("rollback-{label}-call");
        let run_id = format!("run-rollback-{label}");
        let conversation_id = format!("missing-conversation-{label}");
        let assistant_message_id = format!("missing-assistant-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &storage_id,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let error = service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("assistant message does not exist"),
            "unexpected {label} trace failure: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );
    }
}

#[test]
fn manual_non_command_settlement_rejects_command_result_json_without_partial_state() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-command-column-{label}:{label}-call");
        let call_id = format!("command-column-{label}-call");
        let run_id = format!("run-command-column-{label}");
        let conversation_id = format!("conversation-command-column-{label}");
        let assistant_message_id = format!("assistant-command-column-{label}");
        let (pending, approved, mut terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &storage_id,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        terminal.command_result_json = Some("{}".to_string());
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let error = service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("non-command manual file-effect audit unexpectedly contains"),
            "unexpected {label} command-result validation error: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );
    }
}

#[test]
fn manual_non_command_settlement_strictly_binds_frozen_action_tool_and_call_identity() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-identity-{label}:{label}-call");
        let call_id = format!("identity-{label}-call");
        let run_id = format!("run-identity-{label}");
        let conversation_id = format!("conversation-identity-{label}");
        let assistant_message_id = format!("assistant-identity-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &storage_id,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let frozen_action_json = pending.action_json.clone();
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();

        let mut different_action = terminal.clone();
        different_action.action_json =
            serde_json::to_string(&effect.frozen_action(&call_id, true)).unwrap();
        let error = service
            .commit_pending_agent_action_audited_result_trace(
                &different_action,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("does not match the frozen pending action"),
            "unexpected {label} frozen-action conflict: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );

        let mut different_tool = terminal.clone();
        different_tool.tool_name = "run_command".to_string();
        let error = service
            .commit_pending_agent_action_audited_result_trace(
                &different_tool,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("audit type does not match the frozen action"),
            "unexpected {label} tool conflict: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );

        let different_call_id = format!("different-{call_id}");
        let mut different_call = terminal.clone();
        different_call.action_json =
            serde_json::to_string(&effect.frozen_action(&different_call_id, false)).unwrap();
        let mut different_call_tool_result = serde_json::from_str::<AgentToolResult>(
            different_call.tool_result_json.as_deref().unwrap(),
        )
        .unwrap();
        different_call_tool_result.call_id = different_call_id.clone();
        different_call.tool_result_json =
            Some(serde_json::to_string(&different_call_tool_result).unwrap());
        let mut different_call_trace = trace.clone();
        for item in &mut different_call_trace.items {
            match item {
                ConversationTurnTraceItem::ToolCall { call_id, .. }
                | ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                    *call_id = different_call_id.clone();
                }
                _ => {}
            }
        }
        let error = service
            .commit_pending_agent_action_audited_result_trace(
                &different_call,
                "approved",
                "completed",
                &different_call_trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("does not match the frozen pending action"),
            "unexpected {label} call identity conflict: {error}"
        );
        assert_manual_file_effect_is_uncommitted(
            &service,
            &storage_id,
            &assistant_message_id,
            &frozen_action_json,
        );
    }
}

#[test]
fn manual_non_command_terminal_conflict_preserves_first_atomic_settlement() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-terminal-conflict-{label}:{label}-call");
        let call_id = format!("terminal-conflict-{label}-call");
        let run_id = format!("run-terminal-conflict-{label}");
        let conversation_id = format!("conversation-terminal-conflict-{label}");
        let assistant_message_id = format!("assistant-terminal-conflict-{label}");
        let (pending, approved, terminal, trace) = manual_non_command_file_effect_settlement(
            effect,
            &storage_id,
            &call_id,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );
        let original_tool_result_json = terminal.tool_result_json.clone().unwrap();
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap();

        let mut conflicting = terminal.clone();
        let mut conflicting_tool_result = serde_json::from_str::<AgentToolResult>(
            conflicting.tool_result_json.as_deref().unwrap(),
        )
        .unwrap();
        conflicting_tool_result.result = Some(serde_json::json!({
            "status": "applied",
            "effect": label,
            "conflicting": true,
        }));
        conflicting.tool_result_json =
            Some(serde_json::to_string(&conflicting_tool_result).unwrap());
        let error = service
            .commit_pending_agent_action_audited_result_trace(
                &conflicting,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap_err();
        assert!(
            error.contains("different terminal result"),
            "unexpected {label} terminal conflict: {error}"
        );

        let connection = service.state.connection().unwrap();
        let persisted: (Option<String>, String, Option<String>, Option<String>) = connection
            .query_row(
                "
                SELECT pending.target_status,
                       audit.status,
                       audit.command_result_json,
                       audit.tool_result_json
                FROM agent_pending_actions pending
                JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                WHERE pending.action_id = ?1
                ",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(persisted.0.as_deref(), Some("completed"));
        assert_eq!(persisted.1, "completed");
        assert_eq!(persisted.2, None);
        assert_eq!(
            persisted.3.as_deref(),
            Some(original_tool_result_json.as_str())
        );
        drop(connection);
        assert_eq!(
            service
                .get_conversation_turn_trace(&assistant_message_id)
                .unwrap()
                .as_ref(),
            Some(&trace)
        );
        assert_eq!(
            service
                .commit_pending_agent_action_audited_result_trace(
                    &terminal,
                    "approved",
                    "completed",
                    &trace,
                    12,
                )
                .unwrap(),
            AgentPendingActionResultCommitOutcome::Idempotent
        );
    }
}

#[test]
fn manual_command_audit_target_and_trace_commit_as_one_idempotent_transaction() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "run-1:manual-command",
        "manual-command",
        "conversation-manual-command",
        "assistant-manual-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-manual-command",
        "assistant-manual-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();

    let outcome = service
        .commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap();
    assert_eq!(
        outcome,
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true
        }
    );

    let connection = service.state.connection().unwrap();
    let (target_status, audit_status, decided_at, completed_at): (
        Option<String>,
        String,
        Option<i64>,
        Option<i64>,
    ) = connection
        .query_row(
            "
            SELECT pending.target_status, audit.status, audit.decided_at, audit.completed_at
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            ["run-1:manual-command"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(target_status.as_deref(), Some("completed"));
    assert_eq!(audit_status, "completed");
    assert_eq!(decided_at, Some(11), "approval timestamp must be preserved");
    assert_eq!(completed_at, Some(12));
    drop(connection);

    let retry = service
        .commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap();
    assert_eq!(retry, AgentPendingActionResultCommitOutcome::Idempotent);
}

#[test]
fn manual_command_audit_failure_wrapper_preserves_the_exact_execution_evidence() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, mut terminal, mut trace) = manual_command_settlement(
        "run-1:audit-wrapper-command",
        "audit-wrapper-command",
        "conversation-audit-wrapper-command",
        "assistant-audit-wrapper-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-audit-wrapper-command",
        "assistant-audit-wrapper-command",
    );
    let command_result = serde_json::from_str::<AgentCommandExecutionResult>(
        terminal.command_result_json.as_deref().unwrap(),
    )
    .unwrap();
    let message = "The command finished, but its final action audit could not be persisted. Inspect the observed artifacts before retrying.";
    let fallback = AgentToolResult {
        exact_archive_file: None,
        call_id: "audit-wrapper-command".to_string(),
        tool: "run_command".to_string(),
        ok: false,
        result: Some(serde_json::json!({
            "type": "command_execution",
            "code": "auditPersistenceFailed",
            "recovery": "inspectArtifacts",
            "phase": "afterExecution",
            "executionAttempted": true,
            "effectsMayHaveOccurred": true,
            "auditError": "simulated first-commit failure",
            "execution": command_result,
        })),
        error: Some(message.to_string()),
    };
    terminal.status = "failed".to_string();
    terminal.tool_result_json = Some(serde_json::to_string(&fallback).unwrap());
    terminal.error = fallback.error.clone();
    let call = AgentToolCall {
        id: "audit-wrapper-command".to_string(),
        tool: "run_command".to_string(),
        args: serde_json::json!({
            "command": "node script.mjs",
            "timeoutMs": 5_000,
            "reason": "test atomic settlement",
        }),
        approval_status: crate::AgentApprovalStatus::Approved,
        reason: Some("test atomic settlement".to_string()),
    };
    trace.items[1] =
        crate::conversation_trace::projected_tool_result_trace_item(1, &call, &fallback);

    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    service
        .commit_pending_agent_action_audited_result_trace(
            &terminal, "approved", "failed", &trace, 12,
        )
        .unwrap();
    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());
}

#[test]
fn manual_command_settlement_inspection_distinguishes_commit_boundaries() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "run-1:inspect-command",
        "inspect-command",
        "conversation-inspect-command",
        "assistant-inspect-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-inspect-command",
        "assistant-inspect-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();

    assert_eq!(
        service
            .inspect_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::DefinitelyUncommitted
    );

    service
        .commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap();
    assert_eq!(
        service
            .inspect_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::CommittedAtBoundary
    );

    let mut advanced = trace.clone();
    advanced
        .items
        .push(ConversationTurnTraceItem::AssistantNarration {
            sequence: 2,
            content: "continued".to_string(),
            truncated: false,
        });
    service
        .append_in_progress_conversation_turn_trace(&advanced, 10, 13)
        .unwrap();
    assert_eq!(
        service
            .inspect_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap(),
        AgentPendingActionSettlementInspection::CommittedAndAdvanced
    );

    let mut conflicting_terminal = terminal;
    conflicting_terminal.command_result_json = conflicting_terminal
        .command_result_json
        .map(|json| json.replace("created workbook", "different output"));
    let error = service
        .inspect_pending_agent_action_audited_result_trace(
            &conflicting_terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap_err();
    assert!(error.contains("durable command execution result"));
}

#[test]
fn manual_command_trace_failure_rolls_back_audit_and_target() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "run-1:rollback-command",
        "rollback-command",
        "conversation-without-message",
        "assistant-without-message",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();

    let error = service
        .commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap_err();
    assert!(error.contains("assistant message does not exist"));

    let connection = service.state.connection().unwrap();
    let (target_status, audit_status): (Option<String>, String) = connection
        .query_row(
            "
            SELECT pending.target_status, audit.status
            FROM agent_pending_actions pending
            JOIN agent_action_audit audit ON audit.action_id = pending.action_id
            WHERE pending.action_id = ?1
            ",
            ["run-1:rollback-command"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(target_status, None);
    assert_eq!(audit_status, "approved");
}

#[test]
fn manual_command_settlement_repairs_a_compatible_legacy_partial_commit() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, _approved, terminal, trace) = manual_command_settlement(
        "run-1:partial-command",
        "partial-command",
        "conversation-partial-command",
        "assistant-partial-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-partial-command",
        "assistant-partial-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    // Reproduce a row created by the former split-commit implementation: the exact terminal
    // audit exists, while target_status and the paired trace were never committed.
    service.upsert_agent_action_audit(terminal.clone()).unwrap();

    let outcome = service
        .commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap();
    assert_eq!(
        outcome,
        AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true
        }
    );
    assert!(service
        .get_conversation_turn_trace("assistant-partial-command")
        .unwrap()
        .is_some());
}

#[test]
fn manual_command_terminal_retry_with_different_result_conflicts_without_overwrite() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "run-1:conflict-command",
        "conflict-command",
        "conversation-conflict-command",
        "assistant-conflict-command",
    );
    save_assistant_conversation(
        &service,
        "conversation-conflict-command",
        "assistant-conflict-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    service
        .commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap();

    let mut conflicting = terminal.clone();
    let mut command_result = serde_json::from_str::<AgentCommandExecutionResult>(
        conflicting.command_result_json.as_deref().unwrap(),
    )
    .unwrap();
    command_result.stdout = "different output".to_string();
    conflicting.command_result_json = Some(serde_json::to_string(&command_result).unwrap());
    let error = service
        .commit_pending_agent_action_audited_result_trace(
            &conflicting,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap_err();
    assert!(error.contains("durable command execution result"));

    let persisted = service.list_agent_command_results_for_run("run-1").unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].stdout, "created workbook");
}

#[test]
fn concurrent_manual_command_settlement_is_exactly_once_across_storage_instances() {
    let fixture = StorageFixture::new();
    let first = fixture.service();
    let (pending, approved, terminal, trace) = manual_command_settlement(
        "run-1:concurrent-command",
        "concurrent-command",
        "conversation-concurrent-command",
        "assistant-concurrent-command",
    );
    save_assistant_conversation(
        &first,
        "conversation-concurrent-command",
        "assistant-concurrent-command",
    );
    first.store_pending_agent_action(pending).unwrap();
    first.upsert_agent_action_audit(approved).unwrap();
    let second = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

    let first_barrier = std::sync::Arc::clone(&barrier);
    let first_terminal = terminal.clone();
    let first_trace = trace.clone();
    let first_thread = std::thread::spawn(move || {
        first_barrier.wait();
        first.commit_pending_agent_action_audited_result_trace(
            &first_terminal,
            "approved",
            "completed",
            &first_trace,
            12,
        )
    });
    let second_barrier = std::sync::Arc::clone(&barrier);
    let second_thread = std::thread::spawn(move || {
        second_barrier.wait();
        second.commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
    });

    let outcomes = [
        first_thread.join().unwrap().unwrap(),
        second_thread.join().unwrap().unwrap(),
    ];
    assert!(
        outcomes.contains(&AgentPendingActionResultCommitOutcome::Committed {
            trace_changed: true,
        })
    );
    assert!(outcomes.contains(&AgentPendingActionResultCommitOutcome::Idempotent));
}

#[test]
fn startup_reconciliation_recovers_every_legacy_manual_non_command_split_commit() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-legacy-{label}:{label}-call");
        let call_id = format!("legacy-{label}-call");
        let run_id = format!("run-legacy-{label}");
        let conversation_id = format!("conversation-legacy-{label}");
        let assistant_message_id = format!("assistant-legacy-{label}");
        let (mut pending, _approved, terminal, expected_trace) =
            manual_non_command_file_effect_settlement(
                effect,
                &storage_id,
                &call_id,
                &run_id,
                &conversation_id,
                &assistant_message_id,
            );
        if matches!(effect, ManualNonCommandFileEffect::SkillMaterialization) {
            // Older materialization execution persisted `executing` before publishing its audit.
            pending.status = "executing".to_string();
        }
        attach_manual_file_effect_recovery_checkpoint(&mut pending, &expected_trace);
        let expected_pending_status = pending.status.clone();
        let frozen_action_json = pending.action_json.clone();
        let terminal_tool_result_json = terminal.tool_result_json.clone().unwrap();
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        // Reproduce the former split boundary: the execution receipt is durable, while neither
        // the pending target nor its paired trace has been published.
        service.upsert_agent_action_audit(terminal.clone()).unwrap();

        let interrupted = service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap_or_else(|error| {
                panic!(
                    "{label} legacy split recovery failed from pending status {expected_pending_status}: {error}"
                )
            });
        assert_eq!(interrupted.len(), 1);

        let connection = service.state.connection().unwrap();
        let state: LegacyManualFileEffectSplitState = connection
            .query_row(
                "
                SELECT pending.status,
                       pending.target_status,
                       audit.status,
                       audit.command_result_json,
                       audit.tool_result_json,
                       audit.action_json
                FROM agent_pending_actions pending
                JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                WHERE pending.action_id = ?1
                ",
                [&storage_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state.0, "completed");
        assert_eq!(state.1.as_deref(), Some("completed"));
        assert_eq!(state.2, "completed");
        assert_eq!(state.3, None, "{label} populated command_result_json");
        assert_eq!(state.4.as_deref(), Some(terminal_tool_result_json.as_str()));
        assert_eq!(state.5, frozen_action_json);
        drop(connection);

        let recovered_trace = service
            .get_conversation_turn_trace(&assistant_message_id)
            .unwrap()
            .unwrap();
        recovered_trace.validate().unwrap();
        assert_eq!(recovered_trace, expected_trace);
        assert!(matches!(
            recovered_trace.items.last(),
            Some(ConversationTurnTraceItem::ToolResult {
                call_id: recovered_call_id,
                tool,
                success: true,
                observation,
                ..
            }) if recovered_call_id == &call_id
                && tool == effect.tool_name()
                && observation["effect"] == label
        ));

        // A repaired row is terminal, so a later startup pass neither reconstructs nor appends
        // the ToolResult a second time.
        assert!(service
            .reconcile_interrupted_pending_agent_actions(43)
            .unwrap()
            .is_empty());
        assert_eq!(
            service
                .get_conversation_turn_trace(&assistant_message_id)
                .unwrap()
                .as_ref(),
            Some(&expected_trace)
        );
    }
}

#[test]
fn startup_reconciliation_repairs_non_command_trace_for_an_exact_preexisting_target() {
    let fixture = StorageFixture::new();
    let service = fixture.service();

    for effect in ManualNonCommandFileEffect::ALL {
        let label = effect.label();
        let storage_id = format!("run-legacy-target-{label}:{label}-call");
        let call_id = format!("legacy-target-{label}-call");
        let run_id = format!("run-legacy-target-{label}");
        let conversation_id = format!("conversation-legacy-target-{label}");
        let assistant_message_id = format!("assistant-legacy-target-{label}");
        let (mut pending, _approved, terminal, expected_trace) =
            manual_non_command_file_effect_settlement(
                effect,
                &storage_id,
                &call_id,
                &run_id,
                &conversation_id,
                &assistant_message_id,
            );
        if matches!(effect, ManualNonCommandFileEffect::SkillMaterialization) {
            pending.status = "executing".to_string();
        }
        pending.target_status = Some("completed".to_string());
        attach_manual_file_effect_recovery_checkpoint(&mut pending, &expected_trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(terminal).unwrap();

        assert_eq!(
            service
                .reconcile_interrupted_pending_agent_actions(42)
                .unwrap()
                .len(),
            1
        );
        let connection = service.state.connection().unwrap();
        let state: (String, Option<String>) = connection
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            state,
            ("completed".to_string(), Some("completed".to_string()))
        );
        drop(connection);
        assert_eq!(
            service
                .get_conversation_turn_trace(&assistant_message_id)
                .unwrap()
                .as_ref(),
            Some(&expected_trace),
            "{label} did not recover its exact paired ToolResult trace"
        );
    }
}

#[test]
fn startup_reconciliation_rejects_tampered_non_command_checkpoint_without_partial_repair() {
    for effect in ManualNonCommandFileEffect::ALL {
        for tamper_tool_identity in [false, true] {
            let fixture = StorageFixture::new();
            let service = fixture.service();
            let label = effect.label();
            let tamper_label = if tamper_tool_identity { "tool" } else { "args" };
            let storage_id = format!("run-tampered-{tamper_label}-{label}:{label}-call");
            let call_id = format!("tampered-{tamper_label}-{label}-call");
            let run_id = format!("run-tampered-{tamper_label}-{label}");
            let conversation_id = format!("conversation-tampered-{tamper_label}-{label}");
            let assistant_message_id = format!("assistant-tampered-{tamper_label}-{label}");
            let (mut pending, _approved, terminal, mut checkpoint_trace) =
                manual_non_command_file_effect_settlement(
                    effect,
                    &storage_id,
                    &call_id,
                    &run_id,
                    &conversation_id,
                    &assistant_message_id,
                );
            if matches!(effect, ManualNonCommandFileEffect::SkillMaterialization) {
                pending.status = "executing".to_string();
            }
            let checkpoint_call = checkpoint_trace
                .items
                .iter_mut()
                .find_map(|item| match item {
                    ConversationTurnTraceItem::ToolCall {
                        tool, operation, ..
                    } => Some((tool, operation)),
                    _ => None,
                })
                .unwrap();
            if tamper_tool_identity {
                *checkpoint_call.0 = "run_command".to_string();
            } else {
                *checkpoint_call.1 = serde_json::json!({
                    "tampered": true,
                    "originalEffect": label,
                });
            }
            attach_manual_file_effect_recovery_checkpoint(&mut pending, &checkpoint_trace);
            let expected_pending_status = pending.status.clone();
            let frozen_action_json = pending.action_json.clone();
            let terminal_tool_result_json = terminal.tool_result_json.clone().unwrap();
            save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
            service.store_pending_agent_action(pending).unwrap();
            service.upsert_agent_action_audit(terminal).unwrap();

            let error = service
                .reconcile_interrupted_pending_agent_actions(42)
                .unwrap_err();
            let expected_error = if tamper_tool_identity {
                "冻结 ToolCall 名称不一致"
            } else {
                "冻结 ToolCall 参数与 action 不一致"
            };
            assert!(
                error.contains(expected_error),
                "unexpected {label} {tamper_label} tamper error: {error}"
            );
            assert_legacy_manual_file_effect_remains_split(
                &service,
                &storage_id,
                &assistant_message_id,
                &expected_pending_status,
                &terminal_tool_result_json,
                &frozen_action_json,
            );
        }
    }
}

#[test]
fn startup_reconciliation_persists_blockers_for_unverifiable_legacy_manual_targets() {
    for effect in ManualNonCommandFileEffect::ALL {
        for target_status in ["completed", "cancelled"] {
            for audit_shape in LegacyManualAuditShape::ALL {
                let fixture = StorageFixture::new();
                let service = fixture.service();
                let label = effect.label();
                let audit_label = audit_shape.label();
                let storage_id =
                    format!("run-unverifiable-{target_status}-{audit_label}-{label}:{label}-call");
                let call_id = format!("unverifiable-{target_status}-{audit_label}-{label}-call");
                let run_id = format!("run-unverifiable-{target_status}-{audit_label}-{label}");
                let conversation_id =
                    format!("conversation-unverifiable-{target_status}-{audit_label}-{label}");
                let assistant_message_id =
                    format!("assistant-unverifiable-{target_status}-{audit_label}-{label}");
                let (mut pending, approved, terminal, expected_trace) =
                    manual_non_command_file_effect_settlement(
                        effect,
                        &storage_id,
                        &call_id,
                        &run_id,
                        &conversation_id,
                        &assistant_message_id,
                    );
                if matches!(effect, ManualNonCommandFileEffect::SkillMaterialization) {
                    pending.status = "executing".to_string();
                }
                pending.target_status = Some(target_status.to_string());
                attach_manual_file_effect_recovery_checkpoint(&mut pending, &expected_trace);
                let expected_audit = match audit_shape {
                    LegacyManualAuditShape::Missing => None,
                    LegacyManualAuditShape::Preterminal => Some(approved),
                    LegacyManualAuditShape::TamperedTerminal => {
                        let mut tampered = terminal;
                        tampered.action_json =
                            serde_json::to_string(&effect.frozen_action(&call_id, true)).unwrap();
                        Some(tampered)
                    }
                };
                let expected_audit_status =
                    expected_audit.as_ref().map(|audit| audit.status.clone());
                let expected_tool_result_json = expected_audit
                    .as_ref()
                    .and_then(|audit| audit.tool_result_json.clone());
                let expected_action_json = expected_audit
                    .as_ref()
                    .map(|audit| audit.action_json.clone());
                save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
                service.store_pending_agent_action(pending).unwrap();
                if let Some(audit) = expected_audit {
                    service.upsert_agent_action_audit(audit).unwrap();
                }

                let interrupted = service
                    .reconcile_interrupted_pending_agent_actions(42)
                    .unwrap_or_else(|error| {
                        panic!(
                            "{label} {target_status} {audit_label} should retire into a durable blocker: {error}"
                        )
                    });
                assert_eq!(interrupted.len(), 1);

                let connection = service.state.connection().unwrap();
                let state: UnverifiableLegacyManualFileEffectState = connection
                    .query_row(
                        "
                        SELECT pending.status,
                               pending.target_status,
                               audit.status,
                               audit.tool_result_json,
                               audit.action_json
                        FROM agent_pending_actions pending
                        LEFT JOIN agent_action_audit audit
                               ON audit.action_id = pending.action_id
                        WHERE pending.action_id = ?1
                        ",
                        [&storage_id],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                            ))
                        },
                    )
                    .unwrap();
                assert_eq!(state.0, target_status);
                assert_eq!(state.1.as_deref(), Some(target_status));
                assert_eq!(state.2, expected_audit_status);
                assert_eq!(state.3, expected_tool_result_json);
                assert_eq!(state.4, expected_action_json);
                drop(connection);
                assert!(service
                    .get_conversation_turn_trace(&assistant_message_id)
                    .unwrap()
                    .is_none());
                let expected_blockers = vec![AgentUnsettledFileEffect {
                    project_id: Some("project-1".to_string()),
                    conversation_id: conversation_id.clone(),
                    run_id: run_id.clone(),
                    action_id: storage_id.clone(),
                }];
                assert_eq!(
                    service.list_unsettled_file_effects().unwrap(),
                    expected_blockers,
                    "{label} {target_status} {audit_label} lost its durable file-effect blocker"
                );
                assert!(service
                    .reconcile_interrupted_pending_agent_actions(43)
                    .unwrap()
                    .is_empty());
                let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
                assert_eq!(
                    reopened.list_unsettled_file_effects().unwrap(),
                    expected_blockers,
                    "{label} {target_status} {audit_label} blocker did not survive restart"
                );
            }
        }
    }
}

#[test]
fn startup_reconciliation_persists_command_blockers_for_unverifiable_legacy_targets() {
    for target_status in ["completed", "cancelled"] {
        for audit_shape in LegacyManualAuditShape::ALL {
            let fixture = StorageFixture::new();
            let service = fixture.service();
            let audit_label = audit_shape.label();
            let storage_id = format!("run-command-{target_status}-{audit_label}:command-call");
            let call_id = format!("command-{target_status}-{audit_label}-call");
            let conversation_id = format!("conversation-command-{target_status}-{audit_label}");
            let assistant_message_id = format!("assistant-command-{target_status}-{audit_label}");
            let (mut pending, approved, terminal, expected_trace) = manual_command_settlement(
                &storage_id,
                &call_id,
                &conversation_id,
                &assistant_message_id,
            );
            pending.target_status = Some(target_status.to_string());
            attach_manual_file_effect_recovery_checkpoint(&mut pending, &expected_trace);
            let expected_audit = match audit_shape {
                LegacyManualAuditShape::Missing => None,
                LegacyManualAuditShape::Preterminal => Some(approved),
                LegacyManualAuditShape::TamperedTerminal => {
                    let mut tampered = terminal;
                    let mut action =
                        serde_json::from_str::<AgentProposedAction>(&tampered.action_json).unwrap();
                    let AgentProposedAction::Command { command } = &mut action else {
                        unreachable!("command settlement must freeze a command action");
                    };
                    command.command = "node another-script.mjs".to_string();
                    tampered.action_json = serde_json::to_string(&action).unwrap();
                    Some(tampered)
                }
            };
            save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
            service.store_pending_agent_action(pending).unwrap();
            if let Some(audit) = expected_audit {
                service.upsert_agent_action_audit(audit).unwrap();
            }

            assert_eq!(
                service
                    .reconcile_interrupted_pending_agent_actions(42)
                    .unwrap()
                    .len(),
                1
            );
            let expected_blocker = AgentUnsettledFileEffect {
                project_id: Some("project-1".to_string()),
                conversation_id: conversation_id.clone(),
                run_id: "run-1".to_string(),
                action_id: storage_id.clone(),
            };
            assert_eq!(
                service.list_unsettled_file_effects().unwrap(),
                vec![expected_blocker.clone()],
                "command {target_status} {audit_label} lost its durable blocker"
            );
            assert!(service
                .get_conversation_turn_trace(&assistant_message_id)
                .unwrap()
                .is_none());
            let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
            assert_eq!(
                reopened.list_unsettled_file_effects().unwrap(),
                vec![expected_blocker]
            );
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ManualTraceReceiptTamper {
    Observation,
    Error,
    CallApproval,
}

fn assert_tampered_manual_trace_receipt_remains_unsettled(
    mut pending: AgentPendingActionRecord,
    approved: AgentActionAuditRecord,
    terminal: AgentActionAuditRecord,
    mut trace: ConversationTurnTrace,
    tamper: ManualTraceReceiptTamper,
) {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let storage_id = pending.action_id.clone();
    let run_id = pending.run_id.clone();
    let conversation_id = pending.conversation_id.clone().unwrap();
    let assistant_message_id = pending.assistant_message_id.clone().unwrap();
    attach_manual_file_effect_recovery_checkpoint(&mut pending, &trace);
    save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(approved).unwrap();
    service
        .commit_pending_agent_action_audited_result_trace(
            &terminal,
            "approved",
            "completed",
            &trace,
            12,
        )
        .unwrap();
    assert_eq!(
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap()
            .len(),
        1
    );
    assert!(service.list_unsettled_file_effects().unwrap().is_empty());

    let tampered_item = match tamper {
        ManualTraceReceiptTamper::Observation | ManualTraceReceiptTamper::Error => {
            let result_item = trace
                .items
                .iter_mut()
                .find(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
                .unwrap();
            let ConversationTurnTraceItem::ToolResult {
                observation, error, ..
            } = result_item
            else {
                unreachable!("selected trace item must be a ToolResult");
            };
            match tamper {
                ManualTraceReceiptTamper::Observation => {
                    *observation = serde_json::json!({ "tampered": true });
                }
                ManualTraceReceiptTamper::Error => {
                    *error = Some("tampered trace error".to_string());
                }
                ManualTraceReceiptTamper::CallApproval => unreachable!(),
            }
            result_item
        }
        ManualTraceReceiptTamper::CallApproval => {
            let call_item = trace
                .items
                .iter_mut()
                .find(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
                .unwrap();
            let ConversationTurnTraceItem::ToolCall {
                approval_status, ..
            } = call_item
            else {
                unreachable!("selected trace item must be a ToolCall");
            };
            *approval_status = crate::AgentApprovalStatus::Rejected;
            call_item
        }
    };
    let sequence = tampered_item.sequence();
    let item_json = serde_json::to_string(tampered_item).unwrap();
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversation_turn_trace_items
             SET item_json = ?3
             WHERE assistant_message_id = ?1 AND sequence = ?2",
            rusqlite::params![assistant_message_id, sequence, item_json],
        )
        .unwrap();

    assert!(service
        .reconcile_interrupted_pending_agent_actions(43)
        .unwrap()
        .is_empty());
    let expected = AgentUnsettledFileEffect {
        project_id: Some("project-1".to_string()),
        conversation_id,
        run_id,
        action_id: storage_id,
    };
    assert_eq!(
        service.list_unsettled_file_effects().unwrap(),
        vec![expected.clone()]
    );
    let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
    assert_eq!(
        reopened.list_unsettled_file_effects().unwrap(),
        vec![expected]
    );
}

#[test]
fn tampered_trace_evidence_never_settles_a_manual_file_effect() {
    for tamper in [
        ManualTraceReceiptTamper::Observation,
        ManualTraceReceiptTamper::Error,
        ManualTraceReceiptTamper::CallApproval,
    ] {
        let settlement = manual_command_settlement(
            "run-1:tampered-trace-command",
            "tampered-trace-command",
            "conversation-tampered-trace-command",
            "assistant-tampered-trace-command",
        );
        assert_tampered_manual_trace_receipt_remains_unsettled(
            settlement.0,
            settlement.1,
            settlement.2,
            settlement.3,
            tamper,
        );

        for effect in ManualNonCommandFileEffect::ALL {
            let label = effect.label();
            let settlement = manual_non_command_file_effect_settlement(
                effect,
                &format!("run-tampered-trace-{label}:{label}-call"),
                &format!("tampered-trace-{label}-call"),
                &format!("run-tampered-trace-{label}"),
                &format!("conversation-tampered-trace-{label}"),
                &format!("assistant-tampered-trace-{label}"),
            );
            assert_tampered_manual_trace_receipt_remains_unsettled(
                settlement.0,
                settlement.1,
                settlement.2,
                settlement.3,
                tamper,
            );
        }
    }
}

#[test]
fn tampered_command_trace_arguments_restore_the_durable_blocker() {
    let base_operation = serde_json::json!({
        "command": "node script.mjs",
        "timeoutMs": 5_000,
        "reason": "test atomic settlement",
    });
    let mut cases = Vec::new();
    for (label, field, value) in [
        ("command", "command", serde_json::json!("node other.mjs")),
        ("cwd", "cwd", serde_json::json!("other")),
        ("timeout", "timeoutMs", serde_json::json!(1)),
        ("reason", "reason", serde_json::json!("different reason")),
    ] {
        let mut operation = base_operation.clone();
        operation[field] = value;
        cases.push((label, operation));
    }
    let mut observe = base_operation.clone();
    observe["observe"] = serde_json::json!({
        "kinds": ["office"],
        "expectedOutputs": ["unexpected.xlsx"],
    });
    cases.push(("observe", observe));
    let mut runtime = base_operation.clone();
    runtime["runtime"] = serde_json::json!({
        "provider": "managedArtifact",
        "kind": "node",
        "requiredPackages": [],
    });
    cases.push(("runtime", runtime));
    let mut unknown = base_operation;
    unknown["executable"] = serde_json::json!("/tmp/untrusted-node");
    cases.push(("unknown", unknown));

    for (label, tampered_operation) in cases {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let storage_id = format!("run-1:tampered-command-{label}");
        let call_id = format!("tampered-command-{label}");
        let conversation_id = format!("conversation-tampered-command-{label}");
        let assistant_message_id = format!("assistant-tampered-command-{label}");
        let (mut pending, approved, terminal, mut trace) = manual_command_settlement(
            &storage_id,
            &call_id,
            &conversation_id,
            &assistant_message_id,
        );
        attach_manual_file_effect_recovery_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap();
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();
        assert!(service.list_unsettled_file_effects().unwrap().is_empty());

        let call_item = trace
            .items
            .iter_mut()
            .find(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
            .unwrap();
        let ConversationTurnTraceItem::ToolCall { operation, .. } = call_item else {
            unreachable!("selected trace item must be a ToolCall");
        };
        *operation = tampered_operation;
        service
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE conversation_turn_trace_items
                 SET item_json = ?3
                 WHERE assistant_message_id = ?1 AND sequence = ?2",
                rusqlite::params![
                    assistant_message_id,
                    call_item.sequence(),
                    serde_json::to_string(call_item).unwrap(),
                ],
            )
            .unwrap();

        assert_eq!(
            service.list_unsettled_file_effects().unwrap(),
            vec![AgentUnsettledFileEffect {
                project_id: Some("project-1".to_string()),
                conversation_id,
                run_id: "run-1".to_string(),
                action_id: storage_id,
            }],
            "tampered command {label} field was treated as authoritative"
        );
    }
}

#[test]
fn tampered_command_execution_evidence_restores_the_durable_blocker() {
    for tamper_artifact_observation in [false, true] {
        let label = if tamper_artifact_observation {
            "artifact-observation"
        } else {
            "stdout"
        };
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let storage_id = format!("run-1:tampered-command-result-{label}");
        let call_id = format!("tampered-command-result-{label}");
        let conversation_id = format!("conversation-tampered-command-result-{label}");
        let assistant_message_id = format!("assistant-tampered-command-result-{label}");
        let (mut pending, approved, terminal, trace) = manual_command_settlement(
            &storage_id,
            &call_id,
            &conversation_id,
            &assistant_message_id,
        );
        attach_manual_file_effect_recovery_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap();
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();
        assert!(service.list_unsettled_file_effects().unwrap().is_empty());

        let mut command_result = serde_json::from_str::<AgentCommandExecutionResult>(
            terminal.command_result_json.as_deref().unwrap(),
        )
        .unwrap();
        if tamper_artifact_observation {
            command_result.artifact_observation = Some(crate::AgentCommandArtifactObservation {
                schema_version: 1,
                status: crate::AgentCommandArtifactObservationStatus::Complete,
                partial: None,
                stop_reasons: Vec::new(),
                scanned: None,
                returned: None,
                omitted: None,
                coverage: crate::AgentCommandArtifactObservationCoverage {
                    workspace_included: true,
                    expected_output_count: 0,
                    additional_root_count: 0,
                    before: crate::AgentCommandArtifactSnapshotCoverage::default(),
                    after: crate::AgentCommandArtifactSnapshotCoverage::default(),
                },
                changes: Vec::new(),
                changes_truncated: false,
                changes_omitted: 0,
                expected_outputs: Vec::new(),
                warnings: Vec::new(),
            });
        } else {
            command_result.stdout = "tampered stdout".to_string();
        }
        service
            .state
            .connection()
            .unwrap()
            .execute(
                "UPDATE agent_action_audit
                 SET command_result_json = ?2
                 WHERE action_id = ?1",
                rusqlite::params![storage_id, serde_json::to_string(&command_result).unwrap(),],
            )
            .unwrap();

        assert_eq!(
            service.list_unsettled_file_effects().unwrap(),
            vec![AgentUnsettledFileEffect {
                project_id: Some("project-1".to_string()),
                conversation_id,
                run_id: "run-1".to_string(),
                action_id: storage_id,
            }],
            "tampered command {label} evidence was treated as authoritative"
        );
    }
}

#[test]
fn command_terminal_outcome_cannot_diverge_from_execution_evidence() {
    for case in [
        "successful-as-failed",
        "failed-as-successful",
        "failed-as-cancelled",
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let storage_id = format!("run-1:command-outcome-{case}");
        let call_id = format!("command-outcome-{case}");
        let conversation_id = format!("conversation-command-outcome-{case}");
        let assistant_message_id = format!("assistant-command-outcome-{case}");
        let (mut pending, approved, terminal, mut trace) = manual_command_settlement(
            &storage_id,
            &call_id,
            &conversation_id,
            &assistant_message_id,
        );
        attach_manual_file_effect_recovery_checkpoint(&mut pending, &trace);
        save_assistant_conversation(&service, &conversation_id, &assistant_message_id);
        service.store_pending_agent_action(pending).unwrap();
        service.upsert_agent_action_audit(approved).unwrap();
        service
            .commit_pending_agent_action_audited_result_trace(
                &terminal,
                "approved",
                "completed",
                &trace,
                12,
            )
            .unwrap();
        service
            .reconcile_interrupted_pending_agent_actions(42)
            .unwrap();
        assert!(service.list_unsettled_file_effects().unwrap().is_empty());

        let mut command_result = serde_json::from_str::<AgentCommandExecutionResult>(
            terminal.command_result_json.as_deref().unwrap(),
        )
        .unwrap();
        if case != "successful-as-failed" {
            command_result.exit_code = Some(1);
        }
        let mut tool_result = crate::command::command_tool_result(&call_id, &command_result);
        let target_status = match case {
            "successful-as-failed" => {
                tool_result.ok = false;
                tool_result.error = Some("fabricated command failure".to_string());
                "failed"
            }
            "failed-as-successful" => {
                tool_result.ok = true;
                tool_result.error = None;
                "completed"
            }
            "failed-as-cancelled" => "cancelled",
            _ => unreachable!(),
        };
        let call = AgentToolCall {
            id: call_id.clone(),
            tool: "run_command".to_string(),
            args: serde_json::json!({
                "command": "node script.mjs",
                "timeoutMs": 5_000,
                "reason": "test atomic settlement",
            }),
            approval_status: crate::AgentApprovalStatus::Approved,
            reason: Some("test atomic settlement".to_string()),
        };
        trace.items[1] =
            crate::conversation_trace::projected_tool_result_trace_item(1, &call, &tool_result);

        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE agent_pending_actions
                 SET status = ?2, target_status = ?2
                 WHERE action_id = ?1",
                rusqlite::params![storage_id, target_status],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE agent_action_audit
                 SET status = ?2,
                     command_result_json = ?3,
                     tool_result_json = ?4,
                     error = ?5
                 WHERE action_id = ?1",
                rusqlite::params![
                    storage_id,
                    target_status,
                    serde_json::to_string(&command_result).unwrap(),
                    serde_json::to_string(&tool_result).unwrap(),
                    tool_result.error,
                ],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE conversation_turn_trace_items
                 SET item_json = ?3
                 WHERE assistant_message_id = ?1 AND sequence = ?2",
                rusqlite::params![
                    assistant_message_id,
                    1,
                    serde_json::to_string(&trace.items[1]).unwrap(),
                ],
            )
            .unwrap();
        drop(connection);

        let expected = AgentUnsettledFileEffect {
            project_id: Some("project-1".to_string()),
            conversation_id: conversation_id.clone(),
            run_id: "run-1".to_string(),
            action_id: storage_id.clone(),
        };
        assert_eq!(
            service.list_unsettled_file_effects().unwrap(),
            vec![expected.clone()],
            "{case} was treated as an authoritative command settlement"
        );
        let reopened = StorageService::open(&fixture.root.join("storage.sqlite")).unwrap();
        assert_eq!(
            reopened.list_unsettled_file_effects().unwrap(),
            vec![expected]
        );
    }
}

#[test]
fn startup_reconciliation_repairs_legacy_manual_command_target_and_trace_from_terminal_audit() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (mut pending, _approved, terminal, expected_trace) = manual_command_settlement(
        "run-1:legacy-recovered-command",
        "legacy-recovered-command",
        "conversation-legacy-recovered-command",
        "assistant-legacy-recovered-command",
    );
    attach_manual_file_effect_recovery_checkpoint(&mut pending, &expected_trace);
    save_assistant_conversation(
        &service,
        "conversation-legacy-recovered-command",
        "assistant-legacy-recovered-command",
    );
    let mut usage = agent_usage_record(
        "conversation-legacy-recovered-command",
        "assistant-legacy-recovered-command",
    );
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();
    service.store_pending_agent_action(pending).unwrap();
    // Former split ordering: the audit commit succeeded, then the process exited before either
    // target_status or the paired trace was committed.
    service.upsert_agent_action_audit(terminal).unwrap();

    let interrupted = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(interrupted.len(), 1);

    let connection = service.state.connection().unwrap();
    let pending_state: (String, Option<String>) = connection
        .query_row(
            "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
            ["run-1:legacy-recovered-command"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        pending_state,
        ("completed".to_string(), Some("completed".to_string()))
    );
    let usage_error: Option<String> = connection
        .query_row(
            "SELECT error FROM agent_usage_records WHERE run_id = 'run-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(usage_error
        .as_deref()
        .is_some_and(|error| error.contains("result was recovered")));
    drop(connection);

    let trace = service
        .get_conversation_turn_trace("assistant-legacy-recovered-command")
        .unwrap()
        .unwrap();
    trace.validate().unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult {
            call_id,
            tool,
            success: true,
            observation,
            ..
        }) if call_id == "legacy-recovered-command"
            && tool == "run_command"
            && observation["stdoutTail"] == "created workbook"
    ));
}

#[test]
fn startup_reconciliation_repairs_missing_trace_when_legacy_target_already_exists() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (mut pending, _approved, terminal, expected_trace) = manual_command_settlement(
        "run-1:legacy-target-only-command",
        "legacy-target-only-command",
        "conversation-legacy-target-only-command",
        "assistant-legacy-target-only-command",
    );
    pending.target_status = Some("completed".to_string());
    attach_manual_file_effect_recovery_checkpoint(&mut pending, &expected_trace);
    save_assistant_conversation(
        &service,
        "conversation-legacy-target-only-command",
        "assistant-legacy-target-only-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    service.upsert_agent_action_audit(terminal).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    let trace = service
        .get_conversation_turn_trace("assistant-legacy-target-only-command")
        .unwrap()
        .unwrap();
    trace.validate().unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult { call_id, .. })
            if call_id == "legacy-target-only-command"
    ));
}

#[test]
fn startup_reconciliation_rejects_conflicting_terminal_command_audit_without_partial_repair() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let (mut pending, _approved, mut terminal, expected_trace) = manual_command_settlement(
        "run-1:legacy-conflicting-command",
        "legacy-conflicting-command",
        "conversation-legacy-conflicting-command",
        "assistant-legacy-conflicting-command",
    );
    attach_manual_file_effect_recovery_checkpoint(&mut pending, &expected_trace);
    save_assistant_conversation(
        &service,
        "conversation-legacy-conflicting-command",
        "assistant-legacy-conflicting-command",
    );
    service.store_pending_agent_action(pending).unwrap();
    terminal.run_id = "another-run".to_string();
    service.upsert_agent_action_audit(terminal).unwrap();

    let error = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap_err();
    assert!(error.contains("terminal audit 身份冲突"));

    let connection = service.state.connection().unwrap();
    let pending_state: (String, Option<String>) = connection
        .query_row(
            "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
            ["run-1:legacy-conflicting-command"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(pending_state, ("approved".to_string(), None));
    drop(connection);
    assert!(service
        .get_conversation_turn_trace("assistant-legacy-conflicting-command")
        .unwrap()
        .is_none());
}

#[test]
fn pending_target_and_paired_trace_commit_atomically() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut stored_conversation = conversation(
        "conversation-atomic-result",
        Some("project-1"),
        "assistant-atomic-result",
    );
    stored_conversation.messages[0].role = "assistant".to_string();
    service.save_conversation(stored_conversation).unwrap();
    let mut pending = pending_action("action-atomic-result", "conversation-atomic-result");
    pending.assistant_message_id = Some("assistant-atomic-result".to_string());
    pending.status = "approved".to_string();
    service.store_pending_agent_action(pending).unwrap();

    let changed = service
        .commit_pending_agent_action_result_trace(
            "action-atomic-result",
            "approved",
            "completed",
            &in_progress_result_trace("conversation-atomic-result", "assistant-atomic-result"),
            1,
            2,
        )
        .unwrap();
    assert!(changed);

    let target_status: Option<String> = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT target_status FROM agent_pending_actions WHERE action_id = ?1",
            ["action-atomic-result"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target_status.as_deref(), Some("completed"));
    let trace = service
        .get_conversation_turn_trace("assistant-atomic-result")
        .unwrap()
        .unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::ToolResult { call_id, .. })
            if call_id == "action-atomic-result"
    ));
}

#[test]
fn invalid_trace_rolls_back_pending_target_status() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut stored_conversation = conversation(
        "conversation-atomic-rollback",
        Some("project-1"),
        "assistant-atomic-rollback",
    );
    stored_conversation.messages[0].role = "assistant".to_string();
    service.save_conversation(stored_conversation).unwrap();
    let mut pending = pending_action("action-atomic-result", "conversation-atomic-rollback");
    pending.assistant_message_id = Some("assistant-atomic-rollback".to_string());
    pending.status = "approved".to_string();
    service.store_pending_agent_action(pending).unwrap();

    let error = service
        .commit_pending_agent_action_result_trace(
            "action-atomic-result",
            "approved",
            "completed",
            &in_progress_result_trace("missing-conversation", "assistant-atomic-rollback"),
            1,
            2,
        )
        .unwrap_err();
    assert!(error.contains("conversation trace assistant message does not exist"));

    let target_status: Option<String> = service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT target_status FROM agent_pending_actions WHERE action_id = ?1",
            ["action-atomic-result"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target_status, None);
    assert!(service
        .get_conversation_turn_trace("assistant-atomic-rollback")
        .unwrap()
        .is_none());
}

#[test]
fn startup_reconciliation_marks_interrupted_action_message_and_usage_failed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut interrupted_conversation = conversation(
        "conversation-interrupted",
        Some("project-1"),
        "assistant-interrupted",
    );
    let message = &mut interrupted_conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("pending".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "status": "waiting_for_approval",
            "state": {
                "status": "waiting_for_approval",
                "activeRunId": "run-1",
                "updatedAt": 1
            }
        })
        .to_string(),
    );
    service.save_conversation(interrupted_conversation).unwrap();
    let mut usage = agent_usage_record("conversation-interrupted", "assistant-interrupted");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();
    let mut pending = pending_action("action-interrupted", "conversation-interrupted");
    pending.assistant_message_id = Some("assistant-interrupted".to_string());
    pending.status = "approved".to_string();
    pending.agent_input_json = "sensitive continuation".to_string();
    service.store_pending_agent_action(pending).unwrap();

    let reconciled = service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0].status, "approved");
    assert!(service.list_pending_agent_actions().unwrap().is_empty());

    let conversation = service
        .load_conversation("conversation-interrupted")
        .unwrap()
        .unwrap();
    let message = &conversation.messages[0];
    assert_eq!(message.status.as_deref(), Some("error"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "failed");
    assert_eq!(run["state"]["status"], "failed");
    assert!(run["state"]["activeRunId"].is_null());

    let connection = service.state.connection().unwrap();
    let pending_row: (String, String) = connection
        .query_row(
            "SELECT status, agent_input_json FROM agent_pending_actions WHERE action_id = ?1",
            ["action-interrupted"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(pending_row, ("failed".to_string(), "{}".to_string()));
    let usage_row: (String, Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = ?1",
            ["run-1"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(usage_row.0, "failed");
    assert!(usage_row.1.unwrap().contains("outcome is unknown"));
    assert_eq!(usage_row.2, Some(42));
}

#[test]
fn startup_reconciliation_preserves_a_durable_terminal_assistant_commit() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation = conversation(
        "conversation-terminal",
        Some("project-1"),
        "assistant-terminal",
    );
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("sent".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "status": "completed",
            "completedAt": 40,
            "state": { "status": "completed", "activeRunId": null, "updatedAt": 40 }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-terminal", "assistant-terminal");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();
    let mut pending = pending_action("action-terminal", "conversation-terminal");
    pending.assistant_message_id = Some("assistant-terminal".to_string());
    pending.status = "approved".to_string();
    pending.target_status = Some("completed".to_string());
    service.store_pending_agent_action(pending).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    let connection = service.state.connection().unwrap();
    let pending_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = 'action-terminal'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_status, "completed");
    let usage_status: (String, Option<i64>) = connection
        .query_row(
            "SELECT status, completed_at FROM agent_usage_records WHERE run_id = 'run-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(usage_status, ("completed".to_string(), Some(42)));
    drop(connection);
    let conversation = service
        .load_conversation("conversation-terminal")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("sent"));
}

#[test]
fn startup_reconciliation_preserves_a_valid_nested_pending_checkpoint() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation =
        conversation("conversation-nested", Some("project-1"), "assistant-nested");
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("pending".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "running",
            "state": { "status": "running", "activeRunId": "run-1", "updatedAt": 1 }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-nested", "assistant-nested");
    usage.status = Some("running".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();

    let mut parent = pending_action("parent-action", "conversation-nested");
    parent.assistant_message_id = Some("assistant-nested".to_string());
    parent.status = "executing".to_string();
    parent.target_status = Some("completed".to_string());
    service.store_pending_agent_action(parent).unwrap();

    let mut child = pending_action("child-storage-id", "conversation-nested");
    child.assistant_message_id = Some("assistant-nested".to_string());
    child.action_type = "tool_call".to_string();
    child.tool_name = "approval_tool".to_string();
    child.tool_call_id = Some("child-call".to_string());
    child.action_json = serde_json::json!({
        "type": "tool_call",
        "call": {
            "id": "child-call",
            "tool": "approval_tool",
            "args": {},
            "approvalStatus": "required"
        }
    })
    .to_string();
    child.agent_input_json = serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "messages": [],
        "resumeCheckpoint": {
            "version": crate::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
            "runId": "run-1",
            "contextItems": [],
            "nextModelRequestIndex": 1,
            "queuedToolCalls": [],
            "suppressedNarration": false,
            "extensionSnapshots": [],
            "pendingToolCallId": "child-call",
            "conversationTraceItems": [
                {
                    "type": "tool_result",
                    "sequence": 1,
                    "callId": "parent-action",
                    "tool": "run_command",
                    "status": "succeeded",
                    "success": true,
                    "observation": {},
                    "approvalStatus": "approved",
                    "truncated": false
                },
                {
                    "type": "tool_call",
                    "sequence": 2,
                    "callId": "child-call",
                    "tool": "approval_tool",
                    "operation": {},
                    "approvalStatus": "required",
                    "truncated": false
                }
            ],
            "nextConversationTraceSequence": 3,
            "conversationTraceTruncated": false,
            "modelVisibleTraceItemCount": 0,
            "toolSet": test_checkpoint_tool_set(),
            "runContext": null,
            "modelCapabilities": { "imageInput": false },
            "runWorldState": test_checkpoint_run_world_state()
        }
    })
    .to_string();
    service.store_pending_agent_action(child).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    let pending = service.list_pending_agent_actions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].action_id, "child-storage-id");
    let connection = service.state.connection().unwrap();
    let parent_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = 'parent-action'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parent_status, "completed");
    let usage_state: (String, Option<String>, Option<i64>) = connection
        .query_row(
            "SELECT status, error, completed_at FROM agent_usage_records WHERE run_id = 'run-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        usage_state,
        ("waiting_for_approval".to_string(), None, None)
    );
    drop(connection);
    let conversation = service
        .load_conversation("conversation-nested")
        .unwrap()
        .unwrap();
    let message = &conversation.messages[0];
    assert_eq!(message.status.as_deref(), Some("pending"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "waiting_for_approval");
    assert_eq!(run["state"]["status"], "waiting_for_approval");
    assert_eq!(run["state"]["activeRunId"], "run-1");
}

#[test]
fn startup_reconciliation_retires_an_invalid_nested_pending_action() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation = conversation(
        "conversation-invalid-nested",
        Some("project-1"),
        "assistant-invalid-nested",
    );
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("pending".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "runId": "run-1",
            "status": "waiting_for_approval",
            "state": {
                "status": "waiting_for_approval",
                "activeRunId": "run-1",
                "updatedAt": 1
            }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-invalid-nested", "assistant-invalid-nested");
    usage.status = Some("waiting_for_approval".to_string());
    usage.completed_at = None;
    service.upsert_agent_usage(usage).unwrap();

    let mut parent = pending_action("invalid-parent", "conversation-invalid-nested");
    parent.assistant_message_id = Some("assistant-invalid-nested".to_string());
    parent.status = "executing".to_string();
    service.store_pending_agent_action(parent).unwrap();

    let mut child = pending_action("invalid-child", "conversation-invalid-nested");
    child.assistant_message_id = Some("assistant-invalid-nested".to_string());
    child.action_type = "tool_call".to_string();
    child.tool_name = "approval_tool".to_string();
    child.tool_call_id = Some("invalid-child-call".to_string());
    child.action_json = serde_json::json!({
        "type": "tool_call",
        "call": {
            "id": "invalid-child-call",
            "tool": "approval_tool",
            "args": {},
            "approvalStatus": "required"
        }
    })
    .to_string();
    // A pending child without a run checkpoint is not a durable handoff and must never remain
    // approvable after its interrupted parent has been failed.
    child.agent_input_json = serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "messages": []
    })
    .to_string();
    service.store_pending_agent_action(child).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    assert!(service.list_pending_agent_actions().unwrap().is_empty());
    let connection = service.state.connection().unwrap();
    let statuses = connection
        .prepare(
            "SELECT action_id, status, target_status
             FROM agent_pending_actions
             WHERE action_id IN ('invalid-parent', 'invalid-child')
             ORDER BY action_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        statuses,
        vec![
            (
                "invalid-child".to_string(),
                "cancelled".to_string(),
                Some("cancelled".to_string())
            ),
            (
                "invalid-parent".to_string(),
                "failed".to_string(),
                Some("failed".to_string())
            )
        ]
    );
    drop(connection);
    let conversation = service
        .load_conversation("conversation-invalid-nested")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("error"));
}

#[test]
fn startup_reconciliation_keeps_failed_action_outcome_when_assistant_commit_completed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut conversation = conversation(
        "conversation-failed-action",
        Some("project-1"),
        "assistant-failed-action",
    );
    let message = &mut conversation.messages[0];
    message.role = "assistant".to_string();
    message.status = Some("sent".to_string());
    message.agent_run_json = Some(
        serde_json::json!({
            "status": "completed",
            "completedAt": 40,
            "state": { "status": "completed", "activeRunId": null, "updatedAt": 40 }
        })
        .to_string(),
    );
    service.save_conversation(conversation).unwrap();
    let mut usage = agent_usage_record("conversation-failed-action", "assistant-failed-action");
    usage.status = Some("completed".to_string());
    usage.completed_at = Some(40);
    service.upsert_agent_usage(usage).unwrap();
    let mut pending = pending_action("action-failed", "conversation-failed-action");
    pending.assistant_message_id = Some("assistant-failed-action".to_string());
    pending.status = "approved".to_string();
    pending.target_status = Some("failed".to_string());
    service.store_pending_agent_action(pending).unwrap();

    service
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();

    let connection = service.state.connection().unwrap();
    let pending_status: String = connection
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = 'action-failed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_status, "failed");
    let usage_status: String = connection
        .query_row(
            "SELECT status FROM agent_usage_records WHERE run_id = 'run-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(usage_status, "completed");
    drop(connection);
    let conversation = service
        .load_conversation("conversation-failed-action")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("sent"));
}
