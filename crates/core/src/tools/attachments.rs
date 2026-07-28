use super::{
    search_cursor::{
        decode_search_cursor, encode_search_cursor, largest_fitting_page_len,
        verify_search_snapshot, SearchCursorKind, SearchFingerprint,
    },
    AgentTool, ToolExecutionContext,
};
use crate::protocol::{
    AgentAttachmentReference, AgentError, AgentInputAttachmentKind, AgentResult,
    AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

const DEFAULT_ATTACHMENT_LIST_LIMIT: usize = 100;
const MAX_ATTACHMENT_LIST_LIMIT: usize = 500;
const MAX_ATTACHMENT_LIST_CURSOR_BYTES: usize = 2 * 1024;

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
            context,
            context.conversation_attachments(),
            args,
        )
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        attachment_list_model_projection(result)
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
            context,
            context.project_attachments(),
            args,
        )
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        attachment_list_model_projection(result)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentListArgs {
    kind: Option<AgentInputAttachmentKind>,
    limit: Option<usize>,
    cursor: Option<String>,
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
            },
            "cursor": {
                "type": "string",
                "maxLength": MAX_ATTACHMENT_LIST_CURSOR_BYTES,
                "description": "Optional opaque nextCursor from the preceding page. Pass it unchanged and repeat the same kind and limit values. If it expires, omit cursor and list again."
            }
        }
    })
}

fn list_attachments(
    scope: &str,
    scope_note: &str,
    context: &ToolExecutionContext,
    attachments: &[AgentAttachmentReference],
    args: Value,
) -> AgentResult<Value> {
    let args: AttachmentListArgs = serde_json::from_value(args)
        .map_err(|error| AgentError::new(format!("attachments_list 参数无效：{error}")))?;
    validate_attachment_scope(scope, context, attachments)?;
    let limit = args
        .limit
        .unwrap_or(DEFAULT_ATTACHMENT_LIST_LIMIT)
        .clamp(1, MAX_ATTACHMENT_LIST_LIMIT);
    let cursor_kind = attachment_cursor_kind(scope)?;
    let request_hash = attachment_request_hash(scope, context, args.kind, limit)?;
    let cursor = args
        .cursor
        .as_deref()
        .map(|cursor| decode_search_cursor(cursor, cursor_kind, &request_hash))
        .transpose()?;
    let offset = cursor.as_ref().map_or(0, |cursor| cursor.next_index);
    let filtered = attachments
        .iter()
        .filter(|attachment| {
            args.kind
                .map(|kind| attachment.kind == kind)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let total = filtered.len();
    let catalog_revision = attachment_catalog_revision(&filtered);
    verify_search_snapshot(cursor.as_ref(), &catalog_revision, total)?;
    let page = AttachmentPageBuilder {
        scope,
        scope_note,
        attachments: &filtered,
        offset,
        total,
        cursor_kind,
        request_hash: &request_hash,
        catalog_revision: &catalog_revision,
    };
    let available = total.saturating_sub(offset).min(limit);
    let page_len = largest_fitting_page_len(context, available, |candidate_len| {
        let candidate = page.build(candidate_len)?;
        attachment_list_model_value(&candidate)
            .ok_or_else(|| AgentError::new("attachments_list 无法构建模型结果投影。"))
    })?;
    page.build(page_len)
}

struct AttachmentPageBuilder<'a> {
    scope: &'a str,
    scope_note: &'a str,
    attachments: &'a [&'a AgentAttachmentReference],
    offset: usize,
    total: usize,
    cursor_kind: SearchCursorKind,
    request_hash: &'a str,
    catalog_revision: &'a str,
}

