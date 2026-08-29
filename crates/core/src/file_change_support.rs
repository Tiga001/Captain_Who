use crate::storage::models::AgentFileChangeRecord;
use crate::{
    AgentApprovalStatus, AgentFileChangeOperation, AgentFileChangeProposal, AgentFileChangeResult,
    AgentFileChangeResultStatus, AgentFileChangeSnapshot, AgentFileChangeUpdateStrategy,
    AgentPatchPermission, AgentPermissions, AgentProposedAction, AgentWritePermission,
    AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
};
use similar::TextDiff;

/// Returns the one canonical, body-free operation stored for a strict apply_patch ToolCall.
///
/// The model-visible request is validated before projection so malformed calls cannot collapse to
/// the same fail-closed Trace value and accidentally become approval authority. The returned value
/// contains only bounded metadata and content/edit digests; it is safe for durable audit history.
pub fn apply_patch_trace_operation(
    args: &serde_json::Value,
) -> Result<serde_json::Value, &'static str> {
    if !crate::tools::apply_patch_wire_is_valid(args) {
        return Err("invalid apply_patch ToolCall arguments");
    }
    let (sanitized, _) = crate::conversation_trace_projection::sanitize_runtime_value(args);
    Ok(crate::conversation_trace_projection::project_tool_call("apply_patch", &sanitized).value)
}

/// Binds a frozen FileChange action to its canonical body-free durable Trace operation.
pub fn apply_patch_trace_args_digest(args: &serde_json::Value) -> Result<String, &'static str> {
    let operation = apply_patch_trace_operation(args)?;
    crate::file_change::proposal_digest(&operation)
        .map_err(|_| "apply_patch Trace arguments could not be digested")
}

/// The single approval route used by structured tools that publish file
/// changes. Path scope and revision checks remain the responsibility of the
/// concrete writer; this route only answers who may authorize the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeApprovalRoute {
    Denied,
    RequireExplicitApproval,
    AutoApprove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeAuthorizationSource {
    Automatic,
    ExplicitUser,
    RunGrant,
}

/// Proves that one typed terminal result belongs to the exact frozen FileChange proposal.
///
/// This invariant deliberately lives below the runtime caller: durable settlement and later
/// run-grant receipt checks both invoke it, so replacing the result and ToolResult together cannot
/// rebind an approval to another transaction, path, operation, or receipt.
pub fn file_change_result_matches_frozen_proposal(
    result: &AgentFileChangeResult,
    proposal: &AgentFileChangeProposal,
) -> bool {
    if result.validate().is_err()
        || proposal.validate().is_err()
        || result.transaction_id != proposal.transaction_id
        || result.operation != proposal.operation
        || result.update_strategy != proposal.update_strategy
        || result.file_path != proposal.file_path
        || result.additions != proposal.additions
        || result.deletions != proposal.deletions
        || result.line_count != proposal.line_count
        || result.byte_count != proposal.byte_count
    {
        return false;
    }
    let succeeded = matches!(
        result.status,
        AgentFileChangeResultStatus::Applied | AgentFileChangeResultStatus::AlreadyApplied
    );
    if !succeeded {
        return true;
    }
    let Some(receipt) = proposal.execution.receipt.as_ref() else {
        return false;
    };
    let status_matches = match (proposal.execution.transaction.status, result.status) {
        // A normal terminal path reports `applied`. Startup reconciliation is allowed to
        // strengthen the same receipt-backed effect to `already_applied` after proving that the
        // target digest is already present. Both states remain bound to the exact durable receipt
        // below; the reverse transition would incorrectly claim a fresh execution.
        (
            crate::file_change::FileChangeStatus::Applied,
            AgentFileChangeResultStatus::Applied | AgentFileChangeResultStatus::AlreadyApplied,
        )
        | (
            crate::file_change::FileChangeStatus::AlreadyApplied,
            AgentFileChangeResultStatus::AlreadyApplied,
        ) => true,
        _ => false,
    };
    status_matches
        && receipt.transaction_id == result.transaction_id
        && receipt.file_path == result.file_path
        && receipt.target.revision() == result.revision.as_deref()
}

