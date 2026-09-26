use super::deserialize_required_nullable;
use crate::protocol::{AgentInputAttachmentEncoding, AgentInputAttachmentKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Permission choices persisted before this version predate the current `full` semantics and
/// must not be interpreted as an explicit opt-in to those broader privileges.
pub const CURRENT_COMPOSER_PERMISSION_MODE_VERSION: i64 = 2;
const COMPOSER_DRAFT_PAYLOAD_ERROR: &str = "stored_composer_draft_malformed";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredComposerAttachment {
    id: String,
    kind: AgentInputAttachmentKind,
    name: String,
    mime_type: Option<String>,
    size_bytes: u64,
    encoding: AgentInputAttachmentEncoding,
    data: String,
    #[serde(default)]
    content_sha256: Option<String>,
    truncated: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredComposerSkillSelection {
    id: String,
    revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredComposerPermissionMode {
    Default,
    Full,
    Custom,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredComposerQueueStatus {
    Pending,
    Submitting,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredComposerQueuedMessage {
    id: String,
    client_message_id: String,
    content: String,
    attachments: Vec<StoredComposerAttachment>,
    model_id: String,
    permission_mode: StoredComposerPermissionMode,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    project_id: Option<String>,
    skills: Vec<StoredComposerSkillSelection>,
    status: StoredComposerQueueStatus,
    error: Option<String>,
    created_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerDraftRecord {
    pub scope_id: String,
    pub message: String,
    pub permission_mode: String,
    pub permission_mode_version: i64,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub model_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    pub attachments_json: String,
    #[serde(default = "empty_json_array")]
    pub folder_references_json: String,
    pub skills_json: String,
    pub queued_messages_json: String,
    pub updated_at: i64,
}

fn empty_json_array() -> String {
    "[]".to_string()
}

impl ComposerDraftRecord {
    pub(crate) fn validate_current_payloads(&self) -> Result<(), String> {
        if !matches!(self.permission_mode.as_str(), "default" | "full" | "custom")
            || self.permission_mode_version < 0
        {
            return Err(COMPOSER_DRAFT_PAYLOAD_ERROR.to_string());
        }
        let attachments =
            serde_json::from_str::<Vec<StoredComposerAttachment>>(&self.attachments_json)
                .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;
        let skills = serde_json::from_str::<Vec<StoredComposerSkillSelection>>(&self.skills_json)
            .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;
        let queued =
            serde_json::from_str::<Vec<StoredComposerQueuedMessage>>(&self.queued_messages_json)
                .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;

        validate_stored_composer_attachments(&attachments)?;
        validate_stored_composer_skills(&skills)?;
        for message in &queued {
            validate_stored_composer_queued_message(message)?;
        }
        Ok(())
    }

    /// Fails closed when a persisted `full` choice was made under different or unknown
    /// semantics. Other modes do not gain authority from this version marker.
    pub fn normalize_permission_mode(mut self) -> Self {
        if self.permission_mode == "full"
            && self.permission_mode_version != CURRENT_COMPOSER_PERMISSION_MODE_VERSION
        {
            self.permission_mode = "default".to_string();
        }
        self
    }
}

fn validate_stored_composer_attachments(
    attachments: &[StoredComposerAttachment],
) -> Result<(), String> {
    for attachment in attachments {
        // Reading every field here makes the current durable contract explicit. These values are
        // intentionally opaque to storage, but their shape must be complete before persistence.
        let _ = (
            &attachment.id,
            &attachment.kind,
            &attachment.name,
            &attachment.mime_type,
            attachment.size_bytes,
            &attachment.encoding,
            &attachment.data,
            &attachment.content_sha256,
            &attachment.truncated,
        );
    }
    Ok(())
}

fn validate_stored_composer_skills(skills: &[StoredComposerSkillSelection]) -> Result<(), String> {
    if skills.len() > 8 {
        return Err(COMPOSER_DRAFT_PAYLOAD_ERROR.to_string());
    }
    let mut ids = BTreeSet::new();
    for selection in skills {
        crate::skills::SkillSelection::parse(&selection.id, &selection.revision)
            .map_err(|_| COMPOSER_DRAFT_PAYLOAD_ERROR.to_string())?;
        if !ids.insert(&selection.id) {
            return Err(COMPOSER_DRAFT_PAYLOAD_ERROR.to_string());
        }
    }
    Ok(())
}

fn validate_stored_composer_queued_message(
    message: &StoredComposerQueuedMessage,
) -> Result<(), String> {
    let _ = (
        &message.id,
        &message.client_message_id,
        &message.content,
        &message.model_id,
        &message.permission_mode,
        &message.project_id,
        &message.status,
        &message.error,
        message.created_at,
    );
    validate_stored_composer_attachments(&message.attachments)?;
    validate_stored_composer_skills(&message.skills)
}
