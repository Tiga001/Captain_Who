// Tool execution context and path resolution helpers.
use super::{clean_relative_path, relative_display};
use crate::cancellation::AgentCancellationToken;
use crate::context::ContextTextBudget;
use crate::file_change::FileObservationRegistry;
use crate::file_input::{
    agent_file_input_ref_from_model_path, resolve_verified_agent_file_input_path,
    AgentFileInputExecutionContext,
};
use crate::protocol::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentError, AgentPermissions,
    AgentReadPermission, AgentResult, AgentRunContext, AgentWritePermission, ModelCapabilities,
};
use crate::resource_locator::ResourceLocator;
use crate::storage::service::StorageService;
use crate::system_paths::expand_system_path;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

const MODEL_IMAGE_DELIVERY_BUDGET_BYTES_PER_RUN: u64 = 16 * 1024 * 1024;
const MAX_CONCURRENT_MODEL_IMAGE_PREPARATIONS: usize = 2;

struct ModelImagePreparationAdmission {
    available: Mutex<usize>,
    changed: Condvar,
}

pub(super) struct ModelImagePreparationPermit(&'static ModelImagePreparationAdmission);

impl Drop for ModelImagePreparationPermit {
    fn drop(&mut self) {
        let mut available = self
            .0
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *available = available
            .saturating_add(1)
            .min(MAX_CONCURRENT_MODEL_IMAGE_PREPARATIONS);
        self.0.changed.notify_one();
    }
}

/// One reservation from the run-scoped raw-image budget.
///
/// Reservations are refunded unless committed after verified bytes have been encoded. Cloned tool
/// contexts share the same atomic budget, so a batch of image-producing calls cannot each claim the
/// full allowance independently.
pub(super) struct ModelImageDeliveryReservation {
    remaining_bytes: Arc<AtomicU64>,
    reserved_bytes: u64,
    committed: bool,
}

impl ModelImageDeliveryReservation {
    pub(super) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ModelImageDeliveryReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.remaining_bytes
                .fetch_add(self.reserved_bytes, Ordering::Release);
        }
    }
}

#[derive(Clone)]
pub struct ToolExecutionContext {
    workspace_root: Option<PathBuf>,
    workspace_context: Option<crate::AgentWorkspaceContext>,
    attachment_library: Option<AgentAttachmentLibraryContext>,
    cancellation_token: AgentCancellationToken,
    model_capabilities: ModelCapabilities,
    model_image_delivery_budget: Arc<AtomicU64>,
    file_observations: Arc<FileObservationRegistry>,
    file_change_permission_revision: String,
    file_change_tool_set_revision: String,
    file_change_provider_wire_revision: String,
    permissions: AgentPermissions,
    conversation_id: Option<String>,
    project_id: Option<String>,
    run_id: Option<String>,
    tool_call_id: Option<String>,
    storage: Option<Arc<StorageService>>,
    text_output_budget: ContextTextBudget,
    skill_resources: Option<Arc<crate::skills::SkillResourceSession>>,
    command_runtime_profile_resolver:
        Option<Arc<dyn crate::command::CommandRuntimeProfileResolver>>,
    command_session_executor: Option<Arc<dyn crate::runtime::AgentCommandSessionExecutor>>,
    agent_collaboration: Option<crate::AgentCollaborationRuntimeServices>,
    assistant_message_id: Option<String>,
    model_batch_index: u64,
    steer_input: Option<crate::runtime::AgentSteerInputQueue>,
}

/// A registry view bound to the Host-authenticated identity of the current dispatch.
///
/// `read_file` deliberately does not accept a source call id from model arguments. Issuance
/// through this view obtains it from `ToolExecutionContext::with_tool_call_id`, while validation
/// remains bound to the frozen conversation/run owner supplied by the same context.
pub(super) struct FileObservationRegistryView<'a> {
    context: &'a ToolExecutionContext,
}

