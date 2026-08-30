use super::*;
use mycopilot_core::command::CommandSessionManager;
use mycopilot_core::skills::{
    LocalSkillInstallRequest, LocalSkillUpdateRequest, SkillInstallationId,
    SkillInstallationOutcome, SkillInstallationService, SkillUninstallExactRequest,
};
use mycopilot_core::storage::models::{
    AgentActionAuditRecord, AgentFileChangeRecord, AgentPendingActionRecord,
    ChatConversationRecord, ChatMessageRecord, ModelConfigRecord, ModelSettingsRecord,
    ProjectRecord,
};
use mycopilot_core::{
    AgentActivatedSkill, AgentCommandRequest, AgentCommandRiskLevel, AgentCommandSessionListInput,
    AgentCommandSessionStatus, AgentContextCheckpointToolCall, AgentFileChangeOperation,
    AgentFileChangeProposal, AgentFileChangeUpdateStrategy, AgentPermissions,
    AgentProviderToolCallIdentity, AgentSkillActivation, AgentSkillMaterializationRequest,
    AgentToolIdentity, AgentUsageSummaryRange, AgentWorkspaceContext, AgentWritePermission,
    ContextCompactionGeneration, ContextCompactionPrefix, ContextCompactionSourceItem,
    ContextCompactionSummary, ContextCompactionSummaryDraft, ContextJournalCursor,
    ConversationModelContextItem, ConversationTraceToolResultStatus, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
    CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use serde_json::{json, Value};
use std::fs;
use tempfile::tempdir;

mod automation_turn;
mod cancellation;
mod collaboration_harness;
mod command_sessions;
mod context_history;
mod context_runtime;
mod deletion;
mod file_change_permissions;
mod file_change_source_boundary;
mod historical_compatibility_boundary;
mod human_root_notifications;
mod image_generation;
mod managed_command_loop;
mod mcp_approval_expiry;
mod mcp_approval_lifecycle;
mod office;
mod pending_actions;
mod provider_profiles;
mod provider_runtime_capability_boundary;
mod provider_transition;
mod skills;
mod staged_file_change_execution;
mod steering;
mod terminal_events;
mod usage_lifecycle;

fn completed_output_for_terminal_gate() -> AgentChatOutput {
    AgentChatOutput {
        content: "finished".to_string(),
        status: AgentRunStatus::Completed,
        run_id: "run-terminal-gate".to_string(),
        events: Vec::new(),
        tool_definitions: Vec::new(),
        todo: None,
        usage: None,
        finish_reason: Some("stop".to_string()),
        proposed_actions: Vec::new(),
        conversation_turn_trace: None,
    }
}

fn command_test_input(workspace: &Path) -> AgentChatInput {
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "token",
        "model": "model-1",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "messages": []
    }))
    .unwrap();
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-command-policy".to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("command-policy-test".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            write: mycopilot_core::AgentWritePermission::WorkspaceOnly,
            command: mycopilot_core::AgentCommandPermission::AutoApprove,
            command_safety: mycopilot_core::AgentCommandSafetyPolicy::Guarded,
            ..Default::default()
        },
    });
    input
}

fn command_request(id: &str, command: &str) -> AgentCommandRequest {
    AgentCommandRequest {
        id: id.to_string(),
        command: command.to_string(),
        cwd: None,
        timeout_ms: Some(5_000),
        approval_status: AgentApprovalStatus::Required,
        risk_level: Some(AgentCommandRiskLevel::ReadOnly),
        reason: Some("exercise server authorization boundary".to_string()),
        observe: None,
        inputs: Vec::new(),
        runtime_binding: None,
        managed_office_script: None,
    }
}

