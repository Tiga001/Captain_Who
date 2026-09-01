use super::*;
use crate::storage::models::{
    AttachmentRecord, ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord,
    ConversationForkPoint, ForkConversationRequest, ModelConfigRecord, ModelSettingsRecord,
    ProjectRecord,
};
use crate::{
    AgentGraphError, AgentMailboxDeliveryStatus, AgentMailboxKind, AgentWakeStatus,
    ConversationMessageOrigin, ConversationTurnTrace, ConversationTurnTraceTerminalStatus,
    CreateAgentTemplateInput, EnqueueAgentMessageInput, EnsureRootAgentInput,
    FinishAgentWakeWithResultInput, ProviderFamilyReasoningPolicy, ProviderFamilySettings,
    ProviderProfileConfig, ProviderProfileConfigV1, ProviderProtocolDialect,
    ProviderReasoningEffort, ProviderVendorId, ReasoningEffort, SendAgentMessageRequest,
    UpdateAgentTemplateInput, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    service: StorageService,
}

impl Fixture {
    fn new(root_model_id: Option<&str>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let service = StorageService::open(&directory.path().join("storage.sqlite")).unwrap();
        service
            .save_project(ProjectRecord {
                id: "project-a".to_string(),
                name: "Project A".to_string(),
                path: None,
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        service
            .save_model_settings(model_settings(vec![
                model("model-a", true),
                model("model-b", true),
                model("model-c", true),
            ]))
            .unwrap();
        service
            .save_conversation_meta(ChatConversationMetaRecord {
                id: "root-conversation".to_string(),
                project_id: Some("project-a".to_string()),
                model_id: root_model_id.map(ToString::to_string),
                title: "Root".to_string(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        service
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: "agent-root".to_string(),
                conversation_id: "root-conversation".to_string(),
                creation_request_id: "ensure-root".to_string(),
                task_name: "Root".to_string(),
            })
            .unwrap();
        service
            .create_agent_template(&CreateAgentTemplateInput {
                template_id: "template-reviewer".to_string(),
                machine_key: "reviewer".to_string(),
                name: "Reviewer".to_string(),
                description: "Review evidence".to_string(),
                instructions: "Review and report to the parent.".to_string(),
                model_config_id: "model-b".to_string(),
                enabled: true,
            })
            .unwrap();
        service
            .set_agent_template_project_assignment("project-a", "template-reviewer", true)
            .unwrap();
        Self {
            _directory: directory,
            service,
        }
    }
}

fn model(id: &str, enabled: bool) -> ModelConfigRecord {
    ModelConfigRecord {
        id: id.to_string(),
        display_name: format!("Display {id}"),
        api_url_override: None,
        api_token_override: None,
        supports_image: false,
        context_window_tokens: Some(64_000),
        provider_profile_config: ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        input_price: "0".to_string(),
        cached_input_price: String::new(),
        output_price: "0".to_string(),
        enabled,
    }
}

fn deepseek_model(
    id: &str,
    enabled: bool,
    mode: crate::ReasoningMode,
    effort: ReasoningEffort,
) -> ModelConfigRecord {
    let mut model = model(id, enabled);
    model.provider_profile_config = ProviderProfileConfig::V1(ProviderProfileConfigV1 {
        schema_version: crate::PROVIDER_PROFILE_CONFIG_SCHEMA_VERSION,
        profile: crate::ProviderProfileRef::deepseek_v4_chat(),
        reasoning: crate::ReasoningPolicy { mode, effort },
    });
    model
}

fn deepseek_v2_model(
    id: &str,
    enabled: bool,
    mode: crate::ReasoningMode,
    effort: ProviderReasoningEffort,
) -> ModelConfigRecord {
    let mut model = model(id, enabled);
    model.provider_profile_config = ProviderProfileConfig::from_family_settings(
        crate::ProviderProfileRef::deepseek_v4_chat(),
        ProviderVendorId::DeepSeek,
        ProviderFamilySettings::DeepseekV4Chat {
            reasoning: ProviderFamilyReasoningPolicy { mode, effort },
        },
    );
    model
}

fn model_settings(models: Vec<ModelConfigRecord>) -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://provider.example/v1/chat/completions".to_string(),
        api_token: "fixture-secret".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models,
    }
}

fn spawn_input(request_id: &str, task_name: &str) -> CreateChildAgentInput {
    CreateChildAgentInput {
        parent_agent_id: "agent-root".to_string(),
        creation_request_id: request_id.to_string(),
        task_name: task_name.to_string(),
        task: format!("Perform {task_name} and report evidence."),
        template_machine_key: None,
        explicit_model_id: None,
        reasoning_effort: None,
        fork_turns: AgentForkTurns::None,
    }
}

fn save_settled_history(fixture: &Fixture, turn_count: usize, active_tail: bool) {
    let mut messages = Vec::new();
    for turn in 0..turn_count {
        messages.push(ChatMessageRecord {
            id: format!("root-user-{turn}"),
            role: "user".to_string(),
            content: format!("question {turn}"),
            created_at: 10 + turn as i64 * 2,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
        messages.push(ChatMessageRecord {
                id: format!("root-assistant-{turn}"),
                role: "assistant".to_string(),
                content: format!("answer {turn}"),
                created_at: 11 + turn as i64 * 2,
                status: Some("completed".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(format!(
                    "{{\"runId\":\"root-run-{turn}\",\"status\":\"completed\",\"usage\":{{\"totalTokens\":99}}}}"
                )),
                ui_state_json: Some("{\"expanded\":true}".to_string()),
            });
    }
    if active_tail {
        messages.push(ChatMessageRecord {
            id: "root-user-active".to_string(),
            role: "user".to_string(),
            content: "must not be copied".to_string(),
            created_at: 100,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
        messages.push(ChatMessageRecord {
            id: "root-assistant-active".to_string(),
            role: "assistant".to_string(),
            content: String::new(),
            created_at: 101,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: Some(
                "{\"runId\":\"root-run-active\",\"status\":\"running\"}".to_string(),
            ),
            ui_state_json: None,
        });
    }
    fixture
        .service
        .save_conversation(ChatConversationRecord {
            id: "root-conversation".to_string(),
            project_id: Some("project-a".to_string()),
            model_id: Some("model-a".to_string()),
            title: "Root".to_string(),
            messages,
            created_at: 1,
            updated_at: 101,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    for turn in 0..turn_count {
        fixture
            .service
            .replace_conversation_turn_trace(
                &ConversationTurnTrace {
                    schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: format!("root-run-{turn}"),
                    conversation_id: "root-conversation".to_string(),
                    assistant_message_id: format!("root-assistant-{turn}"),
                    terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                    terminal_error: None,
                    truncated: false,
                    items: Vec::new(),
                },
                11 + turn as i64 * 2,
                12 + turn as i64 * 2,
            )
            .unwrap();
    }
    if active_tail {
        fixture
            .service
            .append_in_progress_conversation_turn_trace(
                &ConversationTurnTrace {
                    schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                    run_id: "root-run-active".to_string(),
                    conversation_id: "root-conversation".to_string(),
                    assistant_message_id: "root-assistant-active".to_string(),
                    terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
                    terminal_error: None,
                    truncated: false,
                    items: Vec::new(),
                },
                101,
                101,
            )
            .unwrap();
    }
}

fn attach_file_to_first_user_message(
    fixture: &Fixture,
    suffix: &str,
    write_source_file: bool,
) -> AttachmentRecord {
    let message_id = "root-user-0";
    let attachment_id = format!("attachment-{suffix}");
    let bytes = format!("attachment bytes for {suffix}").into_bytes();
    let relative_path = attachment_storage_rel_path(
        "root-conversation",
        message_id,
        &attachment_id,
        "evidence.txt",
    );
    let attachment = AttachmentRecord {
        id: attachment_id,
        conversation_id: "root-conversation".to_string(),
        message_id: message_id.to_string(),
        project_id: Some("project-a".to_string()),
        kind: "file".to_string(),
        original_name: "evidence.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: bytes.len() as u64,
        storage_rel_path: slash_path(&relative_path),
        created_at: 10,
    };
    {
        let connection = fixture.service.state.connection().unwrap();
        attachment_repository::save_attachment(&connection, &attachment).unwrap();
    }
    if write_source_file {
        let path = fixture.service.attachment_root.join(&relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    attachment
}

fn attach_file_to_tree_member_message(
    fixture: &Fixture,
    conversation_id: &str,
    message_id: &str,
    attachment_id: &str,
    bytes: &[u8],
    created_at: i64,
) -> AttachmentRecord {
    let original_name = "member-evidence.txt";
    let relative_path =
        attachment_storage_rel_path(conversation_id, message_id, attachment_id, original_name);
    let attachment = AttachmentRecord {
        id: attachment_id.to_string(),
        conversation_id: conversation_id.to_string(),
        message_id: message_id.to_string(),
        project_id: Some("project-a".to_string()),
        kind: "file".to_string(),
        original_name: original_name.to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: bytes.len() as u64,
        storage_rel_path: slash_path(&relative_path),
        created_at,
    };
    {
        let connection = fixture.service.state.connection().unwrap();
        attachment_repository::save_attachment(&connection, &attachment).unwrap();
    }
    let path = fixture.service.attachment_root.join(&relative_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    attachment
}

fn regular_files_below(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(path) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(regular_files_below(&path));
        } else if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[allow(clippy::too_many_arguments)]
fn append_terminal_assistant_for_tree_fork(
    fixture: &Fixture,
    conversation_id: &str,
    message_id: &str,
    run_id: &str,
    content: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    terminal_error: Option<&str>,
    created_at: i64,
) {
    assert!(terminal_status.is_terminal());
    let status = terminal_status.as_str();
    let agent_run_json = serde_json::json!({
        "runId": run_id,
        "status": status,
    })
    .to_string();
    let connection = fixture.service.state.connection().unwrap();
    connection
        .execute(
            "INSERT INTO messages (
                     id, conversation_id, role, content, status, agent_run_json,
                     created_at, position
                 ) VALUES (
                     ?1, ?2, 'assistant', ?3, ?4, ?5, ?6,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                      WHERE conversation_id = ?2)
                 )",
            rusqlite::params![
                message_id,
                conversation_id,
                content,
                status,
                agent_run_json,
                created_at,
            ],
        )
        .unwrap();
    conversation_trace_repository::commit_trace_in_connection(
        &connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            conversation_id: conversation_id.to_string(),
            assistant_message_id: message_id.to_string(),
            terminal_status,
            terminal_error: terminal_error.map(ToString::to_string),
            truncated: false,
            items: vec![crate::ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: format!("trace: {content}"),
                truncated: false,
            }],
        },
        created_at,
        created_at,
    )
    .unwrap();
}

fn tree_by_task_path(
    fixture: &Fixture,
    root_agent_id: &str,
) -> std::collections::BTreeMap<String, crate::AgentNodeRecord> {
    fixture
        .service
        .list_agent_tree(root_agent_id)
        .unwrap()
        .into_iter()
        .map(|node| (node.task_path.clone(), node))
        .collect()
}

fn tree_history_identity_sets(
    fixture: &Fixture,
    tree: &std::collections::BTreeMap<String, crate::AgentNodeRecord>,
) -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
) {
    let mut message_ids = std::collections::HashSet::new();
    let mut run_ids = std::collections::HashSet::new();
    for node in tree.values() {
        let conversation = fixture
            .service
            .load_conversation(&node.conversation_id)
            .unwrap()
            .expect("every tree member owns a readable conversation");
        for message in conversation.messages {
            assert!(message_ids.insert(message.id.clone()));
            if let Some(agent_run_json) = message.agent_run_json.as_deref() {
                let agent_run = serde_json::from_str::<serde_json::Value>(agent_run_json).unwrap();
                if let Some(run_id) = agent_run.get("runId").and_then(|value| value.as_str()) {
                    run_ids.insert(run_id.to_string());
                }
            }
            if let Some(trace) = fixture
                .service
                .get_conversation_turn_trace(&message.id)
                .unwrap()
            {
                assert_eq!(trace.conversation_id, node.conversation_id);
                run_ids.insert(trace.run_id);
            }
        }
    }
    (message_ids, run_ids)
}

fn assert_tree_identities_are_fresh(
    fixture: &Fixture,
    source: &std::collections::BTreeMap<String, crate::AgentNodeRecord>,
    target: &std::collections::BTreeMap<String, crate::AgentNodeRecord>,
) {
    assert_eq!(
        source.keys().collect::<Vec<_>>(),
        target.keys().collect::<Vec<_>>()
    );
    let source_agent_ids = source
        .values()
        .map(|node| node.agent_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let target_agent_ids = target
        .values()
        .map(|node| node.agent_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(source_agent_ids.is_disjoint(&target_agent_ids));
    let source_conversation_ids = source
        .values()
        .map(|node| node.conversation_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let target_conversation_ids = target
        .values()
        .map(|node| node.conversation_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(source_conversation_ids.is_disjoint(&target_conversation_ids));

    let (source_message_ids, source_run_ids) = tree_history_identity_sets(fixture, source);
    let (target_message_ids, target_run_ids) = tree_history_identity_sets(fixture, target);
    assert!(source_message_ids.is_disjoint(&target_message_ids));
    assert!(source_run_ids.is_disjoint(&target_run_ids));
}

fn member_fork_receipt_count(fixture: &Fixture, request_id: &str) -> u64 {
    fixture
        .service
        .state
        .connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM agent_member_conversation_forks
                 WHERE root_fork_request_id = ?1",
            [request_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn turn_diff_action_ids(fixture: &Fixture, assistant_message_id: &str) -> Vec<String> {
    let connection = fixture.service.state.connection().unwrap();
    let mut statement = connection
        .prepare(
            "SELECT action_id FROM agent_turn_diff_actions
                 WHERE assistant_message_id = ?1
                 ORDER BY action_id ASC",
        )
        .unwrap();
    statement
        .query_map([assistant_message_id], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn member_compaction_summary_draft(
    prefix: &crate::ContextCompactionPrefix,
    summary_id: &str,
    created_at: i64,
) -> crate::ContextCompactionSummaryDraft {
    crate::ContextCompactionSummaryDraft {
        id: summary_id.to_string(),
        source_revision: prefix.source_revision.clone(),
        content: format!("summary {summary_id}"),
        continuity: crate::ContextContinuitySnapshot::from_prefix(prefix).unwrap(),
        generation: crate::ContextCompactionGeneration::test(),
        source_input_tokens: 1_000,
        summary_input_tokens: 100,
        continuity_input_tokens: 100,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 200,
        created_at,
    }
}

#[allow(clippy::too_many_arguments)]
fn record_applied_member_compaction(
    connection: &mut rusqlite::Connection,
    prefix: &crate::ContextCompactionPrefix,
    summary_id: &str,
    operation_id: &str,
    run_id: &str,
    assistant_message_id: &str,
    model_id: &str,
    completed_at: i64,
) -> crate::ContextCompactionReceipt {
    assert!(!operation_id.starts_with("provider-transition-"));
    let started_at = completed_at.saturating_sub(2);
    let mut receipt = crate::ContextCompactionReceipt {
        schema_version: crate::CONTEXT_COMPACTION_RECEIPT_SCHEMA_VERSION,
        operation_id: operation_id.to_string(),
        run_id: run_id.to_string(),
        conversation_id: prefix.conversation_id.clone(),
        assistant_message_id: assistant_message_id.to_string(),
        request_index: 1,
        attempt_index: 1,
        model: model_id.to_string(),
        provider_transition_source_model_display_name: None,
        provider_transition_target_model_display_name: None,
        api_style: crate::AgentApiStyle::OpenAiCompatible,
        status: crate::ContextCompactionReceiptStatus::InProgress,
        stage: crate::ContextCompactionReceiptStage::Planned,
        plan: crate::ContextCompactionReceiptPlan {
            context_revision: prefix.source_revision.clone(),
            persistent_revision: prefix.source_revision.clone(),
            request_input_tokens: 1_000,
            available_input_tokens: Some(1_000),
            request_trigger_input_tokens: Some(900),
            request_target_input_tokens: Some(200),
            source_input_tokens: 1_000,
            retained_input_tokens: 0,
            target_replacement_tokens: 200,
            expected_reclaimed_tokens: 800,
            planned_reclaimed_tokens: 800,
            projected_request_input_tokens: 200,
            best_effort: false,
            protected_input_tokens: 0,
            protected_reasons: Default::default(),
            atomic_unit_count: prefix.source_items.len().max(1),
            previous_summary_id: prefix
                .previous_summary
                .as_ref()
                .map(|summary| summary.id.clone()),
            covered_through: prefix.covered_through.clone(),
        },
        source_revision: None,
        generation_observation_id: None,
        summary_id: None,
        result: None,
        error: None,
        started_at,
        updated_at: started_at,
        completed_at: None,
    };
    receipt.validate().unwrap();
    crate::storage::context_compaction_receipt_repository::record_receipt(
        connection, &receipt, None,
    )
    .unwrap();
    receipt
        .attach_prepared_prefix(prefix, completed_at.saturating_sub(1))
        .unwrap();
    let draft = member_compaction_summary_draft(prefix, summary_id, completed_at);
    let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
        format!("observation-{operation_id}"),
        run_id,
        Some(prefix.conversation_id.clone()),
        Some(assistant_message_id.to_string()),
        Some(operation_id.to_string()),
        1,
        crate::ModelRequestPurpose::ContextCompaction,
        model_id,
        crate::AgentApiStyle::OpenAiCompatible,
        None,
        completed_at.saturating_sub(1),
    )
    .completed(None, Some("stop".to_string()), completed_at)
    .unwrap();
    receipt
        .complete_applied(&draft, &observation, completed_at)
        .unwrap();
    crate::storage::context_compaction_repository::commit_prefix_replacement_with_receipt(
        connection,
        prefix,
        draft,
        &receipt,
        &observation,
    )
    .unwrap();
    receipt
}

mod approval_resume;
mod attachments_spawn;
mod model_recovery;
mod recursive_forks;
mod snapshot_history;

fn release_profile_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{name} must be a positive integer"))
        })
        .unwrap_or(default)
}
