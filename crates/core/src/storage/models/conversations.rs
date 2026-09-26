use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageRecord {
    /// Output-only history proof. Only native answer admission and fork copying may create it.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub human_interaction_response:
        Option<crate::human_interaction::HumanInteractionResponseDisplay>,
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    pub status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<ChatMessageAttachmentRecord>,
    /// Durable JSON carrying model-safe folder references plus Host-only binding metadata.
    /// The storage layer copies this value verbatim during fork; model projections sanitize it.
    #[serde(default)]
    pub folder_references_json: Option<String>,
    pub agent_run_json: Option<String>,
    pub ui_state_json: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageStateRecord {
    pub id: String,
    pub content: String,
    pub status: Option<String>,
    pub agent_run_json: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatConversationMetaRecord {
    pub id: String,
    pub project_id: Option<String>,
    pub model_id: Option<String>,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub pinned_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub unread_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRecord {
    pub id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub project_id: Option<String>,
    pub kind: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub storage_rel_path: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageAttachmentRecord {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub preview_data: Option<String>,
    pub preview_mime_type: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentImageRecord {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub data: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatConversationRecord {
    pub id: String,
    pub project_id: Option<String>,
    pub model_id: Option<String>,
    pub title: String,
    pub messages: Vec<ChatMessageRecord>,
    pub created_at: i64,
    pub updated_at: i64,
    pub pinned_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub unread_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationContinuationOriginRecord {
    pub source_conversation_id: String,
    pub source_message_id: String,
    pub boundary_message_id: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatConversationViewRecord {
    #[serde(flatten)]
    pub conversation: ChatConversationRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_origin: Option<ConversationContinuationOriginRecord>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForkConversationRequest {
    pub request_id: String,
    pub source_conversation_id: String,
    pub fork_point: ConversationForkPoint,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationForkPoint {
    Latest {},
    AssistantReply { assistant_message_id: String },
    ProviderTransitionBoundary { operation_id: String },
    ManualCompactionBoundary { operation_id: String },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatSearchInput {
    pub query: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChatSearchMatchKind {
    Title,
    Message,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatSearchResult {
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub message_id: Option<String>,
    pub snippet: Option<String>,
    pub match_kind: ChatSearchMatchKind,
    pub updated_at: i64,
}
