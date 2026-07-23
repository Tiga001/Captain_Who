use super::{
    block_on_tool_future, AgentTool, AgentToolCancellationSettlement, AgentToolPermissionPolicy,
    ToolExecutionContext,
};
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::image_generation::{
    ImageGenerationDataUrlInput, ImageGenerationEditRequest, ImageGenerationExecutionId,
    ImageGenerationExecutionRequest, ImageGenerationExecutionResult,
    ImageGenerationExecutionService, ImageGenerationExecutionServiceError,
    ImageGenerationExecutionServiceErrorCode, ImageGenerationExecutionStatus,
    ImageGenerationGenerateRequest, ImageGenerationOperation, ImageGenerationRequest,
    ImageGenerationSizePreset, IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION,
    MAX_IMAGE_ARTIFACT_DIMENSION, MAX_IMAGE_ARTIFACT_PIXELS, MAX_IMAGE_GENERATION_INPUT_BYTES,
};
use crate::protocol::{
    AgentError, AgentImageGenerationArtifact, AgentImageGenerationArtifactKind,
    AgentImageGenerationAudit, AgentImageGenerationFailure, AgentImageGenerationOperation,
    AgentImageGenerationResult, AgentImageGenerationResultStatus, AgentInputAttachmentKind,
    AgentResult, AgentToolCall, AgentToolDefinition, AgentToolResult, AgentToolSafety,
    AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
};
use futures_util::future::BoxFuture;
use image::{ImageFormat, ImageReader, Limits};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

const TOOL_NAME: &str = "image_generation";
const MAX_REASON_CHARS: usize = 240;
const MAX_INPUT_DECODE_ALLOC_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CONCURRENT_IMAGE_INPUT_PREPARATIONS: usize = 2;

struct ImageInputPreparationAdmission {
    available: Mutex<usize>,
    changed: Condvar,
}

struct ImageInputPreparationPermit(&'static ImageInputPreparationAdmission);

impl Drop for ImageInputPreparationPermit {
    fn drop(&mut self) {
        let mut available = self
            .0
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *available = available
            .saturating_add(1)
            .min(MAX_CONCURRENT_IMAGE_INPUT_PREPARATIONS);
        self.0.changed.notify_one();
    }
}

fn acquire_image_input_preparation(
    context: &ToolExecutionContext,
) -> AgentResult<ImageInputPreparationPermit> {
    static ADMISSION: OnceLock<ImageInputPreparationAdmission> = OnceLock::new();
    let admission = ADMISSION.get_or_init(|| ImageInputPreparationAdmission {
        available: Mutex::new(MAX_CONCURRENT_IMAGE_INPUT_PREPARATIONS),
        changed: Condvar::new(),
    });
    let mut available = admission
        .available
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    loop {
        context.check_cancelled()?;
        if *available > 0 {
            *available -= 1;
            return Ok(ImageInputPreparationPermit(admission));
        }
        available = admission
            .changed
            .wait_timeout(available, Duration::from_millis(25))
            .unwrap_or_else(|error| error.into_inner())
            .0;
    }
}

pub(super) trait ImageGenerationToolExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: ImageGenerationExecutionRequest,
        cancellation: crate::AgentCancellationToken,
    ) -> BoxFuture<'a, Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError>>;
}

impl ImageGenerationToolExecutor for ImageGenerationExecutionService {
    fn execute<'a>(
        &'a self,
        request: ImageGenerationExecutionRequest,
        cancellation: crate::AgentCancellationToken,
    ) -> BoxFuture<'a, Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError>>
    {
        Box::pin(ImageGenerationExecutionService::execute(
            self,
            request,
            cancellation,
        ))
    }
}

pub(super) struct ImageGenerationTool {
    executor: Arc<dyn ImageGenerationToolExecutor>,
}

impl ImageGenerationTool {
    pub(super) fn new(executor: Arc<ImageGenerationExecutionService>) -> Self {
        Self { executor }
    }

    #[cfg(test)]
    fn with_executor(executor: Arc<dyn ImageGenerationToolExecutor>) -> Self {
        Self { executor }
    }
}

impl AgentTool for ImageGenerationTool {
    fn definition(&self) -> AgentToolDefinition {
        image_generation_tool_definition()
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let args = parse_args(args)?;
        let execution_id = trusted_execution_id(context)?;
        let operation = args.request.operation();
        let request = match args.request {
            ImageGenerationToolRequest::Generate {
                prompt,
                size_preset,
            } => {
                let mut request = ImageGenerationGenerateRequest::new(prompt);
                request.size_preset = size_preset;
                ImageGenerationRequest::Generate(request)
            }
            ImageGenerationToolRequest::Edit {
                prompt,
                input_path,
                size_preset,
            } => {
                let input = load_authorized_image_input(context, &input_path)?;
                let mut request = ImageGenerationEditRequest::new(prompt, input);
                request.size_preset = size_preset;
                ImageGenerationRequest::Edit(request)
            }
        };
        let request = ImageGenerationExecutionRequest {
            execution_id: execution_id.clone(),
            request,
        };
        let cancellation = context.cancellation_token();
        let result = block_on_tool_future(async {
            self.executor
                .execute(request, cancellation)
                .await
                .map_err(|error| {
                    service_error(operation, &args.reason, execution_id.as_str(), error)
                })
        })?;
        execution_result(operation, &args.reason, execution_id.as_str(), result)
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        // Provider enablement is the capability boundary. Generated bytes are published only to
        // the application-managed immutable Artifact store, not to the user's filesystem, so this
        // tool deliberately does not inherit file-write or command permissions.
        AgentToolPermissionPolicy::Default
    }

    fn cancellation_settlement(&self) -> AgentToolCancellationSettlement {
        AgentToolCancellationSettlement::Authoritative
    }

    fn trace_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        let mut projected = call.clone();
        projected.reason = call
            .args
            .get("reason")
            .and_then(Value::as_str)
            .and_then(normalize_reason);
        projected
    }

    fn event_call_projection(&self, call: &AgentToolCall) -> AgentToolCall {
        let mut projected = self.trace_call_projection(call);
        let operation = call
            .args
            .get("request")
            .and_then(|request| request.get("operation"))
            .and_then(Value::as_str)
            .filter(|operation| matches!(*operation, "generate" | "edit"))
            .unwrap_or("unknown");
        let has_input_image = call
            .args
            .get("request")
            .and_then(|request| request.get("inputPath"))
            .is_some();
        projected.args = json!({
            "request": {
                "operation": operation,
                "hasInputImage": has_input_image,
            },
            "reason": projected.reason,
        });
        projected
    }

    fn trace_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }

    fn event_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }

    fn checkpoint_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        canonical_tool_result_for_context(result)
    }
}

