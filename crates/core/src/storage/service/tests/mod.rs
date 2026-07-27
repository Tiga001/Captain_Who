use super::*;
use crate::storage::models::{
    ChatConversationMetaRecord, ChatConversationRecord, ChatMessageRecord, ComposerDraftRecord,
    ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
};
use std::sync::atomic::{AtomicU64, Ordering};

mod attachments;
mod conversations;
mod goals;
mod guidance;
mod message_deletion;
mod reconciliation;
mod settings;
mod trace_reconciliation;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

struct StorageFixture {
    root: PathBuf,
}

impl StorageFixture {
    fn new() -> Self {
        let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("mycopilot-storage-attachment-test-{unique}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn service(&self) -> StorageService {
        let service = StorageService::open(&self.root.join("storage.sqlite")).unwrap();
        for project_id in ["project-1", "project-2"] {
            service
                .save_project(ProjectRecord {
                    id: project_id.to_string(),
                    name: project_id.to_string(),
                    path: Some(self.root.join(project_id).to_string_lossy().to_string()),
                    created_at: 1,
                    pinned_at: None,
                })
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

fn conversation(id: &str, project_id: Option<&str>, message_id: &str) -> ChatConversationRecord {
    ChatConversationRecord {
        id: id.to_string(),
        project_id: project_id.map(ToString::to_string),
        model_id: Some("model-1".to_string()),
        title: id.to_string(),
        messages: vec![ChatMessageRecord {
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
        permission_mode: "workspace".to_string(),
        permission_mode_version: 0,
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
        patch_result_json: None,
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
