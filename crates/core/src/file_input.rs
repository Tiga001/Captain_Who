//! Shared, capability-aware file inputs for trusted Agent execution adapters.
//!
//! Model-visible references are never passed to a child process directly. The trusted host freezes
//! content identity before approval, re-resolves the same logical authority immediately before
//! execution, and copies verified bytes into a private run-scoped directory. Managed processes see
//! only [`AGENT_FILE_INPUT_ROOT_ENV`] plus the stable relative `mountPath` values.

use crate::protocol::{
    AgentAttachmentLibraryContext, AgentError, AgentFileInputBinding, AgentFileInputEvidence,
    AgentFileInputRef, AgentFileInputSourceKind, AgentFileInputSpec, AgentPermissions,
    AgentReadPermission, AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
};
use crate::skills::{SkillResourceSession, SkillResourceUri};
use crate::storage::service::StorageService;
use crate::system_paths::expand_system_path;
use crate::AgentCancellationToken;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

pub const AGENT_FILE_INPUT_ROOT_ENV: &str = "MYCOPILOT_INPUT_ROOT";
pub const MAX_AGENT_FILE_INPUTS: usize = 16;
pub const MAX_AGENT_FILE_INPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_AGENT_FILE_INPUT_TOTAL_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_AGENT_FILE_INPUT_MOUNT_PATH_CHARS: usize = 240;
/// Maximum byte size accepted by the stable `read_image` visual delivery path. Managed command
/// publication shares this bound so every returned image readPath remains directly readable.
pub(crate) const MAX_AGENT_VISUAL_INPUT_BYTES: u64 = 8 * 1024 * 1024;

const ERROR_INVALID_REQUEST: &str = "agent.fileInput.invalidRequest";
const ERROR_AUTHORIZATION_DENIED: &str = "agent.fileInput.authorizationDenied";
const ERROR_NOT_FOUND: &str = "agent.fileInput.notFound";
const ERROR_TOO_LARGE: &str = "agent.fileInput.tooLarge";
const ERROR_INTEGRITY_MISMATCH: &str = "agent.fileInput.integrityMismatch";
const ERROR_SNAPSHOT_UNAVAILABLE: &str = "agent.fileInput.snapshotUnavailable";
const ERROR_IO: &str = "agent.fileInput.io";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFileInputError {
    code: &'static str,
    recovery: &'static str,
    message: String,
}

impl AgentFileInputError {
    fn new(code: &'static str, recovery: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            recovery,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn recovery(&self) -> &'static str {
        self.recovery
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for AgentFileInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentFileInputError {}

impl From<AgentFileInputError> for AgentError {
    fn from(error: AgentFileInputError) -> Self {
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
}

/// One authority-checked immutable byte snapshot for a read-only Agent tool.
///
/// Unlike prepared command inputs, this value never crosses an approval boundary and therefore
/// does not need a temporary materialization directory. The bytes are read once through the same
/// no-follow, receipt-aware authority path, and the digest describes exactly the bytes returned to
/// the caller.
pub(crate) struct VerifiedAgentFileInput {
    pub source: AgentFileInputRef,
    pub bytes: Vec<u8>,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Execution-only authority. It is assembled by the trusted host from the run snapshot and is
/// never serialized into a proposed action, checkpoint, trace, or Tool Result.
#[derive(Clone, Default)]
pub struct AgentFileInputExecutionContext {
    attachment_library: Option<AgentAttachmentLibraryContext>,
    skill_resources: Option<Arc<SkillResourceSession>>,
    storage: Option<Arc<StorageService>>,
    conversation_id: Option<String>,
}

impl AgentFileInputExecutionContext {
    pub fn new(
        attachment_library: Option<AgentAttachmentLibraryContext>,
        skill_resources: Option<Arc<SkillResourceSession>>,
    ) -> Self {
        Self {
            attachment_library,
            skill_resources,
            storage: None,
            conversation_id: None,
        }
    }

    pub(crate) fn attachment_library(&self) -> Option<&AgentAttachmentLibraryContext> {
        self.attachment_library.as_ref()
    }

    pub fn from_attachment_library(
        attachment_library: Option<AgentAttachmentLibraryContext>,
    ) -> Self {
        Self::new(attachment_library, None)
    }

    pub fn with_skill_resources(mut self, resources: Option<Arc<SkillResourceSession>>) -> Self {
        self.skill_resources = resources;
        self
    }

    pub fn with_storage(mut self, storage: Option<Arc<StorageService>>) -> Self {
        self.storage = storage;
        self
    }

    /// Binds generated Artifact lookup to the current trusted conversation identity.
    ///
    /// This value is assembled by the Host, never accepted from model input. Legacy
    /// image-generation Artifacts remain readable through their existing journal; generic
    /// managed-command Artifacts require a matching conversation grant.
    pub fn with_conversation_id(mut self, conversation_id: Option<&str>) -> Self {
        self.conversation_id = conversation_id.map(ToString::to_string);
        self
    }
}

/// Resolves the single model-facing file location into the richer internal authority reference.
///
/// Tool schemas deliberately expose only a path string. The trusted host keeps attachment,
/// generated-artifact, Skill revision, workspace, and external-file authority as an internal
/// concern so every consumer shares the same routing and integrity rules.
pub(crate) fn agent_file_input_ref_from_model_path(
    context: &AgentFileInputExecutionContext,
    model_path: &str,
) -> Result<AgentFileInputRef, AgentFileInputError> {
    let model_path = required_model_path(model_path)?;
    if model_path.starts_with("@attachments/") {
        return Ok(AgentFileInputRef::Attachment {
            read_path: model_path,
        });
    }
    if model_path.starts_with("image-artifact://sha256/")
        || model_path.starts_with("artifact://sha256/")
    {
        let (scheme, digest) = artifact_uri_identity(&model_path)?;
        let storage = context.storage.as_ref().ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_SNAPSHOT_UNAVAILABLE,
                "retry",
                "生成物的权威 Artifact 注册表不可用。",
            )
        })?;
        let artifact_id = format!("sha256:{digest}");
        let registered = resolve_artifact_for_scheme(
            storage,
            &artifact_id,
            context.conversation_id.as_deref(),
            scheme,
        )
        .map_err(|_| {
            AgentFileInputError::new(
                ERROR_SNAPSHOT_UNAVAILABLE,
                "retry",
                "无法读取生成物的权威 Artifact 发布记录。",
            )
        })?
        .ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_NOT_FOUND,
                "regenerate",
                "权威 Artifact 注册表中不存在该已发布生成物。",
            )
        })?;
        if registered.sha256 != digest {
            return Err(AgentFileInputError::new(
                ERROR_INTEGRITY_MISMATCH,
                "regenerate",
                "生成物 URI 与权威 Artifact 发布记录不一致。",
            ));
        }
        return Ok(AgentFileInputRef::GeneratedArtifact {
            uri: model_path,
            path: registered.path.to_string_lossy().into_owned(),
        });
    }
    if model_path.starts_with("skill://") {
        return Ok(AgentFileInputRef::SkillResource { uri: model_path });
    }
    let expanded = expand_system_path(&model_path).map_err(|_| {
        AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "文件路径包含无效的系统路径别名。",
        )
    })?;
    if expanded.is_some() || Path::new(&model_path).is_absolute() {
        Ok(AgentFileInputRef::External { path: model_path })
    } else {
        Ok(AgentFileInputRef::Workspace { path: model_path })
    }
}

/// Compares an original model path with a source already frozen behind an approval boundary.
///
/// This intentionally needs no live storage or attachment session. The authoritative frozen
/// source remains the execution input; the model path is accepted only when it names that source.
pub(crate) fn agent_file_input_ref_matches_model_path(
    source: &AgentFileInputRef,
    model_path: &str,
) -> Result<bool, AgentFileInputError> {
    let model_path = required_model_path(model_path)?;
    Ok(match source {
        AgentFileInputRef::Attachment { read_path } => model_path == read_path.trim(),
        AgentFileInputRef::Workspace { path } | AgentFileInputRef::External { path } => {
            model_path == path.trim()
        }
        AgentFileInputRef::GeneratedArtifact { uri, .. } => {
            artifact_uri_identity(&model_path)? == artifact_uri_identity(uri)?
        }
        AgentFileInputRef::SkillResource { uri } => model_path == uri.trim(),
    })
}