fn image_generation_tool_definition() -> AgentToolDefinition {
    AgentToolDefinition {
        name: TOOL_NAME.to_string(),
        description: "Generate one verified image from text, or edit one authorized workspace or @attachments image, using the application-configured image provider. Pass only a typed request and a short user-facing reason. Provider URLs, credentials, model ids, data URLs, output URLs, and execution ids are resolved by the host and must never be supplied.".to_string(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "request": {
                    "description": "Typed image generation request.",
                    "oneOf": [
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "operation": { "type": "string", "enum": ["generate"] },
                                "prompt": {
                                    "type": "string",
                                    "description": "Detailed generation prompt that faithfully represents the user's request."
                                },
                                "sizePreset": { "type": "string", "enum": ["2K"] }
                            },
                            "required": ["operation", "prompt"]
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "operation": { "type": "string", "enum": ["edit"] },
                                "prompt": {
                                    "type": "string",
                                    "description": "Detailed edit instruction that faithfully represents the user's request."
                                },
                                "inputPath": {
                                    "type": "string",
                                    "description": "An authorized workspace/read-all path or the exact @attachments/... readPath returned by attachments_list."
                                },
                                "sizePreset": { "type": "string", "enum": ["2K"] }
                            },
                            "required": ["operation", "prompt", "inputPath"]
                        }
                    ]
                },
                "reason": {
                    "type": "string",
                    "maxLength": MAX_REASON_CHARS,
                    "description": "One short, single-line, user-facing explanation of this call. It is display and audit metadata, not authorization."
                }
            },
            "required": ["request", "reason"]
        }),
        // The call can consume an external service and create a managed Artifact, but it does not
        // modify the user's filesystem. The current protocol has no external-effect safety class;
        // the explicit provider enablement switch is the user's call authorization.
        safety: AgentToolSafety::ReadOnly,
        requires_workspace: false,
        requires_approval: false,
        approval_mode: crate::protocol::AgentToolApprovalMode::Never,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageGenerationToolArgs {
    request: ImageGenerationToolRequest,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ImageGenerationToolRequest {
    Generate {
        prompt: String,
        size_preset: Option<ImageGenerationSizePreset>,
    },
    Edit {
        prompt: String,
        input_path: String,
        size_preset: Option<ImageGenerationSizePreset>,
    },
}

impl ImageGenerationToolRequest {
    fn operation(&self) -> ImageGenerationOperation {
        match self {
            Self::Generate { .. } => ImageGenerationOperation::Generate,
            Self::Edit { .. } => ImageGenerationOperation::Edit,
        }
    }
}

fn parse_args(value: Value) -> AgentResult<ImageGenerationToolArgs> {
    let args: ImageGenerationToolArgs = serde_json::from_value(value).map_err(|error| {
        AgentError::structured(
            "image_generation.invalid_request",
            format!("image_generation parameters are invalid: {error}"),
            json!({
                "type": "image_generation",
                "code": "invalidRequest",
                "recovery": "changeRequest",
            }),
        )
    })?;
    let reason = validate_reason(&args.reason)?;
    match &args.request {
        ImageGenerationToolRequest::Generate { prompt, .. }
        | ImageGenerationToolRequest::Edit { prompt, .. }
            if prompt.trim().is_empty() =>
        {
            return Err(AgentError::structured(
                "image_generation.prompt_required",
                "image_generation.request.prompt must be non-empty.",
                json!({
                    "type": "image_generation",
                    "code": "promptRequired",
                    "recovery": "changeRequest",
                }),
            ));
        }
        ImageGenerationToolRequest::Edit { input_path, .. } if input_path.trim().is_empty() => {
            return Err(AgentError::structured(
                "image_generation.input_required",
                "image_generation edit requires a non-empty authorized inputPath.",
                json!({
                    "type": "image_generation",
                    "code": "inputRequired",
                    "recovery": "changeRequest",
                }),
            ));
        }
        _ => {}
    }
    Ok(ImageGenerationToolArgs {
        request: args.request,
        reason,
    })
}

fn validate_reason(value: &str) -> AgentResult<String> {
    if has_unsafe_reason_character(value) {
        return Err(AgentError::structured(
            "image_generation.reason_unsafe",
            "image_generation.reason must be one line without control or bidirectional-control characters.",
            json!({
                "type": "image_generation",
                "code": "reasonUnsafe",
                "recovery": "changeRequest",
            }),
        ));
    }
    let reason = value.trim();
    if reason.is_empty() {
        return Err(AgentError::structured(
            "image_generation.reason_required",
            "image_generation.reason must be a non-empty user-facing description.",
            json!({
                "type": "image_generation",
                "code": "reasonRequired",
                "maxLength": MAX_REASON_CHARS,
                "recovery": "changeRequest",
            }),
        ));
    }
    if reason.chars().count() > MAX_REASON_CHARS {
        return Err(AgentError::structured(
            "image_generation.reason_too_long",
            format!("image_generation.reason cannot exceed {MAX_REASON_CHARS} characters."),
            json!({
                "type": "image_generation",
                "code": "reasonTooLong",
                "maxLength": MAX_REASON_CHARS,
                "recovery": "changeRequest",
            }),
        ));
    }
    Ok(reason.to_string())
}

fn normalize_reason(value: &str) -> Option<String> {
    validate_reason(value).ok()
}

/// Returns the canonical bounded presentation reason accepted by the image Tool.
pub fn normalize_agent_image_generation_reason(value: &str) -> Option<String> {
    normalize_reason(value)
}

fn has_unsafe_reason_character(value: &str) -> bool {
    value.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202a}'
                    | '\u{202b}'
                    | '\u{202c}'
                    | '\u{202d}'
                    | '\u{202e}'
                    | '\u{2066}'
                    | '\u{2067}'
                    | '\u{2068}'
                    | '\u{2069}'
            )
    })
}

fn trusted_execution_id(context: &ToolExecutionContext) -> AgentResult<ImageGenerationExecutionId> {
    let run_id = context.run_id()?;
    let call_id = context.tool_call_id()?;
    agent_image_generation_execution_id(run_id, call_id)
}

