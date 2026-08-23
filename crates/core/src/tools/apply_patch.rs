use super::apply_patch_diff::{
    build_unified_diff, edit_error, sanitize_patch, validate_patch_operation,
    StructuredEditErrorCode,
};
use super::apply_patch_paths::sanitize_file_path;
#[cfg(test)]
use super::apply_patch_paths::validate_text_patch_path;
use super::{
    clean_relative_path, AgentTool, AgentToolPermissionPolicy, FileWriteToolAccess,
    ToolExecutionContext,
};
use crate::protocol::{
    AgentApprovalStatus, AgentDiffProposal, AgentError, AgentPatchOperation, AgentProposedAction,
    AgentResult, AgentToolCall, AgentToolDefinition, AgentToolSafety,
};
use crate::revision::content_revision;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_SUMMARY_CHARS: usize = 2_000;
const MAX_EDIT_CONTENT_BYTES: usize = 240_000;
const MAX_INLINE_CONTENT_BYTES: usize = 32 * 1024;

pub(super) struct ApplyPatchTool;

impl AgentTool for ApplyPatchTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "apply_patch".to_string(),
            description: "Request one create, update, or delete operation for a text/code/config file. Before calling create, update, or delete, use read_file on the exact target to confirm its current state and content. Never use create as an existence probe: call create only after read_file explicitly reports that the exact target does not exist or cannot be found. If read_file finds an existing target, use update for edits or delete for deletion; never use create for an existing file. Prefer structured content/edits; Rust generates the unified diff. update supports replace, insert_before, insert_after, append, and prepend. The same tool call remains active through approval and host execution. This tool never writes before host approval.".to_string(),
            input_schema: patch_input_schema(),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: crate::protocol::AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "apply_patch 需要用户审批和 host 执行层，不能由 agent runtime 自动应用。",
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::FileWrite(FileWriteToolAccess::WriteOnly)
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::Diff {
            diff: diff_proposal_from_call(context, call)?,
        })
    }
}