/// Returns the one location string that model-facing tools should reuse.
pub fn model_path_for_agent_file_input_ref(source: &AgentFileInputRef) -> &str {
    match source {
        AgentFileInputRef::Attachment { read_path } => read_path,
        AgentFileInputRef::Workspace { path } | AgentFileInputRef::External { path } => path,
        AgentFileInputRef::GeneratedArtifact { uri, .. }
        | AgentFileInputRef::SkillResource { uri } => uri,
    }
}

/// Derives a predictable private mount name when the model does not need to choose one.
pub(crate) fn default_agent_file_input_mount_path(
    source: &AgentFileInputRef,
    index: usize,
) -> String {
    let source_name = match source {
        AgentFileInputRef::Attachment { read_path: path }
        | AgentFileInputRef::Workspace { path }
        | AgentFileInputRef::External { path } => Path::new(path).file_name(),
        AgentFileInputRef::GeneratedArtifact { path, .. } => Path::new(path).file_name(),
        AgentFileInputRef::SkillResource { uri } => Path::new(
            uri.split_once('?')
                .map(|(path, _)| path)
                .unwrap_or(uri.as_str()),
        )
        .file_name(),
    }
    .and_then(|name| name.to_str())
    .map(str::trim)
    .filter(|name| !name.is_empty() && *name != "." && *name != "..");
    source_name
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("input-{}", index.saturating_add(1)))
}

fn required_model_path(value: &str) -> Result<String, AgentFileInputError> {
    let value = value.trim();
    if value.is_empty()
        || value.contains('\0')
        || value.contains('\n')
        || value.contains('\r')
        || value.chars().count() > 32 * 1024
    {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "文件路径必须是非空的单行路径字符串。",
        ));
    }
    Ok(value.to_string())
}

impl std::fmt::Debug for AgentFileInputExecutionContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentFileInputExecutionContext")
            .field(
                "attachment_count",
                &self
                    .attachment_library
                    .as_ref()
                    .map(|library| {
                        library.conversation_attachments.len() + library.project_attachments.len()
                    })
                    .unwrap_or_default(),
            )
            .field(
                "skill_resource_session",
                &self
                    .skill_resources
                    .as_ref()
                    .map(|resources| resources.len()),
            )
            .field("storage", &self.storage.as_ref().map(|_| "configured"))
            .finish()
    }
}

/// Owns the private run-scoped directory. Dropping this value recursively removes every
/// materialized input, including failure/timeout/cancellation paths.
pub(crate) struct PreparedAgentFileInputs {
    directory: TempDir,
    evidence: Vec<AgentFileInputEvidence>,
}

impl std::fmt::Debug for PreparedAgentFileInputs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedAgentFileInputs")
            .field("file_count", &self.evidence.len())
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl PreparedAgentFileInputs {
    pub(crate) fn root(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn evidence(&self) -> &[AgentFileInputEvidence] {
        &self.evidence
    }
}

impl Drop for PreparedAgentFileInputs {
    fn drop(&mut self) {
        // Windows refuses to delete files carrying FILE_ATTRIBUTE_READONLY. Every materialized
        // input deliberately carries that attribute, while TempDir's destructor performs only a
        // best-effort remove_dir_all and cannot report cleanup failure. Clear the attribute
        // recursively before TempDir runs so normal, failed, timed-out, and cancelled executions
        // do not leave private attachment, Artifact, or Skill bytes in the system temp directory.
        //
        // Never follow a symlink that a managed child may have attempted to add to the input tree.
        #[cfg(windows)]
        clear_windows_readonly_tree(self.directory.path());
    }
}

#[cfg(windows)]
fn clear_windows_readonly_tree(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                clear_windows_readonly_tree(&entry.path());
            }
        }
    }
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}

/// Resolves and freezes model-visible input specs before an approval boundary.
pub(crate) fn prepare_agent_file_input_bindings(
    workspace_root: Option<&Path>,
    permissions: AgentPermissions,
    context: &AgentFileInputExecutionContext,
    specs: &[AgentFileInputSpec],
    cancellation: Option<&AgentCancellationToken>,
) -> Result<Vec<AgentFileInputBinding>, AgentFileInputError> {
    let specs = normalize_agent_file_input_specs(specs)?;
    let root = canonical_workspace_root(workspace_root)?;
    let mut total = 0_u64;
    let mut bindings = Vec::with_capacity(specs.len());
    for spec in &specs {
        check_cancelled(cancellation)?;
        let bytes = read_authorized_source(
            root.as_deref(),
            permissions,
            context,
            &spec.source,
            cancellation,
            MAX_AGENT_FILE_INPUT_BYTES,
        )?;
        let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        total = total.checked_add(size_bytes).ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_TOO_LARGE,
                "changeRequest",
                "声明式输入的总大小超过安全上限。",
            )
        })?;
        if total > MAX_AGENT_FILE_INPUT_TOTAL_BYTES {
            return Err(AgentFileInputError::new(
                ERROR_TOO_LARGE,
                "changeRequest",
                format!(
                    "声明式输入总大小最多允许 {} MiB。",
                    MAX_AGENT_FILE_INPUT_TOTAL_BYTES / (1024 * 1024)
                ),
            ));
        }
        bindings.push(AgentFileInputBinding {
            schema_version: AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
            mount_path: normalize_mount_path(&spec.mount_path)?,
            source: spec.source.clone(),
            size_bytes,
            sha256: sha256_hex(&bytes),
        });
    }
    Ok(bindings)
}

pub(crate) fn normalize_agent_file_input_specs(
    specs: &[AgentFileInputSpec],
) -> Result<Vec<AgentFileInputSpec>, AgentFileInputError> {
    validate_input_specs(specs)?;
    specs
        .iter()
        .map(|spec| {
            Ok(AgentFileInputSpec {
                mount_path: normalize_mount_path(&spec.mount_path)?,
                source: normalize_source_ref(&spec.source)?,
            })
        })
        .collect()
}

/// Revalidates frozen identities and creates the private execution view.
pub(crate) fn materialize_agent_file_inputs(
    workspace_root: Option<&Path>,
    permissions: AgentPermissions,
    context: &AgentFileInputExecutionContext,
    bindings: &[AgentFileInputBinding],
    cancellation: Option<&AgentCancellationToken>,
) -> Result<Option<PreparedAgentFileInputs>, AgentFileInputError> {
    if bindings.is_empty() {
        return Ok(None);
    }
    validate_bindings(bindings)?;
    let root = canonical_workspace_root(workspace_root)?;
    let mut resolved = Vec::with_capacity(bindings.len());
    let mut total = 0_u64;
    for binding in bindings {
        check_cancelled(cancellation)?;
        let bytes = read_authorized_source(
            root.as_deref(),
            permissions,
            context,
            &binding.source,
            cancellation,
            MAX_AGENT_FILE_INPUT_BYTES,
        )?;
        let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let sha256 = sha256_hex(&bytes);
        if size_bytes != binding.size_bytes || sha256 != binding.sha256 {
            return Err(AgentFileInputError::new(
                ERROR_INTEGRITY_MISMATCH,
                "reprepare",
                format!(
                    "输入 `{}` 在准备后发生变化；命令未启动，请重新准备并审批。",
                    binding.mount_path
                ),
            ));
        }
        total = total.checked_add(size_bytes).ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_TOO_LARGE,
                "changeRequest",
                "声明式输入的总大小超过安全上限。",
            )
        })?;
        if total > MAX_AGENT_FILE_INPUT_TOTAL_BYTES {
            return Err(AgentFileInputError::new(
                ERROR_TOO_LARGE,
                "changeRequest",
                "声明式输入的总大小超过安全上限。",
            ));
        }
        resolved.push((binding, bytes));
    }

    let directory = tempfile::Builder::new()
        .prefix("mycopilot-command-inputs-")
        .tempdir()
        .map_err(|_| {
            AgentFileInputError::new(ERROR_IO, "retry", "无法创建托管命令的私有输入目录。")
        })?;
    set_private_directory_permissions(directory.path()).map_err(|_| {
        AgentFileInputError::new(ERROR_IO, "retry", "无法保护托管命令的私有输入目录。")
    })?;

    let mut evidence = Vec::with_capacity(resolved.len());
    for (binding, bytes) in resolved {
        check_cancelled(cancellation)?;
        let target = join_mount_path(directory.path(), &binding.mount_path)?;
        create_private_parent_directories(directory.path(), &target)?;
        write_private_read_only_file(&target, &bytes)?;
        evidence.push(evidence_from_binding(binding));
    }
    sync_directory(directory.path()).map_err(|_| {
        AgentFileInputError::new(ERROR_IO, "retry", "无法持久化托管命令的私有输入目录。")
    })?;

    Ok(Some(PreparedAgentFileInputs {
        directory,
        evidence,
    }))
}

