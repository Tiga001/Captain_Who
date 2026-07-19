use crate::storage::models::AgentFileDraftRecord;
use crate::tools::apply_patch_paths::validate_text_patch_path;
use crate::{
    content_revision, expand_system_path, AgentFileDraftSnapshot, AgentFileWriteMode,
    AgentFileWriteProposal, AgentFileWriteResult, AgentFileWriteResultStatus, AgentPermissions,
    AgentWritePermission,
};
use similar::TextDiff;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use tempfile::NamedTempFile;

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
