// Managed input attachment extraction helpers for agent runtime.
use crate::file_input::image_delivery::prepare_model_image;
use crate::file_input::{snapshot_verified_agent_file_input, AgentFileInputExecutionContext};
use crate::llm::LlmImage;
use crate::protocol::{
    AgentApprovalStatus, AgentAttachmentLibraryContext, AgentError, AgentInputAttachment,
    AgentInputAttachmentEncoding, AgentInputAttachmentKind, AgentResult, AgentRunContext,
    AgentToolCall,
};
use crate::tools::{AgentToolExposure, ToolExecutionContext, ToolRegistry};
use base64::Engine;
use serde_json::{json, Value};
use std::io::BufReader;
use std::path::Path;

// Attachment preprocessing is a direct context consumer rather than an ordinary Tool-result
// exchange, so it cannot rely on the central 10K Model Result Gate. Keep a 40K ceiling
// here as an explicit Consumer Projection Limit; document Tools themselves must still return the
// complete extracted text so Exact History can archive it.
const ATTACHMENT_CONTEXT_TEXT_MAX_CHARS: usize = 40_000;
const ATTACHMENT_INLINE_TEXT_MAX_BYTES: u64 = 256 * 1024;
const ATTACHMENT_CONTEXT_IMAGE_MAX_BYTES: usize = 16 * 1024 * 1024;
const ATTACHMENT_PREPROCESSING_CONVERSATION_ID: &str = "host:attachment-preprocessing";
const ATTACHMENT_PREPROCESSING_RUN_ID: &str = "host:attachment-preprocessing:run";

pub(super) struct AttachmentContext {
    pub(super) text: String,
    pub(super) images: Vec<LlmImage>,
}