fn direct_file_change_fixture(
    run_id: &str,
    conversation_id: &str,
    call_id: &str,
    paths: (&str, &str),
    base_content: Option<&str>,
    target_content: Option<&str>,
    approval_status: AgentApprovalStatus,
) -> (AgentProposedAction, AgentToolCall) {
    use mycopilot_core::file_change::{
        proposal_digest, FileChangeBase, FileChangeCommitter, FileChangeDirectBinding,
        FileChangeMutation, FileChangeOperation, FileChangeOutcome, FileChangePathPolicy,
        FileChangePlanRequest, FileChangePlanner, FileChangeProposal, FileChangeStatus,
        FileChangeTransaction, FileObservationCheckpoint, FileObservationDirectoryIdentity,
        FileObservationIdentity, FileObservationState, FILE_CHANGE_SCHEMA_VERSION,
        FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
    };

    let (file_path, canonical_target) = paths;
    let operation = match (base_content, target_content) {
        (None, Some(_)) => FileChangeOperation::Create,
        (Some(_), Some(_)) => FileChangeOperation::Update,
        (Some(_), None) => FileChangeOperation::Delete,
        (None, None) => panic!("a file-change fixture must have a base or target"),
    };
    let base_revision =
        base_content.map(|content| mycopilot_core::content_revision(content.as_bytes()));
    let base = match (base_content, base_revision.as_deref()) {
        (Some(content), Some(revision)) => FileChangeBase::Existing { content, revision },
        (None, None) => FileChangeBase::Missing,
        _ => unreachable!("fixture base content and revision are total"),
    };
    let mutation = match (operation, target_content) {
        (FileChangeOperation::Delete, None) => FileChangeMutation::Delete,
        (_, Some(content)) => FileChangeMutation::Complete(content.to_string()),
        _ => unreachable!("fixture operation and target content are consistent"),
    };
    let plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation,
            file_path,
            base,
            mutation,
        })
        .expect("valid test FileChange plan");

    let observation_digest = proposal_digest(&call_id).expect("digest observation fixture id");
    let observation_hex = observation_digest
        .rsplit_once(':')
        .expect("content revision has a digest suffix")
        .1;
    let observation_id = format!("fobs_{}", &observation_hex[..32]);
    let mut request = json!({
        "action": "apply",
        "operation": match operation {
            FileChangeOperation::Create => "create",
            FileChangeOperation::Update => "update",
            FileChangeOperation::Delete => "delete",
        },
        "filePath": file_path,
    });
    if operation != FileChangeOperation::Create {
        request["observationId"] = Value::String(observation_id.clone());
    }
    if let Some(content) = target_content {
        request["content"] = Value::String(content.to_string());
    }
    let args = json!({ "request": request });

    let transaction_id = format!("file-change-direct-v1:{call_id}");
    let transaction = FileChangeTransaction {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: transaction_id.clone(),
        operation,
        file_path: plan.file_path.clone(),
        status: FileChangeStatus::WaitingApproval,
        outcome: FileChangeOutcome::DefinitelyNotExecuted,
        base: plan.base.clone(),
        target: plan.target.clone(),
        proposal_digest: plan.proposal_digest.clone(),
        created_at: 1,
        updated_at: 1,
    };
    let proposal = FileChangeProposal {
        schema_version: FILE_CHANGE_SCHEMA_VERSION,
        id: call_id.to_string(),
        transaction_id,
        operation,
        file_path: plan.file_path.clone(),
        base: plan.base.clone(),
        target: plan.target.clone(),
        diff_digest: plan.diff_digest.clone(),
        proposal_digest: plan.proposal_digest.clone(),
        additions: plan.additions,
        deletions: plan.deletions,
    };
    let delete_journal = if operation == FileChangeOperation::Delete {
        let canonical_path = std::path::Path::new(canonical_target);
        let workspace = canonical_path
            .parent()
            .expect("Direct delete fixture target has a parent");
        let target = FileChangePathPolicy::new(Some(workspace), false)
            .resolve(file_path)
            .expect("resolve Direct delete fixture target");
        Some(
            FileChangeCommitter
                .prepare_delete(&transaction.id, &target, &plan, 1)
                .expect("prepare Direct delete fixture journal"),
        )
    } else {
        None
    };
    let canonical_path = std::path::Path::new(canonical_target);
    let parent_directory_identity = FileObservationDirectoryIdentity::read(
        canonical_path
            .parent()
            .expect("Direct fixture target has a parent"),
    )
    .or_else(|_| {
        FileObservationDirectoryIdentity::read(
            &std::env::temp_dir()
                .canonicalize()
                .expect("canonical Direct fixture fallback parent"),
        )
    })
    .expect("Direct fixture parent directory identity");
    let observation_state = match &plan.base {
        mycopilot_core::file_change::FileChangeContentState::Missing => {
            FileObservationState::Missing
        }
        mycopilot_core::file_change::FileChangeContentState::Present { revision, .. } => {
            let target_metadata = std::fs::metadata(canonical_path)
                .unwrap_or_else(|_| std::fs::metadata(canonical_path.parent().unwrap()).unwrap());
            FileObservationState::Existing {
                revision: revision.clone(),
                identity: FileObservationIdentity::from_metadata(&target_metadata),
            }
        }
    };
    let execution = FileChangeDirectBinding {
        schema_version: mycopilot_core::file_change::FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction,
        proposal,
        observation_id: observation_id.clone(),
        observation: FileObservationCheckpoint {
            schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
            observation_id: observation_id.clone(),
            source_tool_call_id: if operation == FileChangeOperation::Create {
                call_id.to_string()
            } else {
                format!("read-{call_id}")
            },
            conversation_id: conversation_id.to_string(),
            run_id: run_id.to_string(),
            canonical_target: canonical_target.to_string(),
            state: observation_state,
            parent_directory_identity,
            created_at_ms: 1,
            expires_at_ms: 1 + FILE_OBSERVATION_TTL_MS,
        },
        source_tool_name: "apply_patch".to_string(),
        source_call_id: call_id.to_string(),
        source_args_digest: proposal_digest(&args).expect("digest fixture Tool Call arguments"),
        trace_args_digest: mycopilot_core::file_change_support::apply_patch_trace_args_digest(
            &args,
        )
        .expect("digest fixture durable Trace arguments"),
        staged_transaction_id: None,
        conversation_id: conversation_id.to_string(),
        project_id: None,
        run_id: run_id.to_string(),
        staged_transaction_revision: None,
        canonical_target: canonical_target.to_string(),
        base_content: plan.base_content.clone(),
        target_content: plan.target_content.clone(),
        delete_journal,
        receipt: None,
        permission_revision: "permission-revision-test".to_string(),
        tool_set_revision: "tool-set-revision-test".to_string(),
        provider_wire_revision: "provider-wire-revision-test".to_string(),
    };
    execution
        .validate()
        .expect("valid direct execution binding");

    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "apply_patch".to_string(),
        args,
        approval_status,
        reason: None,
    };
    let file_change = mycopilot_core::AgentFileChangeProposal {
        schema_version: mycopilot_core::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        id: call_id.to_string(),
        transaction_id: execution.transaction.id.clone(),
        operation: match operation {
            FileChangeOperation::Create => mycopilot_core::AgentFileChangeOperation::Create,
            FileChangeOperation::Update => mycopilot_core::AgentFileChangeOperation::Update,
            FileChangeOperation::Delete => mycopilot_core::AgentFileChangeOperation::Delete,
        },
        update_strategy: None,
        file_path: plan.file_path.clone(),
        inline_diff: Some(mycopilot_core::AgentGitDiffSnapshot {
            patch: plan.diff.clone(),
            truncated: false,
        }),
        base_revision: plan.base.revision().map(ToString::to_string),
        summary: Some("test Direct file change".to_string()),
        additions: plan.additions,
        deletions: plan.deletions,
        line_count: target_content
            .map(|content| content.lines().count())
            .unwrap_or(0) as u64,
        byte_count: target_content.map(str::len).unwrap_or(0) as u64,
        approval_status,
        execution: Box::new(execution),
    };
    file_change
        .validate()
        .expect("valid Direct FileChange proposal");
    let action = AgentProposedAction::FileChange { file_change };
    (action, call)
}

