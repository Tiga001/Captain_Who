use crate::storage::models::AgentFileDraftRecord;
use crate::tools::apply_patch_paths::validate_text_patch_path;
use crate::{
    content_revision, expand_system_path, AgentApprovalStatus, AgentFileDraftSnapshot,
    AgentFileWriteMode, AgentFileWriteProposal, AgentFileWriteResult, AgentFileWriteResultStatus,
    AgentPatchPermission, AgentPermissions, AgentProposedAction, AgentWritePermission,
};
use similar::TextDiff;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use tempfile::NamedTempFile;

/// The single approval route used by structured tools that publish file
/// changes. Path scope and revision checks remain the responsibility of the
/// concrete writer; this route only answers who may authorize the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWriteApprovalRoute {
    Denied,
    RequireExplicitApproval,
    AutoApprove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWriteAuthorizationSource {
    Automatic,
    ExplicitUser,
}

/// Resolves the common write/approval dimensions without granting any broader
/// filesystem scope. Full Access reaches `AutoApprove` because the trusted
/// frontend maps that mode to write=all + patch=auto_approve; custom modes can
/// reach the same route without gaining write=all.
pub fn file_write_approval_route(permissions: AgentPermissions) -> FileWriteApprovalRoute {
    if permissions.write == AgentWritePermission::Denied {
        FileWriteApprovalRoute::Denied
    } else if permissions.patch == AgentPatchPermission::AutoApprove {
        FileWriteApprovalRoute::AutoApprove
    } else {
        FileWriteApprovalRoute::RequireExplicitApproval
    }
}

/// Revalidates the authorization source at the host execution boundary. This
/// prevents an internal caller from labelling a manually-routed write as an
/// automatically approved action and bypassing the user's chosen mode.
pub fn file_write_authorized(
    permissions: AgentPermissions,
    source: FileWriteAuthorizationSource,
) -> bool {
    match (file_write_approval_route(permissions), source) {
        (FileWriteApprovalRoute::AutoApprove, FileWriteAuthorizationSource::Automatic)
        | (
            FileWriteApprovalRoute::AutoApprove | FileWriteApprovalRoute::RequireExplicitApproval,
            FileWriteAuthorizationSource::ExplicitUser,
        ) => true,
        (FileWriteApprovalRoute::Denied, _)
        | (
            FileWriteApprovalRoute::RequireExplicitApproval,
            FileWriteAuthorizationSource::Automatic,
        ) => false,
    }
}

/// Structured host actions that publish files all share the policy above.
/// Command and Skill-script actions stay in their separate process policy,
/// because they can have broader side effects than a validated file writer.
pub fn proposed_action_uses_file_write_policy(action: &AgentProposedAction) -> bool {
    file_write_action_approval_status(action).is_some()
}

/// Returns the approval status carried by every structured file-write action.
/// This exhaustive classifier is shared by runtime and host checks so adding a
/// new action variant cannot silently update one policy boundary but not the
/// other.
pub fn file_write_action_approval_status(
    action: &AgentProposedAction,
) -> Option<AgentApprovalStatus> {
    match action {
        AgentProposedAction::Diff { diff } => Some(diff.approval_status),
        AgentProposedAction::FileWrite { file_write } => Some(file_write.approval_status),
        AgentProposedAction::SkillMaterialization { materialization } => {
            Some(materialization.approval_status)
        }
        AgentProposedAction::OfficeOperation { office_operation } => {
            Some(office_operation.approval_status)
        }
        AgentProposedAction::Command { .. }
        | AgentProposedAction::ToolCall { .. }
        | AgentProposedAction::McpToolCall { .. }
        | AgentProposedAction::SkillScript { .. }
        | AgentProposedAction::SkillInstallation { .. } => None,
    }
}

