use super::*;
use crate::file_change::{
    derive_file_change_run_grant_scope, FileChangeRunGrantRecord, FileChangeRunGrantRef,
    FileChangeRunGrantStatus, APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION,
};
use crate::{
    AgentFileChangeOperation, AgentFileChangeProposal, AgentRunContext, AgentWritePermission,
};

/// Safe, typed boundary for durable FileChange approval-memory failures.
///
/// Repository, SQLite, OS and Rust diagnostics deliberately stop at this boundary. Callers may
/// project only the stable code and safe message into Runtime, ToolResult, IPC or Renderer state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileChangeRunGrantServiceError {
    StorageUnavailable,
    InvalidState,
}

impl FileChangeRunGrantServiceError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::StorageUnavailable => "runGrantStorageUnavailable",
            Self::InvalidState => "runGrantInvalid",
        }
    }

    pub const fn agent_error_code(self) -> &'static str {
        match self {
            Self::StorageUnavailable => "agent.file_change_run_grant_storage_unavailable",
            Self::InvalidState => "agent.file_change_run_grant_invalid",
        }
    }

    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::StorageUnavailable => {
                "File approval memory is temporarily unavailable; this file change was not executed."
            }
            Self::InvalidState => {
                "File approval memory is invalid or changed; this file change was not executed."
            }
        }
    }

    pub fn into_agent_error(self) -> crate::AgentError {
        crate::AgentError::structured(
            self.agent_error_code(),
            self.safe_message(),
            serde_json::json!({
                "type": "file_change_policy",
                "code": self.code(),
                "outcome": "definitely_not_executed",
                "recovery": "requestApproval",
            }),
        )
    }
}

impl std::fmt::Debug for FileChangeRunGrantServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.safe_message())
    }
}

impl std::fmt::Display for FileChangeRunGrantServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.safe_message())
    }
}

impl std::error::Error for FileChangeRunGrantServiceError {}

fn run_grant_storage_error<T>(_: T) -> FileChangeRunGrantServiceError {
    FileChangeRunGrantServiceError::StorageUnavailable
}

fn run_grant_repository_error(error: rusqlite::Error) -> FileChangeRunGrantServiceError {
    match error {
        rusqlite::Error::InvalidParameterName(_)
        | rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..)
        | rusqlite::Error::Utf8Error(_)
        | rusqlite::Error::InvalidColumnIndex(_)
        | rusqlite::Error::InvalidColumnName(_)
        | rusqlite::Error::InvalidColumnType(..) => FileChangeRunGrantServiceError::InvalidState,
        _ => FileChangeRunGrantServiceError::StorageUnavailable,
    }
}

fn run_grant_invalid_state<T>(_: T) -> FileChangeRunGrantServiceError {
    FileChangeRunGrantServiceError::InvalidState
}