impl AttachmentPageBuilder<'_> {
    fn build(&self, page_len: usize) -> AgentResult<Value> {
        let items = self
            .attachments
            .iter()
            .copied()
            .skip(self.offset)
            .take(page_len)
            .map(attachment_json)
            .collect::<Vec<_>>();
        let returned = items.len();
        let next_offset = self.offset.saturating_add(returned);
        let next_cursor = (next_offset < self.total)
            .then(|| {
                encode_search_cursor(
                    self.cursor_kind,
                    self.request_hash,
                    self.catalog_revision,
                    next_offset,
                )
            })
            .transpose()?;
        let mut output = json!({
            "scope": self.scope,
            "scopeNote": self.scope_note,
            "library": {
                "path": "@attachments",
                "note": "This is a virtual attachment-library path. Use each attachment's readPath with the read_* tools; do not treat it as a workspace path."
            },
            "total": self.total,
            "returned": returned,
            "omitted": self.total.saturating_sub(returned),
            "truncated": next_cursor.is_some(),
            "attachments": items
        });
        if let Some(next_cursor) = next_cursor {
            output["nextCursor"] = Value::String(next_cursor);
        }
        Ok(output)
    }
}

fn validate_attachment_scope(
    scope: &str,
    context: &ToolExecutionContext,
    attachments: &[AgentAttachmentReference],
) -> AgentResult<()> {
    let conversation_id = context.conversation_id_optional();
    let project_id = context.project_id();
    let library_matches = context.attachment_library().is_none_or(|library| {
        library.conversation_id.as_deref() == conversation_id
            && library.project_id.as_deref() == project_id
    });
    let valid = library_matches
        && match scope {
            "current_conversation" => attachments
                .iter()
                .all(|attachment| Some(attachment.conversation_id.as_str()) == conversation_id),
            "project_other_conversations" => attachments.iter().all(|attachment| {
                attachment.project_id.as_deref() == project_id
                    && Some(attachment.conversation_id.as_str()) != conversation_id
            }),
            _ => false,
        };
    if valid {
        Ok(())
    } else {
        Err(AgentError::structured(
            "attachments.scope_invalid",
            "附件目录与当前 conversation/project 权限范围不一致，已拒绝列出。",
            json!({
                "type": "attachment_list_scope",
                "code": "scopeInvalid",
                "recovery": "restartRun"
            }),
        ))
    }
}

fn attachment_cursor_kind(scope: &str) -> AgentResult<SearchCursorKind> {
    match scope {
        "current_conversation" => Ok(SearchCursorKind::AttachmentsConversation),
        "project_other_conversations" => Ok(SearchCursorKind::AttachmentsProject),
        _ => Err(AgentError::new("未知的附件列表范围。")),
    }
}

fn attachment_request_hash(
    scope: &str,
    context: &ToolExecutionContext,
    kind: Option<AgentInputAttachmentKind>,
    limit: usize,
) -> AgentResult<String> {
    let library = context.attachment_library();
    let permissions = serde_json::to_vec(&context.permissions())
        .map_err(|error| AgentError::new(format!("无法绑定附件列表范围：{error}")))?;
    let mut fingerprint = SearchFingerprint::new("attachment-list-request-v1");
    fingerprint.string("scope", scope);
    fingerprint.string(
        "conversationId",
        context.conversation_id_optional().unwrap_or(""),
    );
    fingerprint.string("projectId", context.project_id().unwrap_or(""));
    fingerprint.field("permissions", &permissions);
    fingerprint.string(
        "libraryConversationId",
        library
            .and_then(|library| library.conversation_id.as_deref())
            .unwrap_or(""),
    );
    fingerprint.string(
        "libraryProjectId",
        library
            .and_then(|library| library.project_id.as_deref())
            .unwrap_or(""),
    );
    fingerprint.string(
        "libraryRootPath",
        library
            .and_then(|library| library.root_path.as_deref())
            .unwrap_or(""),
    );
    fingerprint.string(
        "kind",
        match kind {
            Some(AgentInputAttachmentKind::File) => "file",
            Some(AgentInputAttachmentKind::Image) => "image",
            None => "all",
        },
    );
    fingerprint.usize("limit", limit);
    Ok(fingerprint.finish())
}