impl FileObservationRegistryView<'_> {
    pub(super) fn issue_existing(
        &self,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
        revision: &str,
        metadata: &Metadata,
        parent_metadata: &Metadata,
    ) -> Result<crate::file_change::FileObservation, crate::file_change::FileChangeError> {
        self.validate_owner(conversation_id, run_id)?;
        let source_tool_call_id = self.context.tool_call_id.as_deref().ok_or_else(|| {
            crate::file_change::FileChangeError::new(
                crate::file_change::FileChangeErrorCode::InvalidArguments,
            )
        })?;
        self.context.file_observations.issue_existing_from_read(
            crate::file_change::FileObservationOwner::new(
                source_tool_call_id,
                conversation_id,
                run_id,
            ),
            canonical_target,
            revision,
            metadata,
            parent_metadata,
        )
    }

    pub(super) fn issue_missing(
        &self,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
        parent_metadata: &Metadata,
    ) -> Result<crate::file_change::FileObservation, crate::file_change::FileChangeError> {
        self.validate_owner(conversation_id, run_id)?;
        let source_tool_call_id = self.context.tool_call_id.as_deref().ok_or_else(|| {
            crate::file_change::FileChangeError::new(
                crate::file_change::FileChangeErrorCode::InvalidArguments,
            )
        })?;
        self.context.file_observations.issue_missing_from_read(
            crate::file_change::FileObservationOwner::new(
                source_tool_call_id,
                conversation_id,
                run_id,
            ),
            canonical_target,
            parent_metadata,
        )
    }

    pub(super) fn validate(
        &self,
        observation_id: &str,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
    ) -> Result<crate::file_change::FileObservation, crate::file_change::FileChangeError> {
        self.validate_owner(conversation_id, run_id)?;
        self.context.file_observations.validate(
            observation_id,
            conversation_id,
            run_id,
            canonical_target,
        )
    }

    /// Claims a read observation only after the caller has formed and validated its Direct
    /// proposal. This method is intentionally separate from `validate` so malformed edits do not
    /// burn an otherwise-current observation.
    pub(super) fn claim(
        &self,
        observation_id: &str,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
    ) -> Result<crate::file_change::FileObservation, crate::file_change::FileChangeError> {
        self.validate_owner(conversation_id, run_id)?;
        let consumer_tool_call_id = self.context.tool_call_id.as_deref().ok_or_else(|| {
            crate::file_change::FileChangeError::new(
                crate::file_change::FileChangeErrorCode::InvalidArguments,
            )
        })?;
        self.context.file_observations.claim(
            observation_id,
            conversation_id,
            run_id,
            canonical_target,
            consumer_tool_call_id,
        )
    }

    fn validate_owner(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<(), crate::file_change::FileChangeError> {
        if self.context.conversation_id.as_deref() != Some(conversation_id)
            || self.context.run_id.as_deref() != Some(run_id)
        {
            return Err(crate::file_change::FileChangeError::new(
                crate::file_change::FileChangeErrorCode::ObservationOwnerMismatch,
            ));
        }
        Ok(())
    }
}

impl ToolExecutionContext {
    pub fn from_run_context(context: Option<&AgentRunContext>) -> Self {
        let workspace_root = context
            .and_then(|context| context.workspace.as_ref())
            .and_then(|workspace| workspace.root_path.as_ref())
            .map(PathBuf::from);
        let workspace_context = context.and_then(|context| context.workspace.clone());
        let attachment_library = context.and_then(|context| context.attachment_library.clone());
        let permissions = context
            .map(|context| context.permissions)
            .unwrap_or_default();
        let conversation_id = context.and_then(|context| context.conversation_id.clone());
        let project_id = context.and_then(|context| context.project_id.clone());

        Self {
            workspace_root,
            workspace_context,
            attachment_library,
            cancellation_token: AgentCancellationToken::new(),
            model_capabilities: ModelCapabilities::default(),
            model_image_delivery_budget: Arc::new(AtomicU64::new(0)),
            file_observations: Arc::new(FileObservationRegistry::default()),
            file_change_permission_revision: crate::file_change::proposal_digest(&permissions)
                .expect("AgentPermissions serialization is infallible"),
            file_change_tool_set_revision: "tool-set-unbound".to_string(),
            file_change_provider_wire_revision: "provider-wire-unbound".to_string(),
            permissions,
            conversation_id,
            project_id,
            run_id: None,
            tool_call_id: None,
            storage: None,
            text_output_budget: ContextTextBudget::heuristic_default(),
            skill_resources: None,
            command_runtime_profile_resolver: None,
            command_session_executor: None,
            agent_collaboration: None,
            assistant_message_id: None,
            model_batch_index: 0,
            steer_input: None,
        }
    }

