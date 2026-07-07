// Rust agent core.
mod apply_patch;
mod attachments;
mod git_diff;
mod read_file;
mod read_image;
mod read_pdf;
mod read_presentation;
mod read_spreadsheet;
mod read_word;
mod run_command;
mod search_code;
mod search_files;
mod web_favicon;
mod web_fetch;
mod web_search;
mod workspace_map;

use crate::cancellation::AgentCancellationToken;
use crate::protocol::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentError, AgentPermissions,
    AgentProposedAction, AgentReadPermission, AgentResult, AgentRunContext, AgentSearchConfig,
    AgentSearchMode, AgentToolCall, AgentToolDefinition, AgentToolResult,
};
use crate::system_paths::expand_system_path;
use apply_patch::ApplyPatchTool;
use attachments::{AttachmentsListProjectTool, AttachmentsListTool};
use git_diff::GitDiffTool;
use read_file::ReadFileTool;
use read_image::ReadImageTool;
use read_pdf::ReadPdfTool;
use read_presentation::ReadPresentationTool;
use read_spreadsheet::ReadSpreadsheetTool;
use read_word::ReadWordTool;
use run_command::RunCommandTool;
use search_code::SearchCodeTool;
use search_files::SearchFilesTool;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::future::Future;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use web_fetch::WebFetchTool;
use web_search::WebSearchTool;
use workspace_map::WorkspaceMapTool;

pub(super) const MAX_READ_FILE_BYTES: u64 = 512 * 1024;
pub(super) const MAX_SEARCH_FILE_BYTES: u64 = 512 * 1024;
pub(super) const DEFAULT_READ_MAX_LINES: usize = 400;
pub(super) const MAX_READ_LINES: usize = 2_000;
pub(super) const DEFAULT_SEARCH_LIMIT: usize = 40;
pub(super) const MAX_SEARCH_LIMIT: usize = 200;
pub(super) const MAX_WALK_ENTRIES: usize = 20_000;
pub(super) const MAX_GIT_DIFF_BYTES: usize = 200 * 1024;
pub(super) const MAX_DOCUMENT_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub(super) const DEFAULT_DOCUMENT_MAX_CHARS: usize = 40_000;
pub(super) const MAX_DOCUMENT_TEXT_CHARS: usize = 120_000;

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

pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn read_only_defaults_with_search(search_config: Option<&AgentSearchConfig>) -> Self {
        let mut registry = Self {
            tools: BTreeMap::new(),
        };
        registry.register(AttachmentsListTool);
        registry.register(AttachmentsListProjectTool);
        registry.register(ReadFileTool);
        registry.register(ReadImageTool);
        registry.register(ReadPdfTool);
        registry.register(ReadWordTool);
        registry.register(ReadPresentationTool);
        registry.register(ReadSpreadsheetTool);
        registry.register(WorkspaceMapTool);
        registry.register(SearchFilesTool);
        registry.register(SearchCodeTool);
        if let Some(api_key) = tavily_api_key(search_config) {
            registry.register(WebSearchTool::new(api_key.clone()));
            registry.register(WebFetchTool::new(api_key));
        }
        registry.register(GitDiffTool);
        registry.register(ApplyPatchTool);
        registry.register(RunCommandTool);
        registry
    }

    pub fn definitions(&self) -> Vec<AgentToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub fn definition_for(&self, tool_name: &str) -> Option<AgentToolDefinition> {
        self.tools.get(tool_name).map(|tool| tool.definition())
    }

    pub fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let Some(tool) = self.tools.get(&call.tool) else {
            return Err(AgentError::new(format!("未知工具：{}", call.tool)));
        };

        tool.proposed_action(context, call)
    }

    pub fn execute(&self, context: &ToolExecutionContext, call: &AgentToolCall) -> AgentToolResult {
        if let Err(error) = context.check_cancelled() {
            return AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(error.to_string()),
            };
        }

        let Some(tool) = self.tools.get(&call.tool) else {
            return AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(format!("未知工具：{}", call.tool)),
            };
        };

        match tool.execute(context, call.args.clone()) {
            Ok(result) => match context.check_cancelled() {
                Ok(()) => AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: true,
                    result: Some(result),
                    error: None,
                },
                Err(error) => AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: false,
                    result: None,
                    error: Some(error.to_string()),
                },
            },
            Err(error) => AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn register<T: AgentTool + 'static>(&mut self, tool: T) {
        self.tools
            .insert(tool.definition().name, Box::new(tool) as Box<dyn AgentTool>);
    }
}

