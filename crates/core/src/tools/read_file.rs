use super::{AgentTool, ToolExecutionContext};
use crate::llm::LlmMessage;
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::revision::{compose_content_revision, ContentRevisionHasher};
use serde::Deserialize;
use serde_json::{json, Value};
#[cfg(not(unix))]
use std::fs::{self, OpenOptions};
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::time::SystemTime;

const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const MODEL_RESULT_METADATA_RESERVE_TOKENS: u64 = 2_000;
const PATH_IS_DIRECTORY_ERROR_CODE: &str = "read_file.path_is_directory";
const PATH_IS_DIRECTORY_CODE: &str = "path_is_directory";
const PATH_IS_DIRECTORY_MESSAGE: &str = "read_file 只能读取普通文本文件。";
const SYMLINK_FORBIDDEN_MESSAGE: &str = "read_file 不允许读取符号链接。";

pub(super) struct ReadFileTool;

impl AgentTool for ReadFileTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_file".to_string(),
            description: "Read an authorized regular UTF-8 text file and issue a run-owned FileChange Observation. Call read_file on the exact target before the first apply_patch Direct apply or Staged begin, whenever current contents are unknown, or whenever the latest successful apply_patch result did not return a reusable fileChangeTarget. A successful apply_patch apply/commit normally returns a new fileChangeTarget for its verified post-write state; copy that newer filePath and observationId into the next change to the same target instead of rereading solely for another token. Do not substitute a parent-directory listing, workspace_map, or search result. A not-found result establishes the missing state required by create but is not itself an executable create call because the model must still supply content or begin a Staged transaction. read_file.path must identify a regular file, never a directory; inspect directories with workspace_map.focusPath. With a workspace, paths may be workspace-relative. Without a workspace, relative paths are invalid: use an authorized absolute path or @home/@desktop/@documents/@downloads. Exact authorized @attachments and published-resource references retain their current meaning. Without a range it returns the complete file when the model-aware output budget permits; larger files return a lossless continuation cursor instead of failing."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "A regular UTF-8 text file only: workspace-relative path, absolute local path, @home/@desktop/@documents/@downloads, an exact @attachments/... readPath, a browser-download:... reference, or a published artifact://... URI. For a directory, call workspace_map with this path as focusPath instead. Availability depends on the current read permission and resource ownership." },
                    "startLine": { "type": "integer", "minimum": 1, "description": "Optional 1-based first line. Omit to start at the beginning." },
                    "startByte": { "type": "integer", "minimum": 0, "description": "Continuation cursor. Pass nextStartByte from a previous truncated result together with that page's expectedRevision; do not combine with startLine." },
                    "expectedRevision": { "type": "string", "description": "Required whenever startByte is present. Pass the exact revision from the previous page so pages from different file versions cannot be spliced." },
                    "maxLines": { "type": "integer", "minimum": 1, "description": "Optional soft strategy bound. There is no fixed maximum; the output token budget still applies." }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        execute_read_file_with_hook(context, args, || {})
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        super::model_projection::compact_model_result(
            result,
            read_file_model_projection(result.result.as_ref()),
        )
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let mut projected = self.trace_projection(result);
        let Some(value) = projected.result.as_mut().and_then(Value::as_object_mut) else {
            return projected;
        };
        value.remove("observationId");
        if let Some(target) = value
            .get_mut("fileChangeTarget")
            .and_then(Value::as_object_mut)
        {
            target.remove("observationId");
        }
        if let Some(args) = value
            .get_mut("continueWith")
            .and_then(Value::as_object_mut)
            .and_then(|continuation| continuation.get_mut("args"))
            .and_then(Value::as_object_mut)
        {
            args.remove("observationId");
        }
        projected
    }
}

fn execute_read_file_with_hook(
    context: &ToolExecutionContext,
    args: Value,
    before_open: impl FnOnce(),
) -> AgentResult<Value> {
    context.check_cancelled()?;
    let args: ReadFileArgs = serde_json::from_value(args).map_err(|_| {
        AgentError::structured(
            "agent.read_file.invalid_arguments",
            "read_file 参数无效。",
            json!({
                "type": "file_read",
                "code": "invalid_arguments",
                "message": "read_file 参数无效。",
                "recovery": "correct_arguments"
            }),
        )
    })?;
    args.validate()?;
    let path = args.path()?;
    let file_path = match context.resolve_existing_path_preserving_leaf(path) {
        Ok(file_path) => file_path,
        Err(read_error) => {
            if let Some(result) = missing_file_observation(context, path)? {
                return Ok(result);
            }
            return Err(read_error);
        }
    };
    let display_path = context.display_path(path, &file_path)?;
    let display_path = if display_path.is_empty() {
        ".".to_string()
    } else {
        display_path
    };
    let mut opened = open_regular_text_file_with_hook(&file_path, &display_path, before_open)?;
    let initial_metadata = opened.initial_metadata().clone();

    let requested_start_line = u64::try_from(args.start_line.unwrap_or(1)).unwrap_or(u64::MAX);
    let inspection = inspect_text_file(
        opened.file_mut(),
        context,
        &initial_metadata,
        requested_start_line,
        args.start_byte,
    )?;
    if let Some(expected_revision) = args.expected_revision.as_deref() {
        if expected_revision != inspection.revision {
            return Err(AgentError::new(
                "read_file 续读失败：文件 revision 已变化，请从开头重新读取。",
            ));
        }
    }
    let fragment = read_fragment(
        opened.file_mut(),
        context,
        &inspection,
        args.max_lines
            .map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
    )?;
    opened.ensure_current(&inspection.identity)?;
    let parent_metadata = opened.parent_metadata()?;
    let observation = context
        .file_observations()
        .issue_existing(
            context.conversation_id()?,
            context.run_id()?,
            &file_path,
            &inspection.revision,
            &initial_metadata,
            &parent_metadata,
        )
        .map_err(file_observation_error)?;

    fit_read_file_page_to_model_budget(
        context,
        display_path,
        &inspection,
        &fragment.content,
        fragment.stop_reason,
        observation.id(),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadFileArgs {
    path: Option<String>,
    start_line: Option<usize>,
    start_byte: Option<u64>,
    expected_revision: Option<String>,
    max_lines: Option<usize>,
}

impl ReadFileArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("read_file.path 不能为空。"))
    }

    fn validate(&self) -> AgentResult<()> {
        if self.start_line.is_some() && self.start_byte.is_some() {
            return Err(AgentError::new(
                "read_file.startLine 和 startByte 不能同时使用；续读时只传上次返回的 nextStartByte。",
            ));
        }
        if self.start_line == Some(0) || self.max_lines == Some(0) {
            return Err(AgentError::new(
                "read_file.startLine 和 maxLines 必须是大于 0 的整数。",
            ));
        }
        if self.start_byte.is_some() != self.expected_revision.is_some()
            || self
                .expected_revision
                .as_deref()
                .is_some_and(|revision| revision.trim().is_empty())
        {
            return Err(AgentError::new(
                "read_file.startByte 与 expectedRevision 必须一起使用。",
            ));
        }
        Ok(())
    }
}

