use super::{AgentTool, ToolExecutionContext};
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::protocol::{
    AgentError, AgentResult, AgentToolDefinition, AgentToolResult, AgentToolSafety,
};
use base64::Engine;
use image::codecs::png::PngEncoder;
use image::{ImageEncoder, ImageReader};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::Cursor;
use std::path::Path;

const MAX_READ_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const THUMBNAIL_MAX_EDGE: u32 = 160;
const MAX_THUMBNAIL_DATA_URL_BYTES: usize = 192 * 1024;

pub(super) struct ReadImageTool;

impl AgentTool for ReadImageTool {
    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_image".to_string(),
            description: "Read an image from the selected workspace or an @attachments path and send it back as visual input for the model.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative image path or @attachments/... readPath."
                    },
                    "filePath": {
                        "type": "string",
                        "description": "Alias for path."
                    }
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
        if !context.model_capabilities().image_input {
            return Err(AgentError::structured(
                "agent.model_capability_unsupported",
                "当前模型不支持图片输入；read_image 无法把图片作为视觉输入提供给该模型，文件尚未读取。请切换到支持图片输入的模型后重试。",
                json!({
                    "type": "model_capability",
                    "code": "modelCapabilityUnsupported",
                    "capability": "imageInput",
                    "required": true,
                    "actual": false,
                    "tool": "read_image",
                    "recovery": "switchToImageCapableModel"
                }),
            ));
        }
        let args: ReadImageArgs = serde_json::from_value(args)
            .map_err(|error| AgentError::new(format!("read_image 参数无效：{error}")))?;
        let path = args.path()?;
        let file_path = context.resolve_existing_path(path)?;
        context.check_cancelled()?;
        let metadata = fs::metadata(&file_path)
            .map_err(|error| AgentError::new(format!("读取图片元数据失败：{error}")))?;

        if !metadata.is_file() {
            return Err(AgentError::new("read_image 只能读取文件。"));
        }

        if metadata.len() > MAX_READ_IMAGE_BYTES {
            return Err(AgentError::new(format!(
                "图片过大：{} bytes，超过 {} bytes 限制。",
                metadata.len(),
                MAX_READ_IMAGE_BYTES
            )));
        }

        let extension = image_extension(&file_path)?;
        let mime_type = image_mime_type(&extension)?;
        let bytes = fs::read(&file_path)
            .map_err(|error| AgentError::new(format!("读取图片失败：{error}")))?;
        context.check_cancelled()?;
        let thumbnail_data_url = image_thumbnail_data_url(&bytes);
        context.check_cancelled()?;
        let data_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

        Ok(json!({
            "path": context.display_path(path, &file_path)?,
            "format": extension,
            "mimeType": mime_type,
            "sizeBytes": metadata.len(),
            "thumbnailDataUrl": thumbnail_data_url,
            "image": {
                "mimeType": mime_type,
                "dataBase64": data_base64
            }
        }))
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        read_image_history_projection(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        read_image_event_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        read_image_history_projection(result)
    }
}

fn read_image_history_projection(result: &AgentToolResult) -> AgentToolResult {
    let mut projected = result.clone();
    if let Some(object) = projected.result.as_mut().and_then(Value::as_object_mut) {
        let thumbnail_omitted = object.remove("thumbnailDataUrl").is_some();
        let image_data_omitted = object
            .get_mut("image")
            .and_then(Value::as_object_mut)
            .is_some_and(|image| image.remove("dataBase64").is_some());
        if thumbnail_omitted || image_data_omitted {
            object.insert("binaryOmittedFromHistory".to_string(), json!(true));
        }
    }
    canonical_tool_result_for_context(&projected)
}

fn read_image_event_projection(result: &AgentToolResult) -> AgentToolResult {
    let thumbnail = result
        .result
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get("thumbnailDataUrl"))
        .and_then(Value::as_str)
        .filter(|value| is_bounded_thumbnail_data_url(value))
        .map(ToString::to_string);

    // Canonicalize every other field, then deliberately restore only the bounded thumbnail that
    // belongs to presentation. The full image payload remains exclusively in the runtime result.
    let mut projected = canonical_tool_result_for_context(result);
    if let Some(object) = projected.result.as_mut().and_then(Value::as_object_mut) {
        object.remove("image");
        object.remove("thumbnailDataUrl");
        if let Some(thumbnail) = thumbnail {
            object.insert("thumbnailDataUrl".to_string(), Value::String(thumbnail));
        } else if result
            .result
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|raw| raw.contains_key("thumbnailDataUrl"))
        {
            object.insert("thumbnailOmittedFromEvent".to_string(), json!(true));
        }
    }
    projected
}

