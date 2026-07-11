use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition, AgentToolSafety};
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

pub(super) struct ReadImageTool;

impl AgentTool for ReadImageTool {
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