fn read_file_model_projection(source: Option<&Value>) -> Option<Value> {
    let mut projected = super::model_projection::retain_fields(
        source,
        &[
            "path",
            "revision",
            "startLine",
            "startByte",
            "endLine",
            "endByteExclusive",
            "totalLines",
            "totalBytes",
            "content",
            "truncated",
            "truncatedReason",
            "nextStartByte",
            "nextStartLine",
            "continueWith",
            "exists",
            "observationId",
            "fileChangeTarget",
            "message",
        ],
    );
    if let (Some(Value::Object(output)), Some(source)) = (projected.as_mut(), source) {
        let path = source.get("path").and_then(Value::as_str);
        let revision = source.get("revision").and_then(Value::as_str);
        let next_start_byte = source.get("nextStartByte").and_then(Value::as_u64);
        if source.get("truncated").and_then(Value::as_bool) == Some(true) {
            if let (Some(path), Some(revision), Some(next_start_byte)) =
                (path, revision, next_start_byte)
            {
                output.insert(
                    "continueWith".to_string(),
                    json!({
                        "tool": "read_file",
                        "args": {
                            "path": path,
                            "startByte": next_start_byte,
                            "expectedRevision": revision
                        }
                    }),
                );
            }
        }
    }
    projected
}

fn path_is_directory_error(path: &str) -> AgentError {
    AgentError::structured(
        PATH_IS_DIRECTORY_ERROR_CODE,
        PATH_IS_DIRECTORY_MESSAGE,
        json!({
            "code": PATH_IS_DIRECTORY_CODE,
            "path": path,
            "message": PATH_IS_DIRECTORY_MESSAGE,
            "continueWith": {
                "tool": "workspace_map",
                "args": {
                    "focusPath": path,
                    "maxDepth": 3
                }
            }
        }),
    )
}

fn fit_read_file_page_to_model_budget(
    context: &ToolExecutionContext,
    display_path: String,
    inspection: &TextFileInspection,
    content: &str,
    stop_reason: FragmentStopReason,
    observation_id: &str,
) -> AgentResult<Value> {
    let full = build_read_file_page(
        context,
        &display_path,
        inspection,
        content,
        stop_reason,
        observation_id,
    );
    // Production runtime always supplies the fixed 10K result budget. Smaller budgets are used
    // only by focused pagination tests and embedded callers as a content-page strategy; treating
    // them as a complete provider-message ceiling would leave no room for even the cursor.
    if context.text_output_budget().max_tokens()
        < MODEL_RESULT_METADATA_RESERVE_TOKENS.saturating_mul(2)
    {
        return Ok(full);
    }
    if read_file_page_fits_model_budget(context, &full)? {
        return Ok(full);
    }

    let mut boundaries = content
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if boundaries.first().copied() != Some(0) {
        boundaries.insert(0, 0);
    }
    boundaries.push(content.len());
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut lower = 0_usize;
    let mut upper = boundaries.len().saturating_sub(1);
    while lower < upper {
        let middle = lower + (upper - lower).div_ceil(2);
        let candidate = build_read_file_page(
            context,
            &display_path,
            inspection,
            &content[..boundaries[middle]],
            FragmentStopReason::OutputBudget,
            observation_id,
        );
        if read_file_page_fits_model_budget(context, &candidate)? {
            lower = middle;
        } else {
            upper = middle.saturating_sub(1);
        }
    }

    if lower == 0 && !content.is_empty() {
        let first_character_end = boundaries.get(1).copied().unwrap_or(content.len());
        let smallest_progressing_page = build_read_file_page(
            context,
            &display_path,
            inspection,
            &content[..first_character_end],
            FragmentStopReason::OutputBudget,
            observation_id,
        );
        if !read_file_page_fits_model_budget(context, &smallest_progressing_page)? {
            return Err(AgentError::new(
                "read_file 无法在 10K 模型结果预算内同时返回一个字符和安全续读游标。",
            ));
        }
        return Ok(smallest_progressing_page);
    }

    Ok(build_read_file_page(
        context,
        &display_path,
        inspection,
        &content[..boundaries[lower]],
        FragmentStopReason::OutputBudget,
        observation_id,
    ))
}

fn build_read_file_page(
    context: &ToolExecutionContext,
    display_path: &str,
    inspection: &TextFileInspection,
    content: &str,
    stop_reason: FragmentStopReason,
    observation_id: &str,
) -> Value {
    let positions = measure_positions(inspection.start.line, inspection.start.column, content);
    let next_byte = inspection
        .start
        .byte
        .saturating_add(u64::try_from(content.len()).unwrap_or(u64::MAX));
    let truncated = next_byte < inspection.total_bytes;
    let truncated_reason = truncated.then(|| stop_reason.as_str());

    json!({
        "path": display_path,
        "exists": true,
        "observationId": observation_id,
        "fileChangeTarget": {
            "filePath": display_path,
            "observationId": observation_id,
            "state": "existing"
        },
        "revision": inspection.revision,
        "startLine": inspection.start.line,
        "startColumn": inspection.start.column,
        "startByte": inspection.start.byte,
        "endLine": positions.end_line,
        "endColumn": positions.end_column,
        "endByteExclusive": next_byte,
        "totalLines": inspection.total_lines,
        "totalBytes": inspection.total_bytes,
        "returnedBytes": content.len(),
        "estimatedContentTokens": context.text_output_budget().estimate(content),
        "outputTokenBudget": context.text_output_budget().max_tokens(),
        "truncated": truncated,
        "truncatedReason": truncated_reason,
        "nextStartByte": truncated.then_some(next_byte),
        "nextStartLine": truncated.then_some(positions.next_line),
        "nextStartColumn": truncated.then_some(positions.next_column),
        "content": content
    })
}

fn missing_file_observation(
    context: &ToolExecutionContext,
    input_path: &str,
) -> AgentResult<Option<Value>> {
    missing_file_observation_with_hook(context, input_path, || {})
}