struct StagedFileChangeFixtureIdentity<'a> {
    run_id: &'a str,
    conversation_id: &'a str,
    call_id: &'a str,
    transaction_id: &'a str,
}

fn staged_file_change_fixture(
    identity: StagedFileChangeFixtureIdentity<'_>,
    paths: (&str, &str),
    content: &str,
    approval_status: AgentApprovalStatus,
) -> (AgentFileChangeProposal, AgentToolCall) {
    let StagedFileChangeFixtureIdentity {
        run_id,
        conversation_id,
        call_id,
        transaction_id,
    } = identity;
    let (action, _) = direct_file_change_fixture(
        run_id,
        conversation_id,
        call_id,
        paths,
        None,
        Some(content),
        approval_status,
    );
    let AgentProposedAction::FileChange { file_change } = action else {
        unreachable!("the fixture constructor returns a FileChange")
    };
    let mut execution = *file_change.execution;
    execution.transaction.id = transaction_id.to_string();
    execution.proposal.transaction_id = transaction_id.to_string();
    execution.staged_transaction_id = Some(transaction_id.to_string());
    execution.staged_transaction_revision = Some(1);
    let args = json!({
        "request": {
            "action": "commit",
            "transactionId": transaction_id,
            "expectedDraftRevision": 1,
        }
    });
    execution.source_args_digest = mycopilot_core::file_change::proposal_digest(&args)
        .expect("digest staged fixture Tool Call arguments");
    execution.trace_args_digest =
        mycopilot_core::file_change_support::apply_patch_trace_args_digest(&args)
            .expect("digest staged fixture durable Trace arguments");
    execution
        .validate()
        .expect("valid staged FileChange execution binding");
    let proposal = AgentFileChangeProposal {
        schema_version: mycopilot_core::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        id: call_id.to_string(),
        transaction_id: transaction_id.to_string(),
        operation: AgentFileChangeOperation::Create,
        update_strategy: None,
        file_path: paths.0.to_string(),
        inline_diff: None,
        base_revision: None,
        summary: None,
        additions: execution.proposal.additions,
        deletions: execution.proposal.deletions,
        line_count: content.lines().count() as u64,
        byte_count: content.len() as u64,
        approval_status,
        execution: Box::new(execution),
    };
    proposal
        .validate()
        .expect("valid Staged FileChange proposal");
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "apply_patch".to_string(),
        args,
        approval_status,
        reason: None,
    };
    (proposal, call)
}

