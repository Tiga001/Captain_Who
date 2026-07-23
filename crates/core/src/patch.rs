use crate::{
    capture_agent_turn_file_content, content_revision, AgentPatchOperation, AgentPermissions,
    AgentTurnFileChange, AgentWritePermission,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_PATCH_BYTES: usize = 512 * 1024;
const UNSUPPORTED_PATCH_EXTENSIONS: &[&str] = &["pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchApplyResult {
    pub file_paths: Vec<String>,
    #[serde(skip)]
    pub file_change: AgentTurnFileChange,
}

pub fn apply_unified_diff_in_workspace(
    workspace_root: Option<&Path>,
    operation: AgentPatchOperation,
    expected_file_path: &str,
    patch: &str,
    expected_base_revision: Option<&str>,
    permissions: AgentPermissions,
) -> Result<PatchApplyResult, String> {
    if patch.len() > MAX_PATCH_BYTES {
        return Err(format!(
            "patch 过大：{} bytes，超过 {} bytes 限制。",
            patch.len(),
            MAX_PATCH_BYTES
        ));
    }
    if patch.contains('\0') {
        return Err("patch 不能包含空字符。".to_string());
    }

    require_write_permission(permissions)?;
    let target = resolve_patch_target(workspace_root, expected_file_path, permissions)?;
    reject_unsupported_extension(&target.display_path)?;
    reject_hidden_system_path(&target.display_path)?;
    validate_patch_operation(&target, operation, patch)?;
    validate_base_revision(&target, operation, expected_base_revision)?;
    let paths = validate_patch_paths(&target, patch)?;
    let apply_patch = rewrite_patch_for_apply_root(patch, &target);
    let before = capture_agent_turn_file_content(&target.absolute_path);

    run_git_apply(&target.apply_root, &apply_patch, true)?;
    run_git_apply(&target.apply_root, &apply_patch, false)?;
    let after = capture_agent_turn_file_content(&target.absolute_path);

    Ok(PatchApplyResult {
        file_paths: paths.into_iter().collect(),
        file_change: AgentTurnFileChange {
            path: target.display_path,
            before,
            after,
        },
    })
}

fn require_write_permission(permissions: AgentPermissions) -> Result<(), String> {
    if permissions.write == AgentWritePermission::Denied {
        return Err("当前写入权限为 denied，不能应用文件修改。".to_string());
    }
    Ok(())
}

fn canonical_workspace_root(workspace_root: &Path) -> Result<PathBuf, String> {
    let root = workspace_root
        .canonicalize()
        .map_err(|error| format!("workspace 路径不可访问：{error}"))?;
    if !root.is_dir() {
        return Err("workspace 路径不是目录。".to_string());
    }
    Ok(root)
}

struct ResolvedPatchTarget {
    absolute_path: PathBuf,
    apply_path: String,
    apply_root: PathBuf,
    display_path: String,
}

fn resolve_patch_target(
    workspace_root: Option<&Path>,
    path: &str,
    permissions: AgentPermissions,
) -> Result<ResolvedPatchTarget, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("patch 目标路径不能为空。".to_string());
    }

    let root = workspace_root.map(canonical_workspace_root).transpose()?;
    if let Some(absolute) = normalize_absolute_patch_target(path)? {
        if let Some(root) = root.as_deref() {
            if let Ok(relative_path) = absolute.strip_prefix(root) {
                let display_path = relative_display(relative_path);
                if display_path.is_empty() {
                    return Err("patch 目标路径不能为空。".to_string());
                }
                return Ok(ResolvedPatchTarget {
                    absolute_path: root.join(relative_path),
                    apply_path: display_path.clone(),
                    apply_root: root.to_path_buf(),
                    display_path,
                });
            }
        }

        if permissions.write != AgentWritePermission::All {
            return Err(
                "patch 目标路径必须位于当前 workspace 内；写入 workspace 外需要 write=all 权限。"
                    .to_string(),
            );
        }
        let apply_root = nearest_existing_directory(&absolute)?;
        let apply_path = absolute
            .strip_prefix(&apply_root)
            .map(relative_display)
            .map_err(|_| "无法计算 patch 应用路径。".to_string())?;
        if apply_path.is_empty() {
            return Err("patch 目标路径不能为空。".to_string());
        }
        return Ok(ResolvedPatchTarget {
            absolute_path: absolute.clone(),
            apply_path,
            apply_root,
            display_path: absolute.to_string_lossy().to_string(),
        });
    }

    let root = root.ok_or_else(|| "当前没有 workspace，不能对相对路径应用 patch。".to_string())?;
    let relative_path = clean_relative_path(path)?;
    let display_path = relative_display(&relative_path);
    if display_path.is_empty() {
        return Err("patch 目标路径不能为空。".to_string());
    }

    Ok(ResolvedPatchTarget {
        absolute_path: root.join(&relative_path),
        apply_path: display_path.clone(),
        apply_root: root,
        display_path,
    })
}