fn missing_file_observation_with_hook(
    context: &ToolExecutionContext,
    input_path: &str,
    before_missing_check: impl FnOnce(),
) -> AgentResult<Option<Value>> {
    let target = match context.resolve_missing_file_observation_target(input_path) {
        Ok(target) => target,
        // This fallback may only replace the original read error after the Host has positively
        // established an authorized missing target. Path-policy rejection (including ancestor
        // symlinks) is not evidence of absence and must not mask an existing read denial.
        Err(_) => return Ok(None),
    };
    #[cfg(unix)]
    let parent_metadata = {
        let parent = match crate::file_change::BoundReadParent::bind(target.absolute_path()) {
            Ok(parent) => parent,
            Err(_) => return Ok(None),
        };
        before_missing_check();
        match parent.is_current_missing_leaf() {
            Ok(true) => {}
            Ok(false) | Err(_) => return Ok(None),
        }
        parent
            .parent_metadata()
            .map_err(|_| AgentError::new("read_file 无法验证目标文件的父目录。"))?
    };
    #[cfg(not(unix))]
    let parent_metadata = {
        before_missing_check();
        match fs::symlink_metadata(target.absolute_path()) {
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Ok(None),
        }
        fs::metadata(target.parent())
            .map_err(|_| AgentError::new("read_file 无法验证目标文件的父目录。"))?
    };
    let observation = context
        .file_observations()
        .issue_missing(
            context.conversation_id()?,
            context.run_id()?,
            target.absolute_path(),
            &parent_metadata,
        )
        .map_err(file_observation_error)?;
    Ok(Some(json!({
        "path": input_path,
        "exists": false,
        "observationId": observation.id(),
        "fileChangeTarget": {
            "filePath": input_path,
            "observationId": observation.id(),
            "state": "missing"
        },
        "message": "文件不存在。",
    })))
}

fn file_observation_error(error: crate::file_change::FileChangeError) -> AgentError {
    AgentError::structured(
        "agent.read_file.observation_failed",
        error.to_string(),
        json!({
            "type": "file_observation",
            "code": error.failure().code,
            "category": error.failure().category,
            "message": error.failure().message,
            "recovery": error.failure().recovery,
        }),
    )
}

fn read_file_page_fits_model_budget(
    context: &ToolExecutionContext,
    source: &Value,
) -> AgentResult<bool> {
    let projected = read_file_model_projection(Some(source))
        .ok_or_else(|| AgentError::new("read_file 无法构建模型结果投影。"))?;
    let content = serde_json::to_string(&projected)
        .map_err(|error| AgentError::new(format!("read_file 无法序列化模型结果：{error}")))?;
    let message = LlmMessage::tool_result(context.tool_call_id()?, content, false);
    Ok(context.text_output_budget().estimate_message(&message)
        <= context.text_output_budget().max_tokens())
}

struct OpenedTextFile {
    file: File,
    initial_metadata: Metadata,
    #[cfg(unix)]
    parent: crate::file_change::BoundReadParent,
    #[cfg(not(unix))]
    path: std::path::PathBuf,
}

impl OpenedTextFile {
    fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    fn initial_metadata(&self) -> &Metadata {
        &self.initial_metadata
    }

    fn ensure_current(&self, expected_identity: &FileIdentity) -> AgentResult<()> {
        ensure_file_unchanged(&self.file, expected_identity)?;
        #[cfg(unix)]
        match self
            .parent
            .is_current_leaf(&self.file, &self.initial_metadata)
        {
            Ok(true) => {}
            Ok(false) => return Err(file_changed_error()),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                return Err(symlink_forbidden_error());
            }
            Err(_) => return Err(file_changed_error()),
        }
        #[cfg(not(unix))]
        {
            let named = fs::symlink_metadata(&self.path).map_err(|_| file_changed_error())?;
            if named.file_type().is_symlink() || !expected_identity.matches(&named) {
                return Err(file_changed_error());
            }
        }
        Ok(())
    }

    fn parent_metadata(&self) -> AgentResult<Metadata> {
        #[cfg(unix)]
        {
            self.parent
                .parent_metadata()
                .map_err(|_| AgentError::new("read_file 无法验证目标文件的父目录。"))
        }
        #[cfg(not(unix))]
        {
            let parent = self
                .path
                .parent()
                .ok_or_else(|| AgentError::new("read_file 无法确定目标文件的父目录。"))?;
            fs::metadata(parent)
                .map_err(|_| AgentError::new("read_file 无法验证目标文件的父目录。"))
        }
    }
}

fn open_regular_text_file_with_hook(
    path: &std::path::Path,
    display_path: &str,
    before_open: impl FnOnce(),
) -> AgentResult<OpenedTextFile> {
    #[cfg(unix)]
    {
        let parent = crate::file_change::BoundReadParent::bind(path).map_err(map_open_error)?;
        // Test-only callers use this hook to deterministically race the already-bound parent or
        // leaf. Production passes a no-op.
        before_open();
        let file = parent.open_leaf().map_err(map_open_error)?;
        let initial_metadata = file
            .metadata()
            .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;
        ensure_regular_file(&initial_metadata, display_path)?;
        Ok(OpenedTextFile {
            file,
            initial_metadata,
            parent,
        })
    }

    #[cfg(not(unix))]
    {
        let path_metadata = fs::symlink_metadata(path)
            .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;
        if path_metadata.file_type().is_symlink() {
            return Err(symlink_forbidden_error());
        }
        ensure_regular_file(&path_metadata, display_path)?;
        let path_identity = FileIdentity::from_metadata(&path_metadata);
        before_open();
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(map_open_error)?;
        let initial_metadata = file
            .metadata()
            .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;
        ensure_regular_file(&initial_metadata, display_path)?;
        if !path_identity.matches(&initial_metadata) {
            return Err(file_changed_error());
        }
        Ok(OpenedTextFile {
            file,
            initial_metadata,
            path: path.to_path_buf(),
        })
    }
}

fn ensure_regular_file(metadata: &Metadata, display_path: &str) -> AgentResult<()> {
    if metadata.is_dir() {
        return Err(path_is_directory_error(display_path));
    }
    if !metadata.is_file() {
        return Err(AgentError::new("read_file 只能读取普通文本文件。"));
    }
    Ok(())
}

fn map_open_error(error: std::io::Error) -> AgentError {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return symlink_forbidden_error();
    }
    if error.kind() == std::io::ErrorKind::NotFound {
        return file_changed_error();
    }
    AgentError::new(format!("打开文件失败：{error}"))
}

fn symlink_forbidden_error() -> AgentError {
    AgentError::new(SYMLINK_FORBIDDEN_MESSAGE)
}

#[derive(Debug, Clone, Copy)]
struct FileCursor {
    byte: u64,
    line: u64,
    column: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }

    fn matches(&self, metadata: &Metadata) -> bool {
        self == &Self::from_metadata(metadata)
    }
}

#[derive(Debug)]
struct TextFileInspection {
    identity: FileIdentity,
    revision: String,
    total_bytes: u64,
    total_lines: u64,
    start: FileCursor,
}

