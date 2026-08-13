use super::{
    complete_document_text_result, normalize_text_output, reserve_zip_xml_entry,
    resolve_document_path, AgentTool, NamedText, ToolExecutionContext,
};
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use roxmltree::Node;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::fs::File;
use std::io::Read;

pub(super) struct ReadSpreadsheetTool;

impl AgentTool for ReadSpreadsheetTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::RequiresCapability(super::ToolCapabilityId::application_owned(
            super::OFFICE_SPREADSHEETS_CAPABILITY,
        ))
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_spreadsheet".to_string(),
            description: "Extract text from .xlsx, .csv, or .tsv spreadsheet files in the selected workspace or an @attachments path. Legacy .xls files are not supported.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative .xlsx/.csv/.tsv path or @attachments/... readPath." },
                    "filePath": { "type": "string", "description": "Alias for path." },
                    "maxChars": { "type": "integer", "minimum": 1, "description": "Deprecated soft compatibility hint. Exact History capture is never limited by this value; model output uses the shared 10K gate." }
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
        let args: ReadSpreadsheetArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_spreadsheet 参数无效：{error}")))?;
        let _requested_max_chars = args.max_chars;
        let path = args.path()?;
        let resolved = resolve_document_path(context, path, &["xlsx", "csv", "tsv"])?;
        let cancellation_token = context.cancellation_token();
        let (text, sheet_count, extractor) = match resolved.extension.as_str() {
            "xlsx" => {
                let sheets = read_xlsx_sheets(&resolved.file_path, &cancellation_token)?;
                let text = sheets
                    .iter()
                    .map(|sheet| format!("## {}\n{}", sheet.name, sheet.text))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (text, sheets.len(), "ooxml")
            }
            "csv" | "tsv" => {
                cancellation_token.check()?;
                let text = fs::read_to_string(&resolved.file_path)
                    .map_err(|error| AgentError::new(format!("读取表格文本失败：{error}")))?;
                cancellation_token.check()?;
                (normalize_text_output(&text), 1, "utf8_text")
            }
            _ => unreachable!("extension validated before dispatch"),
        };
        cancellation_token.check()?;

        Ok(complete_document_text_result(
            json!({
                "path": resolved.relative_path,
                "format": resolved.extension,
                "sizeBytes": resolved.size_bytes,
                "extractor": extractor,
                "sheetCount": sheet_count
            }),
            text,
        ))
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        let projected = super::model_projection::retain_fields(
            result.result.as_ref(),
            &[
                "path",
                "format",
                "sheetCount",
                "originalBytes",
                "capturedBytes",
                "omittedBytes",
                "sourceStopReason",
                "truncatedAtSource",
                "truncated",
                "text",
            ],
        );
        super::model_projection::compact_model_result(result, projected)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSpreadsheetArgs {
    path: Option<String>,
    file_path: Option<String>,
    max_chars: Option<usize>,
}

impl ReadSpreadsheetArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .or(self.file_path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("read_spreadsheet.path 不能为空。"))
    }
}