fn patch_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["create", "update", "delete"],
                "description": "Requested operation. Read the exact target with read_file first. create uses content and is valid only after read_file explicitly reports that target does not exist or cannot be found; an existing target must use update for edits. update uses edits or complete content; delete only needs filePath after the required read."
            },
            "filePath": { "type": "string", "description": "Workspace-relative path, absolute local path, or a system alias such as @desktop/file.txt when permissions allow it." },
            "content": { "type": "string", "maxLength": 32768, "description": "Complete UTF-8 file content for small files only. Required for create; optional for update. Use write_file for content above 32 KiB or content that should be generated in chunks." },
            "edits": {
                "type": "array",
                "minItems": 1,
                "description": "Ordered structured edits for update. Prefer these over writing a unified diff.",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["replace", "insert_before", "insert_after", "append", "prepend"] },
                        "oldText": { "type": "string", "description": "Exact text to replace when kind=replace." },
                        "newText": { "type": "string", "description": "Replacement text when kind=replace." },
                        "anchor": { "type": "string", "description": "Exact unique anchor when kind=insert_before or insert_after." },
                        "text": { "type": "string", "description": "Text to insert when kind is insert_before, insert_after, append, or prepend." },
                        "replaceAll": { "type": "boolean", "description": "For replace only. Defaults to false; false requires oldText to occur exactly once." }
                    },
                    "required": ["kind"]
                }
            },
            "patch": { "type": "string", "description": "Legacy advanced input: a complete unified diff. Do not use when content or edits can express the change." },
            "summary": { "type": "string", "description": "Short human-readable summary of the proposed change." }
        },
        "required": ["operation", "filePath"]
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyPatchArgs {
    operation: AgentPatchOperation,
    file_path: String,
    content: Option<String>,
    edits: Option<Vec<StructuredTextEdit>>,
    expected_revision: Option<String>,
    patch: Option<String>,
    summary: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StructuredTextEdit {
    kind: TextEditKind,
    old_text: Option<String>,
    new_text: Option<String>,
    anchor: Option<String>,
    text: Option<String>,
    replace_all: Option<bool>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum TextEditKind {
    Replace,
    InsertBefore,
    InsertAfter,
    Append,
    Prepend,
}

fn diff_proposal_from_call(
    context: &ToolExecutionContext,
    call: &AgentToolCall,
) -> AgentResult<AgentDiffProposal> {
    let args: ApplyPatchArgs = serde_json::from_value(call.args.clone())
        .map_err(|error| AgentError::new(format!("apply_patch 参数无效：{error}")))?;
    let file_path = sanitize_file_path(&args.file_path, context.permissions().write)?;
    let (patch, base_revision) = build_patch(context, &file_path, &args)?;
    validate_patch_operation(&patch, &file_path, args.operation)?;
    let summary = sanitize_summary(args.summary);

    Ok(AgentDiffProposal {
        id: call.id.clone(),
        operation: args.operation,
        file_path,
        patch,
        base_revision,
        summary,
        approval_status: AgentApprovalStatus::Required,
    })
}

fn build_patch(
    context: &ToolExecutionContext,
    file_path: &str,
    args: &ApplyPatchArgs,
) -> AgentResult<(String, Option<String>)> {
    if let Some(patch) = args.patch.as_ref() {
        if args.content.is_some() || args.edits.is_some() {
            return Err(edit_error(
                StructuredEditErrorCode::ConflictingInput,
                "patch 不能和 content 或 edits 同时提供。",
            ));
        }
        let base_revision = match args.operation {
            AgentPatchOperation::Create => None,
            AgentPatchOperation::Update | AgentPatchOperation::Delete => {
                let current = read_current_text(context, file_path)?;
                validate_expected_revision(&current, args.expected_revision.as_deref())?;
                Some(content_revision(current.as_bytes()))
            }
        };
        return Ok((sanitize_patch(patch.clone())?, base_revision));
    }

    match args.operation {
        AgentPatchOperation::Create => {
            if args.edits.is_some() {
                return Err(edit_error(
                    StructuredEditErrorCode::InvalidCreate,
                    "create 只接受完整 content，不接受 edits。",
                ));
            }
            let content = args.content.as_deref().ok_or_else(|| {
                edit_error(
                    StructuredEditErrorCode::MissingContent,
                    "create 操作必须提供完整 content。",
                )
            })?;
            validate_inline_content_size(content)?;
            let target = target_path_for_create(context, file_path)?;
            if target.exists() {
                return Err(edit_error(
                    StructuredEditErrorCode::FileExists,
                    "文件已存在。",
                ));
            }
            Ok((
                build_unified_diff(file_path, AgentPatchOperation::Create, "", content)?,
                None,
            ))
        }
        AgentPatchOperation::Update => {
            let current = read_current_text(context, file_path)?;
            validate_expected_revision(&current, args.expected_revision.as_deref())?;
            let base_revision = content_revision(current.as_bytes());
            let updated = match (args.content.as_deref(), args.edits.as_deref()) {
                (Some(_), Some(_)) => {
                    return Err(edit_error(
                        StructuredEditErrorCode::ConflictingInput,
                        "update 的 content 和 edits 只能提供一种。",
                    ))
                }
                (Some(content), None) => {
                    validate_inline_content_size(content)?;
                    content.to_string()
                }
                (None, Some(edits)) if !edits.is_empty() => {
                    apply_structured_edits(&current, edits)?
                }
                _ => {
                    return Err(edit_error(
                        StructuredEditErrorCode::MissingEdit,
                        "update 必须提供 content 或至少一个 structured edit。",
                    ))
                }
            };
            validate_content_size(&updated)?;
            if updated == current {
                return Err(edit_error(
                    StructuredEditErrorCode::NoChange,
                    "编辑后的内容与当前文件完全相同。",
                ));
            }
            Ok((
                build_unified_diff(file_path, AgentPatchOperation::Update, &current, &updated)?,
                Some(base_revision),
            ))
        }
        AgentPatchOperation::Delete => {
            if args.content.is_some() || args.edits.is_some() {
                return Err(edit_error(
                    StructuredEditErrorCode::InvalidDelete,
                    "delete 只需要 filePath，不接受 content 或 edits。",
                ));
            }
            let current = read_current_text(context, file_path)?;
            validate_expected_revision(&current, args.expected_revision.as_deref())?;
            let base_revision = content_revision(current.as_bytes());
            Ok((
                build_unified_diff(file_path, AgentPatchOperation::Delete, &current, "")?,
                Some(base_revision),
            ))
        }
    }
}

fn read_current_text(context: &ToolExecutionContext, file_path: &str) -> AgentResult<String> {
    context.check_cancelled()?;
    let resolved = context.resolve_existing_path(file_path)?;
    let metadata = fs::metadata(&resolved).map_err(|error| {
        edit_error(
            StructuredEditErrorCode::ReadFailed,
            format!("读取目标文件元数据失败：{error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(edit_error(
            StructuredEditErrorCode::NotAFile,
            "目标路径不是文件。",
        ));
    }
    if metadata.len() > MAX_EDIT_CONTENT_BYTES as u64 {
        return Err(edit_error(
            StructuredEditErrorCode::FileTooLarge,
            format!(
                "目标文件为 {} bytes，超过结构化编辑限制 {} bytes。",
                metadata.len(),
                MAX_EDIT_CONTENT_BYTES
            ),
        ));
    }
    let content = fs::read_to_string(&resolved).map_err(|error| {
        edit_error(
            StructuredEditErrorCode::ReadFailed,
            format!("读取 UTF-8 文件失败：{error}"),
        )
    })?;
    context.check_cancelled()?;
    Ok(content)
}

fn target_path_for_create(context: &ToolExecutionContext, file_path: &str) -> AgentResult<PathBuf> {
    let path = Path::new(file_path);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(context
        .workspace_root()?
        .join(clean_relative_path(file_path)?))
}

fn validate_expected_revision(content: &str, expected: Option<&str>) -> AgentResult<()> {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let actual = content_revision(content.as_bytes());
    if expected != actual {
        return Err(edit_error(
            StructuredEditErrorCode::StaleFile,
            format!(
                "文件已发生变化：expectedRevision={expected}，currentRevision={actual}。请重新读取后再编辑。"
            ),
        ));
    }
    Ok(())
}

fn validate_content_size(content: &str) -> AgentResult<()> {
    if content.len() > MAX_EDIT_CONTENT_BYTES {
        return Err(edit_error(
            StructuredEditErrorCode::ContentTooLarge,
            format!(
                "编辑内容为 {} bytes，超过 {} bytes 限制。",
                content.len(),
                MAX_EDIT_CONTENT_BYTES
            ),
        ));
    }
    Ok(())
}

fn validate_inline_content_size(content: &str) -> AgentResult<()> {
    if content.len() > MAX_INLINE_CONTENT_BYTES {
        return Err(edit_error(
            StructuredEditErrorCode::UseStagedWrite,
            format!(
                "完整 content 为 {} bytes，超过 apply_patch 的 {} bytes 内联限制；请使用 write_file 分块生成草稿。",
                content.len(),
                MAX_INLINE_CONTENT_BYTES
            ),
        ));
    }
    validate_content_size(content)
}

fn apply_structured_edits(current: &str, edits: &[StructuredTextEdit]) -> AgentResult<String> {
    let mut content = current.to_string();
    for (index, edit) in edits.iter().enumerate() {
        content = apply_structured_edit(content, edit, index)?;
        validate_content_size(&content)?;
    }
    Ok(content)
}

fn apply_structured_edit(
    mut content: String,
    edit: &StructuredTextEdit,
    index: usize,
) -> AgentResult<String> {
    match edit.kind {
        TextEditKind::Replace => {
            reject_unexpected_fields(edit, &["oldText", "newText", "replaceAll"], index)?;
            let old_text = required_edit_text(edit.old_text.as_deref(), "oldText", index, false)?;
            let new_text = required_edit_text(edit.new_text.as_deref(), "newText", index, true)?;
            let count = content.match_indices(old_text).count();
            if count == 0 {
                return Err(edit_match_error(
                    StructuredEditErrorCode::MatchNotFound,
                    index,
                    "oldText",
                    0,
                ));
            }
            if edit.replace_all.unwrap_or(false) {
                content = content.replace(old_text, new_text);
            } else if count == 1 {
                content = content.replacen(old_text, new_text, 1);
            } else {
                return Err(edit_match_error(
                    StructuredEditErrorCode::AmbiguousMatch,
                    index,
                    "oldText",
                    count,
                ));
            }
        }
        TextEditKind::InsertBefore | TextEditKind::InsertAfter => {
            reject_unexpected_fields(edit, &["anchor", "text"], index)?;
            let anchor = required_edit_text(edit.anchor.as_deref(), "anchor", index, false)?;
            let text = required_edit_text(edit.text.as_deref(), "text", index, true)?;
            let matches = content.match_indices(anchor).collect::<Vec<_>>();
            if matches.is_empty() {
                return Err(edit_match_error(
                    StructuredEditErrorCode::MatchNotFound,
                    index,
                    "anchor",
                    0,
                ));
            }
            if matches.len() > 1 {
                return Err(edit_match_error(
                    StructuredEditErrorCode::AmbiguousMatch,
                    index,
                    "anchor",
                    matches.len(),
                ));
            }
            let mut insertion = matches[0].0;
            if matches!(edit.kind, TextEditKind::InsertAfter) {
                insertion += anchor.len();
            }
            content.insert_str(insertion, text);
        }
        TextEditKind::Append => {
            reject_unexpected_fields(edit, &["text"], index)?;
            let text = required_edit_text(edit.text.as_deref(), "text", index, true)?;
            content.push_str(text);
        }
        TextEditKind::Prepend => {
            reject_unexpected_fields(edit, &["text"], index)?;
            let text = required_edit_text(edit.text.as_deref(), "text", index, true)?;
            content.insert_str(0, text);
        }
    }
    Ok(content)
}

fn required_edit_text<'a>(
    value: Option<&'a str>,
    field: &str,
    index: usize,
    allow_empty: bool,
) -> AgentResult<&'a str> {
    let value = value.ok_or_else(|| {
        edit_error(
            StructuredEditErrorCode::InvalidEdit,
            format!("edits[{index}].{field} 是必填字段。"),
        )
    })?;
    if !allow_empty && value.is_empty() {
        return Err(edit_error(
            StructuredEditErrorCode::InvalidEdit,
            format!("edits[{index}].{field} 不能为空。"),
        ));
    }
    Ok(value)
}

fn reject_unexpected_fields(
    edit: &StructuredTextEdit,
    allowed: &[&str],
    index: usize,
) -> AgentResult<()> {
    let supplied = [
        ("oldText", edit.old_text.is_some()),
        ("newText", edit.new_text.is_some()),
        ("anchor", edit.anchor.is_some()),
        ("text", edit.text.is_some()),
        ("replaceAll", edit.replace_all.is_some()),
    ];
    let unexpected = supplied
        .into_iter()
        .filter_map(|(field, present)| (present && !allowed.contains(&field)).then_some(field))
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(edit_error(
            StructuredEditErrorCode::InvalidEdit,
            format!(
                "edits[{index}] 包含不适用于当前 kind 的字段：{}。",
                unexpected.join(", ")
            ),
        ));
    }
    Ok(())
}