/// Resolves the common write/approval dimensions without granting any broader
/// filesystem scope. Full Access reaches `AutoApprove` because the trusted
/// frontend maps that mode to write=all + patch=auto_approve; custom modes can
/// reach the same route without gaining write=all.
pub fn file_change_approval_route(permissions: AgentPermissions) -> FileChangeApprovalRoute {
    if permissions.write == AgentWritePermission::Denied {
        FileChangeApprovalRoute::Denied
    } else if permissions.patch == AgentPatchPermission::AutoApprove {
        FileChangeApprovalRoute::AutoApprove
    } else {
        FileChangeApprovalRoute::RequireExplicitApproval
    }
}

/// Revalidates the authorization source at the host execution boundary. This
/// prevents an internal caller from labelling a manually-routed write as an
/// automatically approved action and bypassing the user's chosen mode.
pub fn file_change_authorized(
    permissions: AgentPermissions,
    source: FileChangeAuthorizationSource,
) -> bool {
    match (file_change_approval_route(permissions), source) {
        (FileChangeApprovalRoute::AutoApprove, FileChangeAuthorizationSource::Automatic)
        | (
            FileChangeApprovalRoute::AutoApprove | FileChangeApprovalRoute::RequireExplicitApproval,
            FileChangeAuthorizationSource::RunGrant,
        )
        | (
            FileChangeApprovalRoute::AutoApprove | FileChangeApprovalRoute::RequireExplicitApproval,
            FileChangeAuthorizationSource::ExplicitUser,
        ) => true,
        (FileChangeApprovalRoute::Denied, _)
        | (
            FileChangeApprovalRoute::RequireExplicitApproval,
            FileChangeAuthorizationSource::Automatic,
        ) => false,
    }
}

/// Structured host actions that publish files all share the policy above.
/// Command and Skill-script actions stay in their separate process policy,
/// because they can have broader side effects than a validated file writer.
pub fn proposed_action_uses_file_change_policy(action: &AgentProposedAction) -> bool {
    file_change_action_approval_status(action).is_some()
}

