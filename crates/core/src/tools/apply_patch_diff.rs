// Unified diff generation and validation helpers for apply_patch.
use crate::protocol::{AgentError, AgentPatchOperation, AgentResult};
use std::path::Path;

const MAX_PATCH_CHARS: usize = 240_000;
const DIFF_CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffLine {
    text: String,
    terminated: bool,
}

pub(super) fn build_unified_diff(
    file_path: &str,
    operation: AgentPatchOperation,
    old_content: &str,
    new_content: &str,
) -> AgentResult<String> {
    if old_content.is_empty() && new_content.is_empty() {
        let (old_header, new_header) = patch_headers(file_path, operation);
        let relative = if Path::new(file_path).is_absolute() {
            Path::new(file_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(file_path)
        } else {
            file_path
        };
        return match operation {
            AgentPatchOperation::Create => sanitize_patch(format!(
                "diff --git a/{relative} b/{relative}\nnew file mode 100644\n--- {old_header}\n+++ {new_header}\n"
            )),
            AgentPatchOperation::Delete => sanitize_patch(format!(
                "diff --git a/{relative} b/{relative}\ndeleted file mode 100644\n--- {old_header}\n+++ {new_header}\n"
            )),
            AgentPatchOperation::Update => Err(edit_error(
                "no_change",
                "编辑后的内容与当前文件完全相同。",
            )),
        };
    }
    let old_lines = split_diff_lines(old_content);
    let new_lines = split_diff_lines(new_content);
    let prefix = common_prefix_len(&old_lines, &new_lines);
    let suffix = common_suffix_len(&old_lines, &new_lines, prefix);

    let old_change_end = old_lines.len().saturating_sub(suffix);
    let new_change_end = new_lines.len().saturating_sub(suffix);
    let hunk_start = prefix.saturating_sub(DIFF_CONTEXT_LINES);
    let trailing_context = suffix.min(DIFF_CONTEXT_LINES);
    let old_hunk_end = old_change_end + trailing_context;
    let new_hunk_end = new_change_end + trailing_context;
    let old_count = old_hunk_end.saturating_sub(hunk_start);
    let new_count = new_hunk_end.saturating_sub(hunk_start);
    let old_start = if old_count == 0 { 0 } else { hunk_start + 1 };
    let new_start = if new_count == 0 { 0 } else { hunk_start + 1 };

    let (old_header, new_header) = patch_headers(file_path, operation);
    let mut patch = format!(
        "--- {old_header}\n+++ {new_header}\n@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
    );

    for line in &old_lines[hunk_start..prefix] {
        push_diff_line(&mut patch, ' ', line);
    }
    for line in &old_lines[prefix..old_change_end] {
        push_diff_line(&mut patch, '-', line);
    }
    for line in &new_lines[prefix..new_change_end] {
        push_diff_line(&mut patch, '+', line);
    }
    for line in &old_lines[old_change_end..old_hunk_end] {
        push_diff_line(&mut patch, ' ', line);
    }

    sanitize_patch(patch)
}

fn split_diff_lines(content: &str) -> Vec<DiffLine> {
    if content.is_empty() {
        return Vec::new();
    }
    content
        .split_inclusive('\n')
        .map(|line| {
            let terminated = line.ends_with('\n');
            let text = line.strip_suffix('\n').unwrap_or(line).to_string();
            DiffLine { text, terminated }
        })
        .collect()
}

fn common_prefix_len(old: &[DiffLine], new: &[DiffLine]) -> usize {
    old.iter()
        .zip(new.iter())
        .take_while(|(old, new)| old == new)
        .count()
}

fn common_suffix_len(old: &[DiffLine], new: &[DiffLine], prefix: usize) -> usize {
    let max_suffix = old.len().min(new.len()).saturating_sub(prefix);
    (0..max_suffix)
        .take_while(|offset| old[old.len() - 1 - offset] == new[new.len() - 1 - offset])
        .count()
}

fn patch_headers(file_path: &str, operation: AgentPatchOperation) -> (String, String) {
    let path = if Path::new(file_path).is_absolute() {
        file_path.to_string()
    } else {
        format!("a/{file_path}")
    };
    let new_path = if Path::new(file_path).is_absolute() {
        file_path.to_string()
    } else {
        format!("b/{file_path}")
    };
    match operation {
        AgentPatchOperation::Create => ("/dev/null".to_string(), new_path),
        AgentPatchOperation::Update => (path, new_path),
        AgentPatchOperation::Delete => (path, "/dev/null".to_string()),
    }
}

fn push_diff_line(patch: &mut String, prefix: char, line: &DiffLine) {
    patch.push(prefix);
    patch.push_str(&line.text);
    patch.push('\n');
    if !line.terminated {
        patch.push_str("\\ No newline at end of file\n");
    }
}

pub(super) fn sanitize_patch(patch: String) -> AgentResult<String> {
    if patch.trim().is_empty() {
        return Err(AgentError::new("apply_patch.patch 不能为空。"));
    }
    let mut patch = patch;
    if !patch.ends_with('\n') {
        patch.push('\n');
    }
    if patch.chars().count() > MAX_PATCH_CHARS {
        return Err(AgentError::new(format!(
            "apply_patch.patch 过长，最多允许 {MAX_PATCH_CHARS} 个字符。"
        )));
    }
    if patch.contains('\0') {
        return Err(AgentError::new("apply_patch.patch 不能包含空字符。"));
    }
    if !looks_like_unified_diff(&patch) {
        return Err(AgentError::new(
            "apply_patch.patch 必须是 unified diff，至少包含 ---、+++，以及 @@ hunk 或文件模式变更。",
        ));
    }

    Ok(patch)
}

fn looks_like_unified_diff(patch: &str) -> bool {
    let has_old = patch.lines().any(|line| line.starts_with("--- "));
    let has_new = patch.lines().any(|line| line.starts_with("+++ "));
    let has_change_body = patch.lines().any(|line| line.starts_with("@@ "))
        || patch.lines().any(|line| {
            line.starts_with("new file mode ") || line.starts_with("deleted file mode ")
        });

    has_old && has_new && has_change_body
}

pub(super) fn validate_patch_operation(
    patch: &str,
    file_path: &str,
    operation: AgentPatchOperation,
) -> AgentResult<()> {
    let old_path = patch
        .lines()
        .find_map(|line| line.strip_prefix("--- "))
        .map(normalize_header_path)
        .ok_or_else(|| AgentError::new("apply_patch.patch 缺少 --- header。"))?;
    let new_path = patch
        .lines()
        .find_map(|line| line.strip_prefix("+++ "))
        .map(normalize_header_path)
        .ok_or_else(|| AgentError::new("apply_patch.patch 缺少 +++ header。"))?;
    let old_is_null = old_path == "/dev/null";
    let new_is_null = new_path == "/dev/null";

    let valid = match operation {
        AgentPatchOperation::Create => {
            old_is_null && !new_is_null && patch_path_matches(&new_path, file_path)
        }
        AgentPatchOperation::Update => {
            !old_is_null
                && !new_is_null
                && patch_path_matches(&old_path, file_path)
                && patch_path_matches(&new_path, file_path)
        }
        AgentPatchOperation::Delete => {
            !old_is_null && new_is_null && patch_path_matches(&old_path, file_path)
        }
    };

    if !valid {
        return Err(AgentError::new(format!(
            "apply_patch.patch 与 operation={operation:?} 或 filePath 不匹配。create 必须从 /dev/null 创建；update 的新旧路径都必须是目标文件；delete 必须写入 /dev/null。"
        )));
    }

    Ok(())
}

fn normalize_header_path(value: &str) -> String {
    value
        .split('\t')
        .next()
        .unwrap_or(value)
        .trim()
        .trim_matches('"')
        .to_string()
}

fn patch_path_matches(candidate: &str, file_path: &str) -> bool {
    candidate == file_path
        || candidate
            .strip_prefix("a/")
            .or_else(|| candidate.strip_prefix("b/"))
            .is_some_and(|candidate| candidate == file_path)
}

fn edit_error(code: &str, message: impl AsRef<str>) -> AgentError {
    AgentError::new(format!(
        "apply_patch structured_edit_error code={code}: {}",
        message.as_ref()
    ))
}