/// Derives the process-independent execution identity used to correlate an Agent ToolCall with
/// the image-generation journal after a restart. Neither identifier is accepted from model input.
pub fn agent_image_generation_execution_id(
    run_id: &str,
    call_id: &str,
) -> AgentResult<ImageGenerationExecutionId> {
    if run_id.trim().is_empty() || call_id.trim().is_empty() {
        return Err(AgentError::new(
            "cannot create an image execution id from empty Agent identities",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(b"mycopilot-agent-image-generation-v1\0");
    hasher.update(run_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(call_id.as_bytes());
    ImageGenerationExecutionId::parse(format!("agent-v1:{:x}", hasher.finalize())).map_err(
        |error| AgentError::new(format!("cannot create trusted image execution id: {error}")),
    )
}

/// Projects a reconciled image execution into the exact Agent ToolResult contract used during a
/// live run. This is intentionally public only through the core facade so startup reconciliation
/// cannot grow a second, subtly different Artifact/audit projection.
pub fn agent_image_generation_tool_result_from_execution(
    call_id: &str,
    reason: &str,
    operation: ImageGenerationOperation,
    execution_id: &str,
    result: ImageGenerationExecutionResult,
) -> AgentToolResult {
    match execution_result(operation, reason, execution_id, result) {
        Ok(result) => AgentToolResult {
            call_id: call_id.to_string(),
            tool: TOOL_NAME.to_string(),
            ok: true,
            result: Some(result),
            error: None,
        },
        Err(error) => image_generation_error_tool_result(call_id, error),
    }
}

/// Projects a terminal journal-inspection failure without exposing its internal diagnostic text.
pub fn agent_image_generation_tool_result_from_service_error(
    call_id: &str,
    reason: &str,
    operation: ImageGenerationOperation,
    execution_id: &str,
    error: ImageGenerationExecutionServiceError,
) -> AgentToolResult {
    image_generation_error_tool_result(
        call_id,
        service_error(operation, reason, execution_id, error),
    )
}

fn image_generation_error_tool_result(call_id: &str, error: AgentError) -> AgentToolResult {
    let structured_result = error.details().cloned().map(|mut details| {
        if let (Some(code), Some(object)) = (error.code(), details.as_object_mut()) {
            object
                .entry("errorCode".to_string())
                .or_insert_with(|| json!(code));
        }
        details
    });
    AgentToolResult {
        call_id: call_id.to_string(),
        tool: TOOL_NAME.to_string(),
        ok: false,
        result: structured_result,
        error: Some(error.to_string()),
    }
}

fn load_authorized_image_input(
    context: &ToolExecutionContext,
    input_path: &str,
) -> AgentResult<ImageGenerationDataUrlInput> {
    context.check_cancelled()?;
    let authorized = authorize_image_input_path(context, input_path.trim())?;
    let _preparation_permit = acquire_image_input_preparation(context)?;
    let mut file = open_authorized_regular_file(&authorized).map_err(input_open_error)?;
    let before = file.metadata().map_err(|error| {
        input_error(
            "inputUnavailable",
            &format!("The input image metadata could not be read: {error}"),
        )
    })?;
    if before.len() == 0 || before.len() > MAX_IMAGE_GENERATION_INPUT_BYTES as u64 {
        return Err(input_error(
            "inputTooLarge",
            "The input image is empty or exceeds the input size limit.",
        ));
    }
    if authorized
        .attachment
        .as_ref()
        .is_some_and(|reference| reference.size_bytes != before.len())
    {
        return Err(input_error(
            "inputConflict",
            "The attachment content changed after it was authorized.",
        ));
    }

    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&mut file)
        .take(MAX_IMAGE_GENERATION_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            input_error(
                "inputUnavailable",
                &format!("The input image could not be read: {error}"),
            )
        })?;
    context.check_cancelled()?;
    let after = file.metadata().map_err(|error| {
        input_error(
            "inputUnavailable",
            &format!("The input image identity could not be rechecked: {error}"),
        )
    })?;
    if bytes.len() as u64 != before.len() || after.len() != before.len() {
        return Err(input_error(
            "inputConflict",
            "The input image changed while it was being read.",
        ));
    }

    validate_decoded_input(&bytes)?;
    let input = ImageGenerationDataUrlInput::from_bytes(&bytes).map_err(|error| {
        input_error(
            "inputInvalid",
            &format!("The input image is not a supported, valid PNG, JPEG, or WebP file: {error}"),
        )
    })?;
    if let Some(reference) = authorized.attachment {
        if let Some(claimed) = reference
            .mime_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if normalize_image_mime_type(claimed).as_deref() != Some(input.media_type().as_str()) {
                return Err(input_error(
                    "inputMimeMismatch",
                    "The attachment MIME type does not match its image bytes.",
                ));
            }
        }
    }
    Ok(input)
}

enum AuthorizedImageInputLocation {
    Anchored { root: PathBuf, relative: PathBuf },
    UnrestrictedAbsolute(PathBuf),
}

struct AuthorizedImageInput {
    location: AuthorizedImageInputLocation,
    attachment: Option<crate::protocol::AgentAttachmentReference>,
}

fn authorize_image_input_path(
    context: &ToolExecutionContext,
    input_path: &str,
) -> AgentResult<AuthorizedImageInput> {
    if input_path.starts_with("@attachments/") {
        let reference = context.attachment_reference_for_path(input_path)?.clone();
        if reference.kind != AgentInputAttachmentKind::Image {
            return Err(input_error(
                "inputNotImageAttachment",
                "The selected attachment was not authorized as an image attachment.",
            ));
        }
        if reference.size_bytes == 0
            || reference.size_bytes > MAX_IMAGE_GENERATION_INPUT_BYTES as u64
        {
            return Err(input_error(
                "inputTooLarge",
                "The selected image attachment is empty or exceeds the input size limit.",
            ));
        }
        let library = context.attachment_library().ok_or_else(|| {
            input_error(
                "inputUnavailable",
                "The attachment library is unavailable for this run.",
            )
        })?;
        let root = library
            .root_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                input_error(
                    "inputUnavailable",
                    "The attachment library root is unavailable.",
                )
            })?;
        let root = PathBuf::from(root).canonicalize().map_err(|error| {
            input_error(
                "inputUnavailable",
                &format!("The attachment library root is unavailable: {error}"),
            )
        })?;
        if !root.is_dir() {
            return Err(input_error(
                "inputUnavailable",
                "The attachment library root is not a directory.",
            ));
        }
        return Ok(AuthorizedImageInput {
            location: AuthorizedImageInputLocation::Anchored {
                root,
                relative: super::clean_relative_path(&reference.storage_rel_path)?,
            },
            attachment: Some(reference),
        });
    }

    let path = Path::new(input_path);
    if path.is_absolute() || is_system_path_alias(input_path) {
        if context.permissions().read != crate::protocol::AgentReadPermission::All {
            return Err(input_error(
                "inputReadScopeDenied",
                "Reading an image outside the workspace requires read access to all locations.",
            ));
        }
        let absolute = crate::system_paths::expand_system_path(input_path)
            .map_err(AgentError::new)?
            .unwrap_or_else(|| path.to_path_buf());
        if !absolute.is_absolute() {
            return Err(input_error(
                "inputInvalidPath",
                "The external image input path must be absolute.",
            ));
        }
        let direct_metadata = std::fs::symlink_metadata(&absolute).map_err(input_open_error)?;
        if direct_metadata.file_type().is_symlink() {
            return Err(input_error(
                "inputSymlinkRejected",
                "Symbolic-link image inputs are not accepted for external image generation.",
            ));
        }
        let absolute = absolute.canonicalize().map_err(input_open_error)?;
        return Ok(AuthorizedImageInput {
            location: AuthorizedImageInputLocation::UnrestrictedAbsolute(absolute),
            attachment: None,
        });
    }

    Ok(AuthorizedImageInput {
        location: AuthorizedImageInputLocation::Anchored {
            root: context.workspace_root()?,
            relative: super::clean_relative_path(input_path)?,
        },
        attachment: None,
    })
}

fn is_system_path_alias(input: &str) -> bool {
    ["~", "@home", "@desktop", "@documents", "@downloads"]
        .iter()
        .any(|alias| {
            input == *alias
                || input
                    .strip_prefix(alias)
                    .is_some_and(|remainder| matches!(remainder.chars().next(), Some('/' | '\\')))
        })
}

fn open_authorized_regular_file(authorized: &AuthorizedImageInput) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        match &authorized.location {
            AuthorizedImageInputLocation::Anchored { root, relative } => {
                open_regular_file_beneath(root, relative)
            }
            AuthorizedImageInputLocation::UnrestrictedAbsolute(path) => {
                open_absolute_regular_file(path)
            }
        }
    }

    #[cfg(windows)]
    {
        open_authorized_regular_file_windows(authorized)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let (target, scope_root) = match &authorized.location {
            AuthorizedImageInputLocation::Anchored { root, relative } => {
                (root.join(relative), Some(root.as_path()))
            }
            AuthorizedImageInputLocation::UnrestrictedAbsolute(path) => (path.clone(), None),
        };
        let canonical = target.canonicalize()?;
        if scope_root.is_some_and(|root| !canonical.starts_with(root)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "authorized image path escaped its trusted root",
            ));
        }
        let metadata = std::fs::symlink_metadata(&target)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "path is not a direct regular file",
            ));
        }
        OpenOptions::new().read(true).open(canonical)
    }
}

