use super::MAX_OUTPUT_BYTES;
use crate::exact_capture::ExactToolResultArchiveFile;
use crate::exact_capture::{EXACT_TEXT_CAPTURE_MAX_BYTES, EXACT_TEXT_CAPTURE_STOP_REASON};
use crate::protocol::AgentToolResult;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use tempfile::NamedTempFile;

use serde::{Deserialize, Serialize};

/// Product-wide safety ceiling for the combined stdout and stderr captured from one process.
///
/// This is a source-safety limit, not a model-context budget. The complete capture below this
/// ceiling is available to Exact History; the model still goes through the fixed 10K result gate.
#[derive(Debug, Clone, Copy)]
pub struct ProcessOutputCapturePolicy {
    preview_bytes: usize,
    max_capture_bytes: u64,
}

impl ProcessOutputCapturePolicy {
    pub fn process_default() -> Self {
        Self {
            preview_bytes: MAX_OUTPUT_BYTES,
            max_capture_bytes: EXACT_TEXT_CAPTURE_MAX_BYTES,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_limits(preview_bytes: usize, max_capture_bytes: u64) -> Self {
        Self {
            preview_bytes,
            max_capture_bytes,
        }
    }

    pub fn preview_bytes(self) -> usize {
        self.preview_bytes
    }

    pub fn max_capture_bytes(self) -> u64 {
        self.max_capture_bytes
    }
}

#[derive(Debug, Clone)]
pub struct ProcessOutputCaptureBudget {
    remaining: Arc<AtomicU64>,
}

impl ProcessOutputCaptureBudget {
    pub fn new(max_capture_bytes: u64) -> Self {
        Self {
            remaining: Arc::new(AtomicU64::new(max_capture_bytes)),
        }
    }

    fn reserve(&self, requested: usize) -> usize {
        let requested = u64::try_from(requested).unwrap_or(u64::MAX);
        let mut remaining = self.remaining.load(Ordering::Acquire);
        loop {
            let granted = remaining.min(requested);
            match self.remaining.compare_exchange_weak(
                remaining,
                remaining - granted,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return usize::try_from(granted).unwrap_or(usize::MAX),
                Err(current) => remaining = current,
            }
        }
    }
}

#[derive(Clone)]
pub struct ProcessOutputSpool {
    file: Option<Arc<NamedTempFile>>,
    redactions: Arc<Vec<(Vec<u8>, Vec<u8>)>>,
}

impl ProcessOutputSpool {
    fn new(file: NamedTempFile) -> Self {
        Self {
            file: Some(Arc::new(file)),
            redactions: Arc::new(Vec::new()),
        }
    }

    pub fn is_present(&self) -> bool {
        self.file.is_some()
    }

    pub fn reopen(&self) -> std::io::Result<File> {
        self.file
            .as_ref()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "process output spool is absent",
                )
            })?
            .reopen()
    }

    pub fn with_redactions(&self, replacements: &[(String, String)]) -> Self {
        let mut redactions = self.redactions.as_ref().clone();
        redactions.extend(
            replacements
                .iter()
                .filter(|(source, _)| !source.is_empty())
                .map(|(source, replacement)| {
                    (source.as_bytes().to_vec(), replacement.as_bytes().to_vec())
                }),
        );
        Self {
            file: self.file.clone(),
            redactions: Arc::new(redactions),
        }
    }

    fn redactions(&self) -> &[(Vec<u8>, Vec<u8>)] {
        self.redactions.as_ref()
    }
}

impl Default for ProcessOutputSpool {
    fn default() -> Self {
        Self {
            file: None,
            redactions: Arc::new(Vec::new()),
        }
    }
}

impl std::fmt::Debug for ProcessOutputSpool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessOutputSpool")
            .field("present", &self.is_present())
            .field("redactions", &self.redactions.len())
            .finish_non_exhaustive()
    }
}