impl StorageService {
    pub fn create_pending_file_change_run_grant(
        &self,
        grant: &FileChangeRunGrantRecord,
    ) -> Result<bool, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::insert_pending_run_grant(&connection, grant)
            .map_err(run_grant_repository_error)
    }

    pub fn get_active_file_change_run_grant(
        &self,
        run_id: &str,
    ) -> Result<Option<FileChangeRunGrantRecord>, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::get_active_run_grant(&connection, run_id)
            .map_err(run_grant_repository_error)
    }

    pub fn get_file_change_run_grant_for_pending_action(
        &self,
        pending_action_id: &str,
    ) -> Result<Option<FileChangeRunGrantRecord>, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::get_run_grant_for_pending_action(
            &connection,
            pending_action_id,
        )
        .map_err(run_grant_repository_error)
    }

    pub fn revoke_active_file_change_run_grant(
        &self,
        run_id: &str,
    ) -> Result<usize, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::revoke_active_run_grant(&connection, run_id, now_ms())
            .map_err(run_grant_repository_error)
    }

    pub fn retire_pending_file_change_run_grant(
        &self,
        pending_action_id: &str,
    ) -> Result<bool, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::retire_pending_run_grant_for_action(
            &connection,
            pending_action_id,
            now_ms(),
        )
        .map_err(run_grant_repository_error)
    }

    pub fn revoke_nonterminal_file_change_run_grants(
        &self,
        run_id: &str,
    ) -> Result<usize, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::revoke_nonterminal_run_grants(
            &connection,
            run_id,
            now_ms(),
        )
        .map_err(run_grant_repository_error)
    }

    pub fn list_active_file_change_run_grant_records(
        &self,
    ) -> Result<Vec<FileChangeRunGrantRecord>, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::list_active_run_grant_records(&connection)
            .map_err(run_grant_repository_error)
    }

    pub fn list_pending_file_change_run_grant_records(
        &self,
    ) -> Result<Vec<FileChangeRunGrantRecord>, FileChangeRunGrantServiceError> {
        let connection = self.state.connection().map_err(run_grant_storage_error)?;
        file_change_run_grant_repository::list_pending_run_grant_records(&connection)
            .map_err(run_grant_repository_error)
    }

    /// Resolves an active grant for one already-frozen current FileChange proposal.
    ///
    /// Absence or a safely narrowed scope returns `Ok(None)` so Runtime retains explicit approval.
    /// Malformed durable authority fails closed as an error.
    pub fn resolve_active_file_change_run_grant(
        &self,
        proposal: &AgentFileChangeProposal,
        run_context: &AgentRunContext,
    ) -> Result<Option<FileChangeRunGrantRef>, FileChangeRunGrantServiceError> {
        let Some(grant) = self.get_active_file_change_run_grant(&proposal.execution.run_id)? else {
            return Ok(None);
        };
        if !file_change_run_grant_authorizes(&grant, None, proposal, run_context)? {
            return Ok(None);
        }
        grant.reference().map(Some).map_err(run_grant_invalid_state)
    }

    /// Effect-boundary validation of the exact checkpoint reference and frozen proposal.
    pub fn validate_file_change_run_grant(
        &self,
        expected: &FileChangeRunGrantRef,
        proposal: &AgentFileChangeProposal,
        run_context: &AgentRunContext,
    ) -> Result<bool, FileChangeRunGrantServiceError> {
        expected.validate().map_err(run_grant_invalid_state)?;
        let Some(grant) = self.get_active_file_change_run_grant(&proposal.execution.run_id)? else {
            return Ok(false);
        };
        file_change_run_grant_authorizes(&grant, Some(expected), proposal, run_context)
    }

    /// Validates that a non-authoritative pending intent is still bound to the exact explicit
    /// FileChange action from which it was created.
    ///
    /// Startup recovery uses this before preserving an intent. The intent remains inactive until
    /// an applied receipt settles it; this check prevents a tampered action, changed contract
    /// revision, or replaced scope directory from becoming authority later.
    pub fn validate_pending_file_change_run_grant_binding(
        &self,
        grant: &FileChangeRunGrantRecord,
        proposal: &AgentFileChangeProposal,
        run_context: &AgentRunContext,
    ) -> Result<bool, FileChangeRunGrantServiceError> {
        grant.validate().map_err(run_grant_invalid_state)?;
        if grant.status != FileChangeRunGrantStatus::Pending
            || grant.granting_pending_action_id
                != crate::canonical_pending_action_id(&grant.run_id, &proposal.id)
        {
            return Ok(false);
        }
        file_change_run_grant_scope_authorizes(grant, proposal, run_context)
    }
}

pub(super) fn file_change_run_grant_authorizes(
    grant: &FileChangeRunGrantRecord,
    expected: Option<&FileChangeRunGrantRef>,
    proposal: &AgentFileChangeProposal,
    run_context: &AgentRunContext,
) -> Result<bool, FileChangeRunGrantServiceError> {
    grant.validate().map_err(run_grant_invalid_state)?;
    if grant.status != FileChangeRunGrantStatus::Active {
        return Ok(false);
    }
    if let Some(expected) = expected {
        if expected.grant_id != grant.grant_id
            || expected.revision != grant.revision
            || expected.schema_version != grant.schema_version
            || expected.apply_patch_contract_revision != grant.apply_patch_contract_revision
        {
            return Ok(false);
        }
    }
    file_change_run_grant_scope_authorizes(grant, proposal, run_context)
}