fn input_open_error(error: std::io::Error) -> AgentError {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return input_error(
            "inputSymlinkRejected",
            "Symbolic-link image inputs are not accepted for external image generation.",
        );
    }
    input_error(
        "inputUnavailable",
        &format!("The authorized input image could not be opened safely: {error}"),
    )
}

#[cfg(unix)]
fn open_absolute_regular_file(path: &Path) -> std::io::Result<File> {
    if !path.is_absolute() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let relative = path.strip_prefix(Path::new("/")).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "absolute image path has no filesystem root",
        )
    })?;
    open_regular_file_beneath(Path::new("/"), relative)
}

#[cfg(unix)]
fn open_regular_file_beneath(root: &Path, relative: &Path) -> std::io::Result<File> {
    use std::ffi::{CString, OsStr};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    fn component_name(component: &OsStr) -> std::io::Result<CString> {
        CString::new(component.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    }

    fn open_directory_at(parent: &File, component: &OsStr) -> std::io::Result<File> {
        let component = component_name(component)?;
        // SAFETY: `parent` owns a live directory descriptor and `component` is NUL-free for the
        // duration of the call. Ownership of a successful descriptor transfers to `File`.
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                component.as_ptr(),
                libc::O_RDONLY
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open("/")?;
    for component in root.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => directory = open_directory_at(&directory, name)?,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
            }
        }
    }

    let components = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(Ok(name)),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                Some(Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)))
            }
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let Some((file_name, parent_components)) = components.split_last() else {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    };
    for component in parent_components {
        directory = open_directory_at(&directory, component)?;
    }
    let file_name = component_name(file_name)?;
    // `O_NONBLOCK` is essential here: an attacker-controlled FIFO must not block before `fstat`
    // can reject it. `O_NOFOLLOW` prevents the final component from becoming a symlink.
    // SAFETY: the parent descriptor and NUL-free filename remain live for the call.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            file_name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let file = unsafe { File::from_raw_fd(descriptor) };
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "path is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn open_authorized_regular_file_windows(
    authorized: &AuthorizedImageInput,
) -> std::io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
    };

    let (target, scope_root) = match &authorized.location {
        AuthorizedImageInputLocation::Anchored { root, relative } => {
            (root.join(relative), Some(root.as_path()))
        }
        AuthorizedImageInputLocation::UnrestrictedAbsolute(path) => (path.clone(), None),
    };
    for path in target.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "reparse-point image paths are not accepted",
            ));
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(&target)?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "path is not a direct regular file",
        ));
    }
    if let Some(root) = scope_root {
        let opened_path = windows_final_path(&file)?;
        if !windows_path_is_within(&opened_path, root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "authorized image path escaped its trusted root",
            ));
        }
    }
    Ok(file)
}

#[cfg(windows)]
fn windows_final_path(file: &File) -> std::io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS,
    };

    let mut buffer = vec![0u16; 512];
    loop {
        let capacity = u32::try_from(buffer.len())
            .map_err(|_| std::io::Error::other("Windows path buffer exceeds u32"))?;
        // SAFETY: the file owns a live handle and the buffer exposes `capacity` writable u16s.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as HANDLE,
                buffer.as_mut_ptr(),
                capacity,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        };
        if written == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let written = usize::try_from(written)
            .map_err(|_| std::io::Error::other("Windows path length exceeds usize"))?;
        if written < buffer.len() {
            buffer.truncate(written);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        buffer.resize(written.saturating_add(1), 0);
    }
}

#[cfg(windows)]
fn windows_path_is_within(path: &Path, root: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;

    fn key(path: &Path) -> Vec<u16> {
        const FORWARD_SLASH: u16 = b'/' as u16;
        const BACKSLASH: u16 = b'\\' as u16;
        let verbatim_prefix = [BACKSLASH, BACKSLASH, b'?' as u16, BACKSLASH];
        let verbatim_unc_prefix = [
            BACKSLASH,
            BACKSLASH,
            b'?' as u16,
            BACKSLASH,
            b'U' as u16,
            b'N' as u16,
            b'C' as u16,
            BACKSLASH,
        ];
        let mut units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if units.starts_with(&verbatim_unc_prefix) {
            units.splice(..verbatim_unc_prefix.len(), [BACKSLASH, BACKSLASH]);
        } else if units.starts_with(&verbatim_prefix) {
            units.drain(..verbatim_prefix.len());
        }
        for unit in &mut units {
            if *unit == FORWARD_SLASH {
                *unit = BACKSLASH;
            } else if (b'A' as u16..=b'Z' as u16).contains(unit) {
                *unit += u16::from(b'a' - b'A');
            }
        }
        while units.last() == Some(&BACKSLASH) {
            units.pop();
        }
        units
    }

    let path = key(path);
    let root = key(root);
    path == root || (path.starts_with(&root) && path.get(root.len()).copied() == Some(b'\\' as u16))
}

fn normalize_image_mime_type(value: &str) -> Option<String> {
    let value = value
        .split(';')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_ascii_lowercase();
    match value.as_str() {
        "image/png" | "image/jpeg" | "image/webp" => Some(value),
        "image/jpg" => Some("image/jpeg".to_string()),
        _ => None,
    }
}

fn validate_decoded_input(bytes: &[u8]) -> AgentResult<()> {
    let format = image::guess_format(bytes).map_err(|_| {
        input_error(
            "inputInvalid",
            "The input image format could not be identified.",
        )
    })?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(input_error(
            "inputUnsupported",
            "Only PNG, JPEG, and WebP input images are supported.",
        ));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_ARTIFACT_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_ARTIFACT_DIMENSION);
    limits.max_alloc = Some(MAX_INPUT_DECODE_ALLOC_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| {
        input_error(
            "inputInvalid",
            "The input image could not be fully decoded within safety limits.",
        )
    })?;
    let pixels = u64::from(decoded.width()).saturating_mul(u64::from(decoded.height()));
    if decoded.width() == 0
        || decoded.height() == 0
        || decoded.width() > MAX_IMAGE_ARTIFACT_DIMENSION
        || decoded.height() > MAX_IMAGE_ARTIFACT_DIMENSION
        || pixels > MAX_IMAGE_ARTIFACT_PIXELS
    {
        return Err(input_error(
            "inputDimensionsUnsupported",
            "The input image dimensions exceed the supported safety limits.",
        ));
    }
    Ok(())
}

fn input_error(code: &str, message: &str) -> AgentError {
    AgentError::structured(
        format!("image_generation.{code}"),
        message,
        json!({
            "type": "image_generation",
            "code": code,
            "phase": "inputAuthorization",
            "recovery": "changeRequest",
        }),
    )
}