/// Reads one model-visible file reference through the shared authority resolver.
///
/// Absolute or aliased references that canonicalize inside the selected workspace are normalized
/// to a workspace reference before permission checks. This makes read scope depend on the actual
/// file location rather than the spelling of the input while retaining `read=all` for genuinely
/// external files. Generated Artifacts and Skill resources remain bound to their authoritative
/// receipts and revisions.
pub(crate) fn read_verified_agent_file_input(
    workspace_root: Option<&Path>,
    permissions: AgentPermissions,
    context: &AgentFileInputExecutionContext,
    source: &AgentFileInputRef,
    cancellation: Option<&AgentCancellationToken>,
    max_bytes: u64,
) -> Result<VerifiedAgentFileInput, AgentFileInputError> {
    if max_bytes == 0 || max_bytes > MAX_AGENT_FILE_INPUT_BYTES {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "文件输入读取上限无效。",
        ));
    }

    let root = canonical_workspace_root(workspace_root)?;
    let source = normalize_read_source_scope(root.as_deref(), normalize_source_ref(source)?)?;
    let bytes = read_authorized_source(
        root.as_deref(),
        permissions,
        context,
        &source,
        cancellation,
        max_bytes,
    )?;
    let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    ensure_size_with_limit(size_bytes, max_bytes)?;
    let sha256 = sha256_hex(&bytes);

    Ok(VerifiedAgentFileInput {
        source,
        bytes,
        size_bytes,
        sha256,
    })
}

/// Resolves the physical identity of an already-authorized file input when one exists.
///
/// Callers use this only after the ordinary input binding path has verified content authority.
/// Revision-bound Skill resources intentionally return `None`: they have no model-addressable
/// filesystem identity and cannot alias a normal Office save-as destination.
pub(crate) fn resolve_verified_agent_file_input_path(
    workspace_root: Option<&Path>,
    permissions: AgentPermissions,
    context: &AgentFileInputExecutionContext,
    source: &AgentFileInputRef,
) -> Result<Option<PathBuf>, AgentFileInputError> {
    let root = canonical_workspace_root(workspace_root)?;
    let source = normalize_read_source_scope(root.as_deref(), normalize_source_ref(source)?)?;
    match source {
        AgentFileInputRef::Attachment { read_path } => {
            resolve_attachment(context, &read_path).map(|(path, _)| Some(path))
        }
        AgentFileInputRef::Workspace { path } => {
            let root = root.as_deref().ok_or_else(|| {
                AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "selectWorkspace",
                    "workspace 输入需要先选择 workspace。",
                )
            })?;
            let canonical = canonical_regular_path(&root.join(clean_relative_source_path(&path)?))?;
            if !canonical.starts_with(root) {
                return Err(AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "changeRequest",
                    "workspace 输入必须位于当前 workspace 内。",
                ));
            }
            Ok(Some(canonical))
        }
        AgentFileInputRef::External { path } => {
            if permissions.read != AgentReadPermission::All {
                return Err(AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "changePermissions",
                    "external 输入需要 read=all 权限。",
                ));
            }
            resolve_external_path(&path).map(Some)
        }
        AgentFileInputRef::GeneratedArtifact { uri, path } => {
            let (scheme, expected) = artifact_uri_identity(&uri)?;
            let storage = context.storage.as_ref().ok_or_else(|| {
                AgentFileInputError::new(
                    ERROR_SNAPSHOT_UNAVAILABLE,
                    "retry",
                    "生成物的权威 Artifact 注册表不可用。",
                )
            })?;
            let artifact_id = format!("sha256:{expected}");
            let registered = resolve_artifact_for_scheme(
                storage,
                &artifact_id,
                context.conversation_id.as_deref(),
                scheme,
            )
            .map_err(|_| {
                AgentFileInputError::new(
                    ERROR_SNAPSHOT_UNAVAILABLE,
                    "retry",
                    "无法读取生成物的权威 Artifact 发布记录。",
                )
            })?
            .ok_or_else(|| {
                AgentFileInputError::new(
                    ERROR_NOT_FOUND,
                    "regenerate",
                    "权威 Artifact 注册表中不存在该已发布生成物。",
                )
            })?;
            if registered.sha256 != expected {
                return Err(AgentFileInputError::new(
                    ERROR_INTEGRITY_MISMATCH,
                    "regenerate",
                    "生成物 URI 与权威 Artifact 发布记录不一致。",
                ));
            }
            let registered_path = canonical_regular_path(&registered.path)?;
            if resolve_generated_artifact_path(&path)? != registered_path {
                return Err(AgentFileInputError::new(
                    ERROR_INTEGRITY_MISMATCH,
                    "regenerate",
                    "generated_artifact.path 与权威 Artifact 发布位置不一致。",
                ));
            }
            Ok(Some(registered_path))
        }
        AgentFileInputRef::SkillResource { .. } => Ok(None),
    }
}

pub(crate) fn evidence_from_bindings(
    bindings: &[AgentFileInputBinding],
) -> Vec<AgentFileInputEvidence> {
    bindings.iter().map(evidence_from_binding).collect()
}

fn evidence_from_binding(binding: &AgentFileInputBinding) -> AgentFileInputEvidence {
    AgentFileInputEvidence {
        mount_path: binding.mount_path.clone(),
        source_kind: source_kind(&binding.source),
        size_bytes: binding.size_bytes,
        sha256: binding.sha256.clone(),
    }
}

fn source_kind(source: &AgentFileInputRef) -> AgentFileInputSourceKind {
    match source {
        AgentFileInputRef::Attachment { .. } => AgentFileInputSourceKind::Attachment,
        AgentFileInputRef::Workspace { .. } => AgentFileInputSourceKind::Workspace,
        AgentFileInputRef::External { .. } => AgentFileInputSourceKind::External,
        AgentFileInputRef::GeneratedArtifact { .. } => AgentFileInputSourceKind::GeneratedArtifact,
        AgentFileInputRef::SkillResource { .. } => AgentFileInputSourceKind::SkillResource,
    }
}

fn normalize_source_ref(
    source: &AgentFileInputRef,
) -> Result<AgentFileInputRef, AgentFileInputError> {
    let required = |value: &str, field: &str| {
        let value = value.trim();
        if value.is_empty()
            || value.contains('\0')
            || value.contains('\n')
            || value.contains('\r')
            || value.chars().count() > 32 * 1024
        {
            Err(AgentFileInputError::new(
                ERROR_INVALID_REQUEST,
                "changeRequest",
                format!("run_command.inputs.source.{field} 无效。"),
            ))
        } else {
            Ok(value.to_string())
        }
    };
    Ok(match source {
        AgentFileInputRef::Attachment { read_path } => AgentFileInputRef::Attachment {
            read_path: required(read_path, "readPath")?,
        },
        AgentFileInputRef::Workspace { path } => AgentFileInputRef::Workspace {
            path: required(path, "path")?,
        },
        AgentFileInputRef::External { path } => AgentFileInputRef::External {
            path: required(path, "path")?,
        },
        AgentFileInputRef::GeneratedArtifact { uri, path } => {
            AgentFileInputRef::GeneratedArtifact {
                uri: required(uri, "uri")?,
                path: required(path, "path")?,
            }
        }
        AgentFileInputRef::SkillResource { uri } => AgentFileInputRef::SkillResource {
            uri: required(uri, "uri")?,
        },
    })
}