pub fn apply_file_write(
    workspace_root: Option<&Path>,
    proposal: &AgentFileWriteProposal,
    draft: &AgentFileDraftRecord,
    permissions: AgentPermissions,
) -> Result<AgentFileWriteResult, String> {
    require_write_permission(permissions)?;
    validate_proposal(proposal, draft)?;
    validate_text_patch_path(&proposal.file_path).map_err(|error| error.to_string())?;
    let target = resolve_target(workspace_root, &proposal.file_path, permissions)?;
    reject_symlink_target(&target)?;
    validate_base_state(&target, draft.base_revision.as_deref())?;

    let existing_permissions = fs::metadata(&target)
        .ok()
        .map(|metadata| metadata.permissions());
    let parent = target
        .parent()
        .ok_or_else(|| "文件写入目标缺少父目录。".to_string())?;
    let mut temp = NamedTempFile::new_in(parent)
        .map_err(|error| format!("创建文件写入临时文件失败：{error}"))?;
    temp.write_all(draft.content.as_bytes())
        .map_err(|error| format!("写入文件草稿失败：{error}"))?;
    temp.flush()
        .map_err(|error| format!("刷新文件草稿失败：{error}"))?;
    temp.as_file()
        .sync_all()
        .map_err(|error| format!("同步文件草稿失败：{error}"))?;
    if let Some(file_permissions) = existing_permissions {
        temp.as_file()
            .set_permissions(file_permissions)
            .map_err(|error| format!("保留目标文件权限失败：{error}"))?;
    }

    if draft.base_revision.is_none() {
        temp.persist_noclobber(&target)
            .map_err(|error| format!("目标文件已出现或无法创建：{}", error.error))?;
    } else {
        temp.persist(&target)
            .map_err(|error| format!("原子替换目标文件失败：{}", error.error))?;
    }

    let revision = content_revision(draft.content.as_bytes());
    Ok(AgentFileWriteResult {
        status: AgentFileWriteResultStatus::Applied,
        draft_id: draft.id.clone(),
        mode: proposal.mode,
        file_path: proposal.file_path.clone(),
        additions: proposal.additions,
        deletions: proposal.deletions,
        line_count: proposal.line_count,
        byte_count: proposal.byte_count,
        revision: Some(revision),
        error: None,
        message: Some("文件草稿已原子写入。".to_string()),
    })
}

pub fn file_write_diff(draft: &AgentFileDraftRecord) -> String {
    TextDiff::from_lines(&draft.base_content, &draft.content)
        .unified_diff()
        .header(
            &format!("a/{}", draft.file_path),
            &format!("b/{}", draft.file_path),
        )
        .to_string()
}

pub fn file_draft_snapshot(draft: &AgentFileDraftRecord) -> Result<AgentFileDraftSnapshot, String> {
    let mode = serde_json::from_value(serde_json::Value::String(draft.mode.clone()))
        .map_err(|error| format!("文件草稿 mode 无效：{error}"))?;
    let status = serde_json::from_value(serde_json::Value::String(draft.status.clone()))
        .map_err(|error| format!("文件草稿 status 无效：{error}"))?;
    Ok(AgentFileDraftSnapshot {
        draft_id: draft.id.clone(),
        conversation_id: draft.conversation_id.clone(),
        project_id: draft.project_id.clone(),
        file_path: draft.file_path.clone(),
        mode,
        status,
        base_revision: draft.base_revision.clone(),
        additions: draft.additions,
        deletions: draft.deletions,
        line_count: draft.line_count,
        byte_count: draft.byte_count,
        chunk_count: draft.chunk_count,
        next_chunk_index: draft.next_chunk_index,
        stats_final: draft.stats_final,
        summary: draft.summary.clone(),
        created_at: draft.created_at,
        updated_at: draft.updated_at,
    })
}

pub fn failed_file_write_result(
    proposal: &AgentFileWriteProposal,
    status: AgentFileWriteResultStatus,
    error: impl Into<String>,
) -> AgentFileWriteResult {
    let error = error.into();
    AgentFileWriteResult {
        status,
        draft_id: proposal.draft_id.clone(),
        mode: proposal.mode,
        file_path: proposal.file_path.clone(),
        additions: proposal.additions,
        deletions: proposal.deletions,
        line_count: proposal.line_count,
        byte_count: proposal.byte_count,
        revision: None,
        error: Some(error.clone()),
        message: Some(error),
    }
}