fn normalize_absolute_patch_target(path: &str) -> Result<Option<PathBuf>, String> {
    if let Some(expanded) = crate::system_paths::expand_system_path(path)? {
        return Ok(Some(normalize_absolute_path(&expanded)?));
    }
    if Path::new(path).is_absolute() {
        return Ok(Some(normalize_absolute_path(Path::new(path))?));
    }
    Ok(None)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("路径不能包含 ..。".to_string()),
        }
    }
    if !normalized.is_absolute() {
        return Err("绝对路径解析失败。".to_string());
    }
    Ok(normalized)
}

fn nearest_existing_directory(path: &Path) -> Result<PathBuf, String> {
    let mut candidate = path
        .parent()
        .ok_or_else(|| "patch 目标路径缺少父目录。".to_string())?
        .to_path_buf();
    while !candidate.exists() {
        candidate = candidate
            .parent()
            .ok_or_else(|| "找不到可用的 patch 父目录。".to_string())?
            .to_path_buf();
    }
    let metadata = std::fs::symlink_metadata(&candidate)
        .map_err(|error| format!("读取 patch 父目录元数据失败：{error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("patch 父目录不能是符号链接。".to_string());
    }
    if !metadata.is_dir() {
        return Err("patch 父路径不是目录。".to_string());
    }
    Ok(candidate)
}

fn clean_relative_path(input_path: &str) -> Result<PathBuf, String> {
    let trimmed = input_path.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空。".to_string());
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err("路径必须是 workspace 相对路径。".to_string());
    }

    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("路径不能包含 .. 或系统根路径。".to_string());
            }
        }
    }
    if cleaned.as_os_str().is_empty() {
        return Err("路径不能为空。".to_string());
    }
    Ok(cleaned)
}

