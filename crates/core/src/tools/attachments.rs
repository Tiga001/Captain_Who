use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{
    AgentAttachmentReference, AgentError, AgentInputAttachmentKind, AgentResult,
    AgentToolDefinition, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};

const DEFAULT_ATTACHMENT_LIST_LIMIT: usize = 100;
const MAX_ATTACHMENT_LIST_LIMIT: usize = 500;

pub(super) struct AttachmentsListTool;
pub(super) struct AttachmentsListProjectTool;

impl AgentTool for AttachmentsListTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "attachments_list".to_string(),
            description: "List files and images attached to the current conversation only. Use this for attachments shared in this chat. Returns @attachments read paths that can be passed to read_image/read_file/read_pdf/read_word/read_presentation/read_spreadsheet.".to_string(),
            input_schema: attachment_list_schema(),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        list_attachments(
            "current_conversation",
            "Attachments from the current chat only.",
            context.conversation_attachments(),
            args,
        )
    }
}

impl AgentTool for AttachmentsListProjectTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "attachments_list_project".to_string(),
            description: "List files and images attached to other conversations in the current project, excluding the current conversation. Use this to discover historical project attachments from other chats. Returns @attachments read paths that can be passed to read_image/read_file/read_pdf/read_word/read_presentation/read_spreadsheet.".to_string(),
            input_schema: attachment_list_schema(),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        list_attachments(
            "project_other_conversations",
            "Attachments from other conversations in the same project; current chat attachments are intentionally excluded.",
            context.project_attachments(),
            args,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentListArgs {
    kind: Option<AgentInputAttachmentKind>,
    limit: Option<usize>,
}

fn attachment_list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["file", "image"],
                "description": "Optional attachment kind filter."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_ATTACHMENT_LIST_LIMIT,
                "description": "Maximum number of attachments to return."
            }
        }
    })
}

fn list_attachments(
    scope: &str,
    scope_note: &str,
    attachments: &[AgentAttachmentReference],
    args: Value,
) -> AgentResult<Value> {
    let args: AttachmentListArgs = serde_json::from_value(args)
        .map_err(|error| AgentError::new(format!("attachments_list 参数无效：{error}")))?;
    let limit = args
        .limit
        .unwrap_or(DEFAULT_ATTACHMENT_LIST_LIMIT)
        .clamp(1, MAX_ATTACHMENT_LIST_LIMIT);
    let filtered = attachments
        .iter()
        .filter(|attachment| {
            args.kind
                .map(|kind| attachment.kind == kind)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let total = filtered.len();
    let items = filtered
        .into_iter()
        .take(limit)
        .map(attachment_json)
        .collect::<Vec<_>>();

    Ok(json!({
        "scope": scope,
        "scopeNote": scope_note,
        "library": {
            "path": "@attachments",
            "note": "This is a virtual attachment-library path. Use each attachment's readPath with the read_* tools; do not treat it as a workspace path."
        },
        "total": total,
        "truncated": total > limit,
        "attachments": items
    }))
}

fn attachment_json(attachment: &AgentAttachmentReference) -> Value {
    json!({
        "id": attachment.id,
        "conversationId": attachment.conversation_id,
        "messageId": attachment.message_id,
        "projectId": attachment.project_id,
        "kind": attachment.kind,
        "name": attachment.name,
        "mimeType": attachment.mime_type,
        "sizeBytes": attachment.size_bytes,
        "readPath": attachment.read_path,
        "createdAt": attachment.created_at
    })
}