fn is_bounded_thumbnail_data_url(value: &str) -> bool {
    const PREFIX: &str = "data:image/png;base64,";
    value.len() <= MAX_THUMBNAIL_DATA_URL_BYTES
        && value
            .strip_prefix(PREFIX)
            .filter(|encoded| !encoded.is_empty())
            .is_some_and(|encoded| {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .is_ok()
            })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadImageArgs {
    path: Option<String>,
    file_path: Option<String>,
}

impl ReadImageArgs {
    fn path(&self) -> AgentResult<&str> {
        self.path
            .as_deref()
            .or(self.file_path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("read_image.path 不能为空。"))
    }
}

fn image_thumbnail_data_url(bytes: &[u8]) -> Option<String> {
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumbnail = image
        .thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE)
        .to_rgba8();
    let mut thumbnail_bytes = Vec::new();
    PngEncoder::new(&mut thumbnail_bytes)
        .write_image(
            thumbnail.as_raw(),
            thumbnail.width(),
            thumbnail.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;

    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(thumbnail_bytes)
    ))
}

fn image_extension(path: &Path) -> AgentResult<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .ok_or_else(|| AgentError::new("图片文件缺少扩展名，无法判断图片类型。"))
}

fn image_mime_type(extension: &str) -> AgentResult<&'static str> {
    match extension {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "gif" => Ok("image/gif"),
        "webp" => Ok("image/webp"),
        _ => Err(AgentError::new(format!(
            "不支持的图片类型：.{extension}。支持：.png, .jpg, .jpeg, .gif, .webp"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_result(thumbnail: String) -> AgentToolResult {
        AgentToolResult {
            call_id: "call-image".to_string(),
            tool: "read_image".to_string(),
            ok: true,
            result: Some(json!({
                "path": "preview.png",
                "format": "png",
                "mimeType": "image/png",
                "sizeBytes": 3,
                "thumbnailDataUrl": thumbnail,
                "image": {
                    "mimeType": "image/png",
                    "dataBase64": "ZnVsbC1pbWFnZQ=="
                }
            })),
            error: None,
        }
    }

    #[test]
    fn projections_keep_runtime_image_private_and_event_thumbnail_bounded() {
        let thumbnail = "data:image/png;base64,dGh1bWI=".to_string();
        let result = image_result(thumbnail.clone());

        let trace = ReadImageTool.trace_projection(&result);
        let checkpoint = ReadImageTool.checkpoint_projection(&result);
        for durable in [&trace, &checkpoint] {
            let serialized = serde_json::to_string(durable).unwrap();
            assert!(!serialized.contains("data:image"));
            assert!(!serialized.contains("ZnVsbC1pbWFnZQ=="));
            assert!(!serialized.contains("dGh1bWI="));
            assert_eq!(durable.result.as_ref().unwrap()["path"], "preview.png");
            assert_eq!(
                durable.result.as_ref().unwrap()["binaryOmittedFromHistory"],
                true
            );
        }

        let event = ReadImageTool.event_projection(&result);
        let event_value = event.result.as_ref().unwrap();
        assert_eq!(event_value["thumbnailDataUrl"], thumbnail);
        assert!(event_value.get("image").is_none());
        assert!(!serde_json::to_string(&event)
            .unwrap()
            .contains("ZnVsbC1pbWFnZQ=="));

        // Projection must never mutate or replace the runtime observation used to build the
        // current model's visual message.
        assert_eq!(
            result.result.as_ref().unwrap()["image"]["dataBase64"],
            "ZnVsbC1pbWFnZQ=="
        );
    }

    #[test]
    fn oversized_or_non_png_thumbnail_is_omitted_from_presentation() {
        for thumbnail in [
            format!(
                "data:image/png;base64,{}",
                "A".repeat(MAX_THUMBNAIL_DATA_URL_BYTES)
            ),
            "data:image/jpeg;base64,dGh1bWI=".to_string(),
            "data:image/png;base64,not-valid-base64!".to_string(),
        ] {
            let event = ReadImageTool.event_projection(&image_result(thumbnail));
            let value = event.result.as_ref().unwrap();
            assert!(value.get("thumbnailDataUrl").is_none());
            assert_eq!(value["thumbnailOmittedFromEvent"], true);
            assert!(value.get("image").is_none());
        }
    }
}