fn validate_proposal(
    proposal: &AgentFileWriteProposal,
    draft: &AgentFileDraftRecord,
) -> Result<(), String> {
    if proposal.draft_id != draft.id || proposal.file_path != draft.file_path {
        return Err("文件写入提案与持久化草稿不匹配。".to_string());
    }
    if proposal.id != draft.final_action_id.as_deref().unwrap_or_default() {
        return Err("文件写入提案的 action id 与草稿不匹配。".to_string());
    }
    if file_write_mode_label(proposal.mode) != draft.mode {
        return Err("文件写入提案的 mode 与草稿不匹配。".to_string());
    }
    if proposal.base_revision != draft.base_revision {
        return Err("文件写入提案的基础 revision 与草稿不匹配。".to_string());
    }
    if proposal.additions != draft.additions
        || proposal.deletions != draft.deletions
        || proposal.line_count != draft.line_count
        || proposal.byte_count != draft.byte_count
    {
        return Err("文件写入提案的统计信息与草稿不匹配。".to_string());
    }
    if !matches!(draft.status.as_str(), "waiting_approval" | "applying") {
        return Err(format!(
            "文件草稿当前状态为 {}，不能应用此提案。",
            draft.status
        ));
    }
    if draft.content.len() > 4 * 1024 * 1024 {
        return Err("文件草稿超过 4 MiB 限制。".to_string());
    }
    if draft.content.contains('\0') {
        return Err("文件草稿不能包含空字符。".to_string());
    }
    Ok(())
}

fn file_write_mode_label(mode: AgentFileWriteMode) -> &'static str {
    match mode {
        AgentFileWriteMode::Create => "create",
        AgentFileWriteMode::Rewrite => "rewrite",
        AgentFileWriteMode::Modify => "modify",
        AgentFileWriteMode::Append => "append",
        AgentFileWriteMode::Upsert => "upsert",
    }
}

fn validate_base_state(target: &Path, expected_revision: Option<&str>) -> Result<(), String> {
    match expected_revision {
        Some(expected) => {
            let current =
                fs::read(target).map_err(|error| format!("读取文件写入目标失败：{error}"))?;
            if content_revision(&current) != expected {
                return Err("目标文件在草稿创建后发生变化。".to_string());
            }
        }
        None if target.exists() => {
            return Err("目标文件在草稿创建后已出现。".to_string());
        }
        None => {}
    }
    Ok(())
}

fn require_write_permission(permissions: AgentPermissions) -> Result<(), String> {
    if permissions.write == AgentWritePermission::Denied {
        Err("当前写入权限为 denied，不能应用文件草稿。".to_string())
    } else {
        Ok(())
    }
}

fn resolve_target(
    workspace_root: Option<&Path>,
    file_path: &str,
    permissions: AgentPermissions,
) -> Result<PathBuf, String> {
    let root = workspace_root.map(canonical_workspace_root).transpose()?;
    let input = if let Some(expanded) = expand_system_path(file_path)? {
        expanded
    } else {
        PathBuf::from(file_path)
    };
    let target = if input.is_absolute() {
        if let Some(root) = root.as_deref() {
            if input.starts_with(root) || permissions.write == AgentWritePermission::All {
                normalize_absolute(&input)?
            } else {
                return Err("文件写入目标必须位于 workspace 内。".to_string());
            }
        } else if permissions.write == AgentWritePermission::All {
            normalize_absolute(&input)?
        } else {
            return Err("写入绝对路径需要 write=all 权限。".to_string());
        }
    } else {
        let root = root.ok_or_else(|| "相对写入路径需要 workspace。".to_string())?;
        root.join(clean_relative(&input)?)
    };

    let parent = target
        .parent()
        .ok_or_else(|| "文件写入目标缺少父目录。".to_string())?
        .canonicalize()
        .map_err(|error| format!("文件写入父目录不可访问：{error}"))?;
    if let Some(root) = workspace_root.map(canonical_workspace_root).transpose()? {
        if permissions.write != AgentWritePermission::All && !parent.starts_with(&root) {
            return Err("文件写入目标必须位于 workspace 内。".to_string());
        }
    }
    Ok(parent.join(
        target
            .file_name()
            .ok_or_else(|| "文件写入目标缺少文件名。".to_string())?,
    ))
}

fn canonical_workspace_root(path: &Path) -> Result<PathBuf, String> {
    let root = path
        .canonicalize()
        .map_err(|error| format!("workspace 路径不可访问：{error}"))?;
    if !root.is_dir() {
        return Err("workspace 路径不是目录。".to_string());
    }
    Ok(root)
}

fn normalize_absolute(path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("文件写入路径不能包含 ..。".to_string()),
        }
    }
    Ok(normalized)
}

fn clean_relative(path: &Path) -> Result<PathBuf, String> {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("文件写入相对路径不能越过 workspace。".to_string())
            }
        }
    }
    if cleaned.as_os_str().is_empty() {
        return Err("文件写入路径不能为空。".to_string());
    }
    Ok(cleaned)
}