fn attachment_catalog_revision(attachments: &[&AgentAttachmentReference]) -> String {
    let mut fingerprint = SearchFingerprint::new("attachment-list-catalog-v1");
    fingerprint.usize("total", attachments.len());
    for attachment in attachments {
        // Include every capability field so pagination fails closed if an attachment is edited,
        // replaced, reordered, or rebound between pages.
        let bytes = serde_json::to_vec(attachment)
            .expect("AgentAttachmentReference serialization is infallible");
        fingerprint.field("attachment", &bytes);
    }
    fingerprint.finish()
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

fn attachment_list_model_projection(result: &AgentToolResult) -> AgentToolResult {
    let projected = result.result.as_ref().and_then(attachment_list_model_value);
    super::model_projection::compact_model_result(result, projected)
}

fn attachment_list_model_value(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut output = Map::new();
    for field in [
        "scope",
        "total",
        "returned",
        "omitted",
        "truncated",
        "nextCursor",
    ] {
        super::model_projection::insert_field(&mut output, value, field);
    }
    let attachments = source
        .get("attachments")
        .and_then(Value::as_array)
        .map(|attachments| {
            attachments
                .iter()
                .filter_map(|attachment| {
                    super::model_projection::retain_object_fields(
                        attachment,
                        &["name", "kind", "mimeType", "sizeBytes", "readPath"],
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    output
        .entry("returned".to_string())
        .or_insert_with(|| Value::from(u64::try_from(attachments.len()).unwrap_or(u64::MAX)));
    if !attachments.is_empty() {
        output.insert("attachments".to_string(), Value::Array(attachments));
    }
    Some(Value::Object(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentAttachmentLibraryContext, AgentCommandPermission, AgentCommandSafetyPolicy,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentRunContext,
        AgentWritePermission,
    };

    #[test]
    fn cursor_pages_are_stable_without_duplicates_or_skipped_attachments() {
        let context = attachment_context(
            AgentReadPermission::WorkspaceOnly,
            (0..5)
                .map(|index| attachment(index, AgentInputAttachmentKind::File, "conversation-1"))
                .collect(),
            Vec::new(),
        );

        let first = current_page(&context, json!({ "kind": "file", "limit": 2 })).unwrap();
        assert_page(&first, 5, 2, 3, &["attachment-0", "attachment-1"]);
        let first_cursor = first["nextCursor"].as_str().unwrap().to_string();

        let second = current_page(
            &context,
            json!({ "cursor": first_cursor, "kind": "file", "limit": 2 }),
        )
        .unwrap();
        assert_page(&second, 5, 2, 3, &["attachment-2", "attachment-3"]);
        let second_cursor = second["nextCursor"].as_str().unwrap().to_string();

        let third = current_page(
            &context,
            json!({ "cursor": second_cursor, "kind": "file", "limit": 2 }),
        )
        .unwrap();
        assert_page(&third, 5, 1, 4, &["attachment-4"]);
        assert_eq!(third["truncated"], false);
        assert!(third["nextCursor"].is_null());
    }

    #[test]
    fn project_attachment_pages_use_the_same_stable_contract() {
        let context = attachment_context(
            AgentReadPermission::WorkspaceOnly,
            Vec::new(),
            (0..3)
                .map(|index| {
                    attachment(
                        index + 10,
                        AgentInputAttachmentKind::File,
                        "conversation-other",
                    )
                })
                .collect(),
        );

        let first = list_attachments(
            "project_other_conversations",
            "project",
            &context,
            context.project_attachments(),
            json!({ "limit": 2 }),
        )
        .unwrap();
        assert_page(&first, 3, 2, 1, &["attachment-10", "attachment-11"]);
        let second = list_attachments(
            "project_other_conversations",
            "project",
            &context,
            context.project_attachments(),
            json!({ "limit": 2, "cursor": first["nextCursor"].as_str().unwrap() }),
        )
        .unwrap();
        assert_page(&second, 3, 1, 2, &["attachment-12"]);
    }

    #[test]
    fn cursor_rejects_filter_scope_permission_and_catalog_changes() {
        let conversation_attachments = vec![
            attachment(0, AgentInputAttachmentKind::File, "conversation-1"),
            attachment(1, AgentInputAttachmentKind::File, "conversation-1"),
            attachment(2, AgentInputAttachmentKind::Image, "conversation-1"),
        ];
        let project_attachments = vec![attachment(
            10,
            AgentInputAttachmentKind::File,
            "conversation-other",
        )];
        let context = attachment_context(
            AgentReadPermission::WorkspaceOnly,
            conversation_attachments.clone(),
            project_attachments,
        );
        let first = current_page(&context, json!({ "kind": "file", "limit": 1 })).unwrap();
        let cursor = first["nextCursor"].as_str().unwrap();

        let filter_error = current_page(
            &context,
            json!({ "cursor": cursor, "kind": "image", "limit": 1 }),
        )
        .unwrap_err();
        assert_cursor_error(&filter_error, "scope_or_filter_changed");

        let scope_error = list_attachments(
            "project_other_conversations",
            "project",
            &context,
            context.project_attachments(),
            json!({ "cursor": cursor, "kind": "file", "limit": 1 }),
        )
        .unwrap_err();
        assert_cursor_error(&scope_error, "scope_or_filter_changed");

        let permission_context = attachment_context(
            AgentReadPermission::All,
            conversation_attachments.clone(),
            Vec::new(),
        );
        let permission_error = current_page(
            &permission_context,
            json!({ "cursor": cursor, "kind": "file", "limit": 1 }),
        )
        .unwrap_err();
        assert_cursor_error(&permission_error, "scope_or_filter_changed");

        let mut changed = conversation_attachments;
        changed[1].name = "renamed.txt".to_string();
        let changed_context =
            attachment_context(AgentReadPermission::WorkspaceOnly, changed, Vec::new());
        let revision_error = current_page(
            &changed_context,
            json!({ "cursor": cursor, "kind": "file", "limit": 1 }),
        )
        .unwrap_err();
        assert_cursor_error(&revision_error, "catalog_changed");
    }

    #[test]
    fn malformed_or_terminal_cursor_requires_relisting() {
        let context = attachment_context(
            AgentReadPermission::WorkspaceOnly,
            vec![
                attachment(0, AgentInputAttachmentKind::File, "conversation-1"),
                attachment(1, AgentInputAttachmentKind::File, "conversation-1"),
            ],
            Vec::new(),
        );
        let malformed = current_page(&context, json!({ "cursor": "att_v1_not-json" })).unwrap_err();
        assert_eq!(malformed.code(), Some("attachments.cursor_invalid"));
        assert_eq!(malformed.details().unwrap()["recovery"], "restartList");

        let filtered = context
            .conversation_attachments()
            .iter()
            .collect::<Vec<_>>();
        let request_hash =
            attachment_request_hash("current_conversation", &context, None, 1).unwrap();
        let terminal = encode_search_cursor(
            SearchCursorKind::AttachmentsConversation,
            &request_hash,
            &attachment_catalog_revision(&filtered),
            filtered.len(),
        )
        .unwrap();
        let terminal_error =
            current_page(&context, json!({ "cursor": terminal, "limit": 1 })).unwrap_err();
        assert_cursor_error(&terminal_error, "catalog_changed");
    }

    #[test]
    fn long_attachment_names_are_budgeted_before_cursor_advances() {
        let attachments = (0..40)
            .map(|index| {
                let mut attachment =
                    attachment(index, AgentInputAttachmentKind::File, "conversation-1");
                attachment.name = format!("{index}-{}", "very-long-name-".repeat(350));
                attachment
            })
            .collect::<Vec<_>>();
        let context =
            attachment_context(AgentReadPermission::WorkspaceOnly, attachments, Vec::new());
        let gate = crate::context::ContextCapacityDetector::for_model(
            "test-model",
            crate::protocol::AgentApiStyle::OpenAiCompatible,
            &[],
        )
        .model_tool_result_gate();
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();

        loop {
            let mut args = json!({ "limit": MAX_ATTACHMENT_LIST_LIMIT });
            if let Some(cursor) = cursor.as_ref() {
                args["cursor"] = Value::String(cursor.clone());
            }
            let page = current_page(&context, args).unwrap();
            let raw = AgentToolResult {
                exact_archive_file: None,
                call_id: "attachments-budget-page".to_string(),
                tool: "attachments_list".to_string(),
                ok: true,
                result: Some(page.clone()),
                error: None,
            };
            let projected = attachment_list_model_projection(&raw);
            let gated = gate.project(&raw.call_id, false, &projected, None);
            assert!(
                !gated.truncated,
                "attachment page must fit before its cursor is emitted"
            );

            let page_items = page["attachments"].as_array().unwrap();
            assert!(!page_items.is_empty());
            for item in page_items {
                assert!(
                    seen.insert(item["id"].as_str().unwrap().to_string()),
                    "attachment appeared on more than one page"
                );
            }
            assert_eq!(
                page["omitted"].as_u64().unwrap(),
                40_u64.saturating_sub(u64::try_from(page_items.len()).unwrap())
            );

            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), 40);
    }

    #[test]
    fn schema_and_model_projection_expose_only_simple_paging_fields() {
        let schema = attachment_list_schema();
        assert!(schema["properties"]["cursor"]["description"]
            .as_str()
            .unwrap()
            .contains("nextCursor"));

        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "attachments-page".to_string(),
            tool: "attachments_list".to_string(),
            ok: true,
            result: Some(json!({
                "scope": "current_conversation",
                "total": 4,
                "returned": 2,
                "omitted": 2,
                "truncated": true,
                "nextCursor": "att_v1_opaque",
                "attachments": [{
                    "id": "private-id",
                    "conversationId": "conversation-1",
                    "messageId": "message-1",
                    "name": "notes.txt",
                    "kind": "file",
                    "mimeType": "text/plain",
                    "sizeBytes": 10,
                    "readPath": "@attachments/private-id/notes.txt",
                    "createdAt": 1
                }]
            })),
            error: None,
        };
        let projected = attachment_list_model_projection(&raw);
        let value = projected.result.unwrap();
        assert_eq!(value["returned"], 2);
        assert_eq!(value["omitted"], 2);
        assert_eq!(value["nextCursor"], "att_v1_opaque");
        assert!(value["attachments"][0].get("id").is_none());
        assert!(value["attachments"][0].get("conversationId").is_none());
        assert_eq!(
            value["attachments"][0]["readPath"],
            "@attachments/private-id/notes.txt"
        );
    }

    fn current_page(context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        list_attachments(
            "current_conversation",
            "current",
            context,
            context.conversation_attachments(),
            args,
        )
    }

    fn attachment_context(
        read: AgentReadPermission,
        conversation_attachments: Vec<AgentAttachmentReference>,
        project_attachments: Vec<AgentAttachmentReference>,
    ) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: Some("project-1".to_string()),
            workspace: None,
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some("/attachment-library".to_string()),
                conversation_id: Some("conversation-1".to_string()),
                project_id: Some("project-1".to_string()),
                conversation_attachments,
                project_attachments,
            }),
            permissions: AgentPermissions {
                read,
                write: AgentWritePermission::Denied,
                command: AgentCommandPermission::RequireApproval,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                patch: AgentPatchPermission::RequireApproval,
            },
        }))
        .with_tool_call_id("attachments-test-call".to_string())
        .with_text_output_budget(crate::context::ContextTextBudget::heuristic(
            crate::context::MODEL_TOOL_RESULT_MAX_TOKENS,
        ))
    }

    fn attachment(
        index: usize,
        kind: AgentInputAttachmentKind,
        conversation_id: &str,
    ) -> AgentAttachmentReference {
        let id = format!("attachment-{index}");
        AgentAttachmentReference {
            id: id.clone(),
            conversation_id: conversation_id.to_string(),
            message_id: format!("message-{index}"),
            project_id: Some("project-1".to_string()),
            kind,
            name: format!("{id}.txt"),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 10,
            read_path: format!("@attachments/{id}/{id}.txt"),
            storage_rel_path: format!("conversations/{conversation_id}/{id}.txt"),
            created_at: i64::try_from(index).unwrap(),
        }
    }

    fn assert_page(page: &Value, total: u64, returned: u64, omitted: u64, ids: &[&str]) {
        assert_eq!(page["total"], total);
        assert_eq!(page["returned"], returned);
        assert_eq!(page["omitted"], omitted);
        let actual = page["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, ids);
    }

    fn assert_cursor_error(error: &AgentError, reason: &str) {
        assert_eq!(error.code(), Some("attachments.cursor_invalid"));
        assert_eq!(error.details().unwrap()["reason"], reason);
        assert_eq!(error.details().unwrap()["recovery"], "restartList");
    }
}