fn execution_result(
    expected_operation: ImageGenerationOperation,
    reason: &str,
    expected_execution_id: &str,
    result: ImageGenerationExecutionResult,
) -> AgentResult<Value> {
    let receipt = &result.receipt;
    if receipt.schema_version != IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION
        || receipt.execution_id != expected_execution_id
        || receipt.operation != expected_operation
    {
        return Err(AgentError::structured(
            "image_generation.receipt_identity_conflict",
            "The image generation execution receipt does not match this Agent Tool call.",
            json!({
                "type": "image_generation",
                "code": "receiptIdentityConflict",
                "operation": operation_name(expected_operation),
                "reason": reason,
                "executionId": expected_execution_id,
                "recovery": "inspectExecution",
            }),
        ));
    }
    let status = agent_result_status(receipt.status);
    let operation = agent_operation(expected_operation)?;
    let audit = AgentImageGenerationAudit {
        execution_id: receipt.execution_id.clone(),
        request_fingerprint: receipt.request_fingerprint.clone(),
        provider_profile_id: receipt.provider_profile_id.clone(),
        adapter_id: receipt.adapter_id.clone(),
        profile_revision: receipt.profile_revision,
        model_id: receipt.model_id.clone(),
        provider_request_id: receipt.provider_request_id.clone(),
        http_status: receipt.http_status,
        created_at: receipt.created_at,
        completed_at: receipt.completed_at,
        duration_ms: receipt.duration_ms,
    };
    if receipt.status == ImageGenerationExecutionStatus::Succeeded {
        let Some(published) = result.managed_artifact else {
            return Err(AgentError::structured(
                "image_generation.artifact_unavailable",
                "Image generation succeeded remotely, but no verified managed Artifact is available.",
                json!({
                    "type": "image_generation",
                    "code": "artifactUnavailable",
                    "operation": operation_name(operation_from_agent(operation)),
                    "reason": reason,
                    "executionId": receipt.execution_id,
                    "recovery": "inspectExecution",
                }),
            ));
        };
        let Some(candidate) = receipt.artifact.as_ref() else {
            return Err(AgentError::structured(
                "image_generation.artifact_unavailable",
                "Image generation succeeded without a verified Artifact receipt.",
                json!({
                    "type": "image_generation",
                    "code": "artifactUnavailable",
                    "operation": operation_name(operation_from_agent(operation)),
                    "reason": reason,
                    "executionId": receipt.execution_id,
                    "recovery": "inspectExecution",
                }),
            ));
        };
        if published.candidate != *candidate {
            return Err(AgentError::structured(
                "image_generation.artifact_conflict",
                "The published image Artifact does not match its execution receipt.",
                json!({
                    "type": "image_generation",
                    "code": "artifactConflict",
                    "operation": operation_name(operation_from_agent(operation)),
                    "reason": reason,
                    "executionId": receipt.execution_id,
                    "recovery": "inspectExecution",
                }),
            ));
        }
        let contract = AgentImageGenerationResult {
            schema_version: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
            status,
            operation,
            artifact: Some(AgentImageGenerationArtifact {
                artifact_id: candidate.artifact_id.clone(),
                uri: candidate.artifact_uri(),
                kind: AgentImageGenerationArtifactKind::Image,
                format: candidate.format,
                mime_type: candidate.media_type.clone(),
                width: candidate.width,
                height: candidate.height,
                size_bytes: candidate.size_bytes,
                sha256: candidate.sha256.clone(),
            }),
            audit,
            failure: None,
        };
        return serde_json::to_value(contract).map_err(|error| {
            AgentError::new(format!("cannot serialize image generation result: {error}"))
        });
    }

    let failure = receipt
        .error
        .as_ref()
        .map(|failure| AgentImageGenerationFailure {
            code: failure.code,
            phase: failure.phase,
            message: failure.message.clone(),
            recovery: failure.recovery.clone(),
            retryable: failure.retryable,
            generation_may_have_succeeded: failure.generation_may_have_succeeded,
            provider_succeeded: failure.provider_succeeded,
            artifact_commit_may_have_succeeded: failure.artifact_commit_may_have_succeeded,
        })
        .unwrap_or_else(|| AgentImageGenerationFailure {
            code: crate::image_generation::ImageGenerationExecutionFailureCode::JournalUnavailable,
            phase: crate::image_generation::ImageGenerationExecutionPhase::Journal,
            message: "Image generation ended without a terminal failure receipt.".to_string(),
            recovery: "Inspect the execution audit before retrying.".to_string(),
            retryable: false,
            generation_may_have_succeeded: true,
            provider_succeeded: false,
            artifact_commit_may_have_succeeded: false,
        });
    let message = failure.message.clone();
    let code = format!("image_generation.{}", failure_code_name(failure.code));
    let contract = AgentImageGenerationResult {
        schema_version: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
        status,
        operation,
        artifact: None,
        audit,
        failure: Some(failure),
    };
    let value = serde_json::to_value(contract).map_err(|error| {
        AgentError::new(format!(
            "cannot serialize image generation failure: {error}"
        ))
    })?;
    Err(AgentError::structured(code, message, value))
}

fn service_error(
    operation: ImageGenerationOperation,
    reason: &str,
    execution_id: &str,
    error: ImageGenerationExecutionServiceError,
) -> AgentError {
    let code = service_error_code(error.code);
    AgentError::structured(
        format!("image_generation.{code}"),
        service_error_message(error.code),
        json!({
            "type": "image_generation_preflight",
            "status": error.status,
            "operation": operation_name(operation),
            "reason": reason,
            "executionId": execution_id,
            "code": code,
            "phase": error.phase,
            "message": service_error_message(error.code),
            "recovery": service_error_recovery(error.code, error.status),
            "retryable": error.retryable,
            "generationMayHaveSucceeded": error.generation_may_have_succeeded,
            "providerSucceeded": error.provider_succeeded,
            "artifactCommitMayHaveSucceeded": error.artifact_commit_may_have_succeeded,
        }),
    )
}

fn operation_name(operation: ImageGenerationOperation) -> &'static str {
    match operation {
        ImageGenerationOperation::Generate => "generate",
        ImageGenerationOperation::Edit => "edit",
        ImageGenerationOperation::Status => "status",
    }
}

fn agent_result_status(status: ImageGenerationExecutionStatus) -> AgentImageGenerationResultStatus {
    match status {
        ImageGenerationExecutionStatus::Succeeded => AgentImageGenerationResultStatus::Succeeded,
        ImageGenerationExecutionStatus::Failed => AgentImageGenerationResultStatus::Failed,
        ImageGenerationExecutionStatus::Cancelled => AgentImageGenerationResultStatus::Cancelled,
        ImageGenerationExecutionStatus::OutcomeIndeterminate => {
            AgentImageGenerationResultStatus::OutcomeIndeterminate
        }
        ImageGenerationExecutionStatus::CommitIndeterminate => {
            AgentImageGenerationResultStatus::CommitIndeterminate
        }
    }
}

fn agent_operation(
    operation: ImageGenerationOperation,
) -> AgentResult<AgentImageGenerationOperation> {
    match operation {
        ImageGenerationOperation::Generate => Ok(AgentImageGenerationOperation::Generate),
        ImageGenerationOperation::Edit => Ok(AgentImageGenerationOperation::Edit),
        ImageGenerationOperation::Status => Err(AgentError::new(
            "status is not an Artifact-producing image generation operation",
        )),
    }
}