fn normalize_read_source_scope(
    workspace_root: Option<&Path>,
    source: AgentFileInputRef,
) -> Result<AgentFileInputRef, AgentFileInputError> {
    let AgentFileInputRef::External { path } = source else {
        return Ok(source);
    };
    let Some(workspace_root) = workspace_root else {
        return Ok(AgentFileInputRef::External { path });
    };

    let canonical = resolve_external_path(&path)?;
    let Ok(relative) = canonical.strip_prefix(workspace_root) else {
        return Ok(AgentFileInputRef::External { path });
    };
    let relative = portable_relative_path(relative)?;
    Ok(AgentFileInputRef::Workspace { path: relative })
}

fn portable_relative_path(path: &Path) -> Result<String, AgentFileInputError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| {
                    AgentFileInputError::new(
                        ERROR_INVALID_REQUEST,
                        "changeRequest",
                        "workspace 文件输入路径必须是有效 UTF-8。",
                    )
                })?;
                parts.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "changeRequest",
                    "规范化后的 workspace 文件输入路径无效。",
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "workspace 文件输入必须指向普通文件。",
        ));
    }
    Ok(parts.join("/"))
}

fn validate_input_specs(specs: &[AgentFileInputSpec]) -> Result<(), AgentFileInputError> {
    if specs.len() > MAX_AGENT_FILE_INPUTS {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            format!("run_command.inputs 最多允许 {MAX_AGENT_FILE_INPUTS} 项。"),
        ));
    }
    let mounts = specs
        .iter()
        .map(|spec| normalize_mount_path(&spec.mount_path))
        .collect::<Result<Vec<_>, _>>()?;
    validate_mount_set(&mounts)
}

fn validate_bindings(bindings: &[AgentFileInputBinding]) -> Result<(), AgentFileInputError> {
    if bindings.len() > MAX_AGENT_FILE_INPUTS {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "reprepare",
            "冻结的声明式输入数量无效；请重新准备命令。",
        ));
    }
    let mut mounts = Vec::with_capacity(bindings.len());
    let mut total = 0_u64;
    for binding in bindings {
        let normalized_mount = normalize_mount_path(&binding.mount_path)?;
        let normalized_source = normalize_source_ref(&binding.source)?;
        if binding.schema_version != AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION
            || !is_lower_hex_sha256(&binding.sha256)
            || binding.size_bytes > MAX_AGENT_FILE_INPUT_BYTES
            || normalized_mount != binding.mount_path
            || normalized_source != binding.source
        {
            return Err(AgentFileInputError::new(
                ERROR_INVALID_REQUEST,
                "reprepare",
                "冻结的声明式输入契约无效；请重新准备命令。",
            ));
        }
        total = total.checked_add(binding.size_bytes).ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_TOO_LARGE,
                "reprepare",
                "冻结的声明式输入总大小无效；请重新准备命令。",
            )
        })?;
        mounts.push(normalized_mount);
    }
    if total > MAX_AGENT_FILE_INPUT_TOTAL_BYTES {
        return Err(AgentFileInputError::new(
            ERROR_TOO_LARGE,
            "reprepare",
            "冻结的声明式输入总大小超过安全上限；请重新准备命令。",
        ));
    }
    validate_mount_set(&mounts)
}

fn validate_mount_set(mounts: &[String]) -> Result<(), AgentFileInputError> {
    let mut normalized = BTreeSet::new();
    for mount in mounts {
        let folded = mount.to_ascii_lowercase();
        if !normalized.insert(folded.clone()) {
            return Err(AgentFileInputError::new(
                ERROR_INVALID_REQUEST,
                "changeRequest",
                format!("run_command.inputs 包含重复的 mountPath `{mount}`。"),
            ));
        }
        for existing in &normalized {
            if existing == &folded {
                continue;
            }
            if existing
                .strip_prefix(&folded)
                .is_some_and(|suffix| suffix.starts_with('/'))
                || folded
                    .strip_prefix(existing)
                    .is_some_and(|suffix| suffix.starts_with('/'))
            {
                return Err(AgentFileInputError::new(
                    ERROR_INVALID_REQUEST,
                    "changeRequest",
                    "run_command.inputs 的 mountPath 不能互为文件和父目录。",
                ));
            }
        }
    }
    Ok(())
}

fn normalize_mount_path(value: &str) -> Result<String, AgentFileInputError> {
    let value = value.trim();
    if value.is_empty()
        || value.contains('\0')
        || value.contains('\n')
        || value.contains('\r')
        || value.chars().count() > MAX_AGENT_FILE_INPUT_MOUNT_PATH_CHARS
    {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "run_command.inputs.mountPath 为空、过长或包含不支持的控制字符。",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "run_command.inputs.mountPath 必须是私有输入根下的相对路径。",
        ));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| {
                    AgentFileInputError::new(
                        ERROR_INVALID_REQUEST,
                        "changeRequest",
                        "run_command.inputs.mountPath 必须是有效 UTF-8 路径。",
                    )
                })?;
                if part.is_empty() || part == "." || part == ".." {
                    return Err(AgentFileInputError::new(
                        ERROR_INVALID_REQUEST,
                        "changeRequest",
                        "run_command.inputs.mountPath 包含不安全的路径片段。",
                    ));
                }
                parts.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AgentFileInputError::new(
                    ERROR_INVALID_REQUEST,
                    "changeRequest",
                    "run_command.inputs.mountPath 不能包含 `..`、根目录或平台前缀。",
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "run_command.inputs.mountPath 不能为空。",
        ));
    }
    Ok(parts.join("/"))
}

fn canonical_workspace_root(
    workspace_root: Option<&Path>,
) -> Result<Option<PathBuf>, AgentFileInputError> {
    workspace_root
        .map(|root| {
            root.canonicalize()
                .map_err(|_| {
                    AgentFileInputError::new(
                        ERROR_NOT_FOUND,
                        "selectWorkspace",
                        "声明式输入所需的 workspace 不可访问。",
                    )
                })
                .and_then(|root| {
                    if root.is_dir() {
                        Ok(root)
                    } else {
                        Err(AgentFileInputError::new(
                            ERROR_INVALID_REQUEST,
                            "selectWorkspace",
                            "声明式输入所需的 workspace 不是目录。",
                        ))
                    }
                })
        })
        .transpose()
}