fn edit_match_error(
    code: StructuredEditErrorCode,
    index: usize,
    field: &str,
    count: usize,
) -> AgentError {
    edit_error(
        code,
        format!(
            "edits[{index}].{field} 在当前文件中匹配 {count} 次；请重新读取文件并提供唯一、精确的文本。"
        ),
    )
}

fn sanitize_summary(summary: Option<String>) -> Option<String> {
    summary
        .map(|summary| summary.trim().chars().take(MAX_SUMMARY_CHARS).collect())
        .filter(|summary: &String| !summary.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentApprovalStatus, AgentCommandPermission, AgentPermissions, AgentReadPermission,
        AgentRunContext, AgentToolCall, AgentWritePermission,
    };
    use serde_json::json;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn input_schema_does_not_expose_expected_revision() {
        let schema = patch_input_schema();
        let properties = schema["properties"].as_object().unwrap();

        assert!(!properties.contains_key("expectedRevision"));
    }

    #[test]
    fn definition_requires_read_before_selecting_the_patch_operation() {
        let definition = ApplyPatchTool.definition();

        assert!(definition.description.contains(
            "Before calling create, update, or delete, use read_file on the exact target"
        ));
        assert!(definition
            .description
            .contains("Never use create as an existence probe"));
        assert!(definition
            .description
            .contains("call create only after read_file explicitly reports that the exact target does not exist or cannot be found"));
        assert!(definition
            .description
            .contains("If read_file finds an existing target, use update for edits"));

        let operation_description = definition.input_schema["properties"]["operation"]
            ["description"]
            .as_str()
            .unwrap();
        assert!(operation_description.contains("Read the exact target with read_file first"));
        assert!(operation_description.contains(
            "read_file explicitly reports that target does not exist or cannot be found"
        ));
    }

    #[test]
    fn builds_diff_proposal_for_text_patch() {
        let workspace = TestWorkspace::new();
        workspace.write("src/main.rs", "fn main() {}\n");
        let call = AgentToolCall {
            id: "tool-1".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({
                "operation": "update",
                "filePath": "src/main.rs",
                "patch": "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-fn main() {}\n+fn main() { println!(\"hi\"); }",
                "summary": "Add greeting"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let context = workspace.context();
        let proposal = diff_proposal_from_call(&context, &call).unwrap();

        assert_eq!(proposal.id, "tool-1");
        assert_eq!(proposal.operation, AgentPatchOperation::Update);
        assert_eq!(proposal.file_path, "src/main.rs");
        assert_eq!(
            proposal.base_revision,
            Some(content_revision(b"fn main() {}\n"))
        );
        assert!(proposal.patch.ends_with('\n'));
        assert_eq!(proposal.summary.as_deref(), Some("Add greeting"));
        assert_eq!(proposal.approval_status, AgentApprovalStatus::Required);
    }

    #[test]
    fn structured_create_generates_valid_diff() {
        let workspace = TestWorkspace::new();
        let call = tool_call(json!({
            "operation": "create",
            "filePath": "quicksort.py",
            "content": "def quick_sort(values):\n    return sorted(values)\n"
        }));

        let proposal = diff_proposal_from_call(&workspace.context(), &call).unwrap();

        assert!(proposal
            .patch
            .starts_with("--- /dev/null\n+++ b/quicksort.py\n"));
        assert!(proposal.patch.contains("+def quick_sort(values):"));
        assert_eq!(proposal.base_revision, None);
        assert_git_apply_check(&workspace.root, &proposal.patch);
    }

    #[test]
    fn structured_create_supports_empty_file() {
        let workspace = TestWorkspace::new();
        let call = tool_call(json!({
            "operation": "create",
            "filePath": "empty.txt",
            "content": ""
        }));

        let proposal = diff_proposal_from_call(&workspace.context(), &call).unwrap();

        assert!(proposal.patch.contains("new file mode 100644"));
        assert_git_apply_check(&workspace.root, &proposal.patch);
    }

    #[test]
    fn structured_create_reports_existing_file_with_safe_typed_error() {
        let workspace = TestWorkspace::new();
        workspace.write("existing.txt", "keep me\n");
        let call = tool_call(json!({
            "operation": "create",
            "filePath": "existing.txt",
            "content": "replacement\n"
        }));

        let error = diff_proposal_from_call(&workspace.context(), &call).unwrap_err();

        assert_eq!(error.to_string(), "文件已存在。");
        assert_eq!(error.code(), Some("agent.apply_patch.file_exists"));
        let details = error.details().expect("typed structured-edit details");
        assert_eq!(details["type"], "structured_edit_error");
        assert_eq!(details["code"], "file_exists");
        assert_eq!(details["errorCode"], "agent.apply_patch.file_exists");
        assert_eq!(details["recovery"], "useUpdateOrChooseAnotherPath");
        assert!(!error.to_string().contains("structured_edit_error"));
        assert!(!error.to_string().contains("file_exists"));
    }

    #[test]
    fn structured_create_resolves_system_alias_without_workspace() {
        let alias = format!(
            "@home/.my-copilot-no-workspace-{}.txt",
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: Default::default(),
                builtin_execution: Default::default(),
            },
        }));
        let call = tool_call(json!({
            "operation": "create",
            "filePath": alias,
            "content": "hello\n"
        }));

        let proposal = diff_proposal_from_call(&context, &call).unwrap();

        assert!(Path::new(&proposal.file_path).is_absolute());
        assert!(proposal.patch.starts_with("--- /dev/null\n+++ /"));
    }

    #[test]
    fn structured_update_supports_append_and_insert_after() {
        let workspace = TestWorkspace::new();
        let initial = "def quick_sort(values):\n    return sorted(values)\n";
        workspace.write("quicksort.py", initial);
        let call = tool_call(json!({
            "operation": "update",
            "filePath": "quicksort.py",
            "expectedRevision": content_revision(initial.as_bytes()),
            "edits": [
                {
                    "kind": "insert_after",
                    "anchor": "def quick_sort(values):\n",
                    "text": "    values = list(values)\n"
                },
                {
                    "kind": "append",
                    "text": "\nif __name__ == \"__main__\":\n    print(quick_sort([3, 1, 2]))\n"
                }
            ]
        }));

        let proposal = diff_proposal_from_call(&workspace.context(), &call).unwrap();

        assert_eq!(
            proposal.base_revision,
            Some(content_revision(initial.as_bytes()))
        );
        assert!(proposal.patch.contains("+    values = list(values)"));
        assert!(proposal.patch.contains("+if __name__ == \"__main__\":"));
        assert_git_apply_check(&workspace.root, &proposal.patch);
    }

    #[test]
    fn structured_update_reports_ambiguous_anchor() {
        let workspace = TestWorkspace::new();
        workspace.write("notes.txt", "same\nsame\n");
        let call = tool_call(json!({
            "operation": "update",
            "filePath": "notes.txt",
            "edits": [{
                "kind": "insert_after",
                "anchor": "same",
                "text": " updated"
            }]
        }));

        let error = diff_proposal_from_call(&workspace.context(), &call).unwrap_err();

        assert_eq!(error.code(), Some("agent.apply_patch.ambiguous_match"));
        assert_eq!(error.details().unwrap()["code"], "ambiguous_match");
        assert_eq!(error.details().unwrap()["recovery"], "rereadFile");
        assert!(error.to_string().contains("匹配 2 次"));
        assert!(!error.to_string().contains("structured_edit_error"));
    }

    #[test]
    fn structured_update_rejects_stale_revision() {
        let workspace = TestWorkspace::new();
        workspace.write("notes.txt", "current\n");
        let call = tool_call(json!({
            "operation": "update",
            "filePath": "notes.txt",
            "expectedRevision": content_revision(b"old\n"),
            "edits": [{
                "kind": "replace",
                "oldText": "current",
                "newText": "updated"
            }]
        }));

        let error = diff_proposal_from_call(&workspace.context(), &call).unwrap_err();

        assert_eq!(error.code(), Some("agent.apply_patch.stale_file"));
        assert_eq!(error.details().unwrap()["code"], "stale_file");
        assert_eq!(error.details().unwrap()["recovery"], "rereadFile");
        assert!(error.to_string().contains("currentRevision="));
        assert!(!error.to_string().contains("structured_edit_error"));
    }

    #[test]
    fn generated_diff_handles_file_without_trailing_newline() {
        let workspace = TestWorkspace::new();
        workspace.write("notes.txt", "first");
        let call = tool_call(json!({
            "operation": "update",
            "filePath": "notes.txt",
            "edits": [{
                "kind": "append",
                "text": "\nsecond\n"
            }]
        }));

        let proposal = diff_proposal_from_call(&workspace.context(), &call).unwrap();

        assert!(proposal.patch.contains("\\ No newline at end of file"));
        assert_git_apply_check(&workspace.root, &proposal.patch);
    }

    #[test]
    fn accepts_common_text_file_names_and_extensions() {
        for path in [
            ".env",
            ".gitignore",
            "Dockerfile",
            "CMakeLists.txt",
            "README.md",
            "notebook.ipynb",
            "data.csv",
            "types.d.ts",
            "src/App.tsx",
        ] {
            assert!(validate_text_patch_path(path).is_ok(), "{path}");
        }
    }

    #[test]
    fn rejects_office_and_pdf_paths() {
        for path in ["report.pdf", "slides.pptx", "sheet.xlsx", "legacy.doc"] {
            let error = validate_text_patch_path(path).unwrap_err();

            assert!(error.to_string().contains("专用编辑工具"), "{path}");
        }
    }

    #[test]
    fn rejects_non_unified_patch() {
        let error = sanitize_patch("replace hello with world".to_string()).unwrap_err();

        assert!(error.to_string().contains("unified diff"));
        assert!(!error.to_string().contains("apply_patch.patch"));
    }

    #[test]
    fn rejects_patch_for_different_file() {
        let error = validate_patch_operation(
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n",
            "src/main.rs",
            AgentPatchOperation::Update,
        )
        .unwrap_err();

        assert!(error.to_string().contains("不匹配"));
        assert!(!error.to_string().contains("apply_patch.patch"));
    }

    #[test]
    fn validates_create_update_and_delete_headers() {
        assert!(validate_patch_operation(
            "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+new\n",
            "new.txt",
            AgentPatchOperation::Create,
        )
        .is_ok());
        assert!(validate_patch_operation(
            "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n",
            "file.txt",
            AgentPatchOperation::Update,
        )
        .is_ok());
        assert!(validate_patch_operation(
            "--- a/file.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n",
            "file.txt",
            AgentPatchOperation::Delete,
        )
        .is_ok());
    }

    fn tool_call(args: Value) -> AgentToolCall {
        AgentToolCall {
            id: "tool-structured".to_string(),
            tool: "apply_patch".to_string(),
            args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }
    }

    fn assert_git_apply_check(root: &Path, patch: &str) {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("apply")
            .arg("--check")
            .arg("--whitespace=nowarn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(patch.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{patch}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "my-copilot-agent-structured-edit-{}",
                TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: None,
                project_id: None,
                workspace: Some(crate::protocol::AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("test".to_string()),
                    root_path: Some(self.root.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: AgentPermissions {
                    read: AgentReadPermission::WorkspaceOnly,
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: Default::default(),
                    patch: Default::default(),
                    builtin_execution: Default::default(),
                },
            }))
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