    pub fn with_cancellation(mut self, cancellation_token: AgentCancellationToken) -> Self {
        self.cancellation_token = cancellation_token;
        self
    }

    pub(crate) fn with_model_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.model_capabilities = capabilities;
        self.model_image_delivery_budget = Arc::new(AtomicU64::new(if capabilities.image_input {
            MODEL_IMAGE_DELIVERY_BUDGET_BYTES_PER_RUN
        } else {
            0
        }));
        self
    }

    pub fn with_runtime_services(
        mut self,
        run_id: String,
        storage: Option<Arc<StorageService>>,
    ) -> Self {
        self.run_id = Some(run_id);
        self.storage = storage;
        self
    }

    pub(crate) fn with_file_change_tool_set_revision(mut self, revision: String) -> Self {
        self.file_change_tool_set_revision = revision;
        self
    }

    pub(crate) fn with_file_change_provider_wire_revision(mut self, revision: String) -> Self {
        self.file_change_provider_wire_revision = revision;
        self
    }

    pub(crate) fn with_file_observation_registry(
        mut self,
        registry: Arc<FileObservationRegistry>,
    ) -> Self {
        self.file_observations = registry;
        self
    }

    pub(crate) fn replace_file_change_tool_set_revision(&mut self, revision: String) {
        self.file_change_tool_set_revision = revision;
    }

    /// Binds one registry dispatch to its immutable model tool-call identity.
    ///
    /// Tools may use this only for result framing, audit, and other non-authority metadata. The
    /// id is never accepted from model arguments and must not influence permissions.
    pub(crate) fn with_tool_call_id(mut self, tool_call_id: String) -> Self {
        self.tool_call_id = Some(tool_call_id);
        self
    }

    pub(crate) fn with_text_output_budget(mut self, budget: ContextTextBudget) -> Self {
        self.text_output_budget = budget;
        self
    }

    pub(crate) fn with_skill_resources(
        mut self,
        resources: Option<Arc<crate::skills::SkillResourceSession>>,
    ) -> Self {
        self.skill_resources = resources;
        self
    }

    pub(crate) fn with_command_runtime_profile_resolver(
        mut self,
        resolver: Option<Arc<dyn crate::command::CommandRuntimeProfileResolver>>,
    ) -> Self {
        self.command_runtime_profile_resolver = resolver;
        self
    }

    pub(crate) fn with_command_session_executor(
        mut self,
        executor: Option<Arc<dyn crate::runtime::AgentCommandSessionExecutor>>,
    ) -> Self {
        self.command_session_executor = executor;
        self
    }

    pub(crate) fn with_agent_collaboration(
        mut self,
        services: Option<crate::AgentCollaborationRuntimeServices>,
        assistant_message_id: Option<String>,
    ) -> Self {
        self.agent_collaboration = services;
        self.assistant_message_id = assistant_message_id;
        self
    }

    pub(crate) fn with_model_batch_index(mut self, index: u64) -> Self {
        self.model_batch_index = index;
        self
    }

    pub(crate) fn with_steer_input(
        mut self,
        steer_input: Option<crate::runtime::AgentSteerInputQueue>,
    ) -> Self {
        self.steer_input = steer_input;
        self
    }

    /// Replaces the run-scoped attachment catalog after newly admitted guidance is applied.
    ///
    /// Callers must provide a host-built snapshot. This method deliberately is not exposed to
    /// model-authored tool arguments.
    pub(crate) fn replace_attachment_library(&mut self, library: AgentAttachmentLibraryContext) {
        self.attachment_library = Some(library);
    }

    pub(super) fn cancellation_token(&self) -> AgentCancellationToken {
        self.cancellation_token.clone()
    }

    pub(super) fn model_capabilities(&self) -> ModelCapabilities {
        self.model_capabilities
    }

    /// Reserves raw image bytes that may be transiently attached to this run's model context.
    ///
    /// This is an internal resource budget, not an authority decision. The caller must separately
    /// verify the frozen model capability and the image itself.
    pub(super) fn try_reserve_model_image_delivery(
        &self,
        bytes: u64,
    ) -> Option<ModelImageDeliveryReservation> {
        if bytes == 0 || !self.model_capabilities.image_input {
            return None;
        }

        let mut remaining = self.model_image_delivery_budget.load(Ordering::Acquire);
        loop {
            if remaining < bytes {
                return None;
            }
            match self.model_image_delivery_budget.compare_exchange_weak(
                remaining,
                remaining - bytes,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(ModelImageDeliveryReservation {
                        remaining_bytes: Arc::clone(&self.model_image_delivery_budget),
                        reserved_bytes: bytes,
                        committed: false,
                    });
                }
                Err(actual) => remaining = actual,
            }
        }
    }

    /// Admits bounded image decode/encode work across all concurrent Agent runs.
    ///
    /// Waiting remains cancellation-aware so a queued image call never becomes an uninterruptible
    /// memory task after the user has stopped the run.
    pub(super) fn acquire_model_image_preparation(
        &self,
    ) -> AgentResult<ModelImagePreparationPermit> {
        static ADMISSION: OnceLock<ModelImagePreparationAdmission> = OnceLock::new();
        let admission = ADMISSION.get_or_init(|| ModelImagePreparationAdmission {
            available: Mutex::new(MAX_CONCURRENT_MODEL_IMAGE_PREPARATIONS),
            changed: Condvar::new(),
        });
        let mut available = admission
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            self.check_cancelled()?;
            if *available > 0 {
                *available -= 1;
                return Ok(ModelImagePreparationPermit(admission));
            }
            available = admission
                .changed
                .wait_timeout(available, Duration::from_millis(25))
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
    }

    pub(super) fn text_output_budget(&self) -> &ContextTextBudget {
        &self.text_output_budget
    }

    pub(super) fn check_cancelled(&self) -> AgentResult<()> {
        self.cancellation_token.check()
    }

    pub(super) fn workspace_root(&self) -> AgentResult<PathBuf> {
        self.workspace_root_optional()?.ok_or_else(|| {
            AgentError::new("当前没有 workspace；请为该工具提供绝对路径或系统路径别名。")
        })
    }

    pub(super) fn workspace_root_optional(&self) -> AgentResult<Option<PathBuf>> {
        self.check_cancelled()?;
        let Some(root) = &self.workspace_root else {
            return Ok(None);
        };

        let root = root
            .canonicalize()
            .map_err(|error| AgentError::new(format!("workspace 路径不可访问：{error}")))?;
        if !root.is_dir() {
            return Err(AgentError::new("workspace 路径不是目录。"));
        }

        Ok(Some(root))
    }

    pub(super) fn permissions(&self) -> AgentPermissions {
        self.permissions
    }

    pub(super) fn file_observations(&self) -> FileObservationRegistryView<'_> {
        FileObservationRegistryView { context: self }
    }

    pub(crate) fn file_observation_registry(&self) -> &Arc<FileObservationRegistry> {
        &self.file_observations
    }

    pub(super) fn file_change_permission_revision(&self) -> &str {
        &self.file_change_permission_revision
    }

    pub(super) fn file_change_provider_wire_revision(&self) -> &str {
        &self.file_change_provider_wire_revision
    }

    pub(super) fn file_change_tool_set_revision(&self) -> &str {
        &self.file_change_tool_set_revision
    }

    /// Returns the registered attachment capabilities for trusted tool
    /// adapters that perform their own purpose-aware path authorization.
    /// Callers must still resolve an attachment by its exact backend-issued
    /// `readPath`; this accessor does not grant generic filesystem access.
    pub(super) fn attachment_library(&self) -> Option<&AgentAttachmentLibraryContext> {
        self.attachment_library.as_ref()
    }

    pub(crate) fn conversation_id(&self) -> AgentResult<&str> {
        self.conversation_id
            .as_deref()
            .ok_or_else(|| AgentError::new("当前运行缺少 conversationId，无法访问会话级能力。"))
    }

    pub(super) fn conversation_id_optional(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    pub(super) fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub(crate) fn run_id(&self) -> AgentResult<&str> {
        self.run_id
            .as_deref()
            .ok_or_else(|| AgentError::new("当前运行缺少 runId，不能创建文件草稿。"))
    }

    pub(crate) fn resolve_active_file_change_run_grant(
        &self,
        action: &crate::AgentProposedAction,
    ) -> AgentResult<Option<crate::file_change::FileChangeRunGrantRef>> {
        let crate::AgentProposedAction::FileChange { file_change } = action else {
            return Ok(None);
        };
        if !matches!(
            file_change.operation,
            crate::AgentFileChangeOperation::Create | crate::AgentFileChangeOperation::Update
        ) {
            return Ok(None);
        }
        let Some(storage) = self.storage.as_deref() else {
            return Ok(None);
        };
        let run_id = self.run_id()?;
        if file_change.execution.run_id != run_id {
            return Err(AgentError::new(
                "FileChange Run grant proposal owner is invalid.",
            ));
        }
        let context = crate::AgentRunContext {
            conversation_id: self.conversation_id.clone(),
            project_id: self.project_id.clone(),
            workspace: self.workspace_context.clone(),
            attachment_library: None,
            permissions: self.permissions,
            collaboration_identity: None,
        };
        storage
            .resolve_active_file_change_run_grant(file_change, &context)
            .map_err(crate::storage::service::FileChangeRunGrantServiceError::into_agent_error)
    }

    pub(crate) fn tool_call_id(&self) -> AgentResult<&str> {
        self.tool_call_id
            .as_deref()
            .ok_or_else(|| AgentError::new("当前工具执行缺少可信 tool call id。"))
    }

    pub(crate) fn storage(&self) -> AgentResult<&Arc<StorageService>> {
        self.storage
            .as_ref()
            .ok_or_else(|| AgentError::new("当前 host 未提供会话存储服务。"))
    }

    pub(super) fn storage_optional(&self) -> Option<Arc<StorageService>> {
        self.storage.clone()
    }

    pub(super) fn skill_resources(&self) -> AgentResult<&Arc<crate::skills::SkillResourceSession>> {
        self.skill_resources.as_ref().ok_or_else(|| {
            AgentError::new("当前运行没有激活可访问资源的 Skill；请先选择并激活一个 Skill。")
        })
    }

    pub(super) fn skill_resources_optional(
        &self,
    ) -> Option<Arc<crate::skills::SkillResourceSession>> {
        self.skill_resources.clone()
    }

    pub(super) fn command_runtime_profile_resolver(
        &self,
    ) -> AgentResult<&Arc<dyn crate::command::CommandRuntimeProfileResolver>> {
        self.command_runtime_profile_resolver
            .as_ref()
            .ok_or_else(|| {
                AgentError::structured(
                    "artifactRuntime.unavailable",
                    "Managed Artifact Runtime 尚未安装或未由 host 配置。",
                    serde_json::json!({
                        "type": "commandRuntimeProfile",
                        "code": "artifactRuntime.unavailable",
                        "recovery": "installComponent"
                    }),
                )
            })
    }

    pub(super) fn command_session_executor(
        &self,
    ) -> AgentResult<&Arc<dyn crate::runtime::AgentCommandSessionExecutor>> {
        self.command_session_executor.as_ref().ok_or_else(|| {
            AgentError::structured(
                "agent.command_session_unavailable",
                "当前 Host 未提供命令 Session 控制能力。",
                serde_json::json!({
                    "type": "command_session",
                    "code": "commandSessionUnavailable",
                    "recovery": "startNewRun"
                }),
            )
        })
    }

    pub(super) fn steer_input(&self) -> Option<crate::runtime::AgentSteerInputQueue> {
        self.steer_input.clone()
    }

    pub(super) fn agent_collaboration_invocation(
        &self,
        action: crate::AgentCollaborationAction,
    ) -> AgentResult<(
        crate::AgentCollaborationRuntimeServices,
        crate::AgentCollaborationInvocation,
    )> {
        self.check_cancelled()?;
        let services = self.agent_collaboration.clone().ok_or_else(|| {
            AgentError::structured(
                "agent.collaboration.unavailable",
                "当前 Host 未启用 Agent 协作能力。",
                serde_json::json!({
                    "type": "agent_collaboration",
                    "category": "unavailable",
                    "retryable": false,
                }),
            )
        })?;
        if matches!(action, crate::AgentCollaborationAction::Wait(_))
            && !services.try_admit_wait_model_batch(self.model_batch_index)?
        {
            return Err(AgentError::structured(
                "agent.collaboration.wait_batch_conflict",
                "同一个模型 Tool-call 批次只能执行一个 wait_agent；请在下一次模型采样后再等待。",
                serde_json::json!({
                    "type": "agent_collaboration",
                    "category": "conflict",
                    "retryable": false,
                    "recovery": "waitInNextModelBatch",
                }),
            ));
        }
        let invocation = crate::AgentCollaborationInvocation {
            caller: services.caller.clone(),
            selector_authorization: services.selector_authorization(),
            caller_model_capabilities: self.model_capabilities(),
            effective_permissions: self.permissions,
            conversation_id: self.conversation_id()?.to_string(),
            run_id: self.run_id()?.to_string(),
            assistant_message_id: self
                .assistant_message_id
                .clone()
                .ok_or_else(|| AgentError::new("协作工具缺少 assistant message identity。"))?,
            model_batch_index: self.model_batch_index,
            tool_call_id: self.tool_call_id()?.to_string(),
            action,
        };
        Ok((services, invocation))
    }

    pub(super) fn resolve_existing_path(&self, input_path: &str) -> AgentResult<PathBuf> {
        self.check_cancelled()?;
        let locator = ResourceLocator::parse(input_path)
            .map_err(|error| AgentError::new(error.to_string()))?;
        if locator.is_virtual() {
            if matches!(locator, ResourceLocator::OpaqueBrowserArtifact(_)) {
                return Err(AgentError::new(
                    "browser-artifact 是临时浏览器能力句柄，不能作为文件路径；请使用 browser-download 引用。",
                ));
            }
            let file_inputs = self.file_input_execution_context();
            let source = agent_file_input_ref_from_model_path(&file_inputs, input_path)
                .map_err(AgentError::from)?;
            return resolve_verified_agent_file_input_path(
                self.workspace_root_optional()?.as_deref(),
                self.permissions,
                &file_inputs,
                &source,
            )
            .map_err(AgentError::from)?
            .ok_or_else(|| {
                AgentError::new(
                    "该逻辑资源没有可暴露给此工具的物理路径；请使用对应的资源读取工具。",
                )
            });
        }

        let input_path = locator.logical_value();
        if let Some(candidate) = expand_system_path(input_path).map_err(AgentError::new)? {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new(
                    "读取系统路径别名需要将读取范围设为“所有位置”。",
                ));
            }
            return candidate
                .canonicalize()
                .map_err(|error| AgentError::new(format!("路径不可访问：{error}")));
        }
        let candidate = Path::new(input_path);
        if candidate.is_absolute() {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new("当前读取权限仅允许访问 workspace 内路径。"));
            }
            return candidate
                .canonicalize()
                .map_err(|error| AgentError::new(format!("路径不可访问：{error}")));
        }

        let root = self.workspace_root()?;
        let relative = clean_relative_path(input_path)?;
        let resolved = root.join(relative);
        let canonical = resolved
            .canonicalize()
            .map_err(|error| AgentError::new(format!("路径不可访问：{error}")))?;

        if !canonical.starts_with(&root) {
            return Err(AgentError::new("路径必须位于已选择的 workspace 内。"));
        }

        Ok(canonical)
    }

    /// Resolves an authorized existing path while preserving its final directory entry.
    ///
    /// `read_file` opens that entry with `O_NOFOLLOW` on Unix. Canonicalizing the full path here
    /// would resolve a leaf symlink before the descriptor-level guard could inspect it, reopening
    /// a check/open race at the exact boundary the guard is intended to close.
    pub(super) fn resolve_existing_path_preserving_leaf(
        &self,
        input_path: &str,
    ) -> AgentResult<PathBuf> {
        self.check_cancelled()?;
        let locator = ResourceLocator::parse(input_path)
            .map_err(|error| AgentError::new(error.to_string()))?;
        if locator.is_virtual() {
            // Managed logical resources already resolve through their own exact, no-symlink
            // ownership checks. Keep that authority path unchanged.
            return self.resolve_existing_path(input_path);
        }

        let input_path = locator.logical_value();
        if let Some(candidate) = expand_system_path(input_path).map_err(AgentError::new)? {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new(
                    "读取系统路径别名需要将读取范围设为“所有位置”。",
                ));
            }
            return canonicalize_parent_preserving_leaf(&candidate);
        }
        let candidate = Path::new(input_path);
        if candidate.is_absolute() {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new("当前读取权限仅允许访问 workspace 内路径。"));
            }
            return canonicalize_parent_preserving_leaf(candidate);
        }

        let root = self.workspace_root()?;
        let relative = clean_relative_path(input_path)?;
        let resolved = canonicalize_parent_preserving_leaf(&root.join(relative))?;
        if !resolved.starts_with(&root) {
            return Err(AgentError::new("路径必须位于已选择的 workspace 内。"));
        }
        Ok(resolved)
    }

    pub(super) fn resolve_missing_file_observation_target(
        &self,
        input_path: &str,
    ) -> Result<crate::file_change::ResolvedFileChangeTarget, crate::file_change::FileChangeError>
    {
        let workspace_root = self.workspace_root_optional().map_err(|error| {
            crate::file_change::FileChangeError::with_diagnostic(
                crate::file_change::FileChangeErrorCode::Failed,
                error.to_string(),
            )
        })?;
        let allow_outside_workspace = self.permissions.read == AgentReadPermission::All
            || self.permissions.write == AgentWritePermission::All;
        crate::file_change::FileChangePathPolicy::new(
            workspace_root.as_deref(),
            allow_outside_workspace,
        )
        .resolve(input_path)
    }

    pub(super) fn display_path(&self, input_path: &str, file_path: &Path) -> AgentResult<String> {
        self.check_cancelled()?;
        let locator = ResourceLocator::parse(input_path)
            .map_err(|error| AgentError::new(error.to_string()))?;
        if locator.is_virtual() {
            return Ok(locator.logical_value().to_string());
        }

        if let Some(alias_path) = expand_system_path(input_path).map_err(AgentError::new)? {
            if self.permissions.read != AgentReadPermission::All {
                return Err(AgentError::new(
                    "读取系统路径别名需要将读取范围设为“所有位置”。",
                ));
            }
            let alias_path = alias_path
                .canonicalize()
                .map_err(|error| AgentError::new(format!("路径不可访问：{error}")))?;
            if file_path == alias_path {
                return Ok(input_path.trim_end_matches('/').to_string());
            }
            if let Ok(relative) = file_path.strip_prefix(&alias_path) {
                let relative = relative_display(&alias_path, &alias_path.join(relative));
                return Ok(format!("{}/{}", input_path.trim_end_matches('/'), relative));
            }
        }

        if let Some(root) = self.workspace_root_optional()? {
            if file_path.starts_with(&root) {
                return Ok(relative_display(&root, file_path));
            }
        }
        if self.permissions.read == AgentReadPermission::All && file_path.is_absolute() {
            return Ok(file_path.to_string_lossy().to_string());
        }

        Err(AgentError::new("路径必须位于已选择的 workspace 内。"))
    }

    fn file_input_execution_context(&self) -> AgentFileInputExecutionContext {
        AgentFileInputExecutionContext::new(
            self.attachment_library.clone(),
            self.skill_resources.clone(),
        )
        .with_storage(self.storage.clone())
        .with_conversation_id(self.conversation_id.as_deref())
        .with_permissions(self.permissions)
    }

    pub(super) fn conversation_attachments(&self) -> &[AgentAttachmentReference] {
        self.attachment_library
            .as_ref()
            .map(|library| library.conversation_attachments.as_slice())
            .unwrap_or(&[])
    }

    pub(super) fn project_attachments(&self) -> &[AgentAttachmentReference] {
        self.attachment_library
            .as_ref()
            .map(|library| library.project_attachments.as_slice())
            .unwrap_or(&[])
    }

    pub(super) fn validate_relative_path_for_git(&self, input_path: &str) -> AgentResult<PathBuf> {
        clean_relative_path(input_path)
    }

    pub(super) fn attachment_reference_for_path(
        &self,
        input_path: &str,
    ) -> AgentResult<&AgentAttachmentReference> {
        let normalized_path = input_path.trim();
        let attachment_id = attachment_id_from_path(input_path)?;
        let library = self.attachment_library.as_ref().ok_or_else(|| {
            AgentError::new("当前对话没有可用的附件库，无法读取 @attachments 路径。")
        })?;

        let reference = library
            .conversation_attachments
            .iter()
            .chain(library.project_attachments.iter())
            .find(|attachment| attachment.id == attachment_id)
            .ok_or_else(|| AgentError::new(format!("未找到附件：{attachment_id}")))?;

        if reference.read_path != normalized_path {
            return Err(AgentError::new(
                "附件路径必须使用 attachments_list 返回的完整 readPath。",
            ));
        }

        Ok(reference)
    }
}

