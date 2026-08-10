use super::*;
use mycopilot_core::command::CommandSessionManager;
use mycopilot_core::skills::{
    LocalSkillInstallRequest, LocalSkillUpdateRequest, SkillInstallationId,
    SkillInstallationOutcome, SkillInstallationService, SkillUninstallRequest,
};
use mycopilot_core::storage::models::{
    AgentActionAuditRecord, AgentFileDraftRecord, AgentPendingActionRecord, ChatConversationRecord,
    ChatMessageRecord, ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
};
use mycopilot_core::{
    AgentActivatedSkill, AgentCommandRequest, AgentCommandRiskLevel, AgentCommandSessionListInput,
    AgentCommandSessionStatus, AgentFileWriteMode, AgentFileWriteProposal, AgentPermissions,
    AgentSkillActivation, AgentSkillMaterializationRequest, AgentUsageSummaryRange,
    AgentWorkspaceContext, AgentWritePermission, ContextCompactionGeneration,
    ContextCompactionPrefix, ContextCompactionSourceItem, ContextCompactionSummary,
    ContextCompactionSummaryDraft, ContextJournalCursor, ConversationTraceToolResultStatus,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
    AGENT_RUN_CHECKPOINT_SCHEMA_VERSION, CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use serde_json::{json, Value};
use std::fs;
use tempfile::tempdir;

mod cancellation;
mod command_sessions;
mod context_history;
mod context_runtime;
mod file_write_permissions;
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
        "contextWindowTokens": 128000,
        "messages": []
    }))
    .unwrap();
    input.context = Some(AgentRunContext {
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
        runtime: None,
        runtime_binding: None,
    }
}

fn completed_trace(conversation_id: &str, assistant_message_id: &str) -> ConversationTurnTrace {
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
                call_id: "write-history".to_string(),
                tool: "write_file".to_string(),
                operation: json!({
                    "filePath": "src/history.rs",
                    "mode": "create"
                }),
                provenance: None,
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: "write-history".to_string(),
                tool: "write_file".to_string(),
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
            provider_profile_config: None,
            input_price: "0.01".to_string(),
            cached_input_price: String::new(),
            output_price: "0.02".to_string(),
            enabled: true,
        }],
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
                provider_profile_config: None,
                input_price: "0".to_string(),
                cached_input_price: String::new(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        })
        .unwrap();
}

fn save_test_pending_provider_for_input(storage: &StorageService, input: &mut AgentChatInput) {
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
                provider_profile_config: input.provider_profile_config.clone(),
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