fn relative_display(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn validate_base_revision(
    target: &ResolvedPatchTarget,
    operation: AgentPatchOperation,
    expected: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if operation == AgentPatchOperation::Create {
        return Err("create 操作不能携带 baseRevision。".to_string());
    }
    let bytes = std::fs::read(&target.absolute_path)
        .map_err(|error| format!("校验 patch baseRevision 时读取文件失败：{error}"))?;
    let actual = content_revision(&bytes);
    if actual != expected {
        return Err(format!(
            "文件在审批前已发生变化：expected baseRevision `{expected}`，current `{actual}`。请重新读取并生成编辑。"
        ));
    }
    Ok(())
}

fn validate_patch_operation(
    target: &ResolvedPatchTarget,
    operation: AgentPatchOperation,
    patch: &str,
) -> Result<(), String> {
    let old_path = patch_header_path(patch, "--- ")?;
    let new_path = patch_header_path(patch, "+++ ")?;
    let old_is_null = old_path == "/dev/null";
    let new_is_null = new_path == "/dev/null";
    let target_exists = target.absolute_path.exists();

    match operation {
        AgentPatchOperation::Create if target_exists => {
            return Err("create 操作要求目标文件当前不存在。".to_string())
        }
        AgentPatchOperation::Create if !old_is_null || new_is_null => {
            return Err("create patch 必须使用 --- /dev/null 和目标文件 +++ 路径。".to_string())
        }
        AgentPatchOperation::Update if !target_exists => {
            return Err("update 操作要求目标文件当前存在。".to_string())
        }
        AgentPatchOperation::Update if old_is_null || new_is_null => {
            return Err("update patch 的 --- 和 +++ 都必须指向目标文件。".to_string())
        }
        AgentPatchOperation::Delete if !target_exists => {
            return Err("delete 操作要求目标文件当前存在。".to_string())
        }
        AgentPatchOperation::Delete if old_is_null || !new_is_null => {
            return Err("delete patch 必须使用目标文件 --- 路径和 +++ /dev/null。".to_string())
        }
        _ => {}
    }

    if target_exists {
        let metadata = std::fs::symlink_metadata(&target.absolute_path)
            .map_err(|error| format!("读取 patch 目标元数据失败：{error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("patch 目标不能是符号链接。".to_string());
        }
        if !metadata.is_file() {
            return Err("patch 目标必须是文件。".to_string());
        }
    }

    Ok(())
}

fn patch_header_path(patch: &str, prefix: &str) -> Result<String, String> {
    patch
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(|value| {
            value
                .split('\t')
                .next()
                .unwrap_or(value)
                .trim()
                .trim_matches('"')
                .to_string()
        })
        .ok_or_else(|| format!("patch 缺少 {prefix}header。"))
}

fn validate_patch_paths(
    target: &ResolvedPatchTarget,
    patch: &str,
) -> Result<BTreeSet<String>, String> {
    let paths = extract_patch_paths(patch)?;
    if paths.is_empty() {
        return Err("patch 没有声明目标文件路径。".to_string());
    }

    let mut normalized_paths = BTreeSet::new();
    for path in paths {
        let normalized = normalize_declared_patch_path(&target.apply_root, &path)?;
        reject_unsupported_extension(&normalized)?;
        reject_hidden_system_path(&normalized)?;
        if normalized != target.apply_path {
            return Err(format!(
                "patch 只能修改声明的文件：expected `{}`，found `{normalized}`。",
                target.display_path
            ));
        }
        validate_no_symlink_parent(&target.apply_root, &normalized)?;
        normalized_paths.insert(target.display_path.clone());
    }

    Ok(normalized_paths)
}

fn extract_patch_paths(patch: &str) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    let mut has_hunk = false;
    let mut has_file_mode_change = false;

    for line in patch.lines() {
        if line.starts_with("@@ ") {
            has_hunk = true;
            continue;
        }
        if line.starts_with("new file mode ") || line.starts_with("deleted file mode ") {
            has_file_mode_change = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let mut parts = rest.split_whitespace();
            let left = parts.next();
            let right = parts.next();
            for candidate in [left, right].into_iter().flatten() {
                add_patch_path(&mut paths, candidate)?;
            }
            continue;
        }
        if let Some(candidate) = line
            .strip_prefix("--- ")
            .or_else(|| line.strip_prefix("+++ "))
        {
            add_patch_path(&mut paths, candidate)?;
        }
    }

    if !has_hunk && !has_file_mode_change {
        return Err("patch 必须包含 unified diff hunk（@@）或文件模式变更。".to_string());
    }

    Ok(paths)
}

fn add_patch_path(paths: &mut BTreeSet<String>, candidate: &str) -> Result<(), String> {
    let candidate = candidate
        .split('\t')
        .next()
        .unwrap_or(candidate)
        .trim()
        .trim_matches('"');

    if candidate == "/dev/null" {
        return Ok(());
    }
    let candidate = candidate
        .strip_prefix("a/")
        .or_else(|| candidate.strip_prefix("b/"))
        .unwrap_or(candidate);
    if candidate.is_empty() {
        return Err("patch 路径不能为空。".to_string());
    }
    paths.insert(candidate.to_string());
    Ok(())
}

fn normalize_declared_patch_path(root: &Path, path: &str) -> Result<String, String> {
    let declared = Path::new(path);
    if declared.is_absolute() {
        let absolute = normalize_absolute_path(Path::new(path))?;
        return absolute
            .strip_prefix(root)
            .map(relative_display)
            .map_err(|_| "patch header 路径不在允许的应用目录内。".to_string());
    }
    clean_relative_path(path).map(|path| relative_display(&path))
}

fn rewrite_patch_for_apply_root(patch: &str, target: &ResolvedPatchTarget) -> String {
    let mut rewritten = String::with_capacity(patch.len());
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            rewritten.push_str(&format!(
                "diff --git a/{path} b/{path}\n",
                path = target.apply_path
            ));
            continue;
        }
        if let Some(value) = line.strip_prefix("--- ") {
            rewritten.push_str("--- ");
            rewritten.push_str(&rewrite_patch_header_value(value, "a", &target.apply_path));
            rewritten.push('\n');
            continue;
        }
        if let Some(value) = line.strip_prefix("+++ ") {
            rewritten.push_str("+++ ");
            rewritten.push_str(&rewrite_patch_header_value(value, "b", &target.apply_path));
            rewritten.push('\n');
            continue;
        }
        rewritten.push_str(line);
        rewritten.push('\n');
    }
    rewritten
}