fn inspect_text_file(
    file: &mut File,
    context: &ToolExecutionContext,
    initial_metadata: &Metadata,
    requested_start_line: u64,
    requested_start_byte: Option<u64>,
) -> AgentResult<TextFileInspection> {
    let identity = FileIdentity::from_metadata(initial_metadata);
    file.seek(SeekFrom::Start(0))
        .map_err(|error| AgentError::new(format!("定位文件开头失败：{error}")))?;

    let mut forward = ContentRevisionHasher::new();
    let mut buffer = vec![0_u8; STREAM_BUFFER_BYTES];
    let mut utf8_carry = Vec::with_capacity(4);
    let mut total_bytes = 0_u64;
    let mut newline_count = 0_u64;
    let mut last_byte = None;
    let mut current_line = 1_u64;
    let mut current_column = 1_u64;
    let mut line_cursor = (requested_start_line == 1).then_some(FileCursor {
        byte: 0,
        line: 1,
        column: 1,
    });
    let mut byte_cursor = None;

    loop {
        context.check_cancelled()?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| AgentError::new(format!("读取文件失败：{error}")))?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        validate_utf8_chunk(&mut utf8_carry, chunk, total_bytes)?;
        forward.update(chunk);

        for (index, byte) in chunk.iter().copied().enumerate() {
            let absolute = total_bytes.saturating_add(u64::try_from(index).unwrap_or(u64::MAX));
            if requested_start_byte == Some(absolute) {
                if is_utf8_continuation(byte) {
                    return Err(AgentError::new(format!(
                        "read_file.startByte={absolute} 位于 UTF-8 字符中间；请使用工具返回的 nextStartByte。"
                    )));
                }
                byte_cursor = Some(FileCursor {
                    byte: absolute,
                    line: current_line,
                    column: current_column,
                });
            }

            if byte == b'\n' {
                newline_count = newline_count.saturating_add(1);
                current_line = current_line.saturating_add(1);
                current_column = 1;
                if line_cursor.is_none() && current_line == requested_start_line {
                    line_cursor = Some(FileCursor {
                        byte: absolute.saturating_add(1),
                        line: current_line,
                        column: 1,
                    });
                }
            } else if !is_utf8_continuation(byte) {
                current_column = current_column.saturating_add(1);
            }
            last_byte = Some(byte);
        }
        total_bytes = total_bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }

    if !utf8_carry.is_empty() {
        return Err(AgentError::new(format!(
            "read_file 只支持 UTF-8 文本；文件末尾在 byte {total_bytes} 附近包含不完整字符。"
        )));
    }
    if requested_start_byte == Some(total_bytes) {
        byte_cursor = Some(FileCursor {
            byte: total_bytes,
            line: current_line,
            column: current_column,
        });
    }
    if requested_start_byte.is_some_and(|offset| offset > total_bytes) {
        return Err(AgentError::new(format!(
            "read_file.startByte 超出文件范围：文件共 {total_bytes} bytes。"
        )));
    }

    let total_lines = if total_bytes == 0 {
        0
    } else if last_byte == Some(b'\n') {
        newline_count
    } else {
        newline_count.saturating_add(1)
    };
    let start = if requested_start_byte.is_some() {
        byte_cursor.expect("validated byte cursor must exist")
    } else if requested_start_line <= total_lines {
        line_cursor.expect("existing requested line must have a cursor")
    } else {
        FileCursor {
            byte: total_bytes,
            line: total_lines.saturating_add(1),
            column: 1,
        }
    };

    if total_bytes != identity.length {
        return Err(file_changed_error());
    }
    let reverse_hash = hash_file_in_reverse(file, context, total_bytes)?;
    ensure_file_unchanged(file, &identity)?;

    Ok(TextFileInspection {
        identity,
        revision: compose_content_revision(total_bytes, forward.finish(), reverse_hash),
        total_bytes,
        total_lines,
        start,
    })
}

fn hash_file_in_reverse(
    file: &mut File,
    context: &ToolExecutionContext,
    total_bytes: u64,
) -> AgentResult<u64> {
    let mut reverse = ContentRevisionHasher::new();
    let mut buffer = vec![0_u8; STREAM_BUFFER_BYTES];
    let mut remaining = total_bytes;
    while remaining > 0 {
        context.check_cancelled()?;
        let read_size =
            usize::try_from(remaining.min(u64::try_from(STREAM_BUFFER_BYTES).unwrap_or(u64::MAX)))
                .unwrap_or(STREAM_BUFFER_BYTES);
        let start = remaining.saturating_sub(u64::try_from(read_size).unwrap_or(u64::MAX));
        file.seek(SeekFrom::Start(start))
            .map_err(|error| AgentError::new(format!("定位文件失败：{error}")))?;
        file.read_exact(&mut buffer[..read_size])
            .map_err(|error| AgentError::new(format!("读取文件失败：{error}")))?;
        reverse.update_reversed(&buffer[..read_size]);
        remaining = start;
    }
    Ok(reverse.finish())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FragmentStopReason {
    EndOfFile,
    OutputBudget,
    LineLimit,
}

impl FragmentStopReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::EndOfFile => "end_of_file",
            Self::OutputBudget => "output_budget",
            Self::LineLimit => "line_limit",
        }
    }
}

#[derive(Debug)]
struct TextFragment {
    content: String,
    stop_reason: FragmentStopReason,
}

fn read_fragment(
    file: &mut File,
    context: &ToolExecutionContext,
    inspection: &TextFileInspection,
    max_lines: Option<u64>,
) -> AgentResult<TextFragment> {
    if inspection.start.byte >= inspection.total_bytes {
        return Ok(TextFragment {
            content: String::new(),
            stop_reason: FragmentStopReason::EndOfFile,
        });
    }

    file.seek(SeekFrom::Start(inspection.start.byte))
        .map_err(|error| AgentError::new(format!("定位读取起点失败：{error}")))?;
    let result_budget = context.text_output_budget();
    // `read_file` owns a lossless source cursor, so keep enough room for the path, byte/line
    // ranges and executable continuation that surround `content` in the final model message.
    // Tiny synthetic test budgets retain their full allowance so every page still makes progress.
    let content_token_limit =
        if result_budget.max_tokens() > MODEL_RESULT_METADATA_RESERVE_TOKENS.saturating_mul(2) {
            result_budget
                .max_tokens()
                .saturating_sub(MODEL_RESULT_METADATA_RESERVE_TOKENS)
        } else {
            result_budget.max_tokens()
        };
    let content_budget = result_budget.with_max_tokens(content_token_limit);
    let mut buffer = vec![0_u8; STREAM_BUFFER_BYTES];
    let mut utf8_carry = Vec::with_capacity(4);
    let mut content = String::new();
    let mut returned_newlines = 0_u64;
    let mut stop_reason = FragmentStopReason::EndOfFile;
    let mut source_offset = inspection.start.byte;

    'read: loop {
        context.check_cancelled()?;
        let remaining = inspection.total_bytes.saturating_sub(source_offset);
        if remaining == 0 {
            break;
        }
        let read_size =
            usize::try_from(remaining.min(u64::try_from(STREAM_BUFFER_BYTES).unwrap_or(u64::MAX)))
                .unwrap_or(STREAM_BUFFER_BYTES);
        let read = file
            .read(&mut buffer[..read_size])
            .map_err(|error| AgentError::new(format!("读取文件失败：{error}")))?;
        if read == 0 {
            return Err(file_changed_error());
        }
        source_offset = source_offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        let decoded = decode_utf8_chunk(&mut utf8_carry, &buffer[..read], source_offset)?;
        if decoded.is_empty() {
            continue;
        }

        let mut segment = decoded.as_str();
        let mut reached_line_limit = false;
        if let Some(max_lines) = max_lines {
            let remaining_lines = max_lines.saturating_sub(returned_newlines);
            if remaining_lines == 0 {
                stop_reason = FragmentStopReason::LineLimit;
                break;
            }
            if let Some(end) = byte_after_nth_newline(segment, remaining_lines) {
                segment = &segment[..end];
                reached_line_limit = true;
            }
        }
        returned_newlines = returned_newlines.saturating_add(
            u64::try_from(segment.bytes().filter(|byte| *byte == b'\n').count())
                .unwrap_or(u64::MAX),
        );
        content.push_str(segment);

        if !content_budget.fits(&content) {
            let mut fitting = content_budget.fitting_prefix_len(&content);
            if fitting == 0 {
                fitting = content.chars().next().map_or(0, char::len_utf8);
            }
            if let Some(last_newline) = content[..fitting].rfind('\n') {
                fitting = last_newline.saturating_add(1);
            }
            content.truncate(fitting);
            stop_reason = FragmentStopReason::OutputBudget;
            break 'read;
        }
        if reached_line_limit {
            stop_reason = FragmentStopReason::LineLimit;
            break;
        }
    }

    if stop_reason == FragmentStopReason::EndOfFile && !utf8_carry.is_empty() {
        return Err(file_changed_error());
    }

    Ok(TextFragment {
        content,
        stop_reason,
    })
}

