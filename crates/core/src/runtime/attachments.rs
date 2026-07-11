// Input attachment staging and extraction helpers for agent runtime.
use super::RUN_COUNTER;
use crate::llm::LlmImage;
use crate::protocol::{
    AgentApprovalStatus, AgentError, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentResult, AgentRunContext, AgentToolCall, AgentWorkspaceContext,
};
use crate::tools::{ToolExecutionContext, ToolRegistry};
use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

pub(super) struct AttachmentContext {
    pub(super) text: String,
    pub(super) images: Vec<LlmImage>,
}

pub(super) fn build_attachment_context(
    attachments: &[AgentInputAttachment],
) -> AgentResult<AttachmentContext> {
    if attachments.is_empty() {
        return Ok(AttachmentContext {
            text: String::new(),
            images: Vec::new(),
        });
    }

    let temp_root = std::env::temp_dir().join(format!(
        "my-copilot-agent-attachments-{}",
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&temp_root)
        .map_err(|error| AgentError::new(format!("创建附件临时目录失败：{error}")))?;

    let result = build_attachment_context_in_workspace(attachments, &temp_root);
    let _ = fs::remove_dir_all(&temp_root);
    result
}

fn build_attachment_context_in_workspace(
    attachments: &[AgentInputAttachment],
    temp_root: &Path,
) -> AgentResult<AttachmentContext> {
    let registry = ToolRegistry::defaults_with_search(None);
    let tool_context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("input attachments".to_string()),
            root_path: Some(temp_root.to_string_lossy().to_string()),
        }),
        attachment_library: None,
        permissions: Default::default(),
    }));
    let mut sections = Vec::new();
    let mut images = Vec::new();

    for attachment in attachments {
        let safe_name = sanitize_attachment_file_name(&attachment.name, &attachment.id);
        let mime_type = attachment
            .mime_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("application/octet-stream");

        if attachment.kind == AgentInputAttachmentKind::Image
            && attachment.encoding == AgentInputAttachmentEncoding::Base64
            && mime_type.starts_with("image/")
            && mime_type != "image/svg+xml"
        {
            images.push(LlmImage {
                mime_type: mime_type.to_string(),
                data_base64: attachment.data.clone(),
            });
            sections.push(format!(
                "### {}\n类型：图片\nMIME：{}\n大小：{} bytes\n状态：已作为视觉输入发送给模型。",
                attachment.name, mime_type, attachment.size_bytes
            ));
            continue;
        }

        let Some(tool_name) = read_tool_for_attachment(attachment, &safe_name) else {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n状态：已收到附件，但当前没有适合的只读解析工具。",
                attachment.name, mime_type, attachment.size_bytes
            ));
            continue;
        };

        let file_path = temp_root.join(&safe_name);
        let bytes = attachment_bytes(attachment)?;
        fs::write(&file_path, bytes)
            .map_err(|error| AgentError::new(format!("写入附件临时文件失败：{error}")))?;

        let call = AgentToolCall {
            id: format!("attachment-{}", attachment.id),
            tool: tool_name.to_string(),
            args: json!({
                "path": safe_name,
                "maxChars": 40_000,
                "maxLines": 1_200
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: Some("read user input attachment".to_string()),
        };
        let result = registry.execute(&tool_context, &call);

        if result.ok {
            let extracted = result
                .result
                .as_ref()
                .and_then(extracted_text_from_tool_result)
                .unwrap_or_default();
            let truncated = result
                .result
                .as_ref()
                .and_then(|value| value.get("truncated"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || attachment.truncated.unwrap_or(false);
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n读取工具：{}\n截断：{}\n\n{}",
                attachment.name, mime_type, attachment.size_bytes, tool_name, truncated, extracted
            ));
        } else {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n读取工具：{}\n错误：{}",
                attachment.name,
                mime_type,
                attachment.size_bytes,
                tool_name,
                result.error.unwrap_or_else(|| "附件读取失败。".to_string())
            ));
        }
    }

    let text = if sections.is_empty() {
        String::new()
    } else {
        format!(
            "用户输入框附件内容如下。附件来自用户本次输入，不是 workspace 文件；回答时可以引用这些内容，但不要声称它们已经存在于项目目录中。\n\n{}",
            sections.join("\n\n")
        )
    };

    Ok(AttachmentContext { text, images })
}