fn read_xlsx_sheets(
    file_path: &std::path::Path,
    cancellation_token: &crate::cancellation::AgentCancellationToken,
) -> AgentResult<Vec<NamedText>> {
    cancellation_token.check()?;
    let file = File::open(file_path)
        .map_err(|error| AgentError::new(format!("打开 XLSX 文件失败：{error}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| AgentError::new(format!("读取 XLSX 压缩包失败：{error}")))?;
    let mut total_xml_bytes = 0;
    let shared_strings =
        read_shared_strings(&mut archive, cancellation_token, &mut total_xml_bytes)?;
    let mut sheets = Vec::new();

    for index in 0..archive.len() {
        cancellation_token.check()?;
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AgentError::new(format!("读取 XLSX 条目失败：{error}")))?;
        let name = entry.name().to_string();
        if !name.starts_with("xl/worksheets/sheet") || !name.ends_with(".xml") {
            continue;
        }
        reserve_zip_xml_entry(&name, entry.size(), &mut total_xml_bytes)?;

        let mut xml = String::new();
        entry
            .read_to_string(&mut xml)
            .map_err(|error| AgentError::new(format!("读取 sheet XML 失败：{error}")))?;
        cancellation_token.check()?;
        let text = extract_sheet_text(&xml, &shared_strings)?;
        if !text.trim().is_empty() {
            sheets.push(NamedText { name, text });
        }
    }

    cancellation_token.check()?;
    sheets.sort_by_key(|sheet| {
        sheet
            .name
            .trim_start_matches("xl/worksheets/sheet")
            .trim_end_matches(".xml")
            .parse::<usize>()
            .unwrap_or(usize::MAX)
    });

    Ok(sheets)
}

fn read_shared_strings(
    archive: &mut zip::ZipArchive<File>,
    cancellation_token: &crate::cancellation::AgentCancellationToken,
    total_xml_bytes: &mut u64,
) -> AgentResult<Vec<String>> {
    cancellation_token.check()?;
    let Ok(mut entry) = archive.by_name("xl/sharedStrings.xml") else {
        return Ok(Vec::new());
    };
    reserve_zip_xml_entry(entry.name(), entry.size(), total_xml_bytes)?;

    let mut xml = String::new();
    entry
        .read_to_string(&mut xml)
        .map_err(|error| AgentError::new(format!("读取 sharedStrings.xml 失败：{error}")))?;
    cancellation_token.check()?;
    let document = roxmltree::Document::parse(&xml)
        .map_err(|error| AgentError::new(format!("解析 sharedStrings.xml 失败：{error}")))?;
    let strings = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "si")
        .map(collect_node_text)
        .collect::<Vec<_>>();

    Ok(strings)
}

fn extract_sheet_text(xml: &str, shared_strings: &[String]) -> AgentResult<String> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| AgentError::new(format!("解析 sheet XML 失败：{error}")))?;
    let mut rows = Vec::new();

    for row in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "row")
    {
        let mut cells = Vec::new();
        for cell in row
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "c")
        {
            let Some(value) = cell_value(cell, shared_strings) else {
                continue;
            };
            if value.trim().is_empty() {
                continue;
            }

            let reference = cell.attribute("r").unwrap_or("");
            if reference.is_empty() {
                cells.push(value);
            } else {
                cells.push(format!("{reference}={value}"));
            }
        }

        if !cells.is_empty() {
            rows.push(cells.join("\t"));
        }
    }

    Ok(rows.join("\n"))
}

fn cell_value(cell: Node<'_, '_>, shared_strings: &[String]) -> Option<String> {
    let cell_type = cell.attribute("t").unwrap_or("");
    if cell_type == "inlineStr" {
        return Some(collect_node_text(cell));
    }

    let raw_value = cell
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "v")
        .and_then(|node| node.text())
        .unwrap_or("")
        .trim();

    if raw_value.is_empty() {
        return None;
    }

    if cell_type == "s" {
        return raw_value
            .parse::<usize>()
            .ok()
            .and_then(|index| shared_strings.get(index).cloned());
    }

    Some(raw_value.to_string())
}

fn collect_node_text(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter_map(|node| node.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::super::{ToolExecutionContext, ToolRegistry};
    use crate::protocol::{
        AgentApprovalStatus, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
    };
    use serde_json::json;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use zip::write::SimpleFileOptions;

    static TEST_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn read_spreadsheet_extracts_xlsx_cells() {
        let fixture = TestWorkspace::new();
        fixture.write_xlsx("sheet.xlsx");
        let context = fixture.context();
        let registry = ToolRegistry::defaults_with_search(None);
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: "read_spreadsheet".to_string(),
            args: json!({ "path": "sheet.xlsx", "maxChars": 1 }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };

        let result = registry.execute(&context, &call);

        assert!(result.ok, "{:?}", result.error);
        let value = result.result.unwrap();
        let text = value["text"].as_str().unwrap().to_string();
        assert!(text.contains("A1=Name"));
        assert!(text.contains("B1=42"));
        assert_eq!(value["truncatedAtSource"], false);
        assert_eq!(value["omittedBytes"], 0);
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("my-copilot-agent-test-read-spreadsheet-{unique}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_xlsx(&self, path: &str) {
            let file_path = self.root.join(path);
            let file = File::create(file_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = SimpleFileOptions::default();
            zip.start_file("xl/sharedStrings.xml", options).unwrap();
            write!(
                zip,
                r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Name</t></si></sst>"#
            )
            .unwrap();
            zip.start_file("xl/worksheets/sheet1.xml", options).unwrap();
            write!(
                zip,
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>42</v></c></row></sheetData></worksheet>"#
            )
            .unwrap();
            zip.finish().unwrap();
        }

        fn context(&self) -> ToolExecutionContext {
            ToolExecutionContext::from_run_context(Some(&AgentRunContext {
                collaboration_identity: None,
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