fn tavily_api_key(search_config: Option<&AgentSearchConfig>) -> Option<String> {
    let search_config = search_config?;
    if search_config.mode == AgentSearchMode::Disabled {
        return None;
    }

    let api_key = search_config
        .tavily_api_key
        .as_deref()
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())?;

    Some(api_key.to_string())
}

#[derive(Clone)]
pub struct ToolExecutionContext {
    workspace_root: Option<PathBuf>,
    attachment_library: Option<AgentAttachmentLibraryContext>,
    cancellation_token: AgentCancellationToken,
    permissions: AgentPermissions,
}

impl ToolExecutionContext {
    pub fn from_run_context(context: Option<&AgentRunContext>) -> Self {
        let workspace_root = context
            .and_then(|context| context.workspace.as_ref())
            .and_then(|workspace| workspace.root_path.as_ref())
            .map(PathBuf::from);
        let attachment_library = context.and_then(|context| context.attachment_library.clone());
        let permissions = context
            .map(|context| context.permissions)
            .unwrap_or_default();

        Self {
            workspace_root,
            attachment_library,
            cancellation_token: AgentCancellationToken::new(),
            permissions,
        }
    }

    pub fn with_cancellation(mut self, cancellation_token: AgentCancellationToken) -> Self {
        self.cancellation_token = cancellation_token;
        self
    }

    pub(super) fn cancellation_token(&self) -> AgentCancellationToken {
        self.cancellation_token.clone()
    }

    pub(super) fn check_cancelled(&self) -> AgentResult<()> {
        self.cancellation_token.check()
    }

    pub(super) fn workspace_root(&self) -> AgentResult<PathBuf> {
        self.workspace_root_optional()?.ok_or_else(|| {
            AgentError::new("当前没有 workspace；请为该工具提供绝对路径或系统路径别名。")
        })
    }

    pub(super) fn workspace_root_optional(&self) -> AgentResult<Option<PathBuf>> {
        self.check_cancelled()?;
        let Some(root) = &self.workspace_root else {
            return Ok(None);
        };

        let root = root
            .canonicalize()
            .map_err(|error| AgentError::new(format!("workspace 路径不可访问：{error}")))?;
        if !root.is_dir() {
            return Err(AgentError::new("workspace 路径不是目录。"));
        }

        Ok(Some(root))
    }

    pub(super) fn permissions(&self) -> AgentPermissions {
        self.permissions
    }

