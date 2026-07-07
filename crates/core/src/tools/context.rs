// Tool execution context and path resolution helpers.
use super::{clean_relative_path, relative_display};
use crate::cancellation::AgentCancellationToken;
use crate::protocol::{
    AgentAttachmentLibraryContext, AgentAttachmentReference, AgentError, AgentPermissions,
    AgentReadPermission, AgentResult, AgentRunContext,
};
use crate::system_paths::expand_system_path;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct ToolExecutionContext {
    workspace_root: Option<PathBuf>,
    attachment_library: Option<AgentAttachmentLibraryContext>,
    cancellation_token: AgentCancellationToken,
    permissions: AgentPermissions,
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

        Self {
            workspace_root,
            attachment_library,
            cancellation_token: AgentCancellationToken::new(),
            permissions,
        }
    }

    pub fn with_cancellation(mut self, cancellation_token: AgentCancellationToken) -> Self {
        self.cancellation_token = cancellation_token;
        self
    }

    pub(super) fn cancellation_token(&self) -> AgentCancellationToken {
        self.cancellation_token.clone()
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

    fn attachment_reference_for_path(
        &self,
        input_path: &str,
    ) -> AgentResult<&AgentAttachmentReference> {
        let attachment_id = attachment_id_from_path(input_path)?;
        let library = self.attachment_library.as_ref().ok_or_else(|| {
            AgentError::new("当前对话没有可用的附件库，无法读取 @attachments 路径。")
        })?;

        library
            .conversation_attachments
            .iter()
            .chain(library.project_attachments.iter())
            .find(|attachment| attachment.id == attachment_id)
            .ok_or_else(|| AgentError::new(format!("未找到附件：{attachment_id}")))
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