fn operation_from_agent(operation: AgentImageGenerationOperation) -> ImageGenerationOperation {
    match operation {
        AgentImageGenerationOperation::Generate => ImageGenerationOperation::Generate,
        AgentImageGenerationOperation::Edit => ImageGenerationOperation::Edit,
    }
}

fn failure_code_name(
    code: crate::image_generation::ImageGenerationExecutionFailureCode,
) -> &'static str {
    use crate::image_generation::ImageGenerationExecutionFailureCode as Code;
    match code {
        Code::ProviderFailed => "providerFailed",
        Code::Cancelled => "cancelled",
        Code::DeadlineExceeded => "deadlineExceeded",
        Code::ArtifactFailed => "artifactFailed",
        Code::CommitIndeterminate => "commitIndeterminate",
        Code::ExecutionInterrupted => "executionInterrupted",
        Code::JournalUnavailable => "journalUnavailable",
    }
}

fn service_error_code(code: ImageGenerationExecutionServiceErrorCode) -> &'static str {
    match code {
        ImageGenerationExecutionServiceErrorCode::InvalidConfiguration => "invalidConfiguration",
        ImageGenerationExecutionServiceErrorCode::InvalidRequest => "invalidRequest",
        ImageGenerationExecutionServiceErrorCode::ConfigurationDisabled => "configurationDisabled",
        ImageGenerationExecutionServiceErrorCode::ConfigurationIncomplete => {
            "configurationIncomplete"
        }
        ImageGenerationExecutionServiceErrorCode::ConfigurationUnavailable => {
            "configurationUnavailable"
        }
        ImageGenerationExecutionServiceErrorCode::Busy => "busy",
        ImageGenerationExecutionServiceErrorCode::Cancelled => "cancelled",
        ImageGenerationExecutionServiceErrorCode::DeadlineExceeded => "deadlineExceeded",
        ImageGenerationExecutionServiceErrorCode::ShuttingDown => "shuttingDown",
        ImageGenerationExecutionServiceErrorCode::IdempotencyConflict => "idempotencyConflict",
        ImageGenerationExecutionServiceErrorCode::AlreadyClaimed => "alreadyClaimed",
        ImageGenerationExecutionServiceErrorCode::JournalUnavailable => "journalUnavailable",
        ImageGenerationExecutionServiceErrorCode::JournalCorrupt => "journalCorrupt",
        ImageGenerationExecutionServiceErrorCode::CommitIndeterminate => "commitIndeterminate",
    }
}

fn service_error_message(code: ImageGenerationExecutionServiceErrorCode) -> &'static str {
    match code {
        ImageGenerationExecutionServiceErrorCode::ConfigurationDisabled => {
            "Image generation is disabled. Enable it in Settings before retrying."
        }
        ImageGenerationExecutionServiceErrorCode::ConfigurationIncomplete => {
            "Image generation configuration is incomplete. Add the endpoint, API key, and model id in Settings."
        }
        ImageGenerationExecutionServiceErrorCode::Busy => {
            "Image generation capacity is currently full. Retry later with a new request."
        }
        ImageGenerationExecutionServiceErrorCode::Cancelled => {
            "Image generation was cancelled before an external outcome was possible."
        }
        ImageGenerationExecutionServiceErrorCode::DeadlineExceeded => {
            "Image generation exceeded its execution deadline."
        }
        ImageGenerationExecutionServiceErrorCode::CommitIndeterminate => {
            "The generated Artifact commit outcome is indeterminate. Do not retry automatically."
        }
        ImageGenerationExecutionServiceErrorCode::IdempotencyConflict => {
            "This image generation call identity is already bound to different inputs."
        }
        ImageGenerationExecutionServiceErrorCode::AlreadyClaimed => {
            "This image generation call is already being settled."
        }
        ImageGenerationExecutionServiceErrorCode::ShuttingDown => {
            "Image generation is unavailable while the application is shutting down."
        }
        ImageGenerationExecutionServiceErrorCode::JournalUnavailable
        | ImageGenerationExecutionServiceErrorCode::JournalCorrupt => {
            "Image generation audit storage is unavailable."
        }
        ImageGenerationExecutionServiceErrorCode::ConfigurationUnavailable => {
            "Image generation credentials or configuration are temporarily unavailable."
        }
        ImageGenerationExecutionServiceErrorCode::InvalidConfiguration => {
            "Image generation configuration is invalid."
        }
        ImageGenerationExecutionServiceErrorCode::InvalidRequest => {
            "The image generation request is invalid."
        }
    }
}