    pub(super) fn resolve_existing_path(&self, input_path: &str) -> AgentResult<PathBuf> {
        self.check_cancelled()?;
        if is_attachment_path(input_path) {
            return self.resolve_attachment_path(input_path);
        }

        let input_path = input_path.trim();
        if let Some(candidate) = expand_system_path(input_path).map_err(AgentError::new)? {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new(
                    "读取系统路径别名需要将读取范围设为“所有位置”。",
                ));
            }
            return candidate
                .canonicalize()
                .map_err(|error| AgentError::new(format!("路径不可访问：{error}")));
        }
        let candidate = Path::new(input_path);
        if candidate.is_absolute() {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new("当前读取权限仅允许访问 workspace 内路径。"));
            }
            return candidate
                .canonicalize()
                .map_err(|error| AgentError::new(format!("路径不可访问：{error}")));
        }

        let root = self.workspace_root()?;
        let relative = clean_relative_path(input_path)?;
        let resolved = root.join(relative);
        let canonical = resolved
            .canonicalize()
            .map_err(|error| AgentError::new(format!("路径不可访问：{error}")))?;

        if !canonical.starts_with(&root) {
            return Err(AgentError::new("路径必须位于已选择的 workspace 内。"));
        }

        Ok(canonical)
    }

    pub(super) fn display_path(&self, input_path: &str, file_path: &Path) -> AgentResult<String> {
        self.check_cancelled()?;
        if is_attachment_path(input_path) {
            return self
                .attachment_reference_for_path(input_path)
                .map(|reference| reference.read_path.clone());
        }

        if let Some(alias_path) = expand_system_path(input_path).map_err(AgentError::new)? {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new(
                    "读取系统路径别名需要将读取范围设为“所有位置”。",
                ));
            }
            let alias_path = alias_path
                .canonicalize()
                .map_err(|error| AgentError::new(format!("路径不可访问：{error}")))?;
            if file_path == alias_path {
                return Ok(input_path.trim_end_matches('/').to_string());
            }
            if let Ok(relative) = file_path.strip_prefix(&alias_path) {
                let relative = relative_display(&alias_path, &alias_path.join(relative));
                return Ok(format!("{}/{}", input_path.trim_end_matches('/'), relative));
            }
        }

        if let Some(root) = self.workspace_root_optional()? {
            if file_path.starts_with(&root) {
                return Ok(relative_display(&root, file_path));
            }
        }
        if self.permissions.read == AgentReadPermission::All && file_path.is_absolute() {
            return Ok(file_path.to_string_lossy().to_string());
        }

        Err(AgentError::new("路径必须位于已选择的 workspace 内。"))
    }

    pub(super) fn conversation_attachments(&self) -> &[AgentAttachmentReference] {
        self.attachment_library
            .as_ref()
            .map(|library| library.conversation_attachments.as_slice())
            .unwrap_or(&[])
    }

    pub(super) fn project_attachments(&self) -> &[AgentAttachmentReference] {
        self.attachment_library
            .as_ref()
            .map(|library| library.project_attachments.as_slice())
            .unwrap_or(&[])
    }

    pub(super) fn validate_relative_path_for_git(&self, input_path: &str) -> AgentResult<PathBuf> {
        clean_relative_path(input_path)
    }

    fn resolve_attachment_path(&self, input_path: &str) -> AgentResult<PathBuf> {
        self.check_cancelled()?;
        let reference = self.attachment_reference_for_path(input_path)?;
        let library = self.attachment_library.as_ref().ok_or_else(|| {
            AgentError::new("当前对话没有可用的附件库，无法读取 @attachments 路径。")
        })?;
        let root_path = library
            .root_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("附件库根目录不可用。"))?;
        let root = PathBuf::from(root_path)
            .canonicalize()
            .map_err(|error| AgentError::new(format!("附件库目录不可访问：{error}")))?;
        if !root.is_dir() {
            return Err(AgentError::new("附件库根目录不是目录。"));
        }

        let relative = clean_relative_path(&reference.storage_rel_path)?;
        let canonical = root
            .join(relative)
            .canonicalize()
            .map_err(|error| AgentError::new(format!("附件文件不可访问：{error}")))?;

        if !canonical.starts_with(&root) {
            return Err(AgentError::new("附件路径必须位于附件库目录内。"));
        }

        Ok(canonical)
    }

    fn attachment_reference_for_path(
        &self,
        input_path: &str,
    ) -> AgentResult<&AgentAttachmentReference> {
        let attachment_id = attachment_id_from_path(input_path)?;
        let library = self.attachment_library.as_ref().ok_or_else(|| {
            AgentError::new("当前对话没有可用的附件库，无法读取 @attachments 路径。")
        })?;

        library
            .conversation_attachments
            .iter()
            .chain(library.project_attachments.iter())
            .find(|attachment| attachment.id == attachment_id)
            .ok_or_else(|| AgentError::new(format!("未找到附件：{attachment_id}")))
    }
}

fn is_attachment_path(input_path: &str) -> bool {
    input_path.trim().starts_with("@attachments/")
}