fn read_authorized_source(
    workspace_root: Option<&Path>,
    permissions: AgentPermissions,
    context: &AgentFileInputExecutionContext,
    source: &AgentFileInputRef,
    cancellation: Option<&AgentCancellationToken>,
    max_bytes: u64,
) -> Result<Vec<u8>, AgentFileInputError> {
    match source {
        AgentFileInputRef::Attachment { read_path } => {
            let (path, expected_size) = resolve_attachment(context, read_path)?;
            let bytes = read_regular_file(&path, cancellation, max_bytes)?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_size {
                return Err(AgentFileInputError::new(
                    ERROR_INTEGRITY_MISMATCH,
                    "listAttachments",
                    "附件内容大小与权威附件记录不一致。",
                ));
            }
            Ok(bytes)
        }
        AgentFileInputRef::Workspace { path } => {
            let root = workspace_root.ok_or_else(|| {
                AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "selectWorkspace",
                    "workspace 输入需要先选择 workspace。",
                )
            })?;
            let relative = clean_relative_source_path(path)?;
            let lexical = root.join(relative);
            let canonical = canonical_regular_path(&lexical)?;
            if !canonical.starts_with(root) {
                return Err(AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "changeRequest",
                    "workspace 输入必须位于当前 workspace 内。",
                ));
            }
            read_regular_file(&canonical, cancellation, max_bytes)
        }
        AgentFileInputRef::External { path } => {
            if permissions.read != AgentReadPermission::All {
                return Err(AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "changePermissions",
                    "external 输入需要 read=all 权限。",
                ));
            }
            let path = resolve_external_path(path)?;
            read_regular_file(&path, cancellation, max_bytes)
        }
        AgentFileInputRef::GeneratedArtifact { uri, path } => {
            let (scheme, expected) = artifact_uri_identity(uri)?;
            let storage = context.storage.as_ref().ok_or_else(|| {
                AgentFileInputError::new(
                    ERROR_SNAPSHOT_UNAVAILABLE,
                    "retry",
                    "生成物的权威 Artifact 注册表不可用。",
                )
            })?;
            let artifact_id = format!("sha256:{expected}");
            let registered = resolve_artifact_for_scheme(
                storage,
                &artifact_id,
                context.conversation_id.as_deref(),
                scheme,
            )
            .map_err(|_| {
                AgentFileInputError::new(
                    ERROR_SNAPSHOT_UNAVAILABLE,
                    "retry",
                    "无法读取生成物的权威 Artifact 发布记录。",
                )
            })?
            .ok_or_else(|| {
                AgentFileInputError::new(
                    ERROR_NOT_FOUND,
                    "regenerate",
                    "权威 Artifact 注册表中不存在该已发布生成物。",
                )
            })?;
            if registered.sha256 != expected {
                return Err(AgentFileInputError::new(
                    ERROR_INTEGRITY_MISMATCH,
                    "regenerate",
                    "生成物 URI 与权威 Artifact 发布记录不一致。",
                ));
            }
            let registered_path = canonical_regular_path(&registered.path)?;
            let hinted_path = resolve_generated_artifact_path(path)?;
            if hinted_path != registered_path {
                return Err(AgentFileInputError::new(
                    ERROR_INTEGRITY_MISMATCH,
                    "regenerate",
                    "generated_artifact.path 与权威 Artifact 发布位置不一致。",
                ));
            }
            ensure_size_with_limit(registered.size_bytes, max_bytes)?;
            let bytes = read_regular_file(&registered_path, cancellation, max_bytes)?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != registered.size_bytes
                || sha256_hex(&bytes) != registered.sha256
            {
                return Err(AgentFileInputError::new(
                    ERROR_INTEGRITY_MISMATCH,
                    "regenerate",
                    "已发布生成物未通过权威内容身份复验。",
                ));
            }
            Ok(bytes)
        }
        AgentFileInputRef::SkillResource { uri } => {
            let resources = context.skill_resources.as_deref().ok_or_else(|| {
                AgentFileInputError::new(
                    ERROR_SNAPSHOT_UNAVAILABLE,
                    "reactivateSkill",
                    "当前运行没有可访问该输入的 Skill 资源快照。",
                )
            })?;
            let uri = SkillResourceUri::parse(uri).map_err(|_| {
                AgentFileInputError::new(
                    ERROR_INVALID_REQUEST,
                    "listResources",
                    "skill_resource 输入必须使用完整的 revision-bound skill:// URI。",
                )
            })?;
            let snapshot = resources.read_verified_bytes(&uri).map_err(|error| {
                AgentFileInputError::new(
                    match error.code().stable_name() {
                        "integrityMismatch" => ERROR_INTEGRITY_MISMATCH,
                        "snapshotUnavailable" => ERROR_SNAPSHOT_UNAVAILABLE,
                        "resourceNotFound" => ERROR_NOT_FOUND,
                        _ => ERROR_INVALID_REQUEST,
                    },
                    error.recovery().stable_name(),
                    error.message(),
                )
            })?;
            ensure_size_with_limit(
                u64::try_from(snapshot.bytes.len()).unwrap_or(u64::MAX),
                max_bytes,
            )?;
            Ok(snapshot.bytes)
        }
    }
}

fn resolve_attachment(
    context: &AgentFileInputExecutionContext,
    read_path: &str,
) -> Result<(PathBuf, u64), AgentFileInputError> {
    let read_path = read_path.trim();
    let id = read_path
        .strip_prefix("@attachments/")
        .and_then(|rest| rest.split('/').next())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_INVALID_REQUEST,
                "listAttachments",
                "attachment 输入必须使用 attachments_list 返回的完整 readPath。",
            )
        })?;
    let library = context.attachment_library.as_ref().ok_or_else(|| {
        AgentFileInputError::new(
            ERROR_NOT_FOUND,
            "listAttachments",
            "当前运行没有可用的附件库。",
        )
    })?;
    let reference = library
        .conversation_attachments
        .iter()
        .chain(library.project_attachments.iter())
        .find(|reference| reference.id == id)
        .filter(|reference| reference.read_path == read_path)
        .ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_NOT_FOUND,
                "listAttachments",
                "未找到与完整 readPath 匹配的附件。",
            )
        })?;
    ensure_size(reference.size_bytes)?;
    let root = library
        .root_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AgentFileInputError::new(
                ERROR_SNAPSHOT_UNAVAILABLE,
                "retry",
                "附件库的执行授权不可用。",
            )
        })?;
    let root = Path::new(root).canonicalize().map_err(|_| {
        AgentFileInputError::new(
            ERROR_SNAPSHOT_UNAVAILABLE,
            "retry",
            "附件库的执行授权不可用。",
        )
    })?;
    if !root.is_dir() {
        return Err(AgentFileInputError::new(
            ERROR_SNAPSHOT_UNAVAILABLE,
            "retry",
            "附件库的执行授权不可用。",
        ));
    }
    let relative = clean_relative_source_path(&reference.storage_rel_path)?;
    let lexical = root.join(relative);
    let canonical = canonical_regular_path(&lexical)?;
    if !canonical.starts_with(&root) {
        return Err(AgentFileInputError::new(
            ERROR_AUTHORIZATION_DENIED,
            "listAttachments",
            "附件输入未通过附件库边界校验。",
        ));
    }
    Ok((canonical, reference.size_bytes))
}

fn resolve_external_path(value: &str) -> Result<PathBuf, AgentFileInputError> {
    let value = value.trim();
    let expanded = expand_system_path(value).map_err(|_| {
        AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "external 输入包含无效的系统路径别名。",
        )
    })?;
    let path = expanded.as_deref().unwrap_or_else(|| Path::new(value));
    if !path.is_absolute() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "external 输入必须是绝对路径或支持的系统路径别名。",
        ));
    }
    canonical_regular_path(path)
}

fn resolve_generated_artifact_path(value: &str) -> Result<PathBuf, AgentFileInputError> {
    let value = value.trim();
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "generated_artifact 输入必须使用生成结果返回的绝对 savedPath。",
        ));
    }
    canonical_regular_path(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactUriScheme {
    Image,
    Document,
}

fn artifact_uri_identity(uri: &str) -> Result<(ArtifactUriScheme, String), AgentFileInputError> {
    let uri = uri.trim();
    let (scheme, digest) = if let Some(digest) = uri.strip_prefix("image-artifact://sha256/") {
        (ArtifactUriScheme::Image, digest)
    } else if let Some(digest) = uri.strip_prefix("artifact://sha256/") {
        (ArtifactUriScheme::Document, digest)
    } else {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "generated_artifact 输入必须使用内容寻址的 Artifact URI。",
        ));
    };
    if !is_lower_hex_sha256(digest) {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "generated_artifact 输入必须使用内容寻址的 Artifact URI。",
        ));
    }
    Ok((scheme, digest.to_string()))
}

fn resolve_artifact_for_scheme(
    storage: &StorageService,
    artifact_id: &str,
    conversation_id: Option<&str>,
    scheme: ArtifactUriScheme,
) -> Result<Option<crate::storage::service::ResolvedGeneratedArtifactInput>, String> {
    match scheme {
        ArtifactUriScheme::Image => {
            storage.resolve_published_generated_artifact_input(artifact_id, conversation_id)
        }
        ArtifactUriScheme::Document => {
            storage.resolve_published_document_artifact_input(artifact_id, conversation_id)
        }
    }
}