fn service_error_recovery(
    code: ImageGenerationExecutionServiceErrorCode,
    status: ImageGenerationExecutionStatus,
) -> &'static str {
    if matches!(
        status,
        ImageGenerationExecutionStatus::OutcomeIndeterminate
            | ImageGenerationExecutionStatus::CommitIndeterminate
    ) {
        return "inspectExecution";
    }
    match code {
        ImageGenerationExecutionServiceErrorCode::ConfigurationDisabled
        | ImageGenerationExecutionServiceErrorCode::ConfigurationIncomplete
        | ImageGenerationExecutionServiceErrorCode::InvalidConfiguration => "openSettings",
        ImageGenerationExecutionServiceErrorCode::InvalidRequest
        | ImageGenerationExecutionServiceErrorCode::IdempotencyConflict => "changeRequest",
        ImageGenerationExecutionServiceErrorCode::Cancelled => "none",
        _ => "retryLater",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_generation::{
        ImageArtifactFormat, ImageArtifactPublicationStatus, ImageGenerationArtifactCandidate,
        ImageGenerationExecutionFailure, ImageGenerationExecutionFailureCode,
        ImageGenerationExecutionPhase, ImageGenerationExecutionReceipt, PublishedImageArtifact,
    };
    use crate::protocol::{
        AgentAttachmentLibraryContext, AgentAttachmentReference, AgentInputAttachmentKind,
        AgentPermissions, AgentReadPermission, AgentRunContext, AgentWorkspaceContext,
    };
    use image::ImageEncoder;
    use std::fs;
    use tempfile::TempDir;

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &[0x22, 0x44, 0x88, 0xff],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }

    fn receipt(
        status: ImageGenerationExecutionStatus,
        artifact: Option<ImageGenerationArtifactCandidate>,
        error: Option<ImageGenerationExecutionFailure>,
    ) -> ImageGenerationExecutionReceipt {
        ImageGenerationExecutionReceipt {
            schema_version: IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION,
            execution_id: "agent-v1:execution".to_string(),
            request_fingerprint: "request-sha256-v1:fingerprint".to_string(),
            status,
            provider_profile_id: "default".to_string(),
            adapter_id: "smartmlSeedream".to_string(),
            profile_revision: 2,
            model_id: "seedream-model".to_string(),
            operation: ImageGenerationOperation::Generate,
            provider_request_id: Some("opaque-provider-request".to_string()),
            http_status: Some(200),
            artifact,
            error,
            created_at: 10,
            completed_at: 20,
            duration_ms: 10,
        }
    }

    fn context(workspace: &Path) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: Some("project-1".to_string()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-1".to_string()),
                display_name: Some("test".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
        }))
        .with_runtime_services("run-1".to_string(), None)
        .with_tool_call_id("call-1".to_string())
    }

    #[test]
    fn schema_is_typed_and_keeps_provider_authority_out_of_model_input() {
        let definition = image_generation_tool_definition();
        crate::tools::schema::validate_portable_tool_input_schema(
            &definition.name,
            &definition.input_schema,
        )
        .unwrap();
        let serialized = serde_json::to_string(&definition.input_schema).unwrap();
        assert_eq!(definition.name, TOOL_NAME);
        assert!(!definition.requires_workspace);
        assert!(!definition.requires_approval);
        assert!(serialized.contains("inputPath"));
        for forbidden in ["apiKey", "endpoint", "modelId", "dataUrl", "executionId"] {
            assert!(!serialized.contains(forbidden), "schema leaked {forbidden}");
        }
    }

    #[test]
    fn parses_generate_and_edit_but_rejects_untrusted_fields() {
        let generate = parse_args(json!({
            "request": { "operation": "generate", "prompt": "A lighthouse", "sizePreset": "2K" },
            "reason": "Create the requested illustration."
        }))
        .unwrap();
        assert_eq!(
            generate.request.operation(),
            ImageGenerationOperation::Generate
        );

        let edit = parse_args(json!({
            "request": { "operation": "edit", "prompt": "Make it dusk", "inputPath": "@attachments/a/image.png" },
            "reason": "Apply the requested visual edit."
        }))
        .unwrap();
        assert_eq!(edit.request.operation(), ImageGenerationOperation::Edit);

        for field in ["apiKey", "endpoint", "modelId", "dataUrl", "executionId"] {
            let mut value = json!({
                "request": { "operation": "generate", "prompt": "test" },
                "reason": "Generate an image."
            });
            value["request"][field] = json!("untrusted");
            assert!(
                parse_args(value).is_err(),
                "accepted untrusted field {field}"
            );
        }
    }

    #[test]
    fn reason_is_required_bounded_and_safe() {
        for reason in [
            "",
            "   ",
            "line one\nline two",
            "line one\u{2028}line two",
            "unsafe\u{202e}text",
        ] {
            assert!(validate_reason(reason).is_err());
        }
        assert!(validate_reason(&"x".repeat(MAX_REASON_CHARS)).is_ok());
        assert!(validate_reason(&"x".repeat(MAX_REASON_CHARS + 1)).is_err());
    }

    #[test]
    fn trusted_execution_identity_is_stable_and_run_scoped() {
        let workspace = TempDir::new().unwrap();
        let first = context(workspace.path());
        let id = trusted_execution_id(&first).unwrap();
        assert_eq!(id, trusted_execution_id(&first).unwrap());

        let second = first
            .clone()
            .with_runtime_services("run-2".to_string(), None)
            .with_tool_call_id("call-1".to_string());
        assert_ne!(id, trusted_execution_id(&second).unwrap());
    }

    #[test]
    fn workspace_input_is_fully_validated_before_data_url_creation() {
        let workspace = TempDir::new().unwrap();
        fs::write(workspace.path().join("input.png"), png()).unwrap();
        let context = context(workspace.path());

        let input = load_authorized_image_input(&context, "input.png").unwrap();
        assert_eq!(input.media_type().as_str(), "image/png");
        assert!(input.data_url().starts_with("data:image/png;base64,"));

        fs::write(workspace.path().join("fake.png"), b"not an image").unwrap();
        assert!(load_authorized_image_input(&context, "fake.png").is_err());
        assert!(load_authorized_image_input(&context, "../input.png").is_err());
    }

    #[test]
    fn external_input_requires_read_all_and_does_not_require_a_workspace() {
        let external = TempDir::new().unwrap();
        let path = external.path().join("input.png");
        fs::write(&path, png()).unwrap();
        let workspace = TempDir::new().unwrap();
        let workspace_only = context(workspace.path());
        for unauthorized in [path.clone(), external.path().join("missing.png")] {
            let error =
                load_authorized_image_input(&workspace_only, unauthorized.to_str().unwrap())
                    .unwrap_err();
            assert_eq!(error.code(), Some("image_generation.inputReadScopeDenied"));
        }

        let read_all = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::All,
                ..AgentPermissions::default()
            },
        }));
        assert!(load_authorized_image_input(&read_all, path.to_str().unwrap()).is_ok());
        assert!(load_authorized_image_input(&read_all, "relative.png").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn direct_symlink_input_is_rejected_even_when_target_is_authorized() {
        use std::os::unix::fs::symlink;

        let workspace = TempDir::new().unwrap();
        fs::write(workspace.path().join("target.png"), png()).unwrap();
        symlink("target.png", workspace.path().join("linked.png")).unwrap();
        let context = context(workspace.path());

        let error = load_authorized_image_input(&context, "linked.png").unwrap_err();
        assert_eq!(error.code(), Some("image_generation.inputSymlinkRejected"));

        let external = TempDir::new().unwrap();
        fs::write(external.path().join("outside.png"), png()).unwrap();
        symlink(external.path(), workspace.path().join("linked-directory")).unwrap();
        assert!(load_authorized_image_input(&context, "linked-directory/outside.png").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn fifo_input_is_rejected_without_blocking_before_file_type_validation() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::time::{Duration, Instant};

        let workspace = TempDir::new().unwrap();
        let fifo_path = workspace.path().join("input.png");
        let fifo_path = CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is NUL-free and points inside the owned temporary workspace.
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        let context = context(workspace.path());
        let started = Instant::now();
        let error = load_authorized_image_input(&context, "input.png").unwrap_err();

        assert_eq!(error.code(), Some("image_generation.inputUnavailable"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn attachment_input_requires_exact_authorized_image_reference_and_matching_identity() {
        let workspace = TempDir::new().unwrap();
        let attachments = TempDir::new().unwrap();
        fs::create_dir_all(attachments.path().join("a1")).unwrap();
        let bytes = png();
        fs::write(attachments.path().join("a1/image.png"), &bytes).unwrap();
        let mut run_context = AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: None,
                root_path: Some(workspace.path().to_string_lossy().to_string()),
            }),
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: Some(attachments.path().to_string_lossy().to_string()),
                conversation_id: Some("conversation-1".to_string()),
                project_id: None,
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "a1".to_string(),
                    conversation_id: "conversation-1".to_string(),
                    message_id: "message-1".to_string(),
                    project_id: None,
                    kind: AgentInputAttachmentKind::Image,
                    name: "image.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    size_bytes: bytes.len() as u64,
                    read_path: "@attachments/a1/image.png".to_string(),
                    storage_rel_path: "a1/image.png".to_string(),
                    created_at: 1,
                }],
                project_attachments: vec![],
            }),
            permissions: AgentPermissions::default(),
        };
        let context = ToolExecutionContext::from_run_context(Some(&run_context));
        assert!(load_authorized_image_input(&context, "@attachments/a1/image.png").is_ok());
        assert!(load_authorized_image_input(&context, "@attachments/a1/other.png").is_err());

        run_context
            .attachment_library
            .as_mut()
            .unwrap()
            .conversation_attachments[0]
            .kind = AgentInputAttachmentKind::File;
        let context = ToolExecutionContext::from_run_context(Some(&run_context));
        assert!(load_authorized_image_input(&context, "@attachments/a1/image.png").is_err());
    }

    #[test]
    fn event_call_projection_exposes_reason_without_prompt_or_path() {
        struct NeverExecutor;
        impl ImageGenerationToolExecutor for NeverExecutor {
            fn execute<'a>(
                &'a self,
                _request: ImageGenerationExecutionRequest,
                _cancellation: crate::AgentCancellationToken,
            ) -> BoxFuture<
                'a,
                Result<ImageGenerationExecutionResult, ImageGenerationExecutionServiceError>,
            > {
                Box::pin(async { panic!("executor must not run") })
            }
        }
        let tool = ImageGenerationTool::with_executor(Arc::new(NeverExecutor));
        let call = AgentToolCall {
            id: "call-1".to_string(),
            tool: TOOL_NAME.to_string(),
            args: json!({
                "request": {
                    "operation": "edit",
                    "prompt": "private prompt",
                    "inputPath": "/private/input.png"
                },
                "reason": "Edit the supplied image."
            }),
            approval_status: crate::protocol::AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let event = tool.event_call_projection(&call);
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(event.reason.as_deref(), Some("Edit the supplied image."));
        assert!(!encoded.contains("private prompt"));
        assert!(!encoded.contains("/private/input.png"));
        assert!(encoded.contains("hasInputImage"));

        let mut malformed = call;
        malformed.args["request"]["operation"] = json!("untrusted-operation");
        let event = tool.event_call_projection(&malformed);
        assert_eq!(event.args["request"]["operation"], "unknown");
        assert!(!serde_json::to_string(&event)
            .unwrap()
            .contains("untrusted-operation"));
    }

    #[test]
    fn successful_execution_projects_only_the_public_artifact_contract() {
        let candidate = ImageGenerationArtifactCandidate {
            artifact_id: "sha256:artifact".to_string(),
            storage_relative_path: "objects/private/internal.png".to_string(),
            format: ImageArtifactFormat::Png,
            media_type: "image/png".to_string(),
            width: 1024,
            height: 768,
            size_bytes: 42,
            sha256: "artifact".to_string(),
        };
        let value = execution_result(
            ImageGenerationOperation::Generate,
            "Create the requested image.",
            "agent-v1:execution",
            ImageGenerationExecutionResult {
                receipt: receipt(
                    ImageGenerationExecutionStatus::Succeeded,
                    Some(candidate.clone()),
                    None,
                ),
                managed_artifact: Some(PublishedImageArtifact {
                    candidate,
                    absolute_path: PathBuf::from("/private/managed/internal.png"),
                    status: ImageArtifactPublicationStatus::Created,
                }),
            },
        )
        .unwrap();
        let contract: AgentImageGenerationResult = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(contract.status, AgentImageGenerationResultStatus::Succeeded);
        assert_eq!(
            contract
                .artifact
                .as_ref()
                .map(|artifact| artifact.uri.as_str()),
            Some("image-artifact://sha256/artifact")
        );
        let encoded = serde_json::to_string(&value).unwrap();
        assert!(!encoded.contains("storage_relative_path"));
        assert!(!encoded.contains("objects/private"));
        assert!(!encoded.contains("/private/managed"));
        assert!(!encoded.contains("https://"));
        assert!(!encoded.contains("apiKey"));
    }

    #[test]
    fn failed_execution_preserves_uncertainty_in_the_typed_contract() {
        let failure = ImageGenerationExecutionFailure {
            code: ImageGenerationExecutionFailureCode::DeadlineExceeded,
            phase: ImageGenerationExecutionPhase::Provider,
            message: "provider outcome is unknown".to_string(),
            recovery: "Inspect provider usage before retrying.".to_string(),
            retryable: false,
            generation_may_have_succeeded: true,
            provider_succeeded: false,
            artifact_commit_may_have_succeeded: false,
            provider_error_code: None,
            artifact_error_code: None,
        };
        let error = execution_result(
            ImageGenerationOperation::Generate,
            "Create the requested image.",
            "agent-v1:execution",
            ImageGenerationExecutionResult {
                receipt: receipt(
                    ImageGenerationExecutionStatus::OutcomeIndeterminate,
                    None,
                    Some(failure),
                ),
                managed_artifact: None,
            },
        )
        .unwrap_err();
        let contract: AgentImageGenerationResult =
            serde_json::from_value(error.details().unwrap().clone()).unwrap();
        assert_eq!(
            contract.status,
            AgentImageGenerationResultStatus::OutcomeIndeterminate
        );
        assert!(contract.artifact.is_none());
        assert_eq!(
            contract
                .failure
                .as_ref()
                .map(|failure| failure.generation_may_have_succeeded),
            Some(true)
        );
    }

    #[test]
    fn execution_result_rejects_a_receipt_from_another_tool_call() {
        let mut mismatched = receipt(ImageGenerationExecutionStatus::Failed, None, None);
        mismatched.execution_id = "agent-v1:another-call".to_string();
        let error = execution_result(
            ImageGenerationOperation::Generate,
            "Create the requested image.",
            "agent-v1:execution",
            ImageGenerationExecutionResult {
                receipt: mismatched,
                managed_artifact: None,
            },
        )
        .unwrap_err();

        assert_eq!(
            error.code(),
            Some("image_generation.receipt_identity_conflict")
        );
        assert_eq!(
            error.details().and_then(|details| details.get("code")),
            Some(&json!("receiptIdentityConflict"))
        );
    }

    #[test]
    fn preflight_service_failure_is_not_misrepresented_as_an_artifact_result() {
        let error = service_error(
            ImageGenerationOperation::Generate,
            "Create the requested image.",
            "agent-v1:execution",
            ImageGenerationExecutionServiceError::new(
                ImageGenerationExecutionServiceErrorCode::ConfigurationDisabled,
                "private provider detail",
                false,
            ),
        );
        let details = error.details().unwrap();
        assert_eq!(details["type"], "image_generation_preflight");
        assert_eq!(details["code"], "configurationDisabled");
        assert_eq!(details["status"], "failed");
        assert_eq!(details["phase"], "configuration");
        assert_eq!(details["generationMayHaveSucceeded"], false);
        assert_eq!(details["providerSucceeded"], false);
        assert_eq!(details["artifactCommitMayHaveSucceeded"], false);
        assert!(details.get("schemaVersion").is_none());
        assert!(details.get("artifact").is_none());
        assert!(!serde_json::to_string(details)
            .unwrap()
            .contains("private provider detail"));
    }

    #[test]
    fn preflight_service_failure_transmits_domain_uncertainty_verbatim() {
        let error = service_error(
            ImageGenerationOperation::Edit,
            "Edit the requested image.",
            "agent-v1:execution",
            ImageGenerationExecutionServiceError::new(
                ImageGenerationExecutionServiceErrorCode::JournalUnavailable,
                "private journal detail",
                true,
            ),
        );
        let details = error.details().unwrap();
        assert_eq!(details["status"], "outcomeIndeterminate");
        assert_eq!(details["phase"], "journal");
        assert_eq!(details["generationMayHaveSucceeded"], true);
        assert_eq!(details["providerSucceeded"], false);
        assert_eq!(details["artifactCommitMayHaveSucceeded"], true);
        assert_eq!(details["retryable"], false);
        assert_eq!(details["recovery"], "inspectExecution");
        assert!(!serde_json::to_string(details)
            .unwrap()
            .contains("private journal detail"));
    }
}