fn staged_file_change_record(
    proposal: &AgentFileChangeProposal,
    status: &str,
) -> AgentFileChangeRecord {
    let execution = &proposal.execution;
    let operation = match execution.transaction.operation {
        mycopilot_core::file_change::FileChangeOperation::Create => "create",
        mycopilot_core::file_change::FileChangeOperation::Update => "update",
        mycopilot_core::file_change::FileChangeOperation::Delete => {
            panic!("Staged FileChange fixtures do not support delete")
        }
    };
    let strategy = match (proposal.operation, proposal.update_strategy) {
        (AgentFileChangeOperation::Create, None) => None,
        (AgentFileChangeOperation::Update, Some(AgentFileChangeUpdateStrategy::Modify)) => {
            Some("modify".to_string())
        }
        (AgentFileChangeOperation::Update, Some(AgentFileChangeUpdateStrategy::Rewrite)) => {
            Some("rewrite".to_string())
        }
        _ => panic!("invalid current Staged FileChange operation/strategy"),
    };
    let revision = execution
        .staged_transaction_revision
        .expect("Staged fixture has a draft revision");
    AgentFileChangeRecord {
        schema_version:
            mycopilot_core::storage::file_change_repository::AGENT_FILE_CHANGE_SCHEMA_VERSION,
        id: proposal.transaction_id.clone(),
        conversation_id: execution.conversation_id.clone(),
        project_id: execution.project_id.clone(),
        run_id: execution.run_id.clone(),
        source_tool_name: execution.source_tool_name.clone(),
        source_tool_call_id: format!("begin-{}", proposal.transaction_id),
        source_tool_arguments_digest: {
            let mut request = json!({
                "action": "begin",
                "operation": operation,
                "filePath": proposal.file_path,
            });
            if operation == "update" {
                request["observationId"] = Value::String(execution.observation_id.clone());
                request["strategy"] = Value::String(
                    strategy
                        .clone()
                        .expect("Staged update fixture has an explicit strategy"),
                );
            }
            mycopilot_core::file_change::proposal_digest(&json!({ "request": request })).unwrap()
        },
        permission_revision: execution.permission_revision.clone(),
        tool_set_revision: execution.tool_set_revision.clone(),
        provider_wire_revision: execution.provider_wire_revision.clone(),
        observation_id: execution.observation_id.clone(),
        observation_json: serde_json::to_string(&execution.observation).unwrap(),
        file_path: proposal.file_path.clone(),
        operation: operation.to_string(),
        strategy,
        status: status.to_string(),
        base_revision: proposal.base_revision.clone(),
        base_content: execution.base_content.clone().unwrap_or_default(),
        content: execution.target_content.clone().unwrap(),
        draft_revision: revision,
        next_mutation_index: revision,
        additions: proposal.additions,
        deletions: proposal.deletions,
        line_count: proposal.line_count,
        byte_count: proposal.byte_count,
        mutation_count: revision,
        stats_final: true,
        summary: proposal.summary.clone(),
        final_action_id: Some(proposal.id.clone()),
        final_action_arguments_digest: Some(proposal.execution.source_args_digest.clone()),
        final_permission_revision: Some(proposal.execution.permission_revision.clone()),
        final_tool_set_revision: Some(proposal.execution.tool_set_revision.clone()),
        final_provider_wire_revision: Some(proposal.execution.provider_wire_revision.clone()),
        created_at: 1,
        updated_at: 1,
        expires_at: i64::MAX,
    }
}