// A spool is a backend-only transport for content already described by the public capture
// metadata. It deliberately does not participate in semantic receipt equality.
impl PartialEq for ProcessOutputSpool {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ProcessOutputSpool {}

/// Consumer-safe receipt for one process' shared stdout/stderr capture budget.
///
/// `truncated_at_source` means bytes were discarded at the safety boundary. A preview cut is
/// independently represented by the per-stream `*PreviewTruncated` fields and remains exactly
/// recoverable from the corresponding backend-only spool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOutputCaptureMetadata {
    pub original_bytes: u64,
    pub captured_bytes: u64,
    pub omitted_bytes: u64,
    pub truncated_at_source: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub stdout_original_bytes: u64,
    pub stdout_captured_bytes: u64,
    pub stdout_omitted_bytes: u64,
    pub stdout_preview_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_stop_reason: Option<String>,
    pub stderr_original_bytes: u64,
    pub stderr_captured_bytes: u64,
    pub stderr_omitted_bytes: u64,
    pub stderr_preview_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_stop_reason: Option<String>,
}

impl ProcessOutputCaptureMetadata {
    pub fn from_streams(stdout: &CapturedProcessOutput, stderr: &CapturedProcessOutput) -> Self {
        let original_bytes = stdout
            .original_bytes()
            .saturating_add(stderr.original_bytes());
        let captured_bytes = stdout
            .captured_bytes()
            .saturating_add(stderr.captured_bytes());
        let omitted_bytes = stdout
            .omitted_bytes()
            .saturating_add(stderr.omitted_bytes());
        let truncated_at_source = omitted_bytes > 0;
        Self {
            original_bytes,
            captured_bytes,
            omitted_bytes,
            truncated_at_source,
            stop_reason: truncated_at_source.then(|| EXACT_TEXT_CAPTURE_STOP_REASON.to_string()),
            stdout_original_bytes: stdout.original_bytes(),
            stdout_captured_bytes: stdout.captured_bytes(),
            stdout_omitted_bytes: stdout.omitted_bytes(),
            stdout_preview_truncated: stdout.preview_truncated(),
            stdout_stop_reason: stdout.stop_reason().map(str::to_string),
            stderr_original_bytes: stderr.original_bytes(),
            stderr_captured_bytes: stderr.captured_bytes(),
            stderr_omitted_bytes: stderr.omitted_bytes(),
            stderr_preview_truncated: stderr.preview_truncated(),
            stderr_stop_reason: stderr.stop_reason().map(str::to_string),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessOutputSpoolSubstitution {
    pub field: &'static str,
    pub spool: ProcessOutputSpool,
}

pub fn process_output_spool_substitutions(
    stdout: &ProcessOutputSpool,
    stderr: &ProcessOutputSpool,
) -> Vec<ProcessOutputSpoolSubstitution> {
    [("stdout", stdout), ("stderr", stderr)]
        .into_iter()
        .filter(|(_, spool)| spool.is_present())
        .map(|(field, spool)| ProcessOutputSpoolSubstitution {
            field,
            spool: spool.clone(),
        })
        .collect()
}

/// Materializes one valid exact Tool Result JSON file while replacing only `/result/stdout` and
/// `/result/stderr` with their complete safely captured spools.
///
/// Neither stream is loaded contiguously into memory. Invalid provider bytes are cleaned with the
/// same UTF-8-lossy semantics as the in-memory preview, and JSON escaping happens incrementally.
pub fn materialize_process_tool_result_archive(
    result: &AgentToolResult,
    substitutions: &[ProcessOutputSpoolSubstitution],
) -> std::io::Result<Option<ExactToolResultArchiveFile>> {
    if substitutions.is_empty() {
        return Ok(None);
    }
    let mut file = NamedTempFile::new()?;
    write_tool_result_with_process_spools(&mut file, result, substitutions)?;
    file.flush()?;
    Ok(Some(ExactToolResultArchiveFile::new(file)))
}

fn write_tool_result_with_process_spools(
    writer: &mut impl Write,
    result: &AgentToolResult,
    substitutions: &[ProcessOutputSpoolSubstitution],
) -> std::io::Result<()> {
    let mut stdout = None;
    let mut stderr = None;
    for substitution in substitutions {
        match substitution.field {
            "stdout" if stdout.is_none() => stdout = Some(&substitution.spool),
            "stderr" if stderr.is_none() => stderr = Some(&substitution.spool),
            "stdout" | "stderr" => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "duplicate process output spool substitution for {}",
                        substitution.field
                    ),
                ));
            }
            field => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unsupported process output spool substitution field {field}"),
                ));
            }
        }
    }

    writer.write_all(b"{\"callId\":")?;
    serde_json::to_writer(&mut *writer, &result.call_id).map_err(json_io_error)?;
    writer.write_all(b",\"tool\":")?;
    serde_json::to_writer(&mut *writer, &result.tool).map_err(json_io_error)?;
    writer.write_all(b",\"ok\":")?;
    serde_json::to_writer(&mut *writer, &result.ok).map_err(json_io_error)?;
    if let Some(value) = &result.result {
        writer.write_all(b",\"result\":")?;
        write_root_result_with_process_spools(writer, value, stdout, stderr)?;
    } else if stdout.is_some() || stderr.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "process output spools require a Tool Result payload",
        ));
    }
    if let Some(error) = &result.error {
        writer.write_all(b",\"error\":")?;
        serde_json::to_writer(&mut *writer, error).map_err(json_io_error)?;
    }
    writer.write_all(b"}")
}