pub(super) fn build_attachment_context(
    attachments: &[AgentInputAttachment],
    attachment_library: Option<&AgentAttachmentLibraryContext>,
) -> AgentResult<AttachmentContext> {
    if attachments.is_empty() {
        return Ok(AttachmentContext {
            text: String::new(),
            images: Vec::new(),
        });
    }

    let registry = ToolRegistry::defaults_with_search(None);
    let tool_context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        // Stable text readers now issue a run-owned FileObservation for every successful read.
        // Attachment preprocessing is an isolated Host consumer with a fresh registry, so bind
        // it to an explicit internal owner instead of weakening read_file's totality contract.
        conversation_id: Some(ATTACHMENT_PREPROCESSING_CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: attachment_library.cloned(),
        permissions: Default::default(),
    }))
    .with_runtime_services(ATTACHMENT_PREPROCESSING_RUN_ID.to_string(), None);
    let mut sections = Vec::new();
    let mut images = Vec::new();
    let mut remaining_text = ATTACHMENT_CONTEXT_TEXT_MAX_CHARS;
    let mut remaining_images = ATTACHMENT_CONTEXT_IMAGE_MAX_BYTES;

    for attachment in attachments {
        let safe_name = sanitize_attachment_file_name(&attachment.name, &attachment.id);
        let read_path = require_managed_read_path(attachment_library, attachment)?;
        let mime_type = attachment
            .mime_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("application/octet-stream");

        if attachment.kind == AgentInputAttachmentKind::Image
            && mime_type.starts_with("image/")
            && mime_type != "image/svg+xml"
        {
            let prepared = prepare_attachment_image(attachment, attachment_library);
            match prepared {
                Ok(prepared) if prepared.bytes.len() <= remaining_images => {
                    remaining_images -= prepared.bytes.len();
                    images.push(LlmImage {
                        mime_type: prepared.mime_type.to_string(),
                        data_base64: base64::engine::general_purpose::STANDARD.encode(&prepared.bytes),
                    });
                    sections.push(format!(
                        "### {}\n类型：图片\nMIME：{}\n大小：{} bytes\n{}\n状态：已作为视觉输入发送给模型（{}×{}；原图已保留）。",
                        attachment.name, mime_type, attachment.size_bytes, read_path_line(read_path),
                        prepared.width, prepared.height,
                    ));
                }
                Ok(_) => sections.push(format!(
                    "### {}\nMIME：{}\n大小：{} bytes\n{}\n状态：本次附件图片预算已用完；原图已保留，请按需使用 read_image。",
                    attachment.name, mime_type, attachment.size_bytes, read_path_line(read_path),
                )),
                Err(error) => sections.push(format!(
                    "### {}\nMIME：{}\n大小：{} bytes\n{}\n状态：原图已保留，未作为视觉输入发送：{}",
                    attachment.name, mime_type, attachment.size_bytes, read_path_line(read_path), error,
                )),
            }
            continue;
        }

        if attachment.size_bytes > ATTACHMENT_INLINE_TEXT_MAX_BYTES || remaining_text == 0 {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n{}\n状态：仅提供附件元数据；请按需通过 readPath 分段读取，或作为命令的只读文件输入。",
                attachment.name, mime_type, attachment.size_bytes, read_path_line(read_path),
            ));
            continue;
        }

        if is_pdf_attachment(attachment, &safe_name) {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n{}\n状态：正文未读取；仅在本轮实际提供匹配工具或可用 Skill 时，按其说明读取上述 readPath；不要推断未提供的能力。",
                attachment.name,
                mime_type,
                attachment.size_bytes,
                read_path_line(read_path)
            ));
            continue;
        }

        let Some(tool_name) = candidate_read_tool_for_attachment(attachment, &safe_name) else {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n{}\n状态：已收到附件，但当前没有适合的只读解析工具。",
                attachment.name,
                mime_type,
                attachment.size_bytes,
                read_path_line(read_path)
            ));
            continue;
        };

        // Attachment preprocessing is intentionally limited to the same stable Tool surface sent
        // to every model request. Dynamic readers must produce an ordinary, auditable Tool call
        // after the matching Skill is activated; the private registry must not become a backdoor
        // that silently grants preprocessing more capability than the model has.
        match registry.exposure(tool_name) {
            Some(AgentToolExposure::Stable) => {}
            Some(AgentToolExposure::Dynamic | AgentToolExposure::RequiresCapability(_)) => {
                sections.push(format!(
                    "### {}\nMIME：{}\n大小：{} bytes\n{}\n状态：正文未读取；仅在本轮实际提供匹配工具或可用 Skill 时，按其说明读取上述 readPath；不要推断未提供的能力。",
                    attachment.name,
                    mime_type,
                    attachment.size_bytes,
                    read_path_line(read_path)
                ));
                continue;
            }
            None => {
                sections.push(format!(
                    "### {}\nMIME：{}\n大小：{} bytes\n{}\n状态：已收到附件，但对应的只读工具未在当前运行时注册。",
                    attachment.name,
                    mime_type,
                    attachment.size_bytes,
                    read_path_line(read_path)
                ));
                continue;
            }
        }

        let tool_path = read_path;

        let call = AgentToolCall {
            id: format!("attachment-{}", attachment.id),
            tool: tool_name.to_string(),
            args: json!({
                "path": tool_path,
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
            let projection = attachment_text_projection_with_limit(extracted, remaining_text);
            remaining_text = remaining_text.saturating_sub(projection.returned_chars);
            let tool_truncated = result
                .result
                .as_ref()
                .and_then(|value| value.get("truncated"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let truncated =
                tool_truncated || projection.truncated || attachment.truncated.unwrap_or(false);
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n{}\n读取工具：{}\n截断：{}\n正文投影：returned={} chars，total={} chars，omitted={} chars\n\n{}",
                attachment.name,
                mime_type,
                attachment.size_bytes,
                read_path_line(read_path),
                tool_name,
                truncated,
                projection.returned_chars,
                projection.total_chars,
                projection.omitted_chars,
                projection.text
            ));
        } else {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n{}\n读取工具：{}\n错误：{}",
                attachment.name,
                mime_type,
                attachment.size_bytes,
                read_path_line(read_path),
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

/// Host-only image payloads for exact journal binding. New images use the same bounded
/// derivative as the model request; already-hydrated historical payloads are never rewritten.
pub(super) fn normalize_attachment_context_images(
    attachments: &[AgentInputAttachment],
    attachment_library: Option<&AgentAttachmentLibraryContext>,
) -> AgentResult<Vec<AgentInputAttachment>> {
    let mut normalized = Vec::new();
    let mut remaining = ATTACHMENT_CONTEXT_IMAGE_MAX_BYTES;
    for attachment in attachments {
        require_managed_read_path(attachment_library, attachment)?;
        if attachment.kind != AgentInputAttachmentKind::Image {
            continue;
        }
        let Ok(prepared) = prepare_attachment_image(attachment, attachment_library) else {
            continue;
        };
        if prepared.bytes.len() > remaining {
            continue;
        }
        remaining -= prepared.bytes.len();
        normalized.push(AgentInputAttachment {
            id: attachment.id.clone(),
            kind: AgentInputAttachmentKind::Image,
            name: attachment.name.clone(),
            mime_type: Some(prepared.mime_type.to_string()),
            size_bytes: prepared.bytes.len() as u64,
            encoding: AgentInputAttachmentEncoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode(&prepared.bytes),
            content_sha256: None,
            truncated: None,
        });
    }
    Ok(normalized)
}

fn prepare_attachment_image(
    attachment: &AgentInputAttachment,
    attachment_library: Option<&AgentAttachmentLibraryContext>,
) -> AgentResult<crate::file_input::image_delivery::PreparedModelImage> {
    let read_path = require_managed_read_path(attachment_library, attachment)?;
    let source = crate::AgentFileInputRef::Attachment {
        read_path: read_path.to_string(),
    };
    snapshot_verified_agent_file_input(
        None,
        Default::default(),
        &AgentFileInputExecutionContext::from_attachment_library(attachment_library.cloned()),
        &source,
        None,
    )
    .map_err(AgentError::from)
    .and_then(|snapshot| prepare_model_image(BufReader::new(snapshot.file)))
}

fn require_managed_read_path<'a>(
    attachment_library: Option<&'a AgentAttachmentLibraryContext>,
    attachment: &AgentInputAttachment,
) -> AgentResult<&'a str> {
    if attachment.encoding != AgentInputAttachmentEncoding::Managed {
        return Err(AgentError::new(
            "用户附件必须使用托管引用，请重新添加附件。",
        ));
    }
    registered_read_path(attachment_library, attachment)
        .ok_or_else(|| AgentError::new("托管附件没有匹配的附件库授权，请重新添加附件。"))
}

fn registered_read_path<'a>(
    attachment_library: Option<&'a AgentAttachmentLibraryContext>,
    attachment: &AgentInputAttachment,
) -> Option<&'a str> {
    let attachment_library = attachment_library?;
    attachment_library
        .conversation_attachments
        .iter()
        .chain(&attachment_library.project_attachments)
        .find(|reference| {
            reference.id == attachment.id
                && reference.kind == attachment.kind
                && reference.name == attachment.name
                && reference.mime_type == attachment.mime_type
                && reference.size_bytes == attachment.size_bytes
        })
        .map(|reference| reference.read_path.as_str())
}

