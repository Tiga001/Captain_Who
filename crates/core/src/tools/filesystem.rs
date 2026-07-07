// Filesystem traversal, path display, truncation, and blocking runtime helpers.
use super::{DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT, MAX_WALK_ENTRIES};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{AgentError, AgentResult};
use std::fs;
use std::future::Future;
use std::path::{Component, Path, PathBuf};

const EXCLUDED_NAMES: &[&str] = &[
    ".git",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "__pycache__",
    ".next",
    "target",
];

pub(super) struct WalkEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size_bytes: u64,
}

pub(super) struct WalkResult {
    pub entries: Vec<WalkEntry>,
    pub truncated: bool,
}

pub(super) fn walk_workspace_with_cancellation(
    root: &Path,
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<WalkResult> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut truncated = false;

    while let Some(directory) = stack.pop() {
        cancellation_token.check()?;
        let read_dir = fs::read_dir(&directory)
            .map_err(|error| AgentError::new(format!("读取目录失败：{error}")))?;

        for item in read_dir {
            cancellation_token.check()?;
            if entries.len() >= MAX_WALK_ENTRIES {
                truncated = true;
                break;
            }

            let item = item.map_err(|error| AgentError::new(format!("读取目录项失败：{error}")))?;
            let path = item.path();
            let file_name = item.file_name().to_string_lossy().to_string();
            if should_exclude_name(&file_name) {
                continue;
            }

            let file_type = item
                .file_type()
                .map_err(|error| AgentError::new(format!("读取文件类型失败：{error}")))?;
            if file_type.is_symlink() {
                continue;
            }

            let metadata = item
                .metadata()
                .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;
            let is_dir = metadata.is_dir();
            entries.push(WalkEntry {
                path: path.clone(),
                is_dir,
                size_bytes: metadata.len(),
            });

            if is_dir {
                stack.push(path);
            }
        }

        if truncated {
            break;
        }
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(WalkResult { entries, truncated })
}

pub(super) fn clean_relative_path(input_path: &str) -> AgentResult<PathBuf> {
    let trimmed = input_path.trim();
    if trimmed.is_empty() {
        return Err(AgentError::new("路径不能为空。"));
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(AgentError::new("路径必须是 workspace 相对路径。"));
    }

    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AgentError::new("路径不能包含 .. 或系统根路径。"));
            }
        }
    }

    if cleaned.as_os_str().is_empty() {
        return Err(AgentError::new("路径不能为空。"));
    }

    Ok(cleaned)
}

pub(super) fn sanitize_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT)
}

pub(super) fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    let mut output = value.chars().take(max_chars).collect::<String>();
    let truncated = value.chars().count() > max_chars;
    if truncated {
        output.push_str("\n...[truncated]");
    }

    (output, truncated)
}

pub(super) fn block_on_tool_future<T>(
    future: impl Future<Output = AgentResult<T>>,
) -> AgentResult<T> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| AgentError::new(format!("创建工具异步运行时失败：{error}")))?;
    runtime.block_on(future)
}
fn should_exclude_name(name: &str) -> bool {
    EXCLUDED_NAMES.iter().any(|excluded| name == *excluded)
}