fn rewrite_patch_header_value(value: &str, side_prefix: &str, apply_path: &str) -> String {
    let (path, suffix) = value
        .split_once('\t')
        .map(|(path, suffix)| (path.trim(), format!("\t{suffix}")))
        .unwrap_or_else(|| (value.trim(), String::new()));
    if path.trim_matches('"') == "/dev/null" {
        return value.to_string();
    }
    format!("{side_prefix}/{apply_path}{suffix}")
}

fn reject_unsupported_extension(path: &str) -> Result<(), String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if UNSUPPORTED_PATCH_EXTENSIONS
        .iter()
        .any(|unsupported| extension == *unsupported)
    {
        return Err(format!(
            "不支持直接应用 .{extension} patch。PDF/Office 文件需要专用编辑工具。"
        ));
    }
    Ok(())
}

fn reject_hidden_system_path(path: &str) -> Result<(), String> {
    for component in Path::new(path).components() {
        let Component::Normal(part) = component else {
            continue;
        };
        let part = part.to_string_lossy();
        if part == ".git" || part == ".hg" || part == ".svn" {
            return Err(format!("禁止修改版本控制系统目录：{part}。"));
        }
    }
    Ok(())
}

fn validate_no_symlink_parent(root: &Path, relative_path: &str) -> Result<(), String> {
    let relative = clean_relative_path(relative_path)?;
    let mut current = root.to_path_buf();
    let mut components = relative.components().peekable();

    while let Some(component) = components.next() {
        current.push(component.as_os_str());
        if !current.exists() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|error| format!("读取路径元数据失败：{error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "patch 目标路径不能包含符号链接：{}",
                relative_display(&path_relative_to(root, &current)?)
            ));
        }
        if components.peek().is_some() && !metadata.is_dir() {
            return Err(format!(
                "patch 目标父路径不是目录：{}",
                relative_display(&path_relative_to(root, &current)?)
            ));
        }
    }
    Ok(())
}

fn path_relative_to(root: &Path, path: &Path) -> Result<PathBuf, String> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| "路径不在 workspace 内。".to_string())
}