fn byte_after_nth_newline(value: &str, count: u64) -> Option<usize> {
    if count == 0 {
        return Some(0);
    }
    let mut seen = 0_u64;
    for (index, byte) in value.bytes().enumerate() {
        if byte == b'\n' {
            seen = seen.saturating_add(1);
            if seen == count {
                return Some(index.saturating_add(1));
            }
        }
    }
    None
}

fn validate_utf8_chunk(carry: &mut Vec<u8>, chunk: &[u8], chunk_start: u64) -> AgentResult<()> {
    if carry.is_empty() {
        return retain_incomplete_utf8(carry, chunk, chunk_start);
    }

    let carry_len = carry.len();
    let mut combined = Vec::with_capacity(carry_len.saturating_add(chunk.len()));
    combined.extend_from_slice(carry);
    combined.extend_from_slice(chunk);
    carry.clear();
    retain_incomplete_utf8(
        carry,
        &combined,
        chunk_start.saturating_sub(u64::try_from(carry_len).unwrap_or(u64::MAX)),
    )
}

fn decode_utf8_chunk(
    carry: &mut Vec<u8>,
    chunk: &[u8],
    source_offset_after_read: u64,
) -> AgentResult<String> {
    let carry_len = carry.len();
    let mut combined = Vec::with_capacity(carry_len.saturating_add(chunk.len()));
    combined.extend_from_slice(carry);
    combined.extend_from_slice(chunk);
    carry.clear();

    match std::str::from_utf8(&combined) {
        Ok(value) => Ok(value.to_string()),
        Err(error) if error.error_len().is_none() => {
            let valid_up_to = error.valid_up_to();
            let value = std::str::from_utf8(&combined[..valid_up_to])
                .expect("valid_up_to must delimit valid UTF-8")
                .to_string();
            carry.extend_from_slice(&combined[valid_up_to..]);
            Ok(value)
        }
        Err(error) => Err(AgentError::new(format!(
            "read_file 只支持 UTF-8 文本；文件在 byte {} 附近包含无效编码。",
            source_offset_after_read
                .saturating_sub(u64::try_from(combined.len()).unwrap_or(u64::MAX))
                .saturating_add(u64::try_from(error.valid_up_to()).unwrap_or(u64::MAX))
        ))),
    }
}