fn reject_symlink_target(target: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(target) {
        if metadata.file_type().is_symlink() {
            return Err("文件写入不允许符号链接目标。".to_string());
        }
        if !metadata.is_file() {
            return Err("文件写入目标不是普通文件。".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentApprovalStatus, AgentCommandPermission, AgentPatchPermission, AgentReadPermission,
    };
    use tempfile::tempdir;

    fn permissions() -> AgentPermissions {
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: Default::default(),
            patch: AgentPatchPermission::RequireApproval,
        }
    }

    fn draft(file_path: &str, base: &str, content: &str) -> AgentFileDraftRecord {
        AgentFileDraftRecord {
            id: "draft-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            project_id: Some("project-1".to_string()),
            run_id: "run-1".to_string(),
            file_path: file_path.to_string(),
            mode: if base.is_empty() { "create" } else { "rewrite" }.to_string(),
            status: "waiting_approval".to_string(),
            base_revision: (!base.is_empty()).then(|| content_revision(base.as_bytes())),
            base_content: base.to_string(),
            content: content.to_string(),
            additions: 1,
            deletions: u64::from(!base.is_empty()),
            line_count: 1,
            byte_count: content.len() as u64,
            chunk_count: 1,
            next_chunk_index: 1,
            stats_final: true,
            summary: Some("test write".to_string()),
            final_action_id: Some("action-1".to_string()),
            created_at: 1,
            updated_at: 1,
            expires_at: i64::MAX,
        }
    }

    fn proposal(draft: &AgentFileDraftRecord) -> AgentFileWriteProposal {
        AgentFileWriteProposal {
            id: "action-1".to_string(),
            draft_id: draft.id.clone(),
            mode: if draft.base_revision.is_some() {
                AgentFileWriteMode::Rewrite
            } else {
                AgentFileWriteMode::Create
            },
            file_path: draft.file_path.clone(),
            base_revision: draft.base_revision.clone(),
            summary: draft.summary.clone(),
            additions: draft.additions,
            deletions: draft.deletions,
            line_count: draft.line_count,
            byte_count: draft.byte_count,
            approval_status: AgentApprovalStatus::Approved,
        }
    }

    #[test]
    fn shared_file_write_policy_routes_manual_custom_auto_and_full_access() {
        let manual = permissions();
        assert_eq!(
            file_write_approval_route(manual),
            FileWriteApprovalRoute::RequireExplicitApproval
        );
        assert!(!file_write_authorized(
            manual,
            FileWriteAuthorizationSource::Automatic
        ));
        assert!(file_write_authorized(
            manual,
            FileWriteAuthorizationSource::ExplicitUser
        ));

        let custom_auto = AgentPermissions {
            patch: AgentPatchPermission::AutoApprove,
            ..manual
        };
        assert_eq!(
            file_write_approval_route(custom_auto),
            FileWriteApprovalRoute::AutoApprove
        );
        assert!(file_write_authorized(
            custom_auto,
            FileWriteAuthorizationSource::Automatic
        ));

        let full_access = AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: crate::AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
        };
        assert_eq!(
            file_write_approval_route(full_access),
            FileWriteApprovalRoute::AutoApprove
        );
        assert!(file_write_authorized(
            full_access,
            FileWriteAuthorizationSource::Automatic
        ));

        let denied = AgentPermissions {
            write: AgentWritePermission::Denied,
            patch: AgentPatchPermission::AutoApprove,
            ..manual
        };
        assert_eq!(
            file_write_approval_route(denied),
            FileWriteApprovalRoute::Denied
        );
        assert!(!file_write_authorized(
            denied,
            FileWriteAuthorizationSource::Automatic
        ));
        assert!(!file_write_authorized(
            denied,
            FileWriteAuthorizationSource::ExplicitUser
        ));
    }

    #[test]
    fn structured_file_write_actions_share_one_host_policy_domain() {
        let draft = draft("report.txt", "", "hello");
        let file_write = AgentProposedAction::FileWrite {
            file_write: proposal(&draft),
        };
        let diff = AgentProposedAction::Diff {
            diff: crate::AgentDiffProposal {
                id: "diff-1".to_string(),
                operation: crate::AgentPatchOperation::Create,
                file_path: "report.md".to_string(),
                patch: "".to_string(),
                base_revision: None,
                summary: None,
                approval_status: AgentApprovalStatus::Required,
            },
        };
        let materialization = AgentProposedAction::SkillMaterialization {
            materialization: crate::AgentSkillMaterializationRequest {
                id: "materialize-1".to_string(),
                source_uri: "skill://test/revision/asset.txt".to_string(),
                source_prefix: None,
                destination: "asset.txt".to_string(),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            },
        };
        let office = AgentProposedAction::OfficeOperation {
            office_operation: Box::new(crate::AgentOfficeOperationRequest {
                schema_version: crate::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
                id: "office-1".to_string(),
                semantic_args: serde_json::json!({
                    "operation": "create",
                    "filePath": "budget.xlsx",
                    "reason": "create the reviewed workbook"
                }),
                prepared: crate::office::OfficePreparedExecution {
                    schema_version: crate::office::OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
                    provider_id: "test".to_string(),
                    engine_revision: "engine".to_string(),
                    workspace_revision: Some("workspace".to_string()),
                    access: crate::office::OfficeOperationAccess::FileWrite,
                    request: crate::office::OfficeExecutionRequest {
                        document_kind: crate::office::OfficeDocumentKind::Spreadsheet,
                        operation: crate::office::OfficeOperation::Create,
                        document_path: Some("budget.xlsx".to_string()),
                        parameters: crate::office::OfficeOperationParameters::Create {
                            locale: None,
                            minimal: false,
                            overwrite: false,
                        },
                        output_path: None,
                        destination_path: None,
                        inputs: Vec::new(),
                        timeout_ms: None,
                    },
                    argv: vec!["create".to_string(), "budget.xlsx".to_string()],
                    resolved_render_plan: None,
                    paths: Vec::new(),
                    input_bindings: Vec::new(),
                },
                approval_status: AgentApprovalStatus::Approved,
                reason: "create the reviewed workbook".to_string(),
            }),
        };

        for action in [&file_write, &diff, &materialization, &office] {
            assert!(proposed_action_uses_file_write_policy(action));
            assert!(file_write_action_approval_status(action).is_some());
        }

        assert!(!proposed_action_uses_file_write_policy(
            &AgentProposedAction::ToolCall {
                call: crate::AgentToolCall {
                    id: "read-1".to_string(),
                    tool: "read_file".to_string(),
                    args: serde_json::json!({ "path": "report.txt" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    reason: None,
                },
            }
        ));
        assert!(!proposed_action_uses_file_write_policy(
            &AgentProposedAction::Command {
                command: crate::AgentCommandRequest {
                    id: "command-1".to_string(),
                    command: "pwd".to_string(),
                    cwd: None,
                    timeout_ms: None,
                    approval_status: AgentApprovalStatus::Required,
                    risk_level: None,
                    reason: None,
                    observe: None,
                    inputs: Vec::new(),
                    runtime_binding: None,
                    managed_office_script: None,
                },
            }
        ));
    }

    #[test]
    fn atomically_creates_file_from_draft() {
        let root = tempdir().unwrap();
        let draft = draft("report.md", "", "# Report\n");
        let result =
            apply_file_write(Some(root.path()), &proposal(&draft), &draft, permissions()).unwrap();

        assert_eq!(result.status, AgentFileWriteResultStatus::Applied);
        assert_eq!(
            fs::read_to_string(root.path().join("report.md")).unwrap(),
            "# Report\n"
        );
    }

    #[test]
    fn rejects_rewrite_when_base_revision_changed() {
        let root = tempdir().unwrap();
        let target = root.path().join("report.md");
        fs::write(&target, "old\n").unwrap();
        let draft = draft("report.md", "old\n", "new\n");
        fs::write(&target, "changed externally\n").unwrap();

        let error = apply_file_write(Some(root.path()), &proposal(&draft), &draft, permissions())
            .unwrap_err();

        assert!(error.contains("发生变化"));
        assert_eq!(fs::read_to_string(target).unwrap(), "changed externally\n");
    }

    #[test]
    fn rejects_proposal_that_does_not_match_persisted_draft() {
        let root = tempdir().unwrap();
        let draft = draft("report.md", "", "# Report\n");
        let mut proposal = proposal(&draft);
        proposal.mode = AgentFileWriteMode::Rewrite;

        let error =
            apply_file_write(Some(root.path()), &proposal, &draft, permissions()).unwrap_err();

        assert!(error.contains("mode"));
        assert!(!root.path().join("report.md").exists());
    }
}
