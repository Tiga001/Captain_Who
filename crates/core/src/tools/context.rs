// Tool execution context and path resolution helpers.
use super::{clean_relative_path, relative_display};
use crate::cancellation::AgentCancellationToken;
use crate::context::ContextTextBudget;
use crate::protocol::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentError, AgentPermissions,
    AgentReadPermission, AgentResult, AgentRunContext, ModelCapabilities,
};
use crate::storage::service::StorageService;
use crate::system_paths::expand_system_path;
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
    attachment_library: Option<AgentAttachmentLibraryContext>,
    cancellation_token: AgentCancellationToken,
    model_capabilities: ModelCapabilities,
    model_image_delivery_budget: Arc<AtomicU64>,
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
}

impl ToolExecutionContext {
    pub fn from_run_context(context: Option<&AgentRunContext>) -> Self {
        let workspace_root = context
            .and_then(|context| context.workspace.as_ref())
            .and_then(|workspace| workspace.root_path.as_ref())
            .map(PathBuf::from);
        let attachment_library = context.and_then(|context| context.attachment_library.clone());
        let permissions = context
            .map(|context| context.permissions)
            .unwrap_or_default();
        let conversation_id = context.and_then(|context| context.conversation_id.clone());
        let project_id = context.and_then(|context| context.project_id.clone());

        Self {
            workspace_root,
            attachment_library,
            cancellation_token: AgentCancellationToken::new(),
            model_capabilities: ModelCapabilities::default(),
            model_image_delivery_budget: Arc::new(AtomicU64::new(0)),
            permissions,
            conversation_id,
            project_id,
            run_id: None,
            tool_call_id: None,
            storage: None,
            text_output_budget: ContextTextBudget::heuristic_default(),
            skill_resources: None,
            command_runtime_profile_resolver: None,
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

    /// Returns the registered attachment capabilities for trusted tool
    /// adapters that perform their own purpose-aware path authorization.
    /// Callers must still resolve an attachment by its exact backend-issued
    /// `readPath`; this accessor does not grant generic filesystem access.
    pub(super) fn attachment_library(&self) -> Option<&AgentAttachmentLibraryContext> {
        self.attachment_library.as_ref()
    }

    pub(super) fn conversation_id(&self) -> AgentResult<&str> {
        self.conversation_id
            .as_deref()
            .ok_or_else(|| AgentError::new("当前运行缺少 conversationId，无法访问会话级能力。"))
    }

    pub(super) fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub(super) fn run_id(&self) -> AgentResult<&str> {
        self.run_id
            .as_deref()
            .ok_or_else(|| AgentError::new("当前运行缺少 runId，不能创建文件草稿。"))
    }

    pub(crate) fn tool_call_id(&self) -> AgentResult<&str> {
        self.tool_call_id
            .as_deref()
            .ok_or_else(|| AgentError::new("当前工具执行缺少可信 tool call id。"))
    }

    pub(super) fn storage(&self) -> AgentResult<&Arc<StorageService>> {
        self.storage
            .as_ref()
            .ok_or_else(|| AgentError::new("当前 host 未提供会话存储服务。"))
    }

    pub(super) fn skill_resources(&self) -> AgentResult<&Arc<crate::skills::SkillResourceSession>> {
        self.skill_resources.as_ref().ok_or_else(|| {
            AgentError::new("当前运行没有激活可访问资源的 Skill；请先选择并激活一个 Skill。")
        })
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

    pub(super) fn resolve_existing_path(&self, input_path: &str) -> AgentResult<PathBuf> {
        self.check_cancelled()?;
        if is_attachment_path(input_path) {
            return self.resolve_attachment_path(input_path);
        }

        let input_path = input_path.trim();
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

    pub(super) fn display_path(&self, input_path: &str, file_path: &Path) -> AgentResult<String> {
        self.check_cancelled()?;
        if is_attachment_path(input_path) {
            return self
                .attachment_reference_for_path(input_path)
                .map(|reference| reference.read_path.clone());
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

    fn resolve_attachment_path(&self, input_path: &str) -> AgentResult<PathBuf> {
        self.check_cancelled()?;
        let reference = self.attachment_reference_for_path(input_path)?;
        let library = self.attachment_library.as_ref().ok_or_else(|| {
            AgentError::new("当前对话没有可用的附件库，无法读取 @attachments 路径。")
        })?;
        let root_path = library
            .root_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| AgentError::new("附件库根目录不可用。"))?;
        let root = PathBuf::from(root_path)
            .canonicalize()
            .map_err(|error| AgentError::new(format!("附件库目录不可访问：{error}")))?;
        if !root.is_dir() {
            return Err(AgentError::new("附件库根目录不是目录。"));
        }

        let relative = clean_relative_path(&reference.storage_rel_path)?;
        let canonical = root
            .join(relative)
            .canonicalize()
            .map_err(|error| AgentError::new(format!("附件文件不可访问：{error}")))?;

        if !canonical.starts_with(&root) {
            return Err(AgentError::new("附件路径必须位于附件库目录内。"));
        }

        Ok(canonical)
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

fn is_attachment_path(input_path: &str) -> bool {
    input_path.trim().starts_with("@attachments/")
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