fn file_change_run_grant_scope_authorizes(
    grant: &FileChangeRunGrantRecord,
    proposal: &AgentFileChangeProposal,
    run_context: &AgentRunContext,
) -> Result<bool, FileChangeRunGrantServiceError> {
    proposal
        .execution
        .validate()
        .map_err(run_grant_invalid_state)?;
    if grant.apply_patch_contract_revision != APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION
        || !matches!(
            proposal.operation,
            AgentFileChangeOperation::Create | AgentFileChangeOperation::Update
        )
        || proposal.execution.source_tool_name != "apply_patch"
        || proposal.execution.source_call_id != proposal.id
        || grant.run_id != proposal.execution.run_id
        || run_context.conversation_id.as_deref() != Some(grant.conversation_id.as_str())
        || proposal.execution.conversation_id != grant.conversation_id
        || run_context.project_id.as_deref() != grant.project_id.as_deref()
        || proposal.execution.project_id.as_deref() != grant.project_id.as_deref()
        || proposal.execution.permission_revision != grant.granting_permission_revision
        || proposal.execution.tool_set_revision != grant.granting_tool_set_revision
        || proposal.execution.provider_wire_revision != grant.granting_provider_wire_revision
        || run_context.permissions.write == AgentWritePermission::Denied
    {
        return Ok(false);
    }
    derive_file_change_run_grant_scope(proposal, run_context)
        .map(|scope| scope.matches_record(grant))
        .map_err(run_grant_invalid_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_change::{
        file_change_workspace_identity, FileChangeContentState, FileChangeDirectBinding,
        FileChangeDirectoryIdentity, FileChangeOperation, FileChangeOutcome, FileChangeProposal,
        FileChangeRunGrantScopeKind, FileChangeStatus, FileChangeTransaction,
        FileObservationCheckpoint, FileObservationIdentity, FileObservationState,
        FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION, FILE_CHANGE_SCHEMA_VERSION,
        FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
    };
    use crate::{
        AgentApprovalStatus, AgentFileChangeOperation, AgentGitDiffSnapshot, AgentPermissions,
        AgentWorkspaceContext, AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
    };
    use std::fs;
    use tempfile::tempdir;

    fn fixture(
        workspace_root: &Path,
    ) -> (
        FileChangeRunGrantRecord,
        AgentFileChangeProposal,
        AgentRunContext,
    ) {
        let canonical_root = workspace_root.canonicalize().unwrap();
        let target_path = canonical_root.join("file.txt");
        let target_content = "x\n";
        let target_state = FileChangeContentState::Present {
            revision: crate::content_revision(target_content.as_bytes()),
            digest: crate::file_change::content_digest(target_content.as_bytes()),
            byte_count: target_content.len() as u64,
        };
        let proposal_digest = crate::file_change::content_digest(b"proposal");
        let transaction = FileChangeTransaction {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: "transaction-1".to_string(),
            operation: FileChangeOperation::Create,
            file_path: "file.txt".to_string(),
            status: FileChangeStatus::WaitingApproval,
            outcome: FileChangeOutcome::DefinitelyNotExecuted,
            base: FileChangeContentState::Missing,
            target: target_state.clone(),
            proposal_digest: proposal_digest.clone(),
            created_at: 1,
            updated_at: 1,
        };
        let execution = FileChangeDirectBinding {
            schema_version: crate::file_change::FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
            transaction,
            proposal: FileChangeProposal {
                schema_version: FILE_CHANGE_SCHEMA_VERSION,
                id: "call-1".to_string(),
                transaction_id: "transaction-1".to_string(),
                operation: FileChangeOperation::Create,
                file_path: "file.txt".to_string(),
                base: FileChangeContentState::Missing,
                target: target_state,
                diff_digest: crate::file_change::diff_digest(""),
                proposal_digest,
                additions: 1,
                deletions: 0,
            },
            observation_id: format!("fobs_{}", "0".repeat(32)),
            observation: FileObservationCheckpoint {
                schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
                observation_id: format!("fobs_{}", "0".repeat(32)),
                source_tool_call_id: "read-call".to_string(),
                conversation_id: "conversation-1".to_string(),
                run_id: "run-1".to_string(),
                canonical_target: target_path.to_string_lossy().into_owned(),
                state: FileObservationState::Missing,
                parent_identity: FileObservationIdentity::from_metadata(
                    &fs::metadata(&canonical_root).unwrap(),
                ),
                created_at_ms: 1,
                expires_at_ms: 1 + FILE_OBSERVATION_TTL_MS,
            },
            source_tool_name: "apply_patch".to_string(),
            source_call_id: "call-1".to_string(),
            source_args_digest: crate::file_change::content_digest(b"args"),
            trace_args_digest: crate::file_change::content_digest(b"trace-args"),
            staged_transaction_id: None,
            conversation_id: "conversation-1".to_string(),
            project_id: Some("project-1".to_string()),
            run_id: "run-1".to_string(),
            staged_transaction_revision: None,
            canonical_target: target_path.to_string_lossy().into_owned(),
            base_content: None,
            target_content: Some(target_content.to_string()),
            delete_journal: None,
            receipt: None,
            permission_revision: "permission-v1".to_string(),
            tool_set_revision: "tool-set-v1".to_string(),
            provider_wire_revision: "provider-wire-v1".to_string(),
        };
        let proposal = AgentFileChangeProposal {
            schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: "call-1".to_string(),
            transaction_id: "transaction-1".to_string(),
            operation: AgentFileChangeOperation::Create,
            update_strategy: None,
            file_path: "file.txt".to_string(),
            inline_diff: Some(AgentGitDiffSnapshot {
                patch: String::new(),
                truncated: false,
            }),
            base_revision: None,
            summary: None,
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: 2,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(execution),
        };
        let context = AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: Some("project-1".to_string()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-1".to_string()),
                display_name: Some("workspace".to_string()),
                root_path: Some(canonical_root.to_string_lossy().into_owned()),
            }),
            permissions: AgentPermissions {
                write: AgentWritePermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            attachment_library: None,
            collaboration_identity: None,
        };
        let grant = FileChangeRunGrantRecord {
            schema_version: FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
            grant_id: "grant-1".to_string(),
            revision: 1,
            status: FileChangeRunGrantStatus::Active,
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: Some("project-1".to_string()),
            scope_kind: FileChangeRunGrantScopeKind::Workspace,
            workspace_identity: Some(
                file_change_workspace_identity(
                    Some("project-1"),
                    Some("project-1"),
                    &canonical_root,
                )
                .unwrap(),
            ),
            canonical_scope_path: canonical_root.to_string_lossy().into_owned(),
            scope_directory_identity: FileChangeDirectoryIdentity::read(&canonical_root).unwrap(),
            granting_pending_action_id: crate::canonical_pending_action_id(
                "run-1",
                "granting-call",
            ),
            base_write_permission: AgentWritePermission::WorkspaceOnly,
            granting_permission_revision: "permission-v1".to_string(),
            granting_tool_set_revision: "tool-set-v1".to_string(),
            granting_provider_wire_revision: "provider-wire-v1".to_string(),
            apply_patch_contract_revision: APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION.to_string(),
            activation_result_digest: Some(crate::file_change::content_digest(b"receipt")),
            created_at: 1,
            activated_at: Some(2),
            inactive_at: None,
            revoked_at: None,
        };
        (grant, proposal, context)
    }

    #[test]
    fn grant_rejects_revision_permission_and_workspace_identity_drift() {
        let root = tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let (grant, proposal, context) = fixture(&workspace);
        assert!(file_change_run_grant_authorizes(&grant, None, &proposal, &context).unwrap());

        for field in ["permission", "tool_set", "provider_wire"] {
            let mut drifted = proposal.clone();
            match field {
                "permission" => drifted.execution.permission_revision = "permission-v2".into(),
                "tool_set" => drifted.execution.tool_set_revision = "tool-set-v2".into(),
                "provider_wire" => {
                    drifted.execution.provider_wire_revision = "provider-wire-v2".into()
                }
                _ => unreachable!(),
            }
            assert!(!file_change_run_grant_authorizes(&grant, None, &drifted, &context).unwrap());
        }

        let mut denied = context.clone();
        denied.permissions.write = AgentWritePermission::Denied;
        assert!(!file_change_run_grant_authorizes(&grant, None, &proposal, &denied).unwrap());

        let moved = root.path().join("former-workspace");
        fs::rename(&workspace, &moved).unwrap();
        fs::create_dir(&workspace).unwrap();
        assert!(!file_change_run_grant_authorizes(&grant, None, &proposal, &context).unwrap());
    }

    #[test]
    fn grant_scope_derivation_rejects_delete_before_scope_materialization() {
        let root = tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let (_grant, mut proposal, context) = fixture(&workspace);
        proposal.operation = AgentFileChangeOperation::Delete;

        assert_eq!(
            derive_file_change_run_grant_scope(&proposal, &context).unwrap_err(),
            "FileChange Run grants require create or update"
        );
    }

    #[test]
    fn sqlite_failures_stop_at_the_typed_safe_run_grant_boundary() {
        let root = tempdir().unwrap();
        let database_path = root.path().join("storage.sqlite");
        let storage = StorageService::open(&database_path).unwrap();
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .execute_batch("DROP TABLE agent_file_change_run_grants")
            .unwrap();

        let error = storage
            .get_active_file_change_run_grant("run-storage-failure-canary")
            .unwrap_err();

        assert_eq!(error, FileChangeRunGrantServiceError::StorageUnavailable);
        assert_eq!(error.code(), "runGrantStorageUnavailable");
        assert_eq!(
            error.to_string(),
            "File approval memory is temporarily unavailable; this file change was not executed."
        );
        let public = format!("{error:?}\n{error}");
        for forbidden in [
            "SQLite",
            "sqlite",
            "no such table",
            "agent_file_change_run_grants",
            "rusqlite",
            "Database(",
            "FileChangeRunGrantServiceError",
        ] {
            assert!(
                !public.contains(forbidden),
                "leaked `{forbidden}`: {public}"
            );
        }
    }

    #[test]
    fn malformed_durable_grant_is_typed_as_invalid_state_without_its_diagnostic() {
        const CANARY: &str = "RUST_VARIANT_AND_ROW_DIAGNOSTIC_CANARY";
        let error =
            run_grant_repository_error(rusqlite::Error::InvalidParameterName(CANARY.to_string()));

        assert_eq!(error, FileChangeRunGrantServiceError::InvalidState);
        assert_eq!(error.code(), "runGrantInvalid");
        assert!(!error.to_string().contains(CANARY));
        assert!(!format!("{error:?}").contains(CANARY));
        assert!(!format!("{error:?}").contains("InvalidParameterName"));
    }
}