fn read_path_line(read_path: &str) -> String {
    format!("readPath：`{read_path}`")
}

fn candidate_read_tool_for_attachment(
    attachment: &AgentInputAttachment,
    safe_name: &str,
) -> Option<&'static str> {
    let extension = attachment_extension(safe_name);
    let mime_type = attachment
        .mime_type
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "doc" | "docx" => Some("read_word"),
        "pptx" => Some("read_presentation"),
        "xlsx" | "csv" | "tsv" => Some("read_spreadsheet"),
        _ if is_word_mime_type(&mime_type) => Some("read_word"),
        _ if is_presentation_mime_type(&mime_type) => Some("read_presentation"),
        _ if is_spreadsheet_mime_type(&mime_type) => Some("read_spreadsheet"),
        _ if is_text_attachment(attachment, safe_name) => Some("read_file"),
        _ => None,
    }
}

fn is_pdf_attachment(attachment: &AgentInputAttachment, safe_name: &str) -> bool {
    attachment_extension(safe_name) == "pdf"
        || attachment
            .mime_type
            .as_deref()
            .map(str::trim)
            .is_some_and(|mime_type| mime_type.eq_ignore_ascii_case("application/pdf"))
}

fn is_word_mime_type(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "application/msword"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    )
}

fn is_presentation_mime_type(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "application/vnd.ms-powerpoint"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    )
}