fn clean_relative_source_path(value: &str) -> Result<PathBuf, AgentFileInputError> {
    let value = value.trim();
    let path = Path::new(value);
    if value.is_empty() || path.is_absolute() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "文件输入需要非空的安全相对路径。",
        ));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AgentFileInputError::new(
                    ERROR_INVALID_REQUEST,
                    "changeRequest",
                    "文件输入路径不能包含 `..`、根目录或平台前缀。",
                ));
            }
        }
    }
    if clean.as_os_str().is_empty() {
        Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "文件输入路径不能为空。",
        ))
    } else {
        Ok(clean)
    }
}

fn canonical_regular_path(path: &Path) -> Result<PathBuf, AgentFileInputError> {
    reject_symlink_components(path)?;
    let canonical = path.canonicalize().map_err(|_| {
        AgentFileInputError::new(ERROR_NOT_FOUND, "changeRequest", "声明式输入文件不可访问。")
    })?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        AgentFileInputError::new(ERROR_NOT_FOUND, "changeRequest", "声明式输入文件不可访问。")
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "声明式输入必须是非符号链接的普通文件。",
        ));
    }
    ensure_size(metadata.len())?;
    Ok(canonical)
}

fn reject_symlink_components(path: &Path) -> Result<(), AgentFileInputError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if current.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AgentFileInputError::new(
                    ERROR_AUTHORIZATION_DENIED,
                    "changeRequest",
                    "声明式输入路径不能经过符号链接。",
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(AgentFileInputError::new(
                    ERROR_NOT_FOUND,
                    "changeRequest",
                    "声明式输入文件不可访问。",
                ))
            }
            Err(_) => {
                return Err(AgentFileInputError::new(
                    ERROR_IO,
                    "retry",
                    "无法安全检查声明式输入路径。",
                ))
            }
        }
    }
    Ok(())
}

fn read_regular_file(
    path: &Path,
    cancellation: Option<&AgentCancellationToken>,
    max_bytes: u64,
) -> Result<Vec<u8>, AgentFileInputError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(path).map_err(|_| {
        AgentFileInputError::new(
            ERROR_NOT_FOUND,
            "changeRequest",
            "声明式输入文件不可安全读取。",
        )
    })?;
    let metadata = file.metadata().map_err(|_| {
        AgentFileInputError::new(ERROR_IO, "retry", "无法检查已打开的声明式输入文件。")
    })?;
    if !metadata.is_file() {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "changeRequest",
            "声明式输入必须是普通文件。",
        ));
    }
    ensure_size_with_limit(metadata.len(), max_bytes)?;
    let capacity = usize::try_from(metadata.len()).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancelled(cancellation)?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| AgentFileInputError::new(ERROR_IO, "retry", "读取声明式输入文件失败。"))?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > usize::try_from(max_bytes).unwrap_or(usize::MAX) {
            return Err(AgentFileInputError::new(
                ERROR_TOO_LARGE,
                "changeRequest",
                input_size_limit_message(max_bytes),
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

fn ensure_size(size: u64) -> Result<(), AgentFileInputError> {
    ensure_size_with_limit(size, MAX_AGENT_FILE_INPUT_BYTES)
}

fn ensure_size_with_limit(size: u64, max_bytes: u64) -> Result<(), AgentFileInputError> {
    if size > max_bytes {
        Err(AgentFileInputError::new(
            ERROR_TOO_LARGE,
            "changeRequest",
            input_size_limit_message(max_bytes),
        ))
    } else {
        Ok(())
    }
}

fn input_size_limit_message(max_bytes: u64) -> String {
    if max_bytes.is_multiple_of(1024 * 1024) {
        format!("单个文件输入最多允许 {} MiB。", max_bytes / (1024 * 1024))
    } else {
        format!("单个文件输入最多允许 {max_bytes} bytes。")
    }
}

fn check_cancelled(
    cancellation: Option<&AgentCancellationToken>,
) -> Result<(), AgentFileInputError> {
    if cancellation.is_some_and(AgentCancellationToken::is_cancelled) {
        Err(AgentFileInputError::new(
            "agent.fileInput.cancelled",
            "retry",
            "声明式输入准备已取消。",
        ))
    } else {
        Ok(())
    }
}

fn join_mount_path(root: &Path, mount_path: &str) -> Result<PathBuf, AgentFileInputError> {
    let normalized = normalize_mount_path(mount_path)?;
    let target = normalized
        .split('/')
        .fold(root.to_path_buf(), |path, component| path.join(component));
    if !target.starts_with(root) {
        return Err(AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "reprepare",
            "冻结的 mountPath 逃逸了私有输入目录。",
        ));
    }
    Ok(target)
}

fn create_private_parent_directories(
    root: &Path,
    target: &Path,
) -> Result<(), AgentFileInputError> {
    let parent = target.parent().ok_or_else(|| {
        AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "reprepare",
            "冻结的 mountPath 缺少父目录。",
        )
    })?;
    let relative = parent.strip_prefix(root).map_err(|_| {
        AgentFileInputError::new(
            ERROR_INVALID_REQUEST,
            "reprepare",
            "冻结的 mountPath 逃逸了私有输入目录。",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(AgentFileInputError::new(
                ERROR_INVALID_REQUEST,
                "reprepare",
                "冻结的 mountPath 包含不安全的父目录。",
            ));
        };
        current.push(part);
        match fs::create_dir(&current) {
            Ok(()) => set_private_directory_permissions(&current).map_err(|_| {
                AgentFileInputError::new(ERROR_IO, "retry", "无法保护私有输入子目录。")
            })?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&current).map_err(|_| {
                    AgentFileInputError::new(ERROR_IO, "retry", "无法检查私有输入子目录。")
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(AgentFileInputError::new(
                        ERROR_INVALID_REQUEST,
                        "reprepare",
                        "私有输入路径与另一个输入发生冲突。",
                    ));
                }
            }
            Err(_) => {
                return Err(AgentFileInputError::new(
                    ERROR_IO,
                    "retry",
                    "无法创建私有输入子目录。",
                ))
            }
        }
    }
    Ok(())
}

fn write_private_read_only_file(path: &Path, bytes: &[u8]) -> Result<(), AgentFileInputError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o400).custom_flags(libc::O_CLOEXEC);
    }
    let mut file = options.open(path).map_err(|_| {
        AgentFileInputError::new(ERROR_IO, "retry", "无法创建托管命令的只读输入副本。")
    })?;
    file.write_all(bytes).map_err(|_| {
        AgentFileInputError::new(ERROR_IO, "retry", "无法写入托管命令的只读输入副本。")
    })?;
    file.sync_all().map_err(|_| {
        AgentFileInputError::new(ERROR_IO, "retry", "无法持久化托管命令的只读输入副本。")
    })?;
    let mut permissions = file
        .metadata()
        .map_err(|_| {
            AgentFileInputError::new(ERROR_IO, "retry", "无法检查托管命令的只读输入副本。")
        })?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|_| {
        AgentFileInputError::new(ERROR_IO, "retry", "无法保护托管命令的只读输入副本。")
    })?;
    Ok(())
}