fn canonicalize_parent_preserving_leaf(candidate: &Path) -> AgentResult<PathBuf> {
    let resolved = match (candidate.parent(), candidate.file_name()) {
        (Some(parent), Some(file_name)) => parent
            .canonicalize()
            .map(|parent| parent.join(file_name))
            .map_err(|error| AgentError::new(format!("路径不可访问：{error}")))?,
        _ => candidate
            .canonicalize()
            .map_err(|error| AgentError::new(format!("路径不可访问：{error}")))?,
    };
    std::fs::symlink_metadata(&resolved)
        .map_err(|error| AgentError::new(format!("路径不可访问：{error}")))?;
    Ok(resolved)
}

fn attachment_id_from_path(input_path: &str) -> AgentResult<String> {
    let trimmed = input_path.trim();
    let remainder = trimmed
        .strip_prefix("@attachments/")
        .ok_or_else(|| AgentError::new("附件路径必须以 @attachments/ 开头。"))?;
    let attachment_id = remainder
        .split('/')
        .next()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AgentError::new("附件路径缺少附件 id。"))?;

    Ok(attachment_id.to_string())
}

#[cfg(test)]
mod model_image_delivery_budget_tests {
    use super::*;

    #[test]
    fn multimodal_delivery_budget_is_shared_by_cloned_tool_contexts() {
        let context = ToolExecutionContext::from_run_context(None)
            .with_model_capabilities(ModelCapabilities { image_input: true });
        let clone = context.clone();
        let half = MODEL_IMAGE_DELIVERY_BUDGET_BYTES_PER_RUN / 2;

        context
            .try_reserve_model_image_delivery(half)
            .expect("first half should fit")
            .commit();
        clone
            .try_reserve_model_image_delivery(half)
            .expect("second half should fit")
            .commit();

        assert!(context.try_reserve_model_image_delivery(1).is_none());
    }

    #[test]
    fn failed_delivery_refunds_budget_and_text_models_have_no_budget() {
        let multimodal = ToolExecutionContext::from_run_context(None)
            .with_model_capabilities(ModelCapabilities { image_input: true });
        let reservation = multimodal
            .try_reserve_model_image_delivery(MODEL_IMAGE_DELIVERY_BUDGET_BYTES_PER_RUN)
            .expect("full budget should be reservable");
        drop(reservation);
        assert!(multimodal
            .try_reserve_model_image_delivery(MODEL_IMAGE_DELIVERY_BUDGET_BYTES_PER_RUN)
            .is_some());

        let text_only = ToolExecutionContext::from_run_context(None)
            .with_model_capabilities(ModelCapabilities { image_input: false });
        assert!(text_only.try_reserve_model_image_delivery(1).is_none());
    }
}
