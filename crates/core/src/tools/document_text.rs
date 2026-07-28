// Document path validation and text extraction helpers for Office-like files.
use super::{
    ToolExecutionContext, MAX_DOCUMENT_FILE_BYTES, MAX_DOCUMENT_XML_ENTRY_BYTES,
    MAX_DOCUMENT_XML_TOTAL_BYTES,
};
use crate::cancellation::AgentCancellationToken;
use crate::exact_capture::{bounded_utf8_prefix, EXACT_TEXT_CAPTURE_STOP_REASON};
use crate::protocol::{AgentError, AgentResult};
use serde_json::{json, Value};
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

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

/// Adds one completely extracted, security-sanitized document body to a Tool result.
///
/// Length limits that protect file parsing and archive capture must be enforced before this
/// boundary. Model, event, trace, and checkpoint limits are consumer projections and must never
/// change the body supplied here.
pub(super) fn complete_document_text_result(mut metadata: Value, text: String) -> Value {
    let original_bytes = text.len() as u64;
    let original_chars = text.chars().count() as u64;
    let (captured, omitted_bytes) = bounded_utf8_prefix(&text);
    let captured_bytes = captured.len() as u64;
    let captured_chars = captured.chars().count() as u64;
    let truncated_at_source = omitted_bytes > 0;
    let captured_end = captured.len();
    let text = if truncated_at_source {
        text[..captured_end].to_string()
    } else {
        text
    };
    let object = metadata
        .as_object_mut()
        .expect("document result metadata must be an object");
    object.insert("text".to_string(), Value::String(text));
    object.insert("originalBytes".to_string(), json!(original_bytes));
    object.insert("originalChars".to_string(), json!(original_chars));
    object.insert("capturedBytes".to_string(), json!(captured_bytes));
    object.insert("capturedChars".to_string(), json!(captured_chars));
    object.insert("omittedBytes".to_string(), json!(omitted_bytes));
    object.insert(
        "sourceStopReason".to_string(),
        json!(truncated_at_source.then_some(EXACT_TEXT_CAPTURE_STOP_REASON)),
    );
    object.insert("truncatedAtSource".to_string(), json!(truncated_at_source));
    // Keep the legacy field for Event/Trace consumers. It now describes source completeness only;
    // the central Model Result Gate adds its own `truncated` marker when it bounds a projection.
    object.insert("truncated".to_string(), json!(truncated_at_source));
    metadata
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
    let mut total_xml_bytes = 0;

    for index in 0..archive.len() {
        cancellation_token.check()?;
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AgentError::new(format!("读取 OOXML 条目失败：{error}")))?;
        let name = entry.name().to_string();
        if !include_entry(&name) {
            continue;
        }
        reserve_zip_xml_entry(&name, entry.size(), &mut total_xml_bytes)?;

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

pub(super) fn reserve_zip_xml_entry(
    name: &str,
    size: u64,
    total_bytes: &mut u64,
) -> AgentResult<()> {
    if size > MAX_DOCUMENT_XML_ENTRY_BYTES {
        return Err(AgentError::new(format!(
            "文档 XML 条目过大：{name}（{size} bytes）。"
        )));
    }
    let next_total = total_bytes.saturating_add(size);
    if next_total > MAX_DOCUMENT_XML_TOTAL_BYTES {
        return Err(AgentError::new(format!(
            "文档解压后的 XML 总量超过 {} bytes 限制。",
            MAX_DOCUMENT_XML_TOTAL_BYTES
        )));
    }
    *total_bytes = next_total;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;

    #[test]
    fn complete_document_text_result_never_applies_a_model_context_limit() {
        let text = "正文".repeat(80_000);
        let result = complete_document_text_result(json!({ "format": "test" }), text.clone());

        assert_eq!(result["text"], text);
        assert_eq!(result["originalBytes"], text.len() as u64);
        assert_eq!(result["originalChars"], text.chars().count() as u64);
        assert_eq!(result["capturedBytes"], text.len() as u64);
        assert_eq!(result["capturedChars"], text.chars().count() as u64);
        assert_eq!(result["omittedBytes"], 0);
        assert!(result["sourceStopReason"].is_null());
        assert_eq!(result["truncatedAtSource"], false);
        assert_eq!(result["truncated"], false);
    }

    #[test]
    fn document_tool_schemas_keep_only_an_unbounded_compatibility_hint() {
        let registry = ToolRegistry::defaults_with_search(None);
        for tool in [
            "read_pdf",
            "read_word",
            "read_spreadsheet",
            "read_presentation",
        ] {
            let definition = registry
                .definitions()
                .into_iter()
                .find(|definition| definition.name == tool)
                .unwrap();
            let max_chars = &definition.input_schema["properties"]["maxChars"];
            assert!(
                max_chars["maximum"].is_null(),
                "{tool} must not turn the compatibility hint into a source limit"
            );
            assert!(max_chars["description"]
                .as_str()
                .unwrap()
                .contains("shared 10K gate"));
        }
    }

    #[test]
    fn document_zip_safety_limits_remain_hard_parser_rejections() {
        let mut total = MAX_DOCUMENT_XML_TOTAL_BYTES;
        let error = reserve_zip_xml_entry("word/document.xml", 1, &mut total).unwrap_err();
        assert!(error.to_string().contains("文档解压后的 XML 总量超过"));

        let mut total = 0;
        let error = reserve_zip_xml_entry(
            "word/document.xml",
            MAX_DOCUMENT_XML_ENTRY_BYTES + 1,
            &mut total,
        )
        .unwrap_err();
        assert!(error.to_string().contains("文档 XML 条目过大"));
    }
}