fn is_spreadsheet_mime_type(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "application/vnd.ms-excel"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "text/csv"
            | "text/tab-separated-values"
    )
}

fn is_text_attachment(attachment: &AgentInputAttachment, safe_name: &str) -> bool {
    let mime_type = attachment
        .mime_type
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();

    mime_type.starts_with("text/")
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

fn extracted_text_from_tool_result(value: &Value) -> Option<String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.get("content").and_then(Value::as_str))
        .map(ToString::to_string)
}

struct AttachmentTextProjection {
    text: String,
    total_chars: usize,
    returned_chars: usize,
    omitted_chars: usize,
    truncated: bool,
}

#[cfg(test)]
fn attachment_text_projection(text: String) -> AttachmentTextProjection {
    attachment_text_projection_with_limit(text, ATTACHMENT_CONTEXT_TEXT_MAX_CHARS)
}

fn attachment_text_projection_with_limit(
    text: String,
    max_chars: usize,
) -> AttachmentTextProjection {
    let total_chars = text.chars().count();
    let returned_chars = total_chars.min(max_chars);
    let truncated = returned_chars < total_chars;
    let text = if truncated {
        text.chars().take(max_chars).collect()
    } else {
        text
    };
    AttachmentTextProjection {
        text,
        total_chars,
        returned_chars,
        omitted_chars: total_chars.saturating_sub(returned_chars),
        truncated,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    fn managed_attachment_fixture(
        root: &Path,
        name: &str,
        bytes: &[u8],
        kind: AgentInputAttachmentKind,
    ) -> (AgentInputAttachment, AgentAttachmentLibraryContext) {
        let id = "attachment-managed";
        fs::write(root.join(name), bytes).unwrap();
        let mime_type = Some(
            if kind == AgentInputAttachmentKind::Image {
                "image/png"
            } else {
                "text/plain"
            }
            .to_string(),
        );
        let attachment = AgentInputAttachment {
            id: id.into(),
            kind,
            name: name.into(),
            mime_type: mime_type.clone(),
            size_bytes: bytes.len() as u64,
            encoding: AgentInputAttachmentEncoding::Managed,
            data: "opaque-import-id".into(),
            content_sha256: None,
            truncated: None,
        };
        let library = AgentAttachmentLibraryContext {
            root_path: Some(root.to_string_lossy().into()),
            conversation_id: Some("conversation".into()),
            project_id: None,
            conversation_attachments: vec![crate::AgentAttachmentReference {
                id: id.into(),
                conversation_id: "conversation".into(),
                message_id: "message".into(),
                project_id: None,
                kind,
                name: name.into(),
                mime_type,
                size_bytes: bytes.len() as u64,
                read_path: format!("@attachments/{id}/{name}"),
                storage_rel_path: name.into(),
                created_at: 0,
            }],
            project_attachments: Vec::new(),
        };
        (attachment, library)
    }

    #[test]
    fn user_attachment_boundary_rejects_inline_payloads_even_with_registered_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let (mut attachment, library) = managed_attachment_fixture(
            directory.path(),
            "notes.txt",
            b"hello",
            AgentInputAttachmentKind::File,
        );
        attachment.encoding = AgentInputAttachmentEncoding::Base64;
        attachment.data = "aGVsbG8=".into();
        assert!(
            build_attachment_context(std::slice::from_ref(&attachment), Some(&library))
                .err()
                .unwrap()
                .to_string()
                .contains("托管引用")
        );
        assert!(normalize_attachment_context_images(&[attachment], Some(&library)).is_err());
    }

    #[test]
    fn managed_text_reads_authorized_file_and_never_uses_opaque_id_as_content() {
        let directory = tempfile::tempdir().unwrap();
        let (attachment, library) = managed_attachment_fixture(
            directory.path(),
            "notes.txt",
            b"managed file content",
            AgentInputAttachmentKind::File,
        );
        let context =
            build_attachment_context(std::slice::from_ref(&attachment), Some(&library)).unwrap();
        assert!(context.text.contains("managed file content"));
        assert!(!context.text.contains("opaque-import-id"));
        let mut mismatched = attachment;
        mismatched.size_bytes += 1;
        assert!(build_attachment_context(&[mismatched], Some(&library)).is_err());
    }

    #[test]
    fn large_managed_text_is_metadata_only_and_total_inline_text_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let (mut attachment, mut library) = managed_attachment_fixture(
            directory.path(),
            "large.txt",
            b"not read",
            AgentInputAttachmentKind::File,
        );
        attachment.size_bytes = 1024 * 1024 * 1024;
        library.conversation_attachments[0].size_bytes = attachment.size_bytes;
        // No read is attempted: the fake registered size cannot match this tiny file.
        let context = build_attachment_context(&[attachment], Some(&library)).unwrap();
        assert!(context.text.contains("仅提供附件元数据"));
        assert!(!context.text.contains("not read"));
        let mut attachments = Vec::new();
        let mut combined_library = library.clone();
        combined_library.conversation_attachments.clear();
        for id in ["a", "b", "c", "d", "e"] {
            let (mut attachment, mut library) = managed_attachment_fixture(
                directory.path(),
                &format!("{id}.txt"),
                "界".repeat(10_000).as_bytes(),
                AgentInputAttachmentKind::File,
            );
            attachment.id = id.into();
            library.conversation_attachments[0].id = id.into();
            library.conversation_attachments[0].read_path = format!("@attachments/{id}/{id}.txt");
            combined_library
                .conversation_attachments
                .extend(library.conversation_attachments);
            attachments.push(attachment);
        }
        let context = build_attachment_context(&attachments, Some(&combined_library)).unwrap();
        assert_eq!(
            context.text.matches('界').count(),
            ATTACHMENT_CONTEXT_TEXT_MAX_CHARS
        );
    }

    #[test]
    fn managed_image_is_resized_without_replacing_original() {
        let directory = tempfile::tempdir().unwrap();
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2500, 1000)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let original = encoded.into_inner();
        let (attachment, library) = managed_attachment_fixture(
            directory.path(),
            "large.png",
            &original,
            AgentInputAttachmentKind::Image,
        );
        let context = build_attachment_context(&[attachment], Some(&library)).unwrap();
        assert_eq!(context.images.len(), 1);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&context.images[0].data_base64)
            .unwrap();
        let derived = image::load_from_memory(&bytes).unwrap();
        assert_eq!(derived.width(), 2048);
        assert_eq!(
            fs::read(directory.path().join("large.png")).unwrap(),
            original
        );
    }

    #[test]
    fn text_attachment_preprocessing_has_an_internal_observation_owner() {
        let directory = tempfile::tempdir().unwrap();
        let marker = "ATTACHMENT_OBSERVATION_OWNER_MARKER";
        let (attachment, library) = managed_attachment_fixture(
            directory.path(),
            "notes.txt",
            marker.as_bytes(),
            AgentInputAttachmentKind::File,
        );
        let context = build_attachment_context(&[attachment], Some(&library)).unwrap();

        assert!(context.text.contains(marker));
        assert!(!context.text.contains("缺少 conversationId"));
        assert!(!context.text.contains("缺少 runId"));
    }

    #[test]
    fn attachment_text_limit_is_explicit_and_utf8_safe() {
        let projection =
            attachment_text_projection("你".repeat(ATTACHMENT_CONTEXT_TEXT_MAX_CHARS + 7));

        assert_eq!(
            projection.text.chars().count(),
            ATTACHMENT_CONTEXT_TEXT_MAX_CHARS
        );
        assert_eq!(
            projection.total_chars,
            ATTACHMENT_CONTEXT_TEXT_MAX_CHARS + 7
        );
        assert_eq!(projection.returned_chars, ATTACHMENT_CONTEXT_TEXT_MAX_CHARS);
        assert_eq!(projection.omitted_chars, 7);
        assert!(projection.truncated);
    }
}