fn read_tool_for_attachment(
    attachment: &AgentInputAttachment,
    safe_name: &str,
) -> Option<&'static str> {
    let extension = attachment_extension(safe_name);
    match extension.as_str() {
        "pdf" => Some("read_pdf"),
        "doc" | "docx" => Some("read_word"),
        "pptx" => Some("read_presentation"),
        "xlsx" | "csv" | "tsv" => Some("read_spreadsheet"),
        _ if is_text_attachment(attachment, safe_name) => Some("read_file"),
        _ => None,
    }
}

fn is_text_attachment(attachment: &AgentInputAttachment, safe_name: &str) -> bool {
    let mime_type = attachment
        .mime_type
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();

    attachment.encoding == AgentInputAttachmentEncoding::Utf8
        || mime_type.starts_with("text/")
        || matches!(
            mime_type,
            "application/json" | "application/xml" | "image/svg+xml"
        )
        || matches!(
            attachment_extension(safe_name).as_str(),
            "txt"
                | "text"
                | "md"
                | "markdown"
                | "mdx"
                | "rst"
                | "log"
                | "json"
                | "jsonl"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "cfg"
                | "conf"
                | "env"
                | "lock"
                | "properties"
                | "plist"
                | "rc"
                | "gitignore"
                | "gitattributes"
                | "editorconfig"
                | "py"
                | "pyi"
                | "ipynb"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "mjs"
                | "cjs"
                | "html"
                | "htm"
                | "css"
                | "scss"
                | "sass"
                | "less"
                | "xml"
                | "sql"
                | "graphql"
                | "gql"
                | "proto"
                | "prisma"
                | "sh"
                | "bash"
                | "zsh"
                | "fish"
                | "ps1"
                | "bat"
                | "cmd"
                | "rs"
                | "go"
                | "java"
                | "kt"
                | "kts"
                | "c"
                | "h"
                | "cpp"
                | "cc"
                | "cxx"
                | "hpp"
                | "hh"
                | "hxx"
                | "cs"
                | "php"
                | "rb"
                | "swift"
                | "scala"
                | "r"
                | "m"
                | "pl"
                | "pm"
                | "lua"
                | "dart"
                | "ex"
                | "exs"
                | "erl"
                | "hrl"
                | "clj"
                | "cljs"
                | "cljc"
                | "edn"
                | "fs"
                | "fsi"
                | "fsx"
                | "elm"
                | "hs"
                | "lhs"
                | "jl"
                | "ml"
                | "mli"
                | "nim"
                | "nims"
                | "zig"
                | "v"
                | "vh"
                | "sv"
                | "svh"
                | "sol"
                | "tf"
                | "tfvars"
                | "hcl"
                | "gradle"
                | "groovy"
                | "dockerfile"
                | "cmake"
                | "make"
                | "mk"
                | "tex"
                | "bib"
                | "vue"
                | "svelte"
                | "astro"
        )
}

fn attachment_bytes(attachment: &AgentInputAttachment) -> AgentResult<Vec<u8>> {
    match attachment.encoding {
        AgentInputAttachmentEncoding::Utf8 => Ok(attachment.data.as_bytes().to_vec()),
        AgentInputAttachmentEncoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(attachment.data.as_bytes())
            .map_err(|error| AgentError::new(format!("附件 base64 数据无效：{error}"))),
    }
}

fn extracted_text_from_tool_result(value: &Value) -> Option<String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.get("content").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn sanitize_attachment_file_name(name: &str, fallback_id: &str) -> String {
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_id);
    let sanitized = file_name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    if sanitized.trim().is_empty() {
        fallback_id.to_string()
    } else {
        sanitized
    }
}

fn attachment_extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default()
}
