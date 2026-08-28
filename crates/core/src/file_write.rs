use crate::storage::models::AgentFileChangeRecord;
use crate::{
    AgentApprovalStatus, AgentFileDraftSnapshot, AgentFileWriteMode, AgentFileWriteProposal,
    AgentFileWriteResult, AgentFileWriteResultStatus, AgentPatchPermission, AgentPermissions,
    AgentProposedAction, AgentWritePermission,
};
use similar::TextDiff;

/// The single approval route used by structured tools that publish file
/// changes. Path scope and revision checks remain the responsibility of the
/// concrete writer; this route only answers who may authorize the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWriteApprovalRoute {
    Denied,
    RequireExplicitApproval,
    AutoApprove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWriteAuthorizationSource {
    Automatic,
    ExplicitUser,
}

/// Resolves the common write/approval dimensions without granting any broader
/// filesystem scope. Full Access reaches `AutoApprove` because the trusted
/// frontend maps that mode to write=all + patch=auto_approve; custom modes can
/// reach the same route without gaining write=all.
pub fn file_write_approval_route(permissions: AgentPermissions) -> FileWriteApprovalRoute {
    if permissions.write == AgentWritePermission::Denied {
        FileWriteApprovalRoute::Denied
    } else if permissions.patch == AgentPatchPermission::AutoApprove {
        FileWriteApprovalRoute::AutoApprove
    } else {
        FileWriteApprovalRoute::RequireExplicitApproval
    }
}

/// Revalidates the authorization source at the host execution boundary. This
/// prevents an internal caller from labelling a manually-routed write as an
/// automatically approved action and bypassing the user's chosen mode.
pub fn file_write_authorized(
    permissions: AgentPermissions,
    source: FileWriteAuthorizationSource,
) -> bool {
    match (file_write_approval_route(permissions), source) {
        (FileWriteApprovalRoute::AutoApprove, FileWriteAuthorizationSource::Automatic)
        | (
            FileWriteApprovalRoute::AutoApprove | FileWriteApprovalRoute::RequireExplicitApproval,
            FileWriteAuthorizationSource::ExplicitUser,
        ) => true,
        (FileWriteApprovalRoute::Denied, _)
        | (
            FileWriteApprovalRoute::RequireExplicitApproval,
            FileWriteAuthorizationSource::Automatic,
        ) => false,
    }
}

/// Structured host actions that publish files all share the policy above.
/// Command and Skill-script actions stay in their separate process policy,
/// because they can have broader side effects than a validated file writer.
pub fn proposed_action_uses_file_write_policy(action: &AgentProposedAction) -> bool {
    file_write_action_approval_status(action).is_some()
}

/// Returns the approval status carried by every structured file-write action.
/// This exhaustive classifier is shared by runtime and host checks so adding a
/// new action variant cannot silently update one policy boundary but not the
/// other.
pub fn file_write_action_approval_status(
    action: &AgentProposedAction,
) -> Option<AgentApprovalStatus> {
    match action {
        AgentProposedAction::Diff { diff } => Some(diff.approval_status),
        AgentProposedAction::FileWrite { file_write } => Some(file_write.approval_status),
        AgentProposedAction::SkillMaterialization { materialization } => {
            Some(materialization.approval_status)
        }
        AgentProposedAction::OfficeOperation { office_operation } => {
            Some(office_operation.approval_status)
        }
        AgentProposedAction::Command { .. }
        | AgentProposedAction::ToolCall { .. }
        | AgentProposedAction::McpToolCall { .. }
        | AgentProposedAction::BuiltinCapabilityActivation { .. }
        | AgentProposedAction::BuiltinMcpToolApproval { .. }
        | AgentProposedAction::BrowserRiskApproval { .. }
        | AgentProposedAction::SkillScript { .. }
        | AgentProposedAction::SkillInstallation { .. } => None,
    }
}

pub fn file_write_diff(draft: &AgentFileChangeRecord) -> String {
    TextDiff::from_lines(&draft.base_content, &draft.content)
        .unified_diff()
        .header(
            &format!("a/{}", draft.file_path),
            &format!("b/{}", draft.file_path),
        )
        .to_string()
}