fn run_git_apply(root: &Path, patch: &str, check_only: bool) -> Result<(), String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .arg("apply")
        .arg("--whitespace=nowarn");
    if check_only {
        command.arg("--check");
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动 git apply 失败：{error}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "无法写入 git apply stdin。".to_string())?;
        stdin
            .write_all(patch.as_bytes())
            .map_err(|error| format!("写入 git apply patch 失败：{error}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("等待 git apply 完成失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "git apply{} 失败：{}",
            if check_only { " --check" } else { "" },
            if details.is_empty() {
                format!("exit status {}", output.status)
            } else {
                details
            }
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentCommandPermission, AgentPatchPermission, AgentReadPermission};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn applies_update_patch_in_workspace() {
        let workspace = TestWorkspace::new();
        let target = workspace.path.join("src/notes.txt");
        std::fs::write(&target, "old\n").unwrap();
        let revision = content_revision(b"old\n");
        let patch = "--- a/src/notes.txt\n+++ b/src/notes.txt\n@@ -1 +1 @@\n-old\n+new\n";

        let result = apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Update,
            "src/notes.txt",
            patch,
            Some(&revision),
            workspace_permissions(),
        )
        .unwrap();

        assert_eq!(result.file_paths, vec!["src/notes.txt"]);
        assert_eq!(
            result.file_change,
            AgentTurnFileChange {
                path: "src/notes.txt".to_string(),
                before: crate::AgentTurnFileContent::Text("old\n".to_string()),
                after: crate::AgentTurnFileContent::Text("new\n".to_string()),
            }
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), "new\n");
    }

    #[test]
    fn rejects_stale_revision_without_modifying_file() {
        let workspace = TestWorkspace::new();
        let target = workspace.path.join("src/notes.txt");
        std::fs::write(&target, "changed\n").unwrap();
        let stale_revision = content_revision(b"old\n");
        let patch = "--- a/src/notes.txt\n+++ b/src/notes.txt\n@@ -1 +1 @@\n-changed\n+new\n";

        let error = apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Update,
            "src/notes.txt",
            patch,
            Some(&stale_revision),
            workspace_permissions(),
        )
        .unwrap_err();

        assert!(error.contains("审批前已发生变化"));
        assert_eq!(std::fs::read_to_string(target).unwrap(), "changed\n");
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let workspace = TestWorkspace::new();
        let outside = std::env::temp_dir().join(format!(
            "mycopilot-outside-patch-{}",
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&outside, "old\n").unwrap();
        let patch = format!(
            "--- {path}\n+++ {path}\n@@ -1 +1 @@\n-old\n+new\n",
            path = outside.to_string_lossy()
        );

        let error = apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Update,
            &outside.to_string_lossy(),
            &patch,
            None,
            workspace_permissions(),
        )
        .unwrap_err();

        assert!(error.contains("workspace"));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "old\n");
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn rejects_patch_when_write_is_denied() {
        let workspace = TestWorkspace::new();
        let target = workspace.path.join("src/notes.txt");
        std::fs::write(&target, "old\n").unwrap();
        let patch = "--- a/src/notes.txt\n+++ b/src/notes.txt\n@@ -1 +1 @@\n-old\n+new\n";

        let error = apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Update,
            "src/notes.txt",
            patch,
            None,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::Denied,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
            },
        )
        .unwrap_err();

        assert!(error.contains("denied"));
        assert_eq!(std::fs::read_to_string(target).unwrap(), "old\n");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_patch_target() {
        let workspace = TestWorkspace::new();
        let real = workspace.path.join("src/real.txt");
        let link = workspace.path.join("src/link.txt");
        std::fs::write(&real, "old\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let patch = "--- a/src/link.txt\n+++ b/src/link.txt\n@@ -1 +1 @@\n-old\n+new\n";

        let error = apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Update,
            "src/link.txt",
            patch,
            None,
            workspace_permissions(),
        )
        .unwrap_err();

        assert!(error.contains("符号链接"));
        assert_eq!(std::fs::read_to_string(real).unwrap(), "old\n");
    }

    #[test]
    fn applies_absolute_path_outside_workspace_with_full_write() {
        let workspace = TestWorkspace::new();
        let outside = std::env::temp_dir().join(format!(
            "mycopilot-outside-patch-{}",
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&outside, "old\n").unwrap();
        let revision = content_revision(b"old\n");
        let patch = format!(
            "--- {path}\n+++ {path}\n@@ -1 +1 @@\n-old\n+new\n",
            path = outside.to_string_lossy()
        );

        let result = apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Update,
            &outside.to_string_lossy(),
            &patch,
            Some(&revision),
            full_permissions(),
        )
        .unwrap();

        assert_eq!(
            result.file_paths,
            vec![outside.to_string_lossy().to_string()]
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "new\n");
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn applies_create_and_delete_patch() {
        let workspace = TestWorkspace::new();
        let create = "--- /dev/null\n+++ b/src/new.txt\n@@ -0,0 +1 @@\n+created\n";
        apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Create,
            "src/new.txt",
            create,
            None,
            workspace_permissions(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("src/new.txt")).unwrap(),
            "created\n"
        );

        let revision = content_revision(b"created\n");
        let delete = "--- a/src/new.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-created\n";
        apply_unified_diff_in_workspace(
            Some(&workspace.path),
            AgentPatchOperation::Delete,
            "src/new.txt",
            delete,
            Some(&revision),
            workspace_permissions(),
        )
        .unwrap();
        assert!(!workspace.path.join("src/new.txt").exists());
    }

    fn workspace_permissions() -> AgentPermissions {
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: Default::default(),
            patch: AgentPatchPermission::RequireApproval,
        }
    }

    fn full_permissions() -> AgentPermissions {
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: Default::default(),
            patch: AgentPatchPermission::RequireApproval,
        }
    }

    struct TestWorkspace {
        path: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("mycopilot-patch-test-{unique}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("src")).unwrap();
            Self { path }
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