fn set_private_directory_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::image_generation_execution_repository::{
        ImageGenerationArtifactJournalRecord, ImageGenerationExecutionIdentityRecord,
        ImageGenerationExecutionTerminalUpdate, StoredImageGenerationArtifactState,
        StoredImageGenerationExecutionStatus,
    };
    use crate::storage::models::ChatConversationRecord;
    use crate::storage::service::ManagedArtifactAuthority;
    use crate::{
        AgentAttachmentReference, AgentInputAttachmentKind, AgentPatchPermission,
        AgentWritePermission,
    };

    fn permissions(read: AgentReadPermission) -> AgentPermissions {
        AgentPermissions {
            read,
            write: AgentWritePermission::WorkspaceOnly,
            patch: AgentPatchPermission::RequireApproval,
            ..AgentPermissions::default()
        }
    }

    fn attachment_context(
        root: &Path,
        storage_rel_path: &str,
        read_path: &str,
        size_bytes: u64,
    ) -> AgentFileInputExecutionContext {
        AgentFileInputExecutionContext::from_attachment_library(Some(
            AgentAttachmentLibraryContext {
                root_path: Some(root.to_string_lossy().to_string()),
                conversation_id: Some("conversation-1".to_string()),
                project_id: None,
                conversation_attachments: vec![AgentAttachmentReference {
                    id: "attachment-1".to_string(),
                    conversation_id: "conversation-1".to_string(),
                    message_id: "message-1".to_string(),
                    project_id: None,
                    kind: AgentInputAttachmentKind::Image,
                    name: "campus.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    size_bytes,
                    read_path: read_path.to_string(),
                    storage_rel_path: storage_rel_path.to_string(),
                    created_at: 1,
                }],
                project_attachments: Vec::new(),
            },
        ))
    }

    fn publish_generated_artifact(
        root: &Path,
        execution_id: &str,
        bytes: &[u8],
        storage_relative_path: &str,
    ) -> (Arc<StorageService>, PathBuf, String) {
        let storage = Arc::new(StorageService::open(&root.join("storage.sqlite")).unwrap());
        let sha256 = sha256_hex(bytes);
        let artifact_id = format!("sha256:{sha256}");
        let identity = ImageGenerationExecutionIdentityRecord {
            execution_id: execution_id.to_string(),
            request_fingerprint: format!("sha256:{}", "a".repeat(64)),
            safe_request_json: r#"{"schemaVersion":1}"#.to_string(),
            profile_id: "default".to_string(),
            adapter_id: "smartmlSeedream".to_string(),
            profile_revision: 1,
            model_id: "seedream".to_string(),
            operation: "generate".to_string(),
        };
        storage.claim_image_generation_execution(&identity).unwrap();
        storage
            .prepare_image_generation_artifact(
                execution_id,
                &ImageGenerationArtifactJournalRecord {
                    ordinal: 0,
                    artifact_id: artifact_id.clone(),
                    state: StoredImageGenerationArtifactState::Candidate,
                    storage_relative_path: storage_relative_path.to_string(),
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
                execution_id,
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

    fn insert_conversation(storage: &StorageService, conversation_id: &str) {
        storage
            .save_conversation(ChatConversationRecord {
                id: conversation_id.to_string(),
                project_id: None,
                model_id: None,
                title: "artifact authority test".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
    }

    #[test]
    fn freezes_materializes_and_cleans_up_exact_attachment_input() {
        let library = tempfile::tempdir().unwrap();
        fs::create_dir(library.path().join("objects")).unwrap();
        let bytes = b"verified attachment bytes";
        fs::write(library.path().join("objects/campus.png"), bytes).unwrap();
        let context = attachment_context(
            library.path(),
            "objects/campus.png",
            "@attachments/attachment-1/campus.png",
            bytes.len() as u64,
        );
        let specs = vec![AgentFileInputSpec {
            mount_path: "images/campus.png".to_string(),
            source: AgentFileInputRef::Attachment {
                read_path: "@attachments/attachment-1/campus.png".to_string(),
            },
        }];
        let bindings = prepare_agent_file_input_bindings(
            None,
            permissions(AgentReadPermission::WorkspaceOnly),
            &context,
            &specs,
            None,
        )
        .unwrap();
        assert_eq!(bindings[0].size_bytes, bytes.len() as u64);
        assert_eq!(bindings[0].sha256, sha256_hex(bytes));

        let prepared = materialize_agent_file_inputs(
            None,
            permissions(AgentReadPermission::WorkspaceOnly),
            &context,
            &bindings,
            None,
        )
        .unwrap()
        .unwrap();
        let private_root = prepared.root().to_path_buf();
        assert_eq!(
            fs::read(private_root.join("images/campus.png")).unwrap(),
            bytes
        );
        assert!(fs::metadata(private_root.join("images/campus.png"))
            .unwrap()
            .permissions()
            .readonly());
        assert_eq!(prepared.evidence(), evidence_from_bindings(&bindings));
        let audit_json = serde_json::to_string(prepared.evidence()).unwrap();
        assert!(!audit_json.contains(library.path().to_string_lossy().as_ref()));
        assert!(!audit_json.contains("@attachments/"));
        drop(prepared);
        assert!(!private_root.exists());
    }

    #[test]
    fn prepared_input_drop_removes_nested_read_only_tree() {
        let directory = tempfile::Builder::new()
            .prefix("mycopilot-input-drop-test-")
            .tempdir()
            .unwrap();
        set_private_directory_permissions(directory.path()).unwrap();
        let first = directory.path().join("nested/first.bin");
        let second = directory.path().join("nested/deeper/second.bin");
        create_private_parent_directories(directory.path(), &first).unwrap();
        create_private_parent_directories(directory.path(), &second).unwrap();
        write_private_read_only_file(&first, b"first private input").unwrap();
        write_private_read_only_file(&second, b"second private input").unwrap();
        let private_root = directory.path().to_path_buf();
        let prepared = PreparedAgentFileInputs {
            directory,
            evidence: Vec::new(),
        };

        drop(prepared);

        assert!(
            !private_root.exists(),
            "dropping private inputs must remove nested read-only files"
        );
    }

    #[test]
    fn revalidation_rejects_changed_workspace_input_before_materialization() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("source.txt"), b"before").unwrap();
        let specs = vec![AgentFileInputSpec {
            mount_path: "source.txt".to_string(),
            source: AgentFileInputRef::Workspace {
                path: "source.txt".to_string(),
            },
        }];
        let context = AgentFileInputExecutionContext::default();
        let bindings = prepare_agent_file_input_bindings(
            Some(workspace.path()),
            permissions(AgentReadPermission::WorkspaceOnly),
            &context,
            &specs,
            None,
        )
        .unwrap();
        fs::write(workspace.path().join("source.txt"), b"after").unwrap();
        let error = materialize_agent_file_inputs(
            Some(workspace.path()),
            permissions(AgentReadPermission::WorkspaceOnly),
            &context,
            &bindings,
            None,
        )
        .unwrap_err();
        assert_eq!(error.code(), ERROR_INTEGRITY_MISMATCH);
    }

    #[test]
    fn external_inputs_require_read_all_and_generated_artifact_uses_authoritative_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let external_path = root.join("external.png");
        fs::write(&external_path, b"external").unwrap();
        let external = vec![AgentFileInputSpec {
            mount_path: "external.png".to_string(),
            source: AgentFileInputRef::External {
                path: external_path.to_string_lossy().to_string(),
            },
        }];
        let context = AgentFileInputExecutionContext::default();
        let denied = prepare_agent_file_input_bindings(
            None,
            permissions(AgentReadPermission::WorkspaceOnly),
            &context,
            &external,
            None,
        )
        .unwrap_err();
        assert_eq!(denied.code(), ERROR_AUTHORIZATION_DENIED);
        prepare_agent_file_input_bindings(
            None,
            permissions(AgentReadPermission::All),
            &context,
            &external,
            None,
        )
        .unwrap();

        let (storage, path, sha256) = publish_generated_artifact(
            &root,
            "execution-generated",
            b"generated",
            &format!("objects/{}.png", sha256_hex(b"generated")),
        );
        let context = AgentFileInputExecutionContext::default().with_storage(Some(storage));
        let generated = vec![AgentFileInputSpec {
            mount_path: "generated.png".to_string(),
            source: AgentFileInputRef::GeneratedArtifact {
                uri: format!("image-artifact://sha256/{sha256}"),
                path: path.to_string_lossy().to_string(),
            },
        }];
        prepare_agent_file_input_bindings(
            None,
            permissions(AgentReadPermission::WorkspaceOnly),
            &context,
            &generated,
            None,
        )
        .unwrap();

        let forged_path = root.join("forged.png");
        fs::write(&forged_path, b"generated").unwrap();
        let forged = vec![AgentFileInputSpec {
            mount_path: "generated.png".to_string(),
            source: AgentFileInputRef::GeneratedArtifact {
                uri: format!("image-artifact://sha256/{sha256}"),
                path: forged_path.to_string_lossy().to_string(),
            },
        }];
        assert_eq!(
            prepare_agent_file_input_bindings(
                None,
                permissions(AgentReadPermission::WorkspaceOnly),
                &context,
                &forged,
                None,
            )
            .unwrap_err()
            .code(),
            ERROR_INTEGRITY_MISMATCH
        );

        let unregistered = vec![AgentFileInputSpec {
            mount_path: "missing.png".to_string(),
            source: AgentFileInputRef::GeneratedArtifact {
                uri: format!("image-artifact://sha256/{}", "b".repeat(64)),
                path: path.to_string_lossy().to_string(),
            },
        }];
        assert_eq!(
            prepare_agent_file_input_bindings(
                None,
                permissions(AgentReadPermission::WorkspaceOnly),
                &context,
                &unregistered,
                None,
            )
            .unwrap_err()
            .code(),
            ERROR_NOT_FOUND
        );

        let (conflicting_storage, _, _) = publish_generated_artifact(
            &root,
            "execution-conflicting",
            b"generated",
            &format!("conflicting/{}.png", sha256_hex(b"generated")),
        );
        let conflicting_context =
            AgentFileInputExecutionContext::default().with_storage(Some(conflicting_storage));
        assert_eq!(
            prepare_agent_file_input_bindings(
                None,
                permissions(AgentReadPermission::WorkspaceOnly),
                &conflicting_context,
                &generated,
                None,
            )
            .unwrap_err()
            .code(),
            ERROR_SNAPSHOT_UNAVAILABLE
        );
    }

    #[test]
    fn artifact_uri_scheme_is_an_authority_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();

        // Deliberately register the exact same digest in the legacy image journal and the managed
        // document journal. URI scheme, not the guessable digest, decides which authority applies.
        let pdf_bytes = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";
        let (storage, legacy_path, digest) = publish_generated_artifact(
            &root,
            "execution-same-digest",
            pdf_bytes,
            &format!("objects/{}.png", sha256_hex(pdf_bytes)),
        );
        insert_conversation(&storage, "conversation-1");
        insert_conversation(&storage, "conversation-2");
        let document_source = root.join("same-digest.pdf");
        fs::write(&document_source, pdf_bytes).unwrap();
        let document = storage
            .publish_managed_artifact_file(
                &document_source,
                ManagedArtifactAuthority {
                    conversation_id: "conversation-1",
                    run_id: "run-1",
                    call_id: "call-1",
                },
            )
            .unwrap();
        assert_eq!(document.sha256, digest);

        let context = AgentFileInputExecutionContext::default()
            .with_storage(Some(Arc::clone(&storage)))
            .with_conversation_id(Some("conversation-1"));
        let document_ref =
            agent_file_input_ref_from_model_path(&context, &format!("artifact://sha256/{digest}"))
                .unwrap();
        let AgentFileInputRef::GeneratedArtifact { path, .. } = &document_ref else {
            panic!("expected a generated Artifact reference");
        };
        assert_eq!(Path::new(path), document.absolute_path);
        let frozen_document = vec![AgentFileInputSpec {
            mount_path: "same-digest.pdf".to_string(),
            source: document_ref.clone(),
        }];
        prepare_agent_file_input_bindings(
            None,
            permissions(AgentReadPermission::WorkspaceOnly),
            &context,
            &frozen_document,
            None,
        )
        .unwrap();

        let image_ref = agent_file_input_ref_from_model_path(
            &context,
            &format!("image-artifact://sha256/{digest}"),
        )
        .unwrap();
        let AgentFileInputRef::GeneratedArtifact { path, .. } = &image_ref else {
            panic!("expected a generated Artifact reference");
        };
        assert_eq!(Path::new(path), legacy_path.canonicalize().unwrap());
        assert!(!agent_file_input_ref_matches_model_path(
            &document_ref,
            &format!("image-artifact://sha256/{digest}")
        )
        .unwrap());

        let other_conversation = AgentFileInputExecutionContext::default()
            .with_storage(Some(Arc::clone(&storage)))
            .with_conversation_id(Some("conversation-2"));
        let denied = agent_file_input_ref_from_model_path(
            &other_conversation,
            &format!("artifact://sha256/{digest}"),
        )
        .unwrap_err();
        assert_eq!(denied.code(), ERROR_NOT_FOUND);
        let denied_after_freeze = prepare_agent_file_input_bindings(
            None,
            permissions(AgentReadPermission::WorkspaceOnly),
            &other_conversation,
            &frozen_document,
            None,
        )
        .unwrap_err();
        assert_eq!(denied_after_freeze.code(), ERROR_NOT_FOUND);
    }

    #[test]
    fn managed_artifacts_cannot_cross_uri_schemes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let storage = Arc::new(StorageService::open(&root.join("storage.sqlite")).unwrap());
        insert_conversation(&storage, "conversation-1");
        let authority = ManagedArtifactAuthority {
            conversation_id: "conversation-1",
            run_id: "run-1",
            call_id: "call-1",
        };

        let document_source = root.join("document.pdf");
        fs::write(&document_source, b"%PDF-1.7\n%%EOF\n").unwrap();
        let document = storage
            .publish_managed_artifact_file(&document_source, authority)
            .unwrap();

        let image_source = root.join("image.png");
        let mut image_bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut image_bytes, image::ImageFormat::Png)
            .unwrap();
        fs::write(&image_source, image_bytes.into_inner()).unwrap();
        let image = storage
            .publish_managed_artifact_file(&image_source, authority)
            .unwrap();

        let context = AgentFileInputExecutionContext::default()
            .with_storage(Some(storage))
            .with_conversation_id(Some("conversation-1"));
        let document_as_image = agent_file_input_ref_from_model_path(
            &context,
            &format!("image-artifact://sha256/{}", document.sha256),
        )
        .unwrap_err();
        assert_eq!(document_as_image.code(), ERROR_NOT_FOUND);
        let forged_document_as_image = vec![AgentFileInputSpec {
            mount_path: "document.png".to_string(),
            source: AgentFileInputRef::GeneratedArtifact {
                uri: format!("image-artifact://sha256/{}", document.sha256),
                path: document.absolute_path.to_string_lossy().into_owned(),
            },
        }];
        assert_eq!(
            prepare_agent_file_input_bindings(
                None,
                permissions(AgentReadPermission::WorkspaceOnly),
                &context,
                &forged_document_as_image,
                None,
            )
            .unwrap_err()
            .code(),
            ERROR_NOT_FOUND
        );
        let image_as_document = agent_file_input_ref_from_model_path(
            &context,
            &format!("artifact://sha256/{}", image.sha256),
        )
        .unwrap_err();
        assert_eq!(image_as_document.code(), ERROR_NOT_FOUND);
        let forged_image_as_document = vec![AgentFileInputSpec {
            mount_path: "image.pdf".to_string(),
            source: AgentFileInputRef::GeneratedArtifact {
                uri: format!("artifact://sha256/{}", image.sha256),
                path: image.absolute_path.to_string_lossy().into_owned(),
            },
        }];
        assert_eq!(
            prepare_agent_file_input_bindings(
                None,
                permissions(AgentReadPermission::WorkspaceOnly),
                &context,
                &forged_image_as_document,
                None,
            )
            .unwrap_err()
            .code(),
            ERROR_NOT_FOUND
        );
    }

    #[test]
    fn rejects_traversal_duplicates_and_mount_prefix_collisions() {
        let context = AgentFileInputExecutionContext::default();
        let source = AgentFileInputRef::Workspace {
            path: "input.txt".to_string(),
        };
        for specs in [
            vec![AgentFileInputSpec {
                mount_path: "../escape".to_string(),
                source: source.clone(),
            }],
            vec![
                AgentFileInputSpec {
                    mount_path: "A.txt".to_string(),
                    source: source.clone(),
                },
                AgentFileInputSpec {
                    mount_path: "a.TXT".to_string(),
                    source: source.clone(),
                },
            ],
            vec![
                AgentFileInputSpec {
                    mount_path: "folder".to_string(),
                    source: source.clone(),
                },
                AgentFileInputSpec {
                    mount_path: "folder/input.txt".to_string(),
                    source: source.clone(),
                },
            ],
        ] {
            let error = prepare_agent_file_input_bindings(
                None,
                permissions(AgentReadPermission::WorkspaceOnly),
                &context,
                &specs,
                None,
            )
            .unwrap_err();
            assert_eq!(error.code(), ERROR_INVALID_REQUEST);
        }
    }
}