/// Returns the approval status carried by every structured file-change action.
/// This exhaustive classifier is shared by runtime and host checks so adding a
/// new action variant cannot silently update one policy boundary but not the
/// other.
pub fn file_change_action_approval_status(
    action: &AgentProposedAction,
) -> Option<AgentApprovalStatus> {
    match action {
        AgentProposedAction::FileChange { file_change } => Some(file_change.approval_status),
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

pub fn file_change_diff(change: &AgentFileChangeRecord) -> String {
    TextDiff::from_lines(&change.base_content, &change.content)
        .unified_diff()
        .header(
            &format!("a/{}", change.file_path),
            &format!("b/{}", change.file_path),
        )
        .to_string()
}

pub fn file_change_snapshot(
    change: &AgentFileChangeRecord,
) -> Result<AgentFileChangeSnapshot, String> {
    let (operation, update_strategy) = file_change_operation(change)?;
    let status = serde_json::from_value(serde_json::Value::String(change.status.clone()))
        .map_err(|_| "文件变更事务状态无效。".to_string())?;
    let snapshot = AgentFileChangeSnapshot {
        schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        transaction_id: change.id.clone(),
        conversation_id: change.conversation_id.clone(),
        project_id: change.project_id.clone(),
        file_path: change.file_path.clone(),
        operation,
        update_strategy,
        status,
        base_revision: change.base_revision.clone(),
        additions: change.additions,
        deletions: change.deletions,
        line_count: change.line_count,
        byte_count: change.byte_count,
        mutation_count: change.mutation_count,
        next_mutation_index: change.next_mutation_index,
        stats_final: change.stats_final,
        summary: change.summary.clone(),
        created_at: change.created_at,
        updated_at: change.updated_at,
    };
    snapshot.validate().map_err(str::to_string)?;
    Ok(snapshot)
}

fn file_change_operation(
    change: &AgentFileChangeRecord,
) -> Result<
    (
        AgentFileChangeOperation,
        Option<AgentFileChangeUpdateStrategy>,
    ),
    String,
> {
    match (change.operation.as_str(), change.strategy.as_deref()) {
        ("create", None) => Ok((AgentFileChangeOperation::Create, None)),
        ("update", Some("modify")) => Ok((
            AgentFileChangeOperation::Update,
            Some(AgentFileChangeUpdateStrategy::Modify),
        )),
        ("update", Some("rewrite")) => Ok((
            AgentFileChangeOperation::Update,
            Some(AgentFileChangeUpdateStrategy::Rewrite),
        )),
        _ => Err("文件变更事务的 operation/strategy 无效。".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentApprovalStatus, AgentCommandPermission, AgentFileChangeProposal, AgentPatchPermission,
        AgentReadPermission,
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

    #[test]
    fn shared_file_change_policy_routes_manual_custom_auto_and_full_access() {
        let manual = permissions();
        assert_eq!(
            file_change_approval_route(manual),
            FileChangeApprovalRoute::RequireExplicitApproval
        );
        assert!(!file_change_authorized(
            manual,
            FileChangeAuthorizationSource::Automatic
        ));
        assert!(file_change_authorized(
            manual,
            FileChangeAuthorizationSource::ExplicitUser
        ));

        let custom_auto = AgentPermissions {
            patch: AgentPatchPermission::AutoApprove,
            ..manual
        };
        assert_eq!(
            file_change_approval_route(custom_auto),
            FileChangeApprovalRoute::AutoApprove
        );
        assert!(file_change_authorized(
            custom_auto,
            FileChangeAuthorizationSource::Automatic
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
            file_change_approval_route(full_access),
            FileChangeApprovalRoute::AutoApprove
        );
        assert!(file_change_authorized(
            full_access,
            FileChangeAuthorizationSource::Automatic
        ));

        let denied = AgentPermissions {
            write: AgentWritePermission::Denied,
            patch: AgentPatchPermission::AutoApprove,
            ..manual
        };
        assert_eq!(
            file_change_approval_route(denied),
            FileChangeApprovalRoute::Denied
        );
        assert!(!file_change_authorized(
            denied,
            FileChangeAuthorizationSource::Automatic
        ));
        assert!(!file_change_authorized(
            denied,
            FileChangeAuthorizationSource::ExplicitUser
        ));
    }

    #[test]
    fn structured_file_change_actions_share_one_host_policy_domain() {
        let file_change = AgentProposedAction::FileChange {
            file_change: AgentFileChangeProposal {
                schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
                id: "diff-1".to_string(),
                transaction_id: "transaction-1".to_string(),
                operation: AgentFileChangeOperation::Create,
                update_strategy: None,
                file_path: "report.md".to_string(),
                inline_diff: None,
                base_revision: None,
                summary: None,
                additions: 0,
                deletions: 0,
                line_count: 0,
                byte_count: 0,
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

        for action in [&file_change, &materialization, &office] {
            assert!(proposed_action_uses_file_change_policy(action));
            assert!(file_change_action_approval_status(action).is_some());
        }

        assert!(!proposed_action_uses_file_change_policy(
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
        assert!(!proposed_action_uses_file_change_policy(
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

        let digest = crate::file_change::content_digest(b"");
        let diff_digest = crate::file_change::diff_digest("");
        let target = FileChangeContentState::Present {
            revision: crate::content_revision(b""),
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
            schema_version: crate::file_change::FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
            transaction,
            proposal: FileChangeProposal {
                schema_version: FILE_CHANGE_SCHEMA_VERSION,
                id: "diff-1".to_string(),
                transaction_id: "transaction-1".to_string(),
                operation: FileChangeOperation::Create,
                file_path: "report.md".to_string(),
                base: FileChangeContentState::Missing,
                target,
                diff_digest,
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
            trace_args_digest: digest.clone(),
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

        for missing in ["observation", "traceArgsDigest", "deleteJournal", "receipt"] {
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

        for unsupported in [1, 99] {
            let mut malformed = serde_json::to_value(direct_binding_fixture()).unwrap();
            malformed["schemaVersion"] = serde_json::json!(unsupported);
            assert!(
                serde_json::from_value::<crate::file_change::FileChangeDirectBinding>(malformed)
                    .is_err()
            );
        }
    }

    #[test]
    fn terminal_result_is_exactly_bound_to_frozen_proposal_and_receipt() {
        use crate::file_change::{
            FileChangeCommit, FileChangeOutcome, FileChangeReceipt, FileChangeStatus,
        };
        use crate::{
            AgentFileChangeOutcome, AgentFileChangeResultStatus,
            AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        };

        let prepared = direct_binding_fixture();
        let commit = FileChangeCommit {
            status: FileChangeStatus::Applied,
            receipt: FileChangeReceipt {
                schema_version: crate::file_change::FILE_CHANGE_SCHEMA_VERSION,
                transaction_id: prepared.transaction.id.clone(),
                operation: prepared.transaction.operation,
                file_path: prepared.transaction.file_path.clone(),
                outcome: FileChangeOutcome::Applied,
                base: prepared.transaction.base.clone(),
                target: prepared.transaction.target.clone(),
                proposal_digest: prepared.transaction.proposal_digest.clone(),
                committed_at: 2,
            },
            delete_journal: None,
        };
        let committed = prepared.with_commit(&commit).unwrap();
        let proposal = AgentFileChangeProposal {
            schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: "diff-1".to_string(),
            transaction_id: "transaction-1".to_string(),
            operation: AgentFileChangeOperation::Create,
            update_strategy: None,
            file_path: "report.md".to_string(),
            inline_diff: Some(crate::AgentGitDiffSnapshot {
                patch: String::new(),
                truncated: false,
            }),
            base_revision: None,
            summary: None,
            additions: 0,
            deletions: 0,
            line_count: 0,
            byte_count: 0,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(committed),
        };
        let result = AgentFileChangeResult {
            schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            status: AgentFileChangeResultStatus::Applied,
            outcome: AgentFileChangeOutcome::Applied,
            transaction_id: proposal.transaction_id.clone(),
            operation: proposal.operation,
            update_strategy: proposal.update_strategy,
            file_path: proposal.file_path.clone(),
            additions: proposal.additions,
            deletions: proposal.deletions,
            line_count: proposal.line_count,
            byte_count: proposal.byte_count,
            revision: proposal
                .execution
                .transaction
                .target
                .revision()
                .map(str::to_string),
            error_code: None,
            error: None,
            message: None,
        };
        assert!(file_change_result_matches_frozen_proposal(
            &result, &proposal
        ));

        let mut reconciled = result.clone();
        reconciled.status = AgentFileChangeResultStatus::AlreadyApplied;
        assert!(file_change_result_matches_frozen_proposal(
            &reconciled,
            &proposal
        ));

        let mut tampered = result.clone();
        tampered.transaction_id = "other-transaction".to_string();
        assert!(!file_change_result_matches_frozen_proposal(
            &tampered, &proposal
        ));
        let mut tampered = result.clone();
        tampered.file_path = "other.md".to_string();
        assert!(!file_change_result_matches_frozen_proposal(
            &tampered, &proposal
        ));
        let mut tampered = result.clone();
        tampered.revision = Some("other-revision".to_string());
        assert!(!file_change_result_matches_frozen_proposal(
            &tampered, &proposal
        ));
    }
}