fn completed_trace(
    conversation_id: &str,
    assistant_message_id: &str,
    content_bytes: u64,
) -> ConversationTurnTrace {
    let call_id = history_call_id();
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-history".to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "I am creating the requested file.".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolCall {
                sequence: 1,
                call_id: call_id.clone(),
                tool: "apply_patch".to_string(),
                operation: json!({
                    "request": {
                        "action": "apply",
                        "operation": "create",
                        "filePath": "src/history.rs",
                        "contentBytes": content_bytes
                    }
                }),
                provenance: AgentToolIdentity::Builtin {
                    tool_name: "apply_patch".to_string(),
                },
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id,
                tool: "apply_patch".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({
                    "filePath": "src/history.rs",
                    "status": "applied",
                    "additions": 3,
                    "deletions": 0
                }),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: Default::default(),
            },
        ],
    }
}

fn history_call_id() -> String {
    format!("tc1_{}", "A".repeat(43))
}

fn test_compaction_draft(
    prefix: &ContextCompactionPrefix,
    id: &str,
    content: &str,
    source_input_tokens: u64,
    created_at: i64,
) -> ContextCompactionSummaryDraft {
    assert!(source_input_tokens > 20);
    ContextCompactionSummaryDraft {
        id: id.to_string(),
        source_revision: prefix.source_revision.clone(),
        content: content.to_string(),
        continuity: mycopilot_core::ContextContinuitySnapshot::from_prefix(prefix).unwrap(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens,
        summary_input_tokens: 10,
        continuity_input_tokens: 10,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 20,
        created_at,
    }
}

fn test_model_settings() -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: "token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "model-1".to_string(),
            display_name: "Model 1".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(128_000),
            provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0.01".to_string(),
            cached_input_price: String::new(),
            output_price: "0.02".to_string(),
            enabled: true,
        }],
    }
}