fn attachment_id_from_path(input_path: &str) -> AgentResult<String> {
    let trimmed = input_path.trim();
    let remainder = trimmed
        .strip_prefix("@attachments/")
        .ok_or_else(|| AgentError::new("附件路径必须以 @attachments/ 开头。"))?;
    let attachment_id = remainder
        .split('/')
        .next()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AgentError::new("附件路径缺少附件 id。"))?;

    Ok(attachment_id.to_string())
}

pub(super) trait AgentTool: Send + Sync {
    fn definition(&self) -> AgentToolDefinition;
    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value>;
    fn proposed_action(
        &self,
        _context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        Ok(AgentProposedAction::ToolCall { call: call.clone() })
    }
}

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

pub(super) struct ResolvedDocumentPath {
    pub file_path: PathBuf,
    pub relative_path: String,
    pub extension: String,
    pub size_bytes: u64,
}

pub(super) struct NamedText {
    pub name: String,
    pub text: String,
}

pub(super) fn resolve_document_path(
    context: &ToolExecutionContext,
    input_path: &str,
    allowed_extensions: &[&str],
) -> AgentResult<ResolvedDocumentPath> {
    context.check_cancelled()?;
    let file_path = context.resolve_existing_path(input_path)?;
    let metadata = fs::metadata(&file_path)
        .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;

    if !metadata.is_file() {
        return Err(AgentError::new("文档读取工具只能读取文件。"));
    }

    if metadata.len() > MAX_DOCUMENT_FILE_BYTES {
        return Err(AgentError::new(format!(
            "文档过大：{} bytes，超过 {} bytes 限制。",
            metadata.len(),
            MAX_DOCUMENT_FILE_BYTES
        )));
    }

    let extension = file_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .ok_or_else(|| AgentError::new("文件缺少扩展名，无法判断文档类型。"))?;

    if !allowed_extensions
        .iter()
        .any(|allowed| extension == allowed.to_ascii_lowercase())
    {
        return Err(AgentError::new(format!(
            "不支持的文件类型：.{extension}。支持：{}",
            allowed_extensions
                .iter()
                .map(|extension| format!(".{extension}"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    context.check_cancelled()?;
    let relative_path = context.display_path(input_path, &file_path)?;

    Ok(ResolvedDocumentPath {
        file_path,
        relative_path,
        extension,
        size_bytes: metadata.len(),
    })
}

pub(super) fn sanitize_document_max_chars(max_chars: Option<usize>) -> usize {
    max_chars
        .unwrap_or(DEFAULT_DOCUMENT_MAX_CHARS)
        .clamp(1, MAX_DOCUMENT_TEXT_CHARS)
}

pub(super) fn read_zip_xml_text_parts(
    file_path: &Path,
    cancellation_token: &AgentCancellationToken,
    include_entry: impl Fn(&str) -> bool,
) -> AgentResult<Vec<NamedText>> {
    cancellation_token.check()?;
    let file = File::open(file_path)
        .map_err(|error| AgentError::new(format!("打开 OOXML 文档失败：{error}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| AgentError::new(format!("读取 OOXML 压缩包失败：{error}")))?;
    let mut parts = Vec::new();

    for index in 0..archive.len() {
        cancellation_token.check()?;
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AgentError::new(format!("读取 OOXML 条目失败：{error}")))?;
        let name = entry.name().to_string();
        if !include_entry(&name) {
            continue;
        }

        let mut xml = String::new();
        entry
            .read_to_string(&mut xml)
            .map_err(|error| AgentError::new(format!("读取 OOXML XML 失败：{error}")))?;
        cancellation_token.check()?;
        let text = xml_text_content(&xml)?;
        if !text.trim().is_empty() {
            parts.push(NamedText { name, text });
        }
    }

    cancellation_token.check()?;
    parts.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(parts)
}

pub(super) fn xml_text_content(xml: &str) -> AgentResult<String> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| AgentError::new(format!("解析 XML 失败：{error}")))?;
    let mut output = String::new();

    for node in document.descendants() {
        if node.is_element() {
            match node.tag_name().name() {
                "p" | "br" | "tr" | "row" => push_newline(&mut output),
                "tab" => output.push('\t'),
                _ => {}
            }
            continue;
        }

        if node.is_text() {
            let text = node.text().unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }

            if !output.is_empty()
                && !output.ends_with([' ', '\n', '\t'])
                && !text.starts_with([',', '.', ';', ':', ')', ']', '}'])
            {
                output.push(' ');
            }
            output.push_str(text);
        }
    }

    Ok(normalize_text_output(&output))
}

pub(super) fn join_named_text(parts: &[NamedText]) -> String {
    parts
        .iter()
        .map(|part| format!("## {}\n{}", part.name, part.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn extract_with_textutil(
    file_path: &Path,
    cancellation_token: &AgentCancellationToken,
) -> AgentResult<String> {
    cancellation_token.check()?;
    let mut child = Command::new("textutil")
        .arg("-convert")
        .arg("txt")
        .arg("-stdout")
        .arg(file_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            AgentError::new(format!(
                "读取旧版二进制 Office 文档需要系统 textutil 转换器，但启动失败：{error}"
            ))
        })?;

    loop {
        if cancellation_token.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AgentError::cancelled());
        }

        match child
            .try_wait()
            .map_err(|error| AgentError::new(format!("等待 textutil 转换失败：{error}")))?
        {
            Some(_) => break,
            None => thread::sleep(Duration::from_millis(50)),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| AgentError::new(format!("读取 textutil 转换输出失败：{error}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AgentError::new(format!(
            "textutil 转换失败：{}",
            stderr.trim()
        )));
    }

    let text = String::from_utf8(output.stdout)
        .map_err(|error| AgentError::new(format!("textutil 输出不是 UTF-8：{error}")))?;
    Ok(normalize_text_output(&text))
}

pub(super) fn normalize_text_output(value: &str) -> String {
    let mut output = String::new();
    let mut previous_blank = false;

    for line in value.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !previous_blank && !output.is_empty() {
                output.push('\n');
                previous_blank = true;
            }
            continue;
        }

        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
        previous_blank = false;
    }

    output
}

fn push_newline(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn should_exclude_name(name: &str) -> bool {
    EXCLUDED_NAMES.iter().any(|excluded| name == *excluded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentAttachmentLibraryContext, AgentAttachmentReference, AgentInputAttachmentKind,
        AgentRunContext, AgentSearchConfig, AgentSearchMode, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn rejects_paths_outside_workspace() {
        let fixture = TestWorkspace::new();
        let context = fixture.context();
        let error = context.resolve_existing_path("../outside.txt").unwrap_err();

        assert!(error.to_string().contains("路径不能包含"));
    }

    #[test]
    fn registers_tavily_tools_when_configured() {
        let registry = ToolRegistry::read_only_defaults_with_search(Some(&AgentSearchConfig {
            mode: AgentSearchMode::Tavily,
            tavily_api_key: Some("tvly-test".to_string()),
        }));
        let tools = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert!(tools.contains(&"web_search".to_string()));
        assert!(tools.contains(&"web_fetch".to_string()));
    }

    #[test]
    fn registers_run_command_as_approval_tool() {
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let definition = registry.definition_for("run_command").unwrap();

        assert_eq!(definition.name, "run_command");
        assert!(!definition.requires_workspace);
        assert!(definition.requires_approval);
    }

    #[test]
    fn registers_apply_patch_as_approval_tool() {
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let definition = registry.definition_for("apply_patch").unwrap();

        assert_eq!(definition.name, "apply_patch");
        assert!(!definition.requires_workspace);
        assert!(definition.requires_approval);
    }

    #[test]
    fn lists_and_reads_attachment_paths() {
        let fixture = TestWorkspace::new();
        let attachment_root = fixture.root.join("attachments");
        let storage_rel_path = PathBuf::from("conversations/c1/m1/a1/notes.txt");
        let attachment_path = attachment_root.join(&storage_rel_path);
        fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
        fs::write(&attachment_path, "hello from attachment").unwrap();

        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("c1".to_string()),
            project_id: Some("p1".to_string()),
            workspace: None,
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(attachment_root.to_string_lossy().to_string()),
                conversation_id: Some("c1".to_string()),
                project_id: Some("p1".to_string()),
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "a1".to_string(),
                    conversation_id: "c1".to_string(),
                    message_id: "m1".to_string(),
                    project_id: Some("p1".to_string()),
                    kind: AgentInputAttachmentKind::File,
                    name: "notes.txt".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    size_bytes: 21,
                    read_path: "@attachments/a1/notes.txt".to_string(),
                    storage_rel_path: "conversations/c1/m1/a1/notes.txt".to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            }),
            permissions: Default::default(),
        }));
        let registry = ToolRegistry::read_only_defaults_with_search(None);

        let list_result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-list".to_string(),
                tool: "attachments_list".to_string(),
                args: json!({}),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(list_result.ok, "{:?}", list_result.error);
        assert_eq!(
            list_result.result.as_ref().unwrap()["attachments"][0]["readPath"],
            "@attachments/a1/notes.txt"
        );

        let read_result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-read".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": "@attachments/a1/notes.txt" }),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(read_result.ok, "{:?}", read_result.error);
        assert_eq!(
            read_result.result.as_ref().unwrap()["content"],
            "hello from attachment"
        );
    }

    #[test]
    fn read_image_reads_attachment_visual_payload() {
        let fixture = TestWorkspace::new();
        let attachment_root = fixture.root.join("attachments");
        let storage_rel_path = PathBuf::from("conversations/c1/m1/image1/pixel.png");
        let attachment_path = attachment_root.join(&storage_rel_path);
        fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
        fs::write(&attachment_path, b"not-a-real-png-but-valid-tool-bytes").unwrap();

        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("c1".to_string()),
            project_id: Some("p1".to_string()),
            workspace: None,
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(attachment_root.to_string_lossy().to_string()),
                conversation_id: Some("c1".to_string()),
                project_id: Some("p1".to_string()),
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "image1".to_string(),
                    conversation_id: "c1".to_string(),
                    message_id: "m1".to_string(),
                    project_id: Some("p1".to_string()),
                    kind: AgentInputAttachmentKind::Image,
                    name: "pixel.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    size_bytes: 31,
                    read_path: "@attachments/image1/pixel.png".to_string(),
                    storage_rel_path: "conversations/c1/m1/image1/pixel.png".to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            }),
            permissions: Default::default(),
        }));
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let result = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-image".to_string(),
                tool: "read_image".to_string(),
                args: json!({ "path": "@attachments/image1/pixel.png" }),
                approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.as_ref().unwrap();
        assert_eq!(value["path"], "@attachments/image1/pixel.png");
        assert_eq!(value["mimeType"], "image/png");
        assert!(value["image"]["dataBase64"].as_str().unwrap().len() > 10);
    }

    #[test]
    fn absolute_read_requires_all_permission() {
        let fixture = TestWorkspace::new();
        let outside = std::env::temp_dir().join(format!(
            "my-copilot-agent-outside-read-{}",
            TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&outside, "outside content").unwrap();
        let registry = ToolRegistry::read_only_defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-outside".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": outside.to_string_lossy() }),
            approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let denied = registry.execute(&fixture.context(), &call);
        assert!(!denied.ok);
        assert!(denied.error.unwrap().contains("仅允许访问 workspace"));

        let allowed = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("test".to_string()),
                root_path: Some(fixture.root.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                read: crate::protocol::AgentReadPermission::All,
                ..Default::default()
            },
        }));
        let result = registry.execute(&allowed, &call);
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(result.result.unwrap()["content"], "outside content");

        let _ = fs::remove_file(outside);
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("my-copilot-agent-test-tools-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                conversation_id: None,
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("test".to_string()),
                    root_path: Some(self.root.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: Default::default(),
            }))
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
