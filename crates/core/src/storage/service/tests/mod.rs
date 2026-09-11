use super::*;
use crate::storage::models::{
    ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ComposerDraftRecord,
    ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
};
use crate::EnsureRootAgentInput;
use std::sync::atomic::{AtomicU64, Ordering};

mod action_json_cas;
mod attachments;
mod conversations;
mod guidance;
mod ignored_history;
mod message_deletion;
mod notifications;
mod reconciliation;
mod settings;
mod terminal_message_streams;
mod trace_reconciliation;
mod turn_rewrites;
mod waiting_persistence;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

struct StorageFixture {
    root: PathBuf,
    model_credentials: Arc<crate::image_generation::InMemoryCredentialStore>,
}

impl StorageFixture {
    fn new() -> Self {
        let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("mycopilot-storage-attachment-test-{unique}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            model_credentials: Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
        }
    }

    fn service(&self) -> StorageService {
        let service = StorageService::open_with_model_credentials(
            &self.root.join("storage.sqlite"),
            self.model_credentials.clone(),
        )
        .unwrap();
        for project_id in ["project-1", "project-2"] {
            service
                .save_project(ProjectRecord::with_primary_folder(
                    project_id.to_string(),
                    project_id.to_string(),
                    self.root.join(project_id).to_string_lossy().to_string(),
                    1,
                ))
                .unwrap();
        }
        service
    }
}

impl Drop for StorageFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn bind_agent_root(service: &StorageService, agent_id: &str, conversation_id: &str) {
    service
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: agent_id.to_string(),
            conversation_id: conversation_id.to_string(),
            creation_request_id: format!("create-{agent_id}"),
            task_name: agent_id.to_string(),
        })
        .unwrap();
}

fn bind_agent_child(
    service: &StorageService,
    agent_id: &str,
    conversation_id: &str,
    root_agent_id: &str,
    root_conversation_id: &str,
    task_name: &str,
) {
    let connection = service.state.connection().unwrap();
    connection
        .execute(
            "INSERT INTO agent_nodes (
                agent_id, schema_version, root_agent_id, root_conversation_id,
                parent_agent_id, conversation_id, project_id, creation_request_id,
                task_name, task_path,
                model_config_id_snapshot, model_display_name_snapshot,
                model_supports_image_snapshot, model_context_window_tokens_snapshot,
                model_settings_revision_snapshot, provider_connection_revision_snapshot,
                provider_protocol_revision_snapshot, model_selection_source_snapshot,
                lifecycle, revision, created_at, updated_at
             ) VALUES (?1, 1, ?3, ?4, ?3, ?2, NULL, ?1, ?5, '/root/' || ?5,
                       'model-1', 'Model 1', 0, 32000,
                       'settings-1', 'connection-1', 'provider-protocol-v1', 'explicit',
                       'active', 1, 2, 2)",
            rusqlite::params![
                agent_id,
                conversation_id,
                root_agent_id,
                root_conversation_id,
                task_name
            ],
        )
        .unwrap();
}

fn conversation(id: &str, project_id: Option<&str>, message_id: &str) -> ChatConversationRecord {
    ChatConversationRecord {
        id: id.to_string(),
        project_id: project_id.map(ToString::to_string),
        model_id: Some("model-1".to_string()),
        title: id.to_string(),
        messages: vec![ChatMessageRecord {
            human_interaction_response: None,
            id: message_id.to_string(),
            role: "user".to_string(),
            content: "hello".to_string(),
            created_at: 1,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }],
        created_at: 1,
        updated_at: 1,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    }
}

fn assistant_reply_fork_request(
    request_id: impl Into<String>,
    source_conversation_id: impl Into<String>,
    assistant_message_id: impl Into<String>,
) -> ForkConversationRequest {
    ForkConversationRequest {
        request_id: request_id.into(),
        source_conversation_id: source_conversation_id.into(),
        fork_point: ConversationForkPoint::AssistantReply {
            assistant_message_id: assistant_message_id.into(),
        },
    }
}

fn input_attachment(
    id: &str,
    kind: AgentInputAttachmentKind,
    name: &str,
    mime_type: Option<&str>,
    bytes: &[u8],
) -> AgentInputAttachment {
    AgentInputAttachment {
        id: id.to_string(),
        kind,
        name: name.to_string(),
        mime_type: mime_type.map(ToString::to_string),
        size_bytes: bytes.len() as u64,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        truncated: None,
    }
}

fn composer_draft(scope_id: &str, project_id: Option<&str>, message: &str) -> ComposerDraftRecord {
    ComposerDraftRecord {
        scope_id: scope_id.to_string(),
        message: message.to_string(),
        permission_mode: "default".to_string(),
        permission_mode_version: crate::storage::models::CURRENT_COMPOSER_PERMISSION_MODE_VERSION,
        model_id: Some("model-1".to_string()),
        project_id: project_id.map(ToString::to_string),
        attachments_json: "[]".to_string(),
        skills_json: "[]".to_string(),
        queued_messages_json: "[]".to_string(),
        updated_at: 1,
    }
}

fn agent_usage_record(conversation_id: &str, message_id: &str) -> AgentUsageRecordInsert {
    AgentUsageRecordInsert {
        id: format!("usage-{message_id}"),
        conversation_id: conversation_id.to_string(),
        message_id: message_id.to_string(),
        run_id: "run-1".to_string(),
        project_id: Some("project-1".to_string()),
        model_id: "model-1".to_string(),
        model_name: "Model 1".to_string(),
        started_at: Some(900),
        completed_at: Some(1_000),
        status: Some("completed".to_string()),
        error: None,
        created_at: 1_000,
        input_tokens: Some(12),
        output_tokens: Some(8),
        output_thinking_tokens: None,
        total_tokens: Some(20),
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        billable_request_count: 1,
        input_price: Some("0".to_string()),
        cached_input_price: Some("0".to_string()),
        output_price: Some("0".to_string()),
        estimated_cost: Some(0.0),
    }
}

fn pending_action(action_id: &str, conversation_id: &str) -> AgentPendingActionRecord {
    AgentPendingActionRecord {
        action_id: action_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some("message-1".to_string()),
        action_type: "command".to_string(),
        tool_name: "run_command".to_string(),
        tool_call_id: Some(action_id.to_string()),
        status: "pending".to_string(),
        target_status: None,
        action_json: "{}".to_string(),
        agent_input_json: "{}".to_string(),
        created_at: 1,
        updated_at: 1,
    }
}

fn action_audit(action_id: &str, conversation_id: &str) -> AgentActionAuditRecord {
    AgentActionAuditRecord {
        action_id: action_id.to_string(),
        run_id: "run-1".to_string(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some("message-1".to_string()),
        action_type: "command".to_string(),
        tool_name: "run_command".to_string(),
        decision: Some("approved".to_string()),
        status: "completed".to_string(),
        action_json: "{}".to_string(),
        file_change_result_json: None,
        command_result_json: None,
        tool_result_json: None,
        error: None,
        created_at: 1,
        decided_at: Some(2),
        completed_at: Some(3),
        effective_permissions_json: None,
        path_scope: None,
        command_cwd_scope: None,
        blocked_reason: None,
        decision_source: Some("manual".to_string()),
    }
}