fn write_root_result_with_process_spools(
    writer: &mut impl Write,
    value: &serde_json::Value,
    stdout: Option<&ProcessOutputSpool>,
    stderr: Option<&ProcessOutputSpool>,
) -> std::io::Result<()> {
    let Some(object) = value.as_object() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "process output spool substitutions require an object at /result",
        ));
    };
    if stdout.is_some()
        && !object
            .get("stdout")
            .is_some_and(serde_json::Value::is_string)
    {
        return Err(missing_process_stream("stdout"));
    }
    if stderr.is_some()
        && !object
            .get("stderr")
            .is_some_and(serde_json::Value::is_string)
    {
        return Err(missing_process_stream("stderr"));
    }

    writer.write_all(b"{")?;
    for (index, (field, field_value)) in object.iter().enumerate() {
        if index > 0 {
            writer.write_all(b",")?;
        }
        serde_json::to_writer(&mut *writer, field).map_err(json_io_error)?;
        writer.write_all(b":")?;
        match field.as_str() {
            "stdout" if stdout.is_some() => {
                write_json_string_from_spool(writer, stdout.expect("checked above"))?
            }
            "stderr" if stderr.is_some() => {
                write_json_string_from_spool(writer, stderr.expect("checked above"))?
            }
            _ => serde_json::to_writer(&mut *writer, field_value).map_err(json_io_error)?,
        }
    }
    writer.write_all(b"}")
}

fn missing_process_stream(field: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("process output spool has no string target at /result/{field}"),
    )
}

fn json_io_error(error: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}

fn write_json_string_from_spool(
    writer: &mut impl Write,
    spool: &ProcessOutputSpool,
) -> std::io::Result<()> {
    let mut reader = spool.reopen()?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut source_pending = Vec::<u8>::with_capacity(buffer.len() * 2);
    let mut utf8_pending = Vec::<u8>::with_capacity(buffer.len() + 4);
    writer.write_all(b"\"")?;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        source_pending.extend_from_slice(&buffer[..read]);
        write_redacted_json_string_bytes(
            writer,
            &mut source_pending,
            &mut utf8_pending,
            spool.redactions(),
            false,
        )?;
    }
    write_redacted_json_string_bytes(
        writer,
        &mut source_pending,
        &mut utf8_pending,
        spool.redactions(),
        true,
    )?;
    write_decoded_json_string_bytes(writer, &mut utf8_pending, true)?;
    writer.write_all(b"\"")
}

