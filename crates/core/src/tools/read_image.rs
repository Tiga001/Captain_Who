use super::{schema::agent_file_input_ref_schema, AgentTool, ToolExecutionContext};
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::file_input::{
    read_verified_agent_file_input, AgentFileInputError, AgentFileInputExecutionContext,
};
use crate::protocol::{
    AgentError, AgentFileInputRef, AgentResult, AgentToolDefinition, AgentToolResult,
    AgentToolSafety,
};
use crate::system_paths::expand_system_path;
use base64::Engine;
use image::codecs::png::PngEncoder;
use image::{DynamicImage, ImageEncoder, ImageFormat};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const MAX_READ_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const THUMBNAIL_MAX_EDGE: u32 = 160;
const MAX_THUMBNAIL_DATA_URL_BYTES: usize = 192 * 1024;

pub(super) struct ReadImageTool;

impl AgentTool for ReadImageTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "read_image".to_string(),
            description: "Read one authorized image and return it as visual input for the model. Prefer source with the unified AgentFileInputRef returned by attachments, generated-image, and Skill tools. Legacy path/filePath remains supported for workspace-relative, absolute, system-alias, and @attachments paths.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source": agent_file_input_ref_schema(),
                    "path": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Legacy compatibility input: workspace-relative path, absolute path, supported system alias, or exact @attachments/... readPath. Prefer source for generated Artifacts and Skill resources."
                    },
                    "filePath": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Legacy alias for path."
                    }
                },
                "additionalProperties": false
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
        let source = args.source()?;
        let file_inputs = AgentFileInputExecutionContext::new(
            context.attachment_library().cloned(),
            context.skill_resources_optional(),
        )
        .with_storage(context.storage_optional());
        let workspace_root = context.workspace_root_optional()?;
        let snapshot = read_verified_agent_file_input(
            workspace_root.as_deref(),
            context.permissions(),
            &file_inputs,
            &source,
            Some(&context.cancellation_token()),
            MAX_READ_IMAGE_BYTES,
        )
        .map_err(agent_file_input_error)?;
        context.check_cancelled()?;
        if snapshot.size_bytes == 0 {
            return Err(AgentError::new("图片文件为空。"));
        }

        let reservation = context
            .try_reserve_model_image_delivery(snapshot.size_bytes)
            .ok_or_else(|| {
                AgentError::structured(
                    "agent.model_image_delivery_budget_exceeded",
                    "本轮可交给模型查看的图片数据已达到上限；请在下一轮继续读取。",
                    json!({
                        "type": "model_capability",
                        "code": "modelImageDeliveryBudgetExceeded",
                        "capability": "imageInput",
                        "tool": "read_image",
                        "recovery": "retryInNextTurn"
                    }),
                )
            })?;
        let _preparation_permit = context.acquire_model_image_preparation()?;

        let (decoded, format, mime_type) = decode_supported_image(&snapshot.bytes)?;
        let thumbnail_data_url = image_thumbnail_data_url(&decoded);
        context.check_cancelled()?;
        let data_base64 =
            base64::engine::general_purpose::STANDARD.encode(snapshot.bytes.as_slice());
        context.check_cancelled()?;
        let display_path = display_source(&snapshot.source);
        reservation.commit();

        Ok(json!({
            "path": display_path,
            "source": snapshot.source,
            "format": format,
            "mimeType": mime_type,
            "sizeBytes": snapshot.size_bytes,
            "sha256": snapshot.sha256,
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

    fn archive_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        read_image_history_projection(result)
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        read_image_model_projection(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        read_image_event_projection(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        read_image_history_projection(result)
    }
}

fn read_image_model_projection(result: &AgentToolResult) -> AgentToolResult {
    let projected = super::model_projection::retain_fields(
        result.result.as_ref(),
        &["path", "format", "mimeType", "sizeBytes"],
    );
    super::model_projection::compact_model_result(result, projected)
}

pub(super) fn read_image_history_projection(result: &AgentToolResult) -> AgentToolResult {
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadImageArgs {
    source: Option<AgentFileInputRef>,
    path: Option<String>,
    file_path: Option<String>,
}

impl ReadImageArgs {
    fn source(&self) -> AgentResult<AgentFileInputRef> {
        let path = self
            .path
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let file_path = self
            .file_path
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());

        if self.source.is_some() && (path.is_some() || file_path.is_some()) {
            return Err(AgentError::new(
                "read_image.source 不能与旧版 path/filePath 同时提供。",
            ));
        }
        if let Some(source) = &self.source {
            return Ok(source.clone());
        }
        let legacy = match (path, file_path) {
            (Some(path), Some(file_path)) if path == file_path => path,
            (Some(_), Some(_)) => {
                return Err(AgentError::new(
                    "read_image.path 与 read_image.filePath 不能指向不同文件。",
                ))
            }
            (Some(path), None) | (None, Some(path)) => path,
            (None, None) => {
                return Err(AgentError::new(
                    "read_image 需要 source、path 或 filePath。",
                ))
            }
        };
        legacy_file_input_ref(legacy)
    }
}

fn legacy_file_input_ref(path: &str) -> AgentResult<AgentFileInputRef> {
    let path = path.trim();
    if path.starts_with("@attachments/") {
        return Ok(AgentFileInputRef::Attachment {
            read_path: path.to_string(),
        });
    }
    let expanded = expand_system_path(path).map_err(|message| {
        AgentError::structured(
            "agent.fileInput.invalidRequest",
            message,
            json!({
                "type": "agentFileInput",
                "code": "agent.fileInput.invalidRequest",
                "recovery": "changeRequest"
            }),
        )
    })?;
    if expanded.is_some() || Path::new(path).is_absolute() {
        Ok(AgentFileInputRef::External {
            path: path.to_string(),
        })
    } else {
        Ok(AgentFileInputRef::Workspace {
            path: path.to_string(),
        })
    }
}

fn agent_file_input_error(error: AgentFileInputError) -> AgentError {
    AgentError::structured(
        error.code(),
        error.message(),
        json!({
            "type": "agentFileInput",
            "code": error.code(),
            "recovery": error.recovery()
        }),
    )
}

fn display_source(source: &AgentFileInputRef) -> &str {
    match source {
        AgentFileInputRef::Attachment { read_path } => read_path,
        AgentFileInputRef::Workspace { path } | AgentFileInputRef::External { path } => path,
        AgentFileInputRef::GeneratedArtifact { uri, .. }
        | AgentFileInputRef::SkillResource { uri } => uri,
    }
}

fn decode_supported_image(bytes: &[u8]) -> AgentResult<(DynamicImage, &'static str, &'static str)> {
    let detected = image::guess_format(bytes)
        .map_err(|_| AgentError::new("无法识别图片格式，文件可能已损坏或格式不受支持。"))?;
    let (format, mime_type) = match detected {
        ImageFormat::Png => ("png", "image/png"),
        ImageFormat::Jpeg => ("jpeg", "image/jpeg"),
        ImageFormat::Gif => ("gif", "image/gif"),
        ImageFormat::WebP => ("webp", "image/webp"),
        _ => {
            return Err(AgentError::new(
                "不支持的图片类型。支持：PNG、JPEG、GIF、WebP。",
            ))
        }
    };
    let decoded = image::load_from_memory_with_format(bytes, detected)
        .map_err(|_| AgentError::new("图片解码失败，文件可能已损坏。"))?;
    Ok((decoded, format, mime_type))
}

fn image_thumbnail_data_url(image: &DynamicImage) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::{
        OfficePathScope, OfficePublishedOutput, OfficePublishedOutputKind,
        OfficePublishedOutputRole, OfficeRenderPageSelection,
    };
    use crate::protocol::{
        AgentAttachmentLibraryContext, AgentAttachmentReference, AgentInputAttachmentKind,
        AgentPermissions, AgentReadPermission, AgentRunContext, AgentWorkspaceContext,
        ModelCapabilities,
    };
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillResourcePath, SkillRevision, SkillSourceId,
    };
    use crate::storage::image_generation_execution_repository::{
        ImageGenerationArtifactJournalRecord, ImageGenerationExecutionIdentityRecord,
        ImageGenerationExecutionTerminalUpdate, StoredImageGenerationArtifactState,
        StoredImageGenerationExecutionStatus,
    };
    use crate::storage::service::StorageService;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn valid_test_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(
                &[0x20, 0x40, 0x80, 0xff],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    fn context(workspace: Option<&Path>, permissions: AgentPermissions) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("conversation-read-image".to_string()),
            project_id: None,
            workspace: workspace.map(|root| AgentWorkspaceContext {
                project_id: None,
                display_name: Some("read-image-test".to_string()),
                root_path: Some(root.to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions,
        }))
        .with_model_capabilities(ModelCapabilities { image_input: true })
    }

    fn execute(context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        ReadImageTool.execute(context, args)
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn publish_generated_artifact(
        root: &Path,
        bytes: &[u8],
    ) -> (Arc<StorageService>, PathBuf, String) {
        let storage = Arc::new(StorageService::open(&root.join("storage.sqlite")).unwrap());
        let sha256 = sha256_hex(bytes);
        let artifact_id = format!("sha256:{sha256}");
        let storage_relative_path = format!("objects/{sha256}.png");
        let identity = ImageGenerationExecutionIdentityRecord {
            execution_id: "execution-read-image".to_string(),
            request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            safe_request_json: r#"{"schemaVersion":1}"#.to_string(),
            profile_id: "default".to_string(),
            adapter_id: "test".to_string(),
            profile_revision: 1,
            model_id: "test-image-model".to_string(),
            operation: "generate".to_string(),
        };
        storage.claim_image_generation_execution(&identity).unwrap();
        storage
            .prepare_image_generation_artifact(
                &identity.execution_id,
                &ImageGenerationArtifactJournalRecord {
                    ordinal: 0,
                    artifact_id,
                    state: StoredImageGenerationArtifactState::Candidate,
                    storage_relative_path: storage_relative_path.clone(),
                    format: "png".to_string(),
                    media_type: "image/png".to_string(),
                    width: 1,
                    height: 1,
                    size_bytes: bytes.len() as u64,
                    sha256: sha256.clone(),
                    created_at: 1,
                    published_at: None,
                },
                Some("provider-request"),
                Some(200),
            )
            .unwrap();
        storage
            .finalize_image_generation_execution(
                &identity.execution_id,
                &ImageGenerationExecutionTerminalUpdate {
                    expected_request_fingerprint: identity.request_fingerprint,
                    expected_artifact_sha256: Some(sha256.clone()),
                    status: StoredImageGenerationExecutionStatus::Succeeded,
                    remote_outcome_unknown: false,
                    provider_succeeded: true,
                    commit_may_have_succeeded: false,
                    provider_request_id: Some("provider-request".to_string()),
                    http_status: Some(200),
                    terminal_result_json: r#"{"schemaVersion":1,"status":"succeeded"}"#.to_string(),
                },
            )
            .unwrap();
        let path = root
            .join("image-generation-artifacts")
            .join(storage_relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        (storage, path, sha256)
    }

    fn image_result(thumbnail: String) -> AgentToolResult {
        AgentToolResult {
            exact_archive_file: None,
            call_id: "call-image".to_string(),
            tool: "read_image".to_string(),
            ok: true,
            result: Some(json!({
                "path": "preview.png",
                "source": { "type": "workspace", "path": "preview.png" },
                "format": "png",
                "mimeType": "image/png",
                "sizeBytes": 3,
                "sha256": "private-image-digest",
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
        assert_eq!(event_value["sha256"], "private-image-digest");
        assert_eq!(event_value["source"]["type"], "workspace");
        assert!(event_value.get("image").is_none());
        assert!(!serde_json::to_string(&event)
            .unwrap()
            .contains("ZnVsbC1pbWFnZQ=="));

        let model = ReadImageTool.model_projection(&result);
        let model_value = model.result.as_ref().unwrap();
        assert!(model_value.get("image").is_none());
        assert!(model_value.get("thumbnailDataUrl").is_none());
        assert!(model_value.get("source").is_none());
        assert!(model_value.get("sha256").is_none());
        assert!(!serde_json::to_string(&model).unwrap().contains("base64"));

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

    #[test]
    fn schema_prefers_unified_source_but_keeps_legacy_paths() {
        let definition = ReadImageTool.definition();
        let properties = definition.input_schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("source"));
        assert!(properties.contains_key("path"));
        assert!(properties.contains_key("filePath"));
        assert!(definition.input_schema.get("oneOf").is_none());
        assert!(definition.input_schema.get("anyOf").is_none());

        let variants = properties["source"]["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), 5);
        for (kind, required) in [
            ("attachment", vec!["type", "readPath"]),
            ("workspace", vec!["type", "path"]),
            ("external", vec!["type", "path"]),
            ("generated_artifact", vec!["type", "uri", "path"]),
            ("skill_resource", vec!["type", "uri"]),
        ] {
            let variant = variants
                .iter()
                .find(|variant| {
                    variant["properties"]["type"]["enum"]
                        .as_array()
                        .is_some_and(|values| values == &[json!(kind)])
                })
                .unwrap_or_else(|| panic!("missing `{kind}` source schema"));
            assert_eq!(variant["additionalProperties"], false);
            assert_eq!(variant["required"], json!(required));
            let declared = variant["properties"].as_object().unwrap();
            assert_eq!(
                declared.len(),
                required.len(),
                "`{kind}` must expose only its discriminator and required fields"
            );
            for field in required {
                assert!(
                    declared.contains_key(field),
                    "`{kind}` must declare required field `{field}`"
                );
            }
        }
    }

    #[test]
    fn reads_workspace_source_by_content_even_without_an_extension() {
        let workspace = tempfile::tempdir().unwrap();
        let bytes = valid_test_png();
        fs::write(workspace.path().join("preview.bin"), &bytes).unwrap();
        let result = execute(
            &context(Some(workspace.path()), AgentPermissions::default()),
            json!({
                "source": {
                    "type": "workspace",
                    "path": "preview.bin"
                }
            }),
        )
        .unwrap();

        assert_eq!(result["path"], "preview.bin");
        assert_eq!(result["source"]["type"], "workspace");
        assert_eq!(result["format"], "png");
        assert_eq!(result["mimeType"], "image/png");
        assert_eq!(result["sizeBytes"], bytes.len() as u64);
        assert_eq!(result["sha256"], sha256_hex(&bytes));
        assert!(result["image"]["dataBase64"].as_str().unwrap().len() > 10);
    }

    #[test]
    fn consumes_an_office_render_source_without_path_reconstruction() {
        let workspace = tempfile::tempdir().unwrap();
        let bytes = valid_test_png();
        fs::create_dir_all(workspace.path().join("outputs")).unwrap();
        fs::write(workspace.path().join("outputs/report-preview.png"), &bytes).unwrap();
        let published = OfficePublishedOutput {
            role: OfficePublishedOutputRole::Render,
            kind: OfficePublishedOutputKind::Image,
            mime_type: "image/png".to_string(),
            source: AgentFileInputRef::Workspace {
                path: "outputs/report-preview.png".to_string(),
            },
            read_path: "outputs/report-preview.png".to_string(),
            scope: OfficePathScope::Workspace,
            readable_by_agent: true,
            size_bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            width: Some(1),
            height: Some(1),
            page_selection: OfficeRenderPageSelection::All,
        };

        let result = execute(
            &context(Some(workspace.path()), AgentPermissions::default()),
            json!({ "source": published.source }),
        )
        .unwrap();

        assert_eq!(
            result["source"],
            json!({
                "type": "workspace",
                "path": "outputs/report-preview.png"
            })
        );
        assert_eq!(result["sha256"], published.sha256);
        assert_eq!(result["sizeBytes"], published.size_bytes);
    }

    #[test]
    fn legacy_absolute_path_inside_workspace_is_scoped_as_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path().canonicalize().unwrap();
        let image_path = workspace_root.join("absolute.png");
        fs::write(&image_path, valid_test_png()).unwrap();

        let result = execute(
            &context(Some(&workspace_root), AgentPermissions::default()),
            json!({ "filePath": image_path }),
        )
        .unwrap();

        assert_eq!(result["path"], "absolute.png");
        assert_eq!(result["source"]["type"], "workspace");
        assert_eq!(result["source"]["path"], "absolute.png");
    }

    #[test]
    fn workspace_source_requires_a_selected_workspace() {
        let error = execute(
            &context(None, AgentPermissions::default()),
            json!({
                "source": {
                    "type": "workspace",
                    "path": "preview.png"
                }
            }),
        )
        .unwrap_err();

        assert_eq!(error.code(), Some("agent.fileInput.authorizationDenied"));
        assert_eq!(
            error.details().unwrap()["recovery"],
            json!("selectWorkspace")
        );
    }

    #[test]
    fn typed_external_path_is_permission_checked_after_workspace_scope_resolution() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path().canonicalize().unwrap();
        let inside = workspace_root.join("inside.png");
        fs::write(&inside, valid_test_png()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_root = outside.path().canonicalize().unwrap();
        let outside_path = outside_root.join("outside.png");
        fs::write(&outside_path, valid_test_png()).unwrap();

        let inside_result = execute(
            &context(Some(&workspace_root), AgentPermissions::default()),
            json!({
                "source": {
                    "type": "external",
                    "path": inside
                }
            }),
        )
        .unwrap();
        assert_eq!(inside_result["source"]["type"], "workspace");
        assert_eq!(inside_result["source"]["path"], "inside.png");

        let denied = execute(
            &context(Some(&workspace_root), AgentPermissions::default()),
            json!({
                "source": {
                    "type": "external",
                    "path": outside_path
                }
            }),
        )
        .unwrap_err();
        assert_eq!(denied.code(), Some("agent.fileInput.authorizationDenied"));
        assert_eq!(
            denied.details().unwrap()["recovery"],
            json!("changePermissions")
        );

        let all_permissions = AgentPermissions {
            read: AgentReadPermission::All,
            ..AgentPermissions::default()
        };
        let allowed = execute(
            &context(Some(&workspace_root), all_permissions),
            json!({
                "source": {
                    "type": "external",
                    "path": outside_root.join("outside.png")
                }
            }),
        )
        .unwrap();
        assert_eq!(allowed["source"]["type"], "external");
    }

    #[test]
    fn typed_attachment_requires_the_exact_authoritative_read_path_and_size() {
        let directory = tempfile::tempdir().unwrap();
        let attachment_root = directory.path().join("attachments");
        let storage_relative = "conversations/c1/m1/image-1/pixel.png";
        let attachment_path = attachment_root.join(storage_relative);
        fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
        let bytes = valid_test_png();
        fs::write(&attachment_path, &bytes).unwrap();
        let attachment_library = AgentAttachmentLibraryContext {
            root_path: Some(attachment_root.to_string_lossy().into_owned()),
            conversation_id: Some("c1".to_string()),
            project_id: None,
            conversation_attachments: vec![AgentAttachmentReference {
                id: "image-1".to_string(),
                conversation_id: "c1".to_string(),
                message_id: "m1".to_string(),
                project_id: None,
                kind: AgentInputAttachmentKind::Image,
                name: "pixel.png".to_string(),
                mime_type: Some("image/png".to_string()),
                size_bytes: bytes.len() as u64,
                read_path: "@attachments/image-1/pixel.png".to_string(),
                storage_rel_path: storage_relative.to_string(),
                created_at: 1,
            }],
            project_attachments: Vec::new(),
        };
        let run_context = AgentRunContext {
            conversation_id: Some("c1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: Some(attachment_library),
            permissions: AgentPermissions::default(),
        };
        let context = ToolExecutionContext::from_run_context(Some(&run_context))
            .with_model_capabilities(ModelCapabilities { image_input: true });

        let result = execute(
            &context,
            json!({
                "source": {
                    "type": "attachment",
                    "readPath": "@attachments/image-1/pixel.png"
                }
            }),
        )
        .unwrap();
        assert_eq!(result["source"]["type"], "attachment");
        assert_eq!(result["path"], "@attachments/image-1/pixel.png");

        fs::write(&attachment_path, [0_u8; 4]).unwrap();
        let mismatch = execute(
            &context,
            json!({
                "source": {
                    "type": "attachment",
                    "readPath": "@attachments/image-1/pixel.png"
                }
            }),
        )
        .unwrap_err();
        assert_eq!(mismatch.code(), Some("agent.fileInput.integrityMismatch"));
    }

    #[test]
    fn reads_generated_artifact_only_through_its_authoritative_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let bytes = valid_test_png();
        let (storage, path, sha256) = publish_generated_artifact(&root, &bytes);
        let context = context(None, AgentPermissions::default())
            .with_runtime_services("run-generated-image".to_string(), Some(storage));
        let source = json!({
            "type": "generated_artifact",
            "uri": format!("image-artifact://sha256/{sha256}"),
            "path": path.clone()
        });

        let result = execute(&context, json!({ "source": source.clone() })).unwrap();
        assert_eq!(result["source"]["type"], "generated_artifact");
        assert_eq!(result["path"], format!("image-artifact://sha256/{sha256}"));
        assert_eq!(result["sha256"], sha256);

        let forged = root.join("forged.png");
        fs::write(&forged, &bytes).unwrap();
        let mut forged_source = source.clone();
        forged_source["path"] = json!(forged);
        let error = execute(&context, json!({ "source": forged_source })).unwrap_err();
        assert_eq!(error.code(), Some("agent.fileInput.integrityMismatch"));

        let mut tampered = bytes;
        tampered[0] ^= 0xff;
        fs::write(&path, tampered).unwrap();
        let error = execute(&context, json!({ "source": source })).unwrap_err();
        assert_eq!(error.code(), Some("agent.fileInput.integrityMismatch"));
    }

    #[test]
    fn reads_only_revision_bound_resources_from_the_active_skill_session() {
        let bytes = valid_test_png();
        let skill_id = SkillId::parse("bundled:test:image-reader").unwrap();
        let revision = SkillRevision::parse("revision-image-reader").unwrap();
        let source_id = SkillSourceId::parse("bundled:test").unwrap();
        let resource_path = SkillResourcePath::parse("assets/pixel.png").unwrap();
        let uri = SkillPackageUri::new(skill_id.clone(), revision.clone())
            .resource(resource_path)
            .to_string();
        let session = memory_resource_session_for_test(
            skill_id,
            revision,
            source_id,
            vec![(
                "assets/pixel.png".to_string(),
                SkillResourceKind::Asset,
                bytes,
            )],
        )
        .unwrap();
        let active_context = context(None, AgentPermissions::default())
            .with_skill_resources(Some(Arc::new(session)));

        let result = execute(
            &active_context,
            json!({
                "source": {
                    "type": "skill_resource",
                    "uri": uri
                }
            }),
        )
        .unwrap();
        assert_eq!(result["source"]["type"], "skill_resource");
        assert_eq!(result["path"], uri);

        let inactive = execute(
            &context(None, AgentPermissions::default()),
            json!({
                "source": {
                    "type": "skill_resource",
                    "uri": uri
                }
            }),
        )
        .unwrap_err();
        assert_eq!(inactive.code(), Some("agent.fileInput.snapshotUnavailable"));
        assert_eq!(
            inactive.details().unwrap()["recovery"],
            json!("reactivateSkill")
        );
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_input_contracts() {
        for args in [
            json!({}),
            json!({ "path": "a.png", "filePath": "b.png" }),
            json!({
                "source": { "type": "workspace", "path": "a.png" },
                "path": "a.png"
            }),
            json!({ "source": { "type": "attachment" } }),
            json!({ "source": { "type": "workspace" } }),
            json!({ "source": { "type": "external" } }),
            json!({
                "source": {
                    "type": "generated_artifact",
                    "uri": format!("image-artifact://sha256/{}", "a".repeat(64))
                }
            }),
            json!({ "source": { "type": "skill_resource" } }),
        ] {
            let error = execute(&context(None, AgentPermissions::default()), args).unwrap_err();
            assert!(error.to_string().contains("read_image"));
        }
    }
}