pub fn file_draft_snapshot(
    draft: &AgentFileChangeRecord,
) -> Result<AgentFileDraftSnapshot, String> {
    let mode = file_change_write_mode(draft)?;
    let status = serde_json::from_value(serde_json::Value::String(draft.status.clone()))
        .map_err(|error| format!("文件草稿 status 无效：{error}"))?;
    Ok(AgentFileDraftSnapshot {
        draft_id: draft.id.clone(),
        conversation_id: draft.conversation_id.clone(),
        project_id: draft.project_id.clone(),
        file_path: draft.file_path.clone(),
        mode,
        status,
        base_revision: draft.base_revision.clone(),
        additions: draft.additions,
        deletions: draft.deletions,
        line_count: draft.line_count,
        byte_count: draft.byte_count,
        chunk_count: draft.mutation_count,
        next_chunk_index: draft.next_mutation_index,
        stats_final: draft.stats_final,
        summary: draft.summary.clone(),
        created_at: draft.created_at,
        updated_at: draft.updated_at,
    })
}

pub fn failed_file_write_result(
    proposal: &AgentFileWriteProposal,
    status: AgentFileWriteResultStatus,
    error: impl Into<String>,
) -> AgentFileWriteResult {
    let error = error.into();
    AgentFileWriteResult {
        status,
        draft_id: proposal.draft_id.clone(),
        mode: proposal.mode,
        file_path: proposal.file_path.clone(),
        additions: proposal.additions,
        deletions: proposal.deletions,
        line_count: proposal.line_count,
        byte_count: proposal.byte_count,
        revision: None,
        error: Some(error.clone()),
        message: Some(error),
    }
}