fn seed_root_effective_permissions(
    storage: &StorageService,
    root_agent_id: &str,
    conversation_id: &str,
    suffix: &str,
    permissions: AgentPermissions,
) -> mycopilot_core::AgentEffectivePermissionSnapshot {
    let (conversation, revision) = storage.load_conversation_for_turn(conversation_id).unwrap();
    let mut conversation = conversation.unwrap();
    let assistant_message_id = format!("assistant-permission-seed-{suffix}");
    let run_id = format!("run-permission-seed-{suffix}");
    let created_at = mycopilot_core::storage::now_ms().max(conversation.updated_at + 1);
    conversation.messages.push(ChatMessageRecord {
        id: assistant_message_id.clone(),
        role: "assistant".to_string(),
        content: "permission seed".to_string(),
        created_at,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    conversation.updated_at = created_at;
    let trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        &run_id,
        conversation_id,
        &assistant_message_id,
    );
    storage
        .save_conversation_and_begin_turn(
            conversation,
            revision,
            None,
            mycopilot_core::AgentTurnPermissionSource::HostAuthenticatedRoot(permissions),
            &trace,
            created_at,
            created_at,
        )
        .unwrap();
    let snapshot = storage
        .get_agent_effective_permission_snapshot(root_agent_id)
        .unwrap()
        .unwrap();
    let terminal = mycopilot_core::completed_conversation_trace_without_items(
        &run_id,
        conversation_id,
        &assistant_message_id,
    );
    storage
        .finalize_chat_message_with_conversation_trace(
            conversation_id,
            &assistant_message_id,
            "permission seed complete",
            Some("sent"),
            "completed",
            &terminal,
            created_at,
            created_at + 1,
        )
        .unwrap();
    snapshot
}

fn checkpoint_call_for_command(command: &AgentCommandRequest) -> AgentToolCall {
    let mut args = json!({
        "command": command.command,
        "cwd": command.cwd,
        "reason": command.reason,
        "observe": command.observe,
        "inputs": command.inputs.iter().map(|binding| json!({
            "mountPath": binding.mount_path,
            "path": mycopilot_core::file_input::model_path_for_agent_file_input_ref(&binding.source),
        })).collect::<Vec<_>>(),
    });
    if let Some(profile) = command.runtime_binding.as_ref().and_then(|binding| {
        (binding.profile != mycopilot_core::AgentCommandRuntimeProfile::Pdf)
            .then_some(binding.profile)
    }) {
        args.as_object_mut()
            .expect("command checkpoint args are an object")
            .insert("runtimeProfile".to_string(), json!(profile));
    }
    AgentToolCall {
        id: command.id.clone(),
        tool: "run_command".to_string(),
        args,
        approval_status: command.approval_status,
        reason: command.reason.clone(),
    }
}

fn save_test_pending_provider(
    storage: &StorageService,
    model_id: &str,
    api_url: &str,
    api_token: &str,
    search_mode: &str,
    tavily_api_key: &str,
) {
    storage
        .save_model_settings(ModelSettingsRecord {
            api_url: api_url.to_string(),
            api_token: api_token.to_string(),
            search_mode: search_mode.to_string(),
            tavily_api_key: tavily_api_key.to_string(),
            models: vec![ModelConfigRecord {
                id: model_id.to_string(),
                display_name: "Pending provider fixture".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                provider_profile_config: mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                    mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".to_string(),
                cached_input_price: String::new(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        })
        .unwrap();
}

fn save_test_pending_provider_for_input(storage: &StorageService, input: &mut AgentChatInput) {
    // Test callers build the same explicit current Profile that the Host model-settings boundary
    // would have frozen before an Agent run. This is fixture setup, not a production fallback.
    if input.provider_profile_config.is_none() {
        let dialect = input
            .api_style
            .map(mycopilot_core::ProviderProtocolDialect::from)
            .unwrap_or_else(|| {
                mycopilot_core::ProviderProtocolDialect::detect_from_api_url(&input.api_url)
            });
        input.provider_profile_config = Some(
            mycopilot_core::ProviderProfileConfig::generic_for_dialect(dialect),
        );
    }
    let (search_mode, tavily_api_key) = input
        .search_config
        .as_ref()
        .map(|search| {
            let mode = match search.mode {
                mycopilot_core::AgentSearchMode::Auto => "auto",
                mycopilot_core::AgentSearchMode::Disabled => "disabled",
                mycopilot_core::AgentSearchMode::Tavily => "tavily",
            };
            (mode, search.tavily_api_key.as_deref().unwrap_or_default())
        })
        .unwrap_or(("disabled", ""));
    storage
        .save_model_settings(ModelSettingsRecord {
            api_url: input.api_url.clone(),
            api_token: input.api_token.clone(),
            search_mode: search_mode.to_string(),
            tavily_api_key: tavily_api_key.to_string(),
            models: vec![ModelConfigRecord {
                id: input.model.clone(),
                display_name: "Pending input fixture".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: input.model_capabilities.image_input,
                context_window_tokens: input.context_window_tokens.or(Some(128_000)),
                provider_profile_config: input
                    .provider_profile_config
                    .clone()
                    .expect("current test input must freeze an explicit Provider Profile"),
                input_price: "0".to_string(),
                cached_input_price: String::new(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        })
        .unwrap();

    freeze_test_pending_provider_configuration(storage, input);
}

fn freeze_test_pending_provider_configuration(
    storage: &StorageService,
    input: &mut AgentChatInput,
) {
    let snapshot = storage
        .load_model_settings_snapshot()
        .unwrap()
        .expect("test Provider settings snapshot");
    let (provider_protocol_revision, config, key) =
        test_frozen_provider_protocol(storage, &input.model, input.api_style);
    input.provider_configuration_revision = Some(provider_protocol_revision);
    input.provider_connection_revision = Some(
        snapshot
            .provider_connection_revisions
            .get(&input.model)
            .expect("test Provider connection revision")
            .clone(),
    );
    input.search_connection_revision = Some(snapshot.search_connection_revision);
    input.provider_profile_config = Some(config.clone());
    input.provider_protocol_key = Some(key.clone());
    if let Some(checkpoint) = input.resume_checkpoint.as_mut() {
        checkpoint.provider_profile_config = config;
        checkpoint.provider_protocol_key = key;
    }
}

fn test_frozen_provider_protocol(
    storage: &StorageService,
    model_id: &str,
    api_style: Option<mycopilot_core::AgentApiStyle>,
) -> (
    String,
    mycopilot_core::ProviderProfileConfig,
    mycopilot_core::ProviderProtocolKey,
) {
    let snapshot = storage
        .load_model_settings_snapshot()
        .unwrap()
        .expect("test Provider settings snapshot");
    let model = snapshot
        .settings
        .models
        .iter()
        .find(|model| model.id == model_id)
        .expect("test Provider model");
    let connection = snapshot
        .settings
        .effective_connection_for(model)
        .expect("test Provider connection");
    let dialect = api_style
        .map(mycopilot_core::ProviderProtocolDialect::from)
        .unwrap_or_else(|| {
            mycopilot_core::ProviderProtocolDialect::detect_from_api_url(&connection.api_url)
        });
    let config = model
        .resolved_provider_profile_config(dialect)
        .expect("test Provider Profile");
    let provider_protocol_revision = snapshot
        .provider_protocol_revisions
        .get(model_id)
        .expect("test Provider protocol revision")
        .clone();
    let key = mycopilot_core::ProviderProtocolKey::new(
        dialect,
        &config,
        model.id.clone(),
        Some(provider_protocol_revision.clone()),
    )
    .expect("test Provider Protocol key");
    (provider_protocol_revision, config, key)
}

fn write_test_skill(workspace: &Path, body: &str) {
    let skill_directory = workspace.join(".agents").join("skills").join("reviewer");
    fs::create_dir_all(&skill_directory).unwrap();
    fs::write(
        skill_directory.join("SKILL.md"),
        format!(
            "---\nname: repository-reviewer\ndescription: DESCRIPTION_DISCOVERY_ONLY\n---\n{body}\n"
        ),
    )
    .unwrap();
}

fn skill_turn_input(
    project_id: &str,
    selection: mycopilot_protocol_rs::SkillSelectionDto,
    conversation_id: &str,
) -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: Some(conversation_id.to_string()),
        project_id: Some(project_id.to_string()),
        model_id: "model-1".to_string(),
        context_window_indicator_enabled: true,
        content: "Review this repository.".to_string(),
        attachments: Vec::new(),
        skills: vec![selection],
        title: None,
        user_message_id: Some(format!("user-{conversation_id}")),
        assistant_message_id: Some(format!("assistant-{conversation_id}")),
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    }
}

fn global_skill_turn_input(
    selection: mycopilot_protocol_rs::SkillSelectionDto,
    conversation_id: &str,
) -> AgentConversationTurnInput {
    let mut input = skill_turn_input("unused-project", selection, conversation_id);
    input.project_id = None;
    input
}

fn test_context_compaction_generator() -> ContextCompactionSummaryGenerator {
    Arc::new(|request, cancellation| {
        Box::pin(async move {
            cancellation.check()?;
            let observation = mycopilot_core::ModelRequestObservation {
                schema_version: mycopilot_core::MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
                id: format!("model-request-{}", request.operation_id),
                run_id: request.run_id.clone(),
                conversation_id: Some(request.conversation_id.clone()),
                assistant_message_id: Some(request.assistant_message_id.clone()),
                operation_id: Some(request.operation_id.clone()),
                request_index: request.request_index,
                purpose: mycopilot_core::ModelRequestPurpose::ContextCompaction,
                model: "model-1".to_string(),
                api_style: mycopilot_core::AgentApiStyle::OpenAiCompatible,
                status: mycopilot_core::ModelRequestObservationStatus::Completed,
                estimate: None,
                actual_usage: None,
                finish_reason: Some("stop".to_string()),
                error_code: None,
                error_message: None,
                started_at: 1,
                completed_at: 2,
                tool_set: None,
            };
            Ok(AgentContextCompactionGenerationOutput {
                draft: ContextCompactionSummaryDraft {
                    id: format!(
                        "test-summary-{}",
                        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
                    ),
                    source_revision: request.prefix.source_revision.clone(),
                    content: "Test summary of the completed historical turn.".to_string(),
                    continuity: request.continuity,
                    generation: ContextCompactionGeneration::test(),
                    source_input_tokens: request.source_input_tokens,
                    summary_input_tokens: 10,
                    continuity_input_tokens: 10,
                    uncovered_tail_input_tokens: request.uncovered_tail_input_tokens,
                    replacement_input_tokens: 20,
                    created_at: now_ms(),
                },
                observation,
            })
        })
    })
}
