use super::fixtures::*;
use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum ManualNonCommandFileEffect {
    OfficeOperation,
    SkillScript,
    SkillMaterialization,
}

impl ManualNonCommandFileEffect {
    pub(super) const ALL: [Self; 3] = [
        Self::OfficeOperation,
        Self::SkillScript,
        Self::SkillMaterialization,
    ];

    pub(super) fn label(self) -> &'static str {
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

    pub(super) fn tool_name(self) -> &'static str {
        match self {
            Self::OfficeOperation => "office_spreadsheet",
            Self::SkillScript => "skills_run_script",
            Self::SkillMaterialization => "skills_materialize_resource",
        }
    }

    pub(super) fn frozen_action(
        self,
        call_id: &str,
        alternate_payload: bool,
    ) -> AgentProposedAction {
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
                            parameters: crate::office::OfficeOperationParameters::Create {
                                locale: None,
                                minimal: false,
                                overwrite: false,
                            },
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
                        resolved_render_plan: None,
                        paths: Vec::new(),
                        input_bindings: Vec::new(),
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
                    source: crate::AgentSkillScriptSourceProof {
                        source_id: "installed:user".to_string(),
                        source_kind: crate::AgentSkillScriptSourceKind::Installed,
                        trust: crate::AgentSkillScriptTrust::Untrusted,
                    },
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

pub(super) fn manual_non_command_file_effect_settlement(
    effect: ManualNonCommandFileEffect,
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
    let storage_id = current_pending_storage_id(run_id, call_id);
    let action = effect.frozen_action(call_id, false);
    let tool_operation = effect.tool_operation(&action);
    let action_json = serde_json::to_string(&action).unwrap();
    let pending = AgentPendingActionRecord {
        action_id: storage_id.clone(),
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
        action_id: storage_id,
        run_id: run_id.to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action_type: effect.action_type().to_string(),
        tool_name: effect.tool_name().to_string(),
        decision: Some("approved".to_string()),
        status: "approved".to_string(),
        action_json,
        file_change_result_json: None,
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
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: effect.tool_name().to_string(),
                },
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
