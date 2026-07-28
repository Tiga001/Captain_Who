use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use crate::revision::{compose_content_revision, ContentRevisionHasher};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::time::SystemTime;

const STREAM_BUFFER_BYTES: usize = 64 * 1024;

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
            description: "Read an authorized UTF-8 text file. Paths may be workspace-relative, absolute, use a supported system alias, or reference @attachments; the current read permission is enforced at execution time. Without a range it returns the complete file when the model-aware output budget permits; larger files return a lossless continuation cursor instead of failing."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative path, absolute local path, @home/@desktop/@documents/@downloads, or an exact @attachments/... readPath. Availability depends on the current read permission." },
                    "startLine": { "type": "integer", "minimum": 1, "description": "Optional 1-based first line. Omit to start at the beginning." },
                    "startByte": { "type": "integer", "minimum": 0, "description": "Continuation cursor. Pass nextStartByte from a previous truncated result; do not combine with startLine." },
                    "maxLines": { "type": "integer", "minimum": 1, "description": "Optional soft strategy bound. There is no fixed maximum; the output token budget still applies." }
                },
                "required": ["path"]
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args: ReadFileArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_file 参数无效：{error}")))?;
        args.validate()?;
        let path = args.path()?;
        let file_path = context.resolve_existing_path(path)?;
        let mut file = File::open(&file_path)
            .map_err(|error| AgentError::new(format!("打开文件失败：{error}")))?;
        let initial_metadata = file
            .metadata()
            .map_err(|error| AgentError::new(format!("读取文件元数据失败：{error}")))?;
        if !initial_metadata.is_file() {
            return Err(AgentError::new("read_file 只能读取文件。"));
        }

        let requested_start_line = u64::try_from(args.start_line.unwrap_or(1)).unwrap_or(u64::MAX);
        let inspection = inspect_text_file(
            &mut file,
            context,
            &initial_metadata,
            requested_start_line,
            args.start_byte,
        )?;
        let fragment = read_fragment(
            &mut file,
            context,
            &inspection,
            args.max_lines
                .map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
        )?;
        ensure_file_unchanged(&file, &inspection.identity)?;

        let positions = measure_positions(
            inspection.start.line,
            inspection.start.column,
            &fragment.content,
        );
        let next_byte = inspection
            .start
            .byte
            .saturating_add(u64::try_from(fragment.content.len()).unwrap_or(u64::MAX));
        let truncated = next_byte < inspection.total_bytes;
        let truncated_reason = truncated.then(|| fragment.stop_reason.as_str());

        Ok(json!({
            "path": context.display_path(path, &file_path)?,
            "revision": inspection.revision,
            "startLine": inspection.start.line,
            "startColumn": inspection.start.column,
            "startByte": inspection.start.byte,
            "endLine": positions.end_line,
            "endColumn": positions.end_column,
            "endByteExclusive": next_byte,
            "totalLines": inspection.total_lines,
            "totalBytes": inspection.total_bytes,
            "returnedBytes": fragment.content.len(),
            "estimatedContentTokens": context.text_output_budget().estimate(&fragment.content),
            "outputTokenBudget": context.text_output_budget().max_tokens(),
            "truncated": truncated,
            "truncatedReason": truncated_reason,
            "nextStartByte": truncated.then_some(next_byte),
            "nextStartLine": truncated.then_some(positions.next_line),
            "nextStartColumn": truncated.then_some(positions.next_column),
            "content": fragment.content
        }))
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = super::model_projection::retain_fields(
            result.result.as_ref(),
            &[
                "path",
                "startLine",
                "endLine",
                "totalLines",
                "totalBytes",
                "content",
                "truncated",
                "truncatedReason",
                "nextStartByte",
                "nextStartLine",
            ],
        );
        super::model_projection::compact_model_result(result, projected)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadFileArgs {
    path: Option<String>,
    file_path: Option<String>,
    start_line: Option<usize>,
    start_byte: Option<u64>,
    max_lines: Option<usize>,
}

impl ReadFileArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .or(self.file_path.as_deref())
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
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct FileCursor {
    byte: u64,
    line: u64,
    column: u64,
}

#[derive(Debug, Clone)]
struct FileIdentity {
    length: u64,
    modified: Option<SystemTime>,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        }
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
    let budget = context.text_output_budget();
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

        if !budget.fits(&content) {
            let mut fitting = budget.fitting_prefix_len(&content);
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
    if current.len() != expected.length
        || expected
            .modified
            .zip(current.modified().ok())
            .is_some_and(|(expected, current)| expected != current)
    {
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
    use super::STREAM_BUFFER_BYTES;
    use crate::context::ContextTextBudget;
    use crate::protocol::{
        AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
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
        assert!(definition.input_schema.get("anyOf").is_none());
        assert!(definition.input_schema["properties"]
            .get("filePath")
            .is_none());
    }

    #[test]
    fn runtime_still_accepts_the_legacy_file_path_alias() {
        let fixture = TestWorkspace::new();
        fixture.write_file("legacy.txt", "legacy alias");

        let value = execute(&fixture.context(), json!({ "filePath": "legacy.txt" }));

        assert_eq!(value["content"], "legacy alias");
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
            json!({ "path": "notes.txt", "startByte": next_start_byte }),
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
            args: json!({ "path": "unicode.txt", "startByte": 1 }),
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

    fn execute(context: &ToolExecutionContext, args: Value) -> Value {
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-read-file".to_string(),
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