fn write_redacted_json_string_bytes(
    writer: &mut impl Write,
    source_pending: &mut Vec<u8>,
    utf8_pending: &mut Vec<u8>,
    redactions: &[(Vec<u8>, Vec<u8>)],
    end_of_stream: bool,
) -> std::io::Result<()> {
    if redactions.is_empty() {
        utf8_pending.extend_from_slice(source_pending);
        source_pending.clear();
        return write_decoded_json_string_bytes(writer, utf8_pending, end_of_stream);
    }
    let overlap = redactions
        .iter()
        .map(|(source, _)| source.len())
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let process_before = if end_of_stream {
        source_pending.len()
    } else {
        source_pending.len().saturating_sub(overlap)
    };
    let mut cursor = 0;
    while cursor < process_before {
        let Some((start, source_len, replacement)) =
            next_redaction(source_pending, cursor, process_before, redactions)
        else {
            utf8_pending.extend_from_slice(&source_pending[cursor..process_before]);
            cursor = process_before;
            break;
        };
        utf8_pending.extend_from_slice(&source_pending[cursor..start]);
        utf8_pending.extend_from_slice(replacement);
        cursor = start.saturating_add(source_len);
    }
    if cursor > 0 {
        source_pending.drain(..cursor);
    }
    write_decoded_json_string_bytes(writer, utf8_pending, false)
}