fn retain_incomplete_utf8(
    carry: &mut Vec<u8>,
    value: &[u8],
    absolute_start: u64,
) -> AgentResult<()> {
    match std::str::from_utf8(value) {
        Ok(_) => Ok(()),
        Err(error) if error.error_len().is_none() => {
            carry.extend_from_slice(&value[error.valid_up_to()..]);
            Ok(())
        }
        Err(error) => Err(AgentError::new(format!(
            "read_file 只支持 UTF-8 文本；文件在 byte {} 附近包含无效编码。",
            absolute_start.saturating_add(u64::try_from(error.valid_up_to()).unwrap_or(u64::MAX))
        ))),
    }
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

#[derive(Debug)]
struct ContentPositions {
    end_line: Option<u64>,
    end_column: Option<u64>,
    next_line: u64,
    next_column: u64,
}

fn measure_positions(start_line: u64, start_column: u64, content: &str) -> ContentPositions {
    let mut line = start_line;
    let mut column = start_column;
    let mut end_line = None;
    let mut end_column = None;
    for character in content.chars() {
        end_line = Some(line);
        end_column = Some(column);
        if character == '\n' {
            line = line.saturating_add(1);
            column = 1;
        } else {
            column = column.saturating_add(1);
        }
    }
    ContentPositions {
        end_line,
        end_column,
        next_line: line,
        next_column: column,
    }
}

fn ensure_file_unchanged(file: &File, expected: &FileIdentity) -> AgentResult<()> {
    let current = file
        .metadata()
        .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;
    if !expected.matches(&current) {
        return Err(file_changed_error());
    }
    Ok(())
}

fn file_changed_error() -> AgentError {
    AgentError::new("读取期间文件发生变化；请重新调用 read_file 获取一致内容。")
}

#[cfg(test)]
mod tests {
    use super::super::{ToolExecutionContext, ToolRegistry};
    use super::{
        execute_read_file_with_hook, missing_file_observation_with_hook,
        open_regular_text_file_with_hook, ReadFileArgs, PATH_IS_DIRECTORY_CODE,
        PATH_IS_DIRECTORY_ERROR_CODE, PATH_IS_DIRECTORY_MESSAGE, STREAM_BUFFER_BYTES,
        SYMLINK_FORBIDDEN_MESSAGE,
    };
    use crate::context::{ContextCapacityDetector, ContextTextBudget};
    use crate::protocol::{
        AgentApiStyle, AgentApprovalStatus, AgentPermissions, AgentRunContext, AgentToolCall,
        AgentWorkspaceContext, AgentWritePermission,
    };
    use crate::revision::content_revision;
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn schema_advertises_path_without_top_level_composition() {
        let registry = ToolRegistry::defaults_with_search(None);
        let definition = registry.definition_for("read_file").unwrap();

        assert_eq!(definition.input_schema["type"], "object");
        assert_eq!(definition.input_schema["required"], json!(["path"]));
        assert_eq!(definition.input_schema["additionalProperties"], false);
        assert!(definition.input_schema.get("anyOf").is_none());
        assert!(definition.input_schema["properties"]
            .get("filePath")
            .is_none());
        assert!(definition.description.contains("regular UTF-8 text file"));
        assert!(definition
            .description
            .contains("before the first apply_patch Direct apply or Staged begin"));
        assert!(definition
            .description
            .contains("successful apply_patch result did not return a reusable fileChangeTarget"));
        assert!(definition
            .description
            .contains("instead of rereading solely for another token"));
        assert!(definition
            .description
            .contains("A not-found result establishes the missing state required by create"));
        assert!(definition
            .description
            .contains("Without a workspace, relative paths are invalid"));
        for alias in ["@home", "@desktop", "@documents", "@downloads"] {
            assert!(definition.description.contains(alias));
        }
        let path_description = definition.input_schema["properties"]["path"]["description"]
            .as_str()
            .unwrap();
        assert!(path_description.contains("regular UTF-8 text file"));
        assert!(path_description.contains("workspace_map"));
        assert!(path_description.contains("focusPath"));
    }

    #[test]
    fn directory_error_returns_an_executable_workspace_map_recovery() {
        let fixture = TestWorkspace::new();
        fs::create_dir_all(fixture.root.join("crates/mcp-client/src")).unwrap();
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-read-directory".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "crates/mcp-client/src" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let raw = registry.execute(&context, &call);

        assert!(!raw.ok);
        assert_eq!(raw.error.as_deref(), Some(PATH_IS_DIRECTORY_MESSAGE));
        let payload = raw.result.as_ref().unwrap();
        assert_eq!(payload["code"], PATH_IS_DIRECTORY_CODE);
        assert_eq!(payload["errorCode"], PATH_IS_DIRECTORY_ERROR_CODE);
        assert_eq!(payload["path"], "crates/mcp-client/src");
        assert_eq!(payload["message"], PATH_IS_DIRECTORY_MESSAGE);
        assert_eq!(
            payload["continueWith"],
            json!({
                "tool": "workspace_map",
                "args": {
                    "focusPath": "crates/mcp-client/src",
                    "maxDepth": 3
                }
            })
        );

        let model = registry.model_projection(&raw);
        let projected = model.result.unwrap();
        assert_eq!(projected["code"], PATH_IS_DIRECTORY_CODE);
        assert_eq!(projected["path"], "crates/mcp-client/src");
        assert_eq!(projected["continueWith"], payload["continueWith"]);
    }

    #[test]
    fn hidden_file_path_alias_is_rejected_with_safe_error() {
        let fixture = TestWorkspace::new();
        fixture.write_file("legacy.txt", "legacy alias");
        let registry = ToolRegistry::defaults_with_search(None);
        let result = registry.execute(
            &fixture.context(),
            &AgentToolCall {
                id: "call-hidden-alias".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "filePath": "legacy.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("read_file 参数无效。"));
        assert_eq!(result.result.unwrap()["code"], "invalid_arguments");
    }

    #[test]
    fn existing_and_missing_reads_return_actionable_fresh_observations() {
        let fixture = TestWorkspace::new();
        fixture.write_file("present.txt", "present\n");
        let context = fixture.context();
        let existing = execute_with_call_id(
            &context,
            "call-read-present-file",
            json!({ "path": "present.txt" }),
        );
        let missing = execute_with_call_id(
            &context,
            "call-read-missing-file",
            json!({ "path": "missing.txt" }),
        );

        assert_eq!(existing["exists"], true);
        assert!(existing["observationId"]
            .as_str()
            .is_some_and(|id| id.starts_with("fobs_")));
        assert_eq!(missing["exists"], false);
        assert_eq!(missing["message"], "文件不存在。");
        assert!(missing["observationId"]
            .as_str()
            .is_some_and(|id| id.starts_with("fobs_")));
        assert_eq!(
            missing["fileChangeTarget"]["observationId"],
            missing["observationId"]
        );
        assert_eq!(missing["fileChangeTarget"]["filePath"], "missing.txt");
        assert_eq!(missing["fileChangeTarget"]["state"], "missing");
        assert!(missing.get("continueWith").is_none());
        assert_eq!(existing["fileChangeTarget"]["state"], "existing");
        assert_ne!(existing["observationId"], missing["observationId"]);
    }

    #[cfg(unix)]
    #[test]
    fn missing_leaf_creation_race_does_not_issue_an_observation() {
        let fixture = TestWorkspace::new();
        fixture.write_file("inside/keep.txt", "keep\n");
        let target = fixture.root.join("inside/new.txt");
        let context = fixture
            .context()
            .with_tool_call_id("call-missing-leaf-race".to_string());

        let result = missing_file_observation_with_hook(&context, "inside/new.txt", || {
            fs::write(&target, "appeared\n").unwrap();
        })
        .unwrap();

        assert!(
            result.is_none(),
            "a raced leaf must not receive an observation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_ancestor_symlink_swap_does_not_issue_an_observation() {
        use std::os::unix::fs::symlink;

        let fixture = TestWorkspace::new();
        fixture.write_file("inside/keep.txt", "keep\n");
        let outside = tempfile::TempDir::new().unwrap();
        let inside = fixture.root.join("inside");
        let displaced = fixture.root.join("inside-displaced");
        let context = fixture
            .context()
            .with_tool_call_id("call-missing-ancestor-race".to_string());

        let result = missing_file_observation_with_hook(&context, "inside/new.txt", || {
            fs::rename(&inside, &displaced).unwrap();
            symlink(outside.path(), &inside).unwrap();
        })
        .unwrap();

        assert!(
            result.is_none(),
            "a raced ancestor must not receive a missing observation"
        );
    }

    #[test]
    fn write_all_can_observe_an_outside_missing_target_without_blind_read_access() {
        let fixture = TestWorkspace::new();
        let context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("read-file-write-only-conversation".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                write: AgentWritePermission::All,
                ..Default::default()
            },
        }))
        .with_runtime_services("read-file-write-only-run".to_string(), None);
        let registry = ToolRegistry::defaults_with_search(None);
        // macOS commonly exposes the temporary directory through `/var -> /private/var`.
        // FileChange intentionally rejects every ancestor symlink, so use the canonical parent
        // here to exercise the no-workspace absolute-path permission case rather than the
        // separately covered symlink rejection case.
        let canonical_root = fixture.root.canonicalize().unwrap();
        let missing_path = canonical_root.join("missing.txt");
        let missing = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-write-only-missing".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": missing_path }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(missing.ok, "{:?}", missing.error);
        let missing = missing.result.unwrap();
        assert_eq!(missing["exists"], false);
        assert_eq!(missing["message"], "文件不存在。");
        assert!(missing["observationId"]
            .as_str()
            .unwrap()
            .starts_with("fobs_"));

        fixture.write_file("existing.txt", "must remain unread\n");
        let existing = registry.execute(
            &context,
            &AgentToolCall {
                id: "call-write-only-existing".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": canonical_root.join("existing.txt") }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );
        assert!(!existing.ok);
        assert!(existing.result.is_none());
        assert!(existing
            .error
            .as_deref()
            .is_some_and(|error| error.contains("读取权限")));
    }

    #[test]
    fn explicit_line_bounds_remain_available_without_a_global_maximum() {
        let fixture = TestWorkspace::new();
        fixture.write_file("notes.txt", "one\ntwo\nthree\n");

        let value = execute(
            &fixture.context(),
            json!({ "path": "notes.txt", "startLine": 2, "maxLines": 1 }),
        );

        assert_eq!(value["content"], "two\n");
        assert_eq!(value["startLine"], 2);
        assert_eq!(value["endLine"], 2);
        assert_eq!(value["nextStartLine"], 3);
        assert_eq!(value["truncatedReason"], "line_limit");
    }

    #[test]
    fn default_read_returns_a_thousand_line_source_in_full_when_it_fits() {
        let fixture = TestWorkspace::new();
        let content = (1..=1_000)
            .map(|line| format!("def function_{line}(): return {line}\n"))
            .collect::<String>();
        fixture.write_file("module.py", &content);

        let value = execute(&fixture.context(), json!({ "path": "module.py" }));

        assert_eq!(value["content"], content);
        assert_eq!(value["totalLines"], 1_000);
        assert_eq!(value["truncated"], false);
        assert_eq!(value["revision"], content_revision(content.as_bytes()));
    }

    #[test]
    fn explicit_line_strategy_can_exceed_the_old_two_thousand_line_limit() {
        let fixture = TestWorkspace::new();
        let content = "x\n".repeat(3_001);
        fixture.write_file("many-lines.txt", &content);

        let value = execute(
            &fixture.context(),
            json!({ "path": "many-lines.txt", "maxLines": 3_000 }),
        );

        assert_eq!(value["content"].as_str().unwrap(), "x\n".repeat(3_000));
        assert_eq!(value["nextStartLine"], 3_001);
        assert_eq!(value["truncatedReason"], "line_limit");
    }

    #[test]
    fn utf8_and_revision_remain_correct_across_stream_buffer_boundaries() {
        let fixture = TestWorkspace::new();
        let content = format!("{}甲\n", "a".repeat(STREAM_BUFFER_BYTES - 1));
        fixture.write_file("boundary.txt", &content);

        let value = execute(&fixture.context(), json!({ "path": "boundary.txt" }));

        assert_eq!(value["content"], content);
        assert_eq!(value["revision"], content_revision(content.as_bytes()));
        assert_eq!(value["truncated"], false);
    }

    #[test]
    fn token_budget_returns_a_lossless_continuation_cursor() {
        let fixture = TestWorkspace::new();
        let content = "one\ntwo\nthree\nfour\nfive\nsix\n";
        fixture.write_file("notes.txt", content);
        let context = fixture
            .context()
            .with_text_output_budget(ContextTextBudget::heuristic(6));

        let first = execute(&context, json!({ "path": "notes.txt" }));
        assert_eq!(first["truncated"], true);
        assert_eq!(first["truncatedReason"], "output_budget");
        let next_start_byte = first["nextStartByte"].as_u64().unwrap();
        let second = execute(
            &context,
            json!({
                "path": "notes.txt",
                "startByte": next_start_byte,
                "expectedRevision": first["revision"]
            }),
        );

        assert_eq!(
            format!(
                "{}{}",
                first["content"].as_str().unwrap(),
                second["content"].as_str().unwrap()
            ),
            content
        );
        assert_eq!(second["truncated"], false);
    }

    #[test]
    fn continuation_cursor_and_revision_are_an_exact_pair() {
        let missing_revision: ReadFileArgs = serde_json::from_value(json!({
            "path": "notes.txt",
            "startByte": 10
        }))
        .unwrap();
        assert!(missing_revision.validate().is_err());

        let revision_without_cursor: ReadFileArgs = serde_json::from_value(json!({
            "path": "notes.txt",
            "expectedRevision": "sha256:example"
        }))
        .unwrap();
        assert!(revision_without_cursor.validate().is_err());
    }

    #[test]
    fn model_projection_exposes_an_executable_continuation() {
        let fixture = TestWorkspace::new();
        fixture.write_file(
            "notes.txt",
            &"one\ntwo\nthree\nfour\nfive\nsix\n".repeat(1_000),
        );
        let context = fixture
            .context()
            .with_text_output_budget(ContextTextBudget::heuristic(6));
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-model-continuation".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "notes.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let raw = registry.execute(&context, &call);
        let next_start_byte = raw.result.as_ref().unwrap()["nextStartByte"]
            .as_u64()
            .unwrap();
        let model = registry.model_projection(&raw);
        let projected = model.result.unwrap();

        assert_eq!(projected["truncated"], true);
        assert_eq!(projected["endByteExclusive"], next_start_byte);
        assert_eq!(projected["continueWith"]["tool"], "read_file");
        assert_eq!(
            projected["continueWith"]["args"],
            json!({
                "path": projected["path"],
                "startByte": next_start_byte,
                "expectedRevision": projected["revision"]
            })
        );
    }

    #[test]
    fn escaped_unicode_pages_fit_the_complete_10k_message_and_continue_without_a_gap() {
        let fixture = TestWorkspace::new();
        let content = "\"\\\\\u{0001}天地🙂\n".repeat(20_000);
        fixture.write_file("escaped.txt", &content);
        let budget = ContextTextBudget::heuristic(10_000);
        let context = fixture.context().with_text_output_budget(budget);
        let registry = ToolRegistry::defaults_with_search(None);
        let first_call = AgentToolCall {
            id: "call-read-escaped-1".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "escaped.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let first_raw = registry.execute(&context, &first_call);
        assert!(first_raw.ok, "{:?}", first_raw.error);
        assert!(
            !crate::tools::tool_result_truncated_at_source(&first_raw),
            "a losslessly pageable read is not an unrecoverable source truncation"
        );
        let first_model = registry.model_projection(&first_raw);
        let gate =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[])
                .model_tool_result_gate();
        assert!(!gate.would_truncate(&first_call.id, false, &first_model));
        let first = first_raw.result.as_ref().unwrap();
        let first_text = first["content"].as_str().unwrap();
        let next_byte = first["nextStartByte"].as_u64().unwrap();
        assert_eq!(next_byte, u64::try_from(first_text.len()).unwrap());
        assert_eq!(
            &content.as_bytes()[..first_text.len()],
            first_text.as_bytes()
        );

        let second_call = AgentToolCall {
            id: "call-read-escaped-2".to_string(),
            tool: "read_file".to_string(),
            args: first_model.result.as_ref().unwrap()["continueWith"]["args"].clone(),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let second_raw = registry.execute(&context, &second_call);
        assert!(second_raw.ok, "{:?}", second_raw.error);
        let second = second_raw.result.as_ref().unwrap();
        let second_text = second["content"].as_str().unwrap();
        let start = usize::try_from(next_byte).unwrap();
        assert_eq!(
            &content.as_bytes()[start..start + second_text.len()],
            second_text.as_bytes()
        );
    }

    #[test]
    fn continuation_revision_rejects_a_changed_file() {
        let fixture = TestWorkspace::new();
        fixture.write_file("changing.txt", &"old\n".repeat(20_000));
        let context = fixture
            .context()
            .with_text_output_budget(ContextTextBudget::heuristic(1_000));
        let registry = ToolRegistry::defaults_with_search(None);
        let first_call = AgentToolCall {
            id: "call-read-changing-1".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "changing.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let first = registry.execute(&context, &first_call);
        assert!(first.ok, "{:?}", first.error);
        let projected = registry.model_projection(&first);
        fixture.write_file("changing.txt", &"new\n".repeat(20_000));
        let continuation = AgentToolCall {
            id: "call-read-changing-2".to_string(),
            tool: "read_file".to_string(),
            args: projected.result.unwrap()["continueWith"]["args"].clone(),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &continuation);

        assert!(!result.ok);
        assert!(result.error.unwrap().contains("revision 已变化"));
    }

    #[test]
    fn files_larger_than_the_previous_limit_are_paginated_not_rejected() {
        let fixture = TestWorkspace::new();
        let content = "a\n".repeat(300_000);
        fixture.write_file("large.txt", &content);

        let value = execute(&fixture.context(), json!({ "path": "large.txt" }));

        assert_eq!(value["totalBytes"], content.len());
        assert_eq!(value["totalLines"], 300_000);
        assert_eq!(value["truncated"], true);
        assert!(value["content"].as_str().unwrap().len() < content.len());
    }

    #[test]
    fn a_single_oversized_line_still_makes_cursor_progress() {
        let fixture = TestWorkspace::new();
        let content = "x".repeat(10_000);
        fixture.write_file("minified.js", &content);
        let context = fixture
            .context()
            .with_text_output_budget(ContextTextBudget::heuristic(8));

        let value = execute(&context, json!({ "path": "minified.js" }));

        let returned = value["content"].as_str().unwrap();
        assert!(!returned.is_empty());
        assert!(returned.len() < content.len());
        assert!(value["nextStartByte"].as_u64().unwrap() > 0);
    }

    #[test]
    fn continuation_cursor_must_be_on_a_utf8_boundary() {
        let fixture = TestWorkspace::new();
        fixture.write_file("unicode.txt", "甲乙丙");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-invalid-cursor".to_string(),
            tool: "read_file".to_string(),
            args: json!({
                "path": "unicode.txt",
                "startByte": 1,
                "expectedRevision": crate::content_revision("天地".as_bytes())
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(!result.ok);
        assert!(result.error.unwrap().contains("UTF-8 字符中间"));
    }

    #[test]
    fn invalid_utf8_is_rejected_without_loading_the_file_as_a_string() {
        let fixture = TestWorkspace::new();
        fixture.write_bytes("binary.txt", &[b'a', 0xff, b'b']);
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-invalid-utf8".to_string(),
            tool: "read_file".to_string(),
            args: json!({ "path": "binary.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(!result.ok);
        assert!(result.error.unwrap().contains("只支持 UTF-8 文本"));
    }

    #[cfg(unix)]
    #[test]
    fn leaf_symlink_is_rejected_without_following_it() {
        use std::os::unix::fs::symlink;

        let fixture = TestWorkspace::new();
        fixture.write_file("target.txt", "must not be exposed\n");
        symlink(
            fixture.root.join("target.txt"),
            fixture.root.join("link.txt"),
        )
        .unwrap();
        let registry = ToolRegistry::defaults_with_search(None);
        let result = registry.execute(
            &fixture.context(),
            &AgentToolCall {
                id: "call-read-leaf-symlink".to_string(),
                tool: "read_file".to_string(),
                args: json!({ "path": "link.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        );

        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some(SYMLINK_FORBIDDEN_MESSAGE));
        assert!(result.result.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn leaf_symlink_swap_between_metadata_and_open_is_rejected_safely() {
        use std::os::unix::fs::symlink;

        let fixture = TestWorkspace::new();
        fixture.write_file("target.txt", "must not be exposed\n");
        fixture.write_file("victim.txt", "safe contents\n");
        let target = fixture.root.join("target.txt");
        let victim = fixture.root.join("victim.txt");

        let error = open_regular_text_file_with_hook(&victim, "victim.txt", || {
            fs::remove_file(&victim).unwrap();
            symlink(&target, &victim).unwrap();
        })
        .err()
        .expect("a raced leaf symlink must fail");

        assert_eq!(error.to_string(), SYMLINK_FORBIDDEN_MESSAGE);
        assert!(!error.to_string().contains("Too many symbolic links"));
        assert!(!error.to_string().contains("ELOOP"));
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlink_swap_cannot_expose_content_or_issue_an_observation() {
        use std::os::unix::fs::symlink;

        let fixture = TestWorkspace::new();
        fixture.write_file("inside/victim.txt", "authorized contents\n");
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(
            outside.path().join("victim.txt"),
            "outside secret sentinel\n",
        )
        .unwrap();
        let inside = fixture.root.join("inside");
        let displaced = fixture.root.join("inside-displaced");
        let context = fixture
            .context()
            .with_tool_call_id("call-read-ancestor-swap".to_string());

        let error =
            execute_read_file_with_hook(&context, json!({ "path": "inside/victim.txt" }), || {
                fs::rename(&inside, &displaced).unwrap();
                symlink(outside.path(), &inside).unwrap();
            })
            .expect_err("a swapped ancestor must fail before returning content or an observation");

        assert_eq!(error.to_string(), SYMLINK_FORBIDDEN_MESSAGE);
        assert!(!error.to_string().contains("outside secret sentinel"));
    }

    fn execute(context: &ToolExecutionContext, args: Value) -> Value {
        execute_with_call_id(context, "call-read-file", args)
    }

    fn execute_with_call_id(context: &ToolExecutionContext, call_id: &str, args: Value) -> Value {
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: call_id.to_string(),
            tool: "read_file".to_string(),
            args,
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let result = registry.execute(context, &call);
        assert!(result.ok, "{:?}", result.error);
        result.result.unwrap()
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("my-copilot-agent-test-read-file-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_file(&self, path: &str, content: &str) {
            self.write_bytes(path, content.as_bytes());
        }

        fn write_bytes(&self, path: &str, content: &[u8]) {
            let file_path = self.root.join(path);
            fs::create_dir_all(file_path.parent().unwrap()).unwrap();
            fs::write(file_path, content).unwrap();
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
                conversation_id: Some("read-file-test-conversation".to_string()),
                project_id: None,
                workspace: Some(AgentWorkspaceContext {
                    project_id: None,
                    display_name: Some("test".to_string()),
                    root_path: Some(self.root.to_string_lossy().to_string()),
                }),
                attachment_library: None,
                permissions: Default::default(),
            }))
            .with_runtime_services("read-file-test-run".to_string(), None)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