fn file_change_write_mode(change: &AgentFileChangeRecord) -> Result<AgentFileWriteMode, String> {
    match (change.operation.as_str(), change.strategy.as_deref()) {
        ("create", None) => Ok(AgentFileWriteMode::Create),
        ("update", Some("modify")) => Ok(AgentFileWriteMode::Modify),
        ("update", Some("rewrite")) => Ok(AgentFileWriteMode::Rewrite),
        _ => Err("文件变更事务的 operation/strategy 无效。".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentApprovalStatus, AgentCommandPermission, AgentPatchPermission, AgentReadPermission,
    };

    fn permissions() -> AgentPermissions {
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: Default::default(),
            patch: AgentPatchPermission::RequireApproval,
            builtin_execution: Default::default(),
        }
    }

    fn draft(file_path: &str, base: &str, content: &str) -> AgentFileChangeRecord {
        AgentFileChangeRecord {
            schema_version: 1,
            id: "draft-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: Some("project-1".to_string()),
            run_id: "run-1".to_string(),
            source_tool_name: "write_file".to_string(),
            source_tool_call_id: "call-begin-1".to_string(),
            source_tool_arguments_digest: "digest-begin-1".to_string(),
            permission_revision: "permission-1".to_string(),
            tool_set_revision: "tool-set-1".to_string(),
            provider_wire_revision: "provider-protocol-v1".to_string(),
            observation_id: "fobs_fixture".to_string(),
            observation_json: "{}".to_string(),
            file_path: file_path.to_string(),
            operation: if base.is_empty() { "create" } else { "update" }.to_string(),
            strategy: (!base.is_empty()).then(|| "rewrite".to_string()),
            status: "waiting_approval".to_string(),
            base_revision: (!base.is_empty()).then(|| crate::content_revision(base.as_bytes())),
            base_content: base.to_string(),
            content: content.to_string(),
            draft_revision: 1,
            next_mutation_index: 1,
            additions: 1,
            deletions: u64::from(!base.is_empty()),
            line_count: 1,
            byte_count: content.len() as u64,
            mutation_count: 1,
            stats_final: true,
            summary: Some("test write".to_string()),
            final_action_id: Some("action-1".to_string()),
            created_at: 1,
            updated_at: 1,
            expires_at: i64::MAX,
        }
    }

    fn proposal(draft: &AgentFileChangeRecord) -> AgentFileWriteProposal {
        AgentFileWriteProposal {
            id: "action-1".to_string(),
            draft_id: draft.id.clone(),
            mode: if draft.base_revision.is_some() {
                AgentFileWriteMode::Rewrite
            } else {
                AgentFileWriteMode::Create
            },
            file_path: draft.file_path.clone(),
            base_revision: draft.base_revision.clone(),
            summary: draft.summary.clone(),
            additions: draft.additions,
            deletions: draft.deletions,
            line_count: draft.line_count,
            byte_count: draft.byte_count,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(direct_binding_fixture()),
        }
    }

    #[test]
    fn shared_file_write_policy_routes_manual_custom_auto_and_full_access() {
        let manual = permissions();
        assert_eq!(
            file_write_approval_route(manual),
            FileWriteApprovalRoute::RequireExplicitApproval
        );
        assert!(!file_write_authorized(
            manual,
            FileWriteAuthorizationSource::Automatic
        ));
        assert!(file_write_authorized(
            manual,
            FileWriteAuthorizationSource::ExplicitUser
        ));

        let custom_auto = AgentPermissions {
            patch: AgentPatchPermission::AutoApprove,
            ..manual
        };
        assert_eq!(
            file_write_approval_route(custom_auto),
            FileWriteApprovalRoute::AutoApprove
        );
        assert!(file_write_authorized(
            custom_auto,
            FileWriteAuthorizationSource::Automatic
        ));

        let full_access = AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: crate::AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
            builtin_execution: crate::AgentBuiltinExecutionPermission::AutoApprove,
        };
        assert_eq!(
            file_write_approval_route(full_access),
            FileWriteApprovalRoute::AutoApprove
        );
        assert!(file_write_authorized(
            full_access,
            FileWriteAuthorizationSource::Automatic
        ));

        let denied = AgentPermissions {
            write: AgentWritePermission::Denied,
            patch: AgentPatchPermission::AutoApprove,
            ..manual
        };
        assert_eq!(
            file_write_approval_route(denied),
            FileWriteApprovalRoute::Denied
        );
        assert!(!file_write_authorized(
            denied,
            FileWriteAuthorizationSource::Automatic
        ));
        assert!(!file_write_authorized(
            denied,
            FileWriteAuthorizationSource::ExplicitUser
        ));
    }

    #[test]
    fn structured_file_write_actions_share_one_host_policy_domain() {
        let draft = draft("report.txt", "", "hello");
        let file_write = AgentProposedAction::FileWrite {
            file_write: proposal(&draft),
        };
        let diff = AgentProposedAction::Diff {
            diff: crate::AgentDiffProposal {
                id: "diff-1".to_string(),
                operation: crate::AgentPatchOperation::Create,
                file_path: "report.md".to_string(),
                patch: "".to_string(),
                base_revision: None,
                summary: None,
                approval_status: AgentApprovalStatus::Required,
                execution: Box::new(direct_binding_fixture()),
            },
        };
        let materialization = AgentProposedAction::SkillMaterialization {
            materialization: crate::AgentSkillMaterializationRequest {
                id: "materialize-1".to_string(),
                source_uri: "skill://test/revision/asset.txt".to_string(),
                source_prefix: None,
                destination: "asset.txt".to_string(),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            },
        };
        let office = AgentProposedAction::OfficeOperation {
            office_operation: Box::new(crate::AgentOfficeOperationRequest {
                schema_version: crate::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
                id: "office-1".to_string(),
                semantic_args: serde_json::json!({
                    "operation": "create",
                    "filePath": "budget.xlsx",
                    "reason": "create the reviewed workbook"
                }),
                prepared: crate::office::OfficePreparedExecution {
                    schema_version: crate::office::OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
                    provider_id: "test".to_string(),
                    engine_revision: "engine".to_string(),
                    workspace_revision: Some("workspace".to_string()),
                    access: crate::office::OfficeOperationAccess::FileWrite,
                    request: crate::office::OfficeExecutionRequest {
                        document_kind: crate::office::OfficeDocumentKind::Spreadsheet,
                        operation: crate::office::OfficeOperation::Create,
                        document_path: Some("budget.xlsx".to_string()),
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
                    argv: vec!["create".to_string(), "budget.xlsx".to_string()],
                    resolved_render_plan: None,
                    paths: Vec::new(),
                    input_bindings: Vec::new(),
                },
                approval_status: AgentApprovalStatus::Approved,
                reason: "create the reviewed workbook".to_string(),
            }),
        };

        for action in [&file_write, &diff, &materialization, &office] {
            assert!(proposed_action_uses_file_write_policy(action));
            assert!(file_write_action_approval_status(action).is_some());
        }

        assert!(!proposed_action_uses_file_write_policy(
            &AgentProposedAction::ToolCall {
                call: crate::AgentToolCall {
                    id: "read-1".to_string(),
                    tool: "read_file".to_string(),
                    args: serde_json::json!({ "path": "report.txt" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    reason: None,
                },
            }
        ));
        assert!(!proposed_action_uses_file_write_policy(
            &AgentProposedAction::Command {
                command: crate::AgentCommandRequest {
                    id: "command-1".to_string(),
                    command: "pwd".to_string(),
                    cwd: None,
                    timeout_ms: None,
                    approval_status: AgentApprovalStatus::Required,
                    risk_level: None,
                    reason: None,
                    observe: None,
                    inputs: Vec::new(),
                    runtime_binding: None,
                    managed_office_script: None,
                },
            }
        ));
    }

    fn direct_binding_fixture() -> crate::file_change::FileChangeDirectBinding {
        use crate::file_change::{
            FileChangeContentState, FileChangeDirectBinding, FileChangeOperation,
            FileChangeOutcome, FileChangeProposal, FileChangeStatus, FileChangeTransaction,
            FileObservationCheckpoint, FileObservationIdentity, FileObservationState,
            FILE_CHANGE_SCHEMA_VERSION, FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
            FILE_OBSERVATION_TTL_MS,
        };

        let digest = format!("file-change-sha256-v1:{}", "0".repeat(64));
        let target = FileChangeContentState::Present {
            revision: "revision".to_string(),
            digest: digest.clone(),
            byte_count: 0,
        };
        let transaction = FileChangeTransaction {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: "transaction-1".to_string(),
            operation: FileChangeOperation::Create,
            file_path: "report.md".to_string(),
            status: FileChangeStatus::WaitingApproval,
            outcome: FileChangeOutcome::DefinitelyNotExecuted,
            base: FileChangeContentState::Missing,
            target: target.clone(),
            proposal_digest: digest.clone(),
            created_at: 1,
            updated_at: 1,
        };
        FileChangeDirectBinding {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            transaction,
            proposal: FileChangeProposal {
                schema_version: FILE_CHANGE_SCHEMA_VERSION,
                id: "diff-1".to_string(),
                transaction_id: "transaction-1".to_string(),
                operation: FileChangeOperation::Create,
                file_path: "report.md".to_string(),
                base: FileChangeContentState::Missing,
                target,
                diff_digest: digest.clone(),
                proposal_digest: digest.clone(),
                additions: 0,
                deletions: 0,
            },
            observation_id: format!("fobs_{}", "0".repeat(32)),
            observation: FileObservationCheckpoint {
                schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
                observation_id: format!("fobs_{}", "0".repeat(32)),
                source_tool_call_id: "read-fixture".to_string(),
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                canonical_target: "/tmp/report.md".to_string(),
                state: FileObservationState::Missing,
                parent_identity: FileObservationIdentity::from_metadata(
                    &std::fs::metadata("/tmp").expect("test temporary directory metadata"),
                ),
                created_at_ms: 1,
                expires_at_ms: 1 + FILE_OBSERVATION_TTL_MS,
            },
            source_tool_name: "apply_patch".to_string(),
            source_call_id: "diff-1".to_string(),
            source_args_digest: digest.clone(),
            staged_transaction_id: None,
            conversation_id: "conversation-1".to_string(),
            project_id: None,
            run_id: "run-1".to_string(),
            staged_transaction_revision: None,
            canonical_target: "/tmp/report.md".to_string(),
            base_content: None,
            target_content: Some(String::new()),
            delete_journal: None,
            receipt: None,
            permission_revision: "permission-v1".to_string(),
            tool_set_revision: "tool-set-v1".to_string(),
            provider_wire_revision: "provider-wire-v1".to_string(),
        }
    }

    #[test]
    fn direct_binding_persistence_requires_observation_journal_and_receipt() {
        let encoded = serde_json::to_value(direct_binding_fixture()).unwrap();
        assert!(encoded.get("observation").is_some());
        assert_eq!(encoded["deleteJournal"], serde_json::Value::Null);
        assert_eq!(encoded["receipt"], serde_json::Value::Null);

        for missing in ["observation", "deleteJournal", "receipt"] {
            let mut malformed = encoded.clone();
            malformed.as_object_mut().unwrap().remove(missing);
            assert!(
                serde_json::from_value::<crate::file_change::FileChangeDirectBinding>(malformed)
                    .is_err()
            );
        }
        let mut extra = encoded;
        extra["internalCause"] = serde_json::json!("must not be accepted");
        assert!(
            serde_json::from_value::<crate::file_change::FileChangeDirectBinding>(extra).is_err()
        );
    }
}