fn next_redaction<'a>(
    source: &[u8],
    cursor: usize,
    start_before: usize,
    redactions: &'a [(Vec<u8>, Vec<u8>)],
) -> Option<(usize, usize, &'a [u8])> {
    let mut selected: Option<(usize, usize, &'a [u8])> = None;
    for (pattern, replacement) in redactions {
        if pattern.is_empty() || pattern.len() > source.len().saturating_sub(cursor) {
            continue;
        }
        let Some(relative) = source[cursor..]
            .windows(pattern.len())
            .position(|window| window == pattern.as_slice())
        else {
            continue;
        };
        let start = cursor.saturating_add(relative);
        if start >= start_before {
            continue;
        }
        let replace = match selected {
            None => true,
            Some((selected_start, selected_len, _)) => {
                start < selected_start || (start == selected_start && pattern.len() > selected_len)
            }
        };
        if replace {
            selected = Some((start, pattern.len(), replacement.as_slice()));
        }
    }
    selected
}

fn write_decoded_json_string_bytes(
    writer: &mut impl Write,
    pending: &mut Vec<u8>,
    end_of_stream: bool,
) -> std::io::Result<()> {
    let mut consumed = 0;
    loop {
        match std::str::from_utf8(&pending[consumed..]) {
            Ok(text) => {
                write_json_string_contents(writer, text)?;
                consumed = pending.len();
                break;
            }
            Err(error) => {
                let valid_end = consumed.saturating_add(error.valid_up_to());
                if valid_end > consumed {
                    let valid = std::str::from_utf8(&pending[consumed..valid_end])
                        .expect("valid_up_to always identifies valid UTF-8");
                    write_json_string_contents(writer, valid)?;
                }
                match error.error_len() {
                    Some(invalid_bytes) => {
                        write_json_string_contents(writer, "\u{fffd}")?;
                        consumed = valid_end.saturating_add(invalid_bytes);
                    }
                    None if end_of_stream => {
                        write_json_string_contents(writer, "\u{fffd}")?;
                        consumed = pending.len();
                        break;
                    }
                    None => {
                        consumed = valid_end;
                        break;
                    }
                }
            }
        }
        if consumed >= pending.len() {
            break;
        }
    }
    if consumed > 0 {
        pending.drain(..consumed);
    }
    Ok(())
}

fn write_json_string_contents(writer: &mut impl Write, text: &str) -> std::io::Result<()> {
    for character in text.chars() {
        match character {
            '"' => writer.write_all(br#"\""#)?,
            '\\' => writer.write_all(br#"\\"#)?,
            '\u{08}' => writer.write_all(br#"\b"#)?,
            '\u{0c}' => writer.write_all(br#"\f"#)?,
            '\n' => writer.write_all(br#"\n"#)?,
            '\r' => writer.write_all(br#"\r"#)?,
            '\t' => writer.write_all(br#"\t"#)?,
            character if character <= '\u{1f}' => {
                write!(writer, "\\u{:04x}", u32::from(character))?;
            }
            character => {
                let mut encoded = [0_u8; 4];
                writer.write_all(character.encode_utf8(&mut encoded).as_bytes())?;
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct CapturedProcessOutput {
    preview: String,
    original_bytes: u64,
    captured_bytes: u64,
    omitted_bytes: u64,
    preview_truncated: bool,
    spool: ProcessOutputSpool,
}

impl std::fmt::Debug for CapturedProcessOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapturedProcessOutput")
            .field("preview_bytes", &self.preview.len())
            .field("original_bytes", &self.original_bytes)
            .field("captured_bytes", &self.captured_bytes)
            .field("omitted_bytes", &self.omitted_bytes)
            .field("preview_truncated", &self.preview_truncated)
            .finish_non_exhaustive()
    }
}

impl CapturedProcessOutput {
    pub fn preview(&self) -> &str {
        &self.preview
    }

    pub fn original_bytes(&self) -> u64 {
        self.original_bytes
    }

    pub fn captured_bytes(&self) -> u64 {
        self.captured_bytes
    }

    pub fn omitted_bytes(&self) -> u64 {
        self.omitted_bytes
    }

    pub fn preview_truncated(&self) -> bool {
        self.preview_truncated
    }

    pub fn truncated_at_source(&self) -> bool {
        self.omitted_bytes > 0
    }

    pub fn stop_reason(&self) -> Option<&'static str> {
        self.truncated_at_source()
            .then_some(EXACT_TEXT_CAPTURE_STOP_REASON)
    }

    /// Reads the complete safely captured text.
    ///
    /// Process execution itself stays bounded to the preview plus a disk spool. This method is
    /// intentionally called only at the Exact History handoff while the existing archive API
    /// still accepts a textual projection. Replacing that final handoff with a streaming archive
    /// source does not require changing process readers or their safety semantics.
    pub fn read_captured_text(&self) -> std::io::Result<String> {
        let mut file = self.spool.reopen()?;
        file.seek(SeekFrom::Start(0))?;
        let capacity = usize::try_from(self.captured_bytes).unwrap_or(usize::MAX);
        let mut bytes = Vec::with_capacity(capacity);
        file.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    #[cfg(test)]
    fn spool_file(&self) -> std::io::Result<File> {
        self.spool.reopen()
    }

    pub fn spool(&self) -> ProcessOutputSpool {
        self.spool.clone()
    }
}

pub type ProcessOutputCaptureHandle = thread::JoinHandle<std::io::Result<CapturedProcessOutput>>;

/// Drains one process pipe without allowing either memory growth or a full pipe deadlock.
///
/// `budget` is shared by stdout and stderr. After the hard capture ceiling is reached the reader
/// continues draining and counting bytes, but does not retain them. This makes the omitted range
/// explicit instead of silently presenting the retained prefix as complete.
pub fn spawn_process_output_capture<R>(
    mut reader: R,
    budget: ProcessOutputCaptureBudget,
    policy: ProcessOutputCapturePolicy,
) -> ProcessOutputCaptureHandle
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut spool = NamedTempFile::new()?;
        let mut preview = Vec::with_capacity(policy.preview_bytes());
        let mut original_bytes = 0_u64;
        let mut captured_bytes = 0_u64;
        let mut buffer = [0_u8; 16 * 1024];

        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            original_bytes = original_bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            let retained = budget.reserve(read);
            if retained > 0 {
                spool.write_all(&buffer[..retained])?;
                captured_bytes =
                    captured_bytes.saturating_add(u64::try_from(retained).unwrap_or(u64::MAX));
                let preview_remaining = policy.preview_bytes().saturating_sub(preview.len());
                let preview_from_chunk = preview_remaining.min(retained);
                preview.extend_from_slice(&buffer[..preview_from_chunk]);
            }
        }
        spool.flush()?;
        let omitted_bytes = original_bytes.saturating_sub(captured_bytes);
        let preview_truncated = original_bytes > u64::try_from(preview.len()).unwrap_or(u64::MAX);
        Ok(CapturedProcessOutput {
            preview: utf8_boundary_safe_lossy_preview(&preview),
            original_bytes,
            captured_bytes,
            omitted_bytes,
            preview_truncated,
            spool: ProcessOutputSpool::new(spool),
        })
    })
}

fn utf8_boundary_safe_lossy_preview(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(error) if error.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

pub fn join_process_output_capture(
    reader: ProcessOutputCaptureHandle,
    stream: &str,
) -> Result<CapturedProcessOutput, String> {
    reader
        .join()
        .map_err(|_| format!("读取命令 {stream} 的线程异常退出。"))?
        .map_err(|error| format!("读取命令 {stream} 失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn exact_capture_keeps_preview_and_spools_complete_text() {
        let policy = ProcessOutputCapturePolicy::with_limits(5, 32);
        let budget = ProcessOutputCaptureBudget::new(policy.max_capture_bytes());
        let captured = join_process_output_capture(
            spawn_process_output_capture(
                Cursor::new("你好abcdef".as_bytes().to_vec()),
                budget,
                policy,
            ),
            "stdout",
        )
        .unwrap();

        assert_eq!(captured.preview(), "你");
        assert_eq!(captured.original_bytes(), 12);
        assert_eq!(captured.captured_bytes(), 12);
        assert_eq!(captured.omitted_bytes(), 0);
        assert!(captured.preview_truncated());
        assert!(!captured.truncated_at_source());
        assert_eq!(captured.read_captured_text().unwrap(), "你好abcdef");
        assert_eq!(
            captured.spool_file().unwrap().metadata().unwrap().len(),
            captured.captured_bytes()
        );
    }

    #[test]
    fn stdout_and_stderr_share_the_safety_capture_budget() {
        let policy = ProcessOutputCapturePolicy::with_limits(16, 10);
        let budget = ProcessOutputCaptureBudget::new(policy.max_capture_bytes());
        let stdout =
            spawn_process_output_capture(Cursor::new(vec![b'o'; 8]), budget.clone(), policy);
        let stderr = spawn_process_output_capture(Cursor::new(vec![b'e'; 8]), budget, policy);
        let stdout = join_process_output_capture(stdout, "stdout").unwrap();
        let stderr = join_process_output_capture(stderr, "stderr").unwrap();

        assert_eq!(stdout.original_bytes() + stderr.original_bytes(), 16);
        assert_eq!(stdout.captured_bytes() + stderr.captured_bytes(), 10);
        assert_eq!(stdout.omitted_bytes() + stderr.omitted_bytes(), 6);
        assert!(stdout.truncated_at_source() || stderr.truncated_at_source());
        assert!(stdout.stop_reason().is_some() || stderr.stop_reason().is_some());
        let metadata = ProcessOutputCaptureMetadata::from_streams(&stdout, &stderr);
        assert_eq!(metadata.original_bytes, 16);
        assert_eq!(metadata.captured_bytes, 10);
        assert_eq!(metadata.omitted_bytes, 6);
        assert!(metadata.truncated_at_source);
        assert_eq!(
            metadata.stop_reason.as_deref(),
            Some(EXACT_TEXT_CAPTURE_STOP_REASON)
        );
    }

    #[test]
    fn materializer_streams_exact_stdout_and_stderr_into_valid_tool_result_json() {
        let policy = ProcessOutputCapturePolicy::with_limits(4, 1_024);
        let budget = ProcessOutputCaptureBudget::new(policy.max_capture_bytes());
        let stdout = join_process_output_capture(
            spawn_process_output_capture(
                Cursor::new(b"hello\\\"\nworld".to_vec()),
                budget.clone(),
                policy,
            ),
            "stdout",
        )
        .unwrap();
        let stderr = join_process_output_capture(
            spawn_process_output_capture(
                Cursor::new(vec![b'e', 0xff, b'r', b'\n']),
                budget,
                policy,
            ),
            "stderr",
        )
        .unwrap();
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-process".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(serde_json::json!({
                "stdout": stdout.preview(),
                "stderr": stderr.preview(),
                "exitCode": 0,
            })),
            error: None,
        };
        let substitutions = process_output_spool_substitutions(&stdout.spool(), &stderr.spool());
        let archive = materialize_process_tool_result_archive(&raw, &substitutions)
            .unwrap()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_reader(archive.reopen().unwrap()).unwrap();

        assert_eq!(parsed["callId"], "call-process");
        assert_eq!(parsed["result"]["stdout"], "hello\\\"\nworld");
        assert_eq!(parsed["result"]["stderr"], "e\u{fffd}r\n");
        assert_eq!(parsed["result"]["exitCode"], 0);
    }

    #[test]
    fn exact_archive_file_survives_tool_result_clone_but_never_serializes() {
        let policy = ProcessOutputCapturePolicy::with_limits(4, 1_024);
        let budget = ProcessOutputCaptureBudget::new(policy.max_capture_bytes());
        let stdout = join_process_output_capture(
            spawn_process_output_capture(
                Cursor::new(b"complete stdout beyond preview".to_vec()),
                budget,
                policy,
            ),
            "stdout",
        )
        .unwrap();
        let mut raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-clone".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(serde_json::json!({
                "stdout": stdout.preview(),
                "stderr": "",
                "stdoutPreviewTruncated": true,
            })),
            error: None,
        };
        raw.exact_archive_file = materialize_process_tool_result_archive(
            &raw,
            &process_output_spool_substitutions(&stdout.spool(), &ProcessOutputSpool::default()),
        )
        .unwrap();

        let cloned = raw.clone();
        let serialized = serde_json::to_value(&cloned).unwrap();
        assert!(serialized.get("exactArchiveFile").is_none());
        drop(raw);

        let archived: serde_json::Value = serde_json::from_reader(
            cloned
                .exact_archive_file
                .as_ref()
                .unwrap()
                .reopen()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            archived["result"]["stdout"],
            "complete stdout beyond preview"
        );
    }

    #[test]
    fn materializer_rejects_any_pointer_other_than_root_result_streams() {
        let policy = ProcessOutputCapturePolicy::with_limits(4, 32);
        let budget = ProcessOutputCaptureBudget::new(policy.max_capture_bytes());
        let stdout = join_process_output_capture(
            spawn_process_output_capture(Cursor::new(b"nested".to_vec()), budget, policy),
            "stdout",
        )
        .unwrap();
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-nested".to_string(),
            tool: "run_command".to_string(),
            ok: false,
            result: Some(serde_json::json!({
                "execution": {
                    "stdout": stdout.preview(),
                }
            })),
            error: Some("failed".to_string()),
        };
        let substitutions =
            process_output_spool_substitutions(&stdout.spool(), &ProcessOutputSpool::default());
        let error = materialize_process_tool_result_archive(&raw, &substitutions).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("/result/stdout"));
    }

    #[test]
    fn materializer_applies_private_path_redaction_across_reader_chunks() {
        let private_path = "/private/runtime/input";
        let mut output = "x".repeat(64 * 1024 - 5).into_bytes();
        output.extend_from_slice(private_path.as_bytes());
        output.extend_from_slice(b"/file.txt");
        let policy = ProcessOutputCapturePolicy::with_limits(8, 128 * 1024);
        let budget = ProcessOutputCaptureBudget::new(policy.max_capture_bytes());
        let stdout = join_process_output_capture(
            spawn_process_output_capture(Cursor::new(output), budget, policy),
            "stdout",
        )
        .unwrap();
        let stdout_spool = stdout
            .spool()
            .with_redactions(&[(private_path.to_string(), "$INPUT".to_string())]);
        let raw = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-redacted".to_string(),
            tool: "office_document".to_string(),
            ok: true,
            result: Some(serde_json::json!({
                "stdout": stdout.preview(),
                "stderr": "",
            })),
            error: None,
        };
        let substitutions =
            process_output_spool_substitutions(&stdout_spool, &ProcessOutputSpool::default());
        let archive = materialize_process_tool_result_archive(&raw, &substitutions)
            .unwrap()
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_reader(archive.reopen().unwrap()).unwrap();
        let archived_stdout = parsed["result"]["stdout"].as_str().unwrap();
        assert!(!archived_stdout.contains(private_path));
        assert!(archived_stdout.ends_with("$INPUT/file.txt"));
    }
}
