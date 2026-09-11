//! One authorization boundary for every Agent-collaboration caller.
//!
//! Renderer requests, model-facing Harness adapters and application services all resolve the
//! caller from durable `agent_nodes`.  A caller-provided `isChild`, root id, project id or task path
//! is never an authority fact.

use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{AgentLifecycle, AgentNodeRecord};
use std::fmt;
use std::sync::Arc;

/// Small product limits. Tree capacity is enforced atomically by persistence; message size is
/// rejected before enqueue as well as at the transactional write boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentAccessPolicy {
    pub(crate) max_tree_depth: u32,
    pub(crate) max_nodes_per_tree: u32,
    pub(crate) max_message_bytes: usize,
}

impl Default for AgentAccessPolicy {
    fn default() -> Self {
        Self {
            max_tree_depth: 8,
            max_nodes_per_tree: 64,
            max_message_bytes: 64 * 1024,
        }
    }
}

impl AgentAccessPolicy {
    pub(crate) fn validate(self) -> Result<Self, CollaborationAuthorizationError> {
        if self.max_tree_depth == 0 || self.max_tree_depth > 32 {
            return Err(CollaborationAuthorizationError::InvalidPolicy(
                "max_tree_depth must be between 1 and 32".to_string(),
            ));
        }
        if self.max_nodes_per_tree < 2 || self.max_nodes_per_tree > 1_024 {
            return Err(CollaborationAuthorizationError::InvalidPolicy(
                "max_nodes_per_tree must be between 2 and 1024".to_string(),
            ));
        }
        if self.max_message_bytes == 0 || self.max_message_bytes > 1024 * 1024 {
            return Err(CollaborationAuthorizationError::InvalidPolicy(
                "max_message_bytes must be between 1 and 1048576".to_string(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentManagementOperation {
    FollowUp,
    Interrupt,
    Wait,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CollaborationAuthorizationError {
    InvalidPolicy(String),
    ReadOnlyChildConversation,
    CallerUnavailable,
    PermissionDenied,
    ResourceLimit { resource: &'static str, limit: u64 },
    StorageUnavailable(String),
}

impl CollaborationAuthorizationError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::InvalidPolicy(_) => "invalid_argument",
            Self::ReadOnlyChildConversation => "read_only_child",
            Self::CallerUnavailable | Self::PermissionDenied => "permission_denied",
            Self::ResourceLimit { .. } => "resource_limit",
            Self::StorageUnavailable(_) => "unavailable",
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        matches!(self, Self::StorageUnavailable(_))
    }
}

impl fmt::Display for CollaborationAuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy(reason) => {
                write!(formatter, "invalid collaboration policy: {reason}")
            }
            Self::ReadOnlyChildConversation => {
                formatter.write_str("用户只能写入根 Agent；子 Agent Conversation 是只读观察视图。")
            }
            Self::CallerUnavailable | Self::PermissionDenied => {
                formatter.write_str("Agent collaboration operation is not authorized.")
            }
            Self::ResourceLimit { resource, limit } => {
                write!(
                    formatter,
                    "Agent {resource} resource limit exceeded ({limit})"
                )
            }
            Self::StorageUnavailable(reason) => {
                write!(
                    formatter,
                    "Agent authorization storage is unavailable: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for CollaborationAuthorizationError {}

#[derive(Clone)]
pub(crate) struct CollaborationAuthorizer {
    storage: Arc<StorageService>,
    policy: AgentAccessPolicy,
}

impl CollaborationAuthorizer {
    pub(crate) fn new(storage: Arc<StorageService>) -> Self {
        Self::with_policy(storage, AgentAccessPolicy::default())
            .expect("default collaboration policy is valid")
    }

    pub(crate) fn with_policy(
        storage: Arc<StorageService>,
        policy: AgentAccessPolicy,
    ) -> Result<Self, CollaborationAuthorizationError> {
        Ok(Self {
            storage,
            policy: policy.validate()?,
        })
    }

    pub(crate) fn policy(&self) -> AgentAccessPolicy {
        self.policy
    }

    /// Legacy, unbound Conversations remain writable. Once a Conversation is bound to an Agent,
    /// only the durable root node is a user-write target.
    pub(crate) fn authorize_user_conversation_write(
        &self,
        conversation_id: &str,
    ) -> Result<Option<AgentNodeRecord>, CollaborationAuthorizationError> {
        let node = self.node_by_conversation(conversation_id)?;
        if node
            .as_ref()
            .is_some_and(|node| node.parent_agent_id.is_some())
        {
            return Err(CollaborationAuthorizationError::ReadOnlyChildConversation);
        }
        Ok(node)
    }

    pub(crate) fn authorize_exact_observer_read(
        &self,
        root_conversation_id: &str,
        observed_conversation_id: &str,
    ) -> Result<AgentNodeRecord, CollaborationAuthorizationError> {
        let root = self
            .node_by_conversation(root_conversation_id)?
            .filter(|node| node.parent_agent_id.is_none())
            .ok_or(CollaborationAuthorizationError::CallerUnavailable)?;
        let observed = self
            .node_by_conversation(observed_conversation_id)?
            .ok_or(CollaborationAuthorizationError::PermissionDenied)?;
        if observed.parent_agent_id.is_none()
            || observed.root_agent_id != root.agent_id
            || observed.project_id != root.project_id
        {
            return Err(CollaborationAuthorizationError::PermissionDenied);
        }
        Ok(observed)
    }

    pub(crate) fn authorize_root_conversation(
        &self,
        root_conversation_id: &str,
    ) -> Result<AgentNodeRecord, CollaborationAuthorizationError> {
        self.node_by_conversation(root_conversation_id)?
            .filter(|node| node.parent_agent_id.is_none())
            .ok_or(CollaborationAuthorizationError::CallerUnavailable)
    }

    pub(crate) fn authorize_spawn(
        &self,
        caller_agent_id: &str,
    ) -> Result<AgentNodeRecord, CollaborationAuthorizationError> {
        // Identity/lifecycle authorization is intentionally separate from capacity enforcement.
        // Node/depth limits are checked under the child-creation BEGIN IMMEDIATE transaction so
        // concurrent spawns cannot race and an already-created idempotent request remains
        // resolvable after an administrator lowers a limit.
        self.required_active_caller(caller_agent_id)
    }

    pub(crate) fn authorize_send(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        message: &str,
    ) -> Result<(AgentNodeRecord, AgentNodeRecord), CollaborationAuthorizationError> {
        self.authorize_message_size(message)?;
        let caller = self.required_active_caller(caller_agent_id)?;
        let target = self.required_target(target_agent_id)?;
        self.ensure_same_tree(&caller, &target)?;
        Ok((caller, target))
    }

    pub(crate) fn authorize_management(
        &self,
        caller_agent_id: &str,
        target_agent_id: &str,
        _operation: AgentManagementOperation,
    ) -> Result<(AgentNodeRecord, AgentNodeRecord), CollaborationAuthorizationError> {
        let caller = self.required_active_caller(caller_agent_id)?;
        let target = self.required_target(target_agent_id)?;
        self.ensure_same_tree(&caller, &target)?;
        let tree = self.tree_for(&caller)?;
        if !is_strict_descendant(&tree, &caller.agent_id, &target.agent_id) {
            return Err(CollaborationAuthorizationError::PermissionDenied);
        }
        Ok((caller, target))
    }

    /// Model addressing is an exact task name within the caller's durable root tree. Names and
    /// node identities are immutable; the resolved ID still passes the ordinary operation
    /// authorization and transactional scheduling checks before any effect is committed.
    pub(crate) fn resolve_task_name(
        &self,
        caller_agent_id: &str,
        task_name: &str,
    ) -> Result<Option<AgentNodeRecord>, CollaborationAuthorizationError> {
        let caller = self.required_active_caller(caller_agent_id)?;
        Ok(self
            .tree_for(&caller)?
            .into_iter()
            .find(|node| node.task_name == task_name))
    }

    /// Authorizes a root-card decision without trusting any source identity carried by the UI.
    pub(crate) fn authorize_root_projection(
        &self,
        root_conversation_id: &str,
        source_conversation_id: &str,
    ) -> Result<(AgentNodeRecord, AgentNodeRecord), CollaborationAuthorizationError> {
        let root = self
            .node_by_conversation(root_conversation_id)?
            .filter(|node| node.parent_agent_id.is_none())
            .ok_or(CollaborationAuthorizationError::CallerUnavailable)?;
        let source = self
            .node_by_conversation(source_conversation_id)?
            .ok_or(CollaborationAuthorizationError::PermissionDenied)?;
        self.ensure_same_tree(&root, &source)?;
        if source.agent_id == root.agent_id {
            return Err(CollaborationAuthorizationError::PermissionDenied);
        }
        Ok((root, source))
    }

    pub(crate) fn visible_tree(
        &self,
        caller_agent_id: &str,
    ) -> Result<Vec<AgentNodeRecord>, CollaborationAuthorizationError> {
        let caller = self.required_active_caller(caller_agent_id)?;
        self.tree_for(&caller)
    }

    pub(crate) fn authorize_message_size(
        &self,
        message: &str,
    ) -> Result<(), CollaborationAuthorizationError> {
        if message.trim().is_empty() {
            return Err(CollaborationAuthorizationError::InvalidPolicy(
                "message must be non-empty".to_string(),
            ));
        }
        if message.len() > self.policy.max_message_bytes {
            return Err(CollaborationAuthorizationError::ResourceLimit {
                resource: "message_bytes",
                limit: self.policy.max_message_bytes as u64,
            });
        }
        Ok(())
    }

    fn required_active_caller(
        &self,
        agent_id: &str,
    ) -> Result<AgentNodeRecord, CollaborationAuthorizationError> {
        let node = self
            .node(agent_id)?
            .ok_or(CollaborationAuthorizationError::CallerUnavailable)?;
        if node.lifecycle != AgentLifecycle::Active {
            return Err(CollaborationAuthorizationError::CallerUnavailable);
        }
        Ok(node)
    }

    fn required_target(
        &self,
        agent_id: &str,
    ) -> Result<AgentNodeRecord, CollaborationAuthorizationError> {
        self.node(agent_id)?
            .ok_or(CollaborationAuthorizationError::PermissionDenied)
    }

    fn ensure_same_tree(
        &self,
        caller: &AgentNodeRecord,
        target: &AgentNodeRecord,
    ) -> Result<(), CollaborationAuthorizationError> {
        if caller.root_agent_id != target.root_agent_id || caller.project_id != target.project_id {
            return Err(CollaborationAuthorizationError::PermissionDenied);
        }
        Ok(())
    }

    fn tree_for(
        &self,
        caller: &AgentNodeRecord,
    ) -> Result<Vec<AgentNodeRecord>, CollaborationAuthorizationError> {
        self.storage
            .list_agent_tree(&caller.root_agent_id)
            .map_err(|error| CollaborationAuthorizationError::StorageUnavailable(error.to_string()))
    }

    fn node(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentNodeRecord>, CollaborationAuthorizationError> {
        self.storage
            .get_agent_node(agent_id)
            .map_err(|error| CollaborationAuthorizationError::StorageUnavailable(error.to_string()))
    }

    fn node_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<AgentNodeRecord>, CollaborationAuthorizationError> {
        self.storage
            .get_agent_node_by_conversation(conversation_id)
            .map_err(|error| CollaborationAuthorizationError::StorageUnavailable(error.to_string()))
    }
}

fn is_strict_descendant(tree: &[AgentNodeRecord], ancestor_id: &str, target_id: &str) -> bool {
    let mut current = tree.iter().find(|node| node.agent_id == target_id);
    let mut remaining = tree.len();
    while let Some(node) = current {
        let Some(parent_id) = node.parent_agent_id.as_deref() else {
            return false;
        };
        if parent_id == ancestor_id {
            return true;
        }
        if remaining == 0 {
            return false;
        }
        remaining -= 1;
        current = tree
            .iter()
            .find(|candidate| candidate.agent_id == parent_id);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::{
        ChatConversationMetaRecord, ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
    };
    use mycopilot_core::{
        AgentForkTurns, CreateChildAgentInput, EnsureRootAgentInput, ProviderProfileConfig,
        ProviderProtocolDialect,
    };

    fn model_settings() -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://provider.example/v1/chat/completions".to_string(),
            api_token: "fixture-secret".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "model-a".to_string(),
                provider_model_id: "model-a".to_string(),
                display_name: "Model A".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(64_000),
                provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".to_string(),
                cached_input_price: String::new(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        }
    }

    fn save_root(
        storage: &StorageService,
        project_id: &str,
        root_agent_id: &str,
        conversation_id: &str,
    ) {
        storage
            .save_project(ProjectRecord::without_folders(
                project_id.to_string(),
                project_id.to_string(),
                1,
            ))
            .unwrap();
        storage
            .save_conversation_meta(ChatConversationMetaRecord {
                id: conversation_id.to_string(),
                project_id: Some(project_id.to_string()),
                model_id: Some("model-a".to_string()),
                title: "Root".to_string(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        storage
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: root_agent_id.to_string(),
                conversation_id: conversation_id.to_string(),
                creation_request_id: format!("ensure-{root_agent_id}"),
                task_name: "Root".to_string(),
            })
            .unwrap();
    }

    fn spawn(storage: &StorageService, parent_agent_id: &str, request_id: &str) -> AgentNodeRecord {
        storage
            .create_child_agent(&CreateChildAgentInput {
                parent_agent_id: parent_agent_id.to_string(),
                creation_request_id: request_id.to_string(),
                task_name: request_id.to_string(),
                task: "Inspect and report.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: AgentForkTurns::None,
            })
            .unwrap()
            .agent
    }

    #[test]
    fn task_name_resolution_is_exact_tree_scoped_and_never_falls_back_to_ids_or_paths() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open(&directory.path().join("name-resolution.sqlite")).unwrap(),
        );
        storage.save_model_settings(model_settings()).unwrap();
        save_root(&storage, "project-a", "root-a", "conversation-root-a");
        save_root(&storage, "project-b", "root-b", "conversation-root-b");
        let child = spawn(&storage, "root-a", "Review");
        let other = spawn(&storage, "root-b", "Review");
        let wildcard = spawn(&storage, "root-a", "review_%");
        let authorizer = CollaborationAuthorizer::new(Arc::clone(&storage));
        assert_eq!(
            authorizer
                .resolve_task_name("root-a", "Review")
                .unwrap()
                .unwrap()
                .agent_id,
            child.agent_id
        );
        assert_eq!(
            authorizer
                .resolve_task_name("root-b", "Review")
                .unwrap()
                .unwrap()
                .agent_id,
            other.agent_id
        );
        assert_eq!(
            authorizer
                .resolve_task_name("root-a", "review_%")
                .unwrap()
                .unwrap()
                .agent_id,
            wildcard.agent_id
        );
        for target in [
            &child.agent_id,
            &child.task_path,
            &other.agent_id,
            "review",
            " Review",
            "Review ",
            "Rev",
            "%",
        ] {
            assert!(
                authorizer
                    .resolve_task_name("root-a", target)
                    .unwrap()
                    .is_none(),
                "unexpected alias resolution for {target}"
            );
        }
        assert!(authorizer
            .resolve_task_name("unavailable-caller", "Review")
            .is_err());
        assert!(
            authorizer
                .authorize_management(
                    &child.agent_id,
                    &wildcard.agent_id,
                    AgentManagementOperation::Wait
                )
                .is_err(),
            "name resolution must not broaden descendant authority"
        );
    }

    #[test]
    fn durable_nodes_authorize_root_writes_observer_reads_and_tree_management_without_leaks() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("authorization.sqlite")).unwrap());
        storage.save_model_settings(model_settings()).unwrap();
        save_root(&storage, "project-a", "root-a", "conversation-root-a");
        let child = spawn(&storage, "root-a", "child-a");
        let grandchild = spawn(&storage, &child.agent_id, "grandchild-a");
        save_root(&storage, "project-b", "root-b", "conversation-root-b");
        let other_child = spawn(&storage, "root-b", "child-b");

        let authorizer = CollaborationAuthorizer::new(Arc::clone(&storage));
        assert!(authorizer
            .authorize_user_conversation_write("conversation-root-a")
            .is_ok());
        assert_eq!(
            authorizer
                .authorize_user_conversation_write(&child.conversation_id)
                .unwrap_err(),
            CollaborationAuthorizationError::ReadOnlyChildConversation
        );
        assert_eq!(
            authorizer
                .authorize_exact_observer_read("conversation-root-a", &child.conversation_id)
                .unwrap()
                .agent_id,
            child.agent_id
        );
        assert_eq!(
            authorizer
                .authorize_exact_observer_read("conversation-root-a", &other_child.conversation_id,)
                .unwrap_err(),
            CollaborationAuthorizationError::PermissionDenied
        );
        assert_eq!(
            authorizer
                .authorize_root_projection("conversation-root-a", &other_child.conversation_id)
                .unwrap_err(),
            CollaborationAuthorizationError::PermissionDenied
        );
        assert_eq!(
            authorizer
                .authorize_root_projection("conversation-root-a", "conversation-root-a")
                .unwrap_err(),
            CollaborationAuthorizationError::PermissionDenied
        );

        assert!(authorizer
            .authorize_send("root-a", &child.agent_id, "queued only")
            .is_ok());
        assert_eq!(
            authorizer
                .authorize_send("root-a", &other_child.agent_id, "must not leak")
                .unwrap_err(),
            CollaborationAuthorizationError::PermissionDenied
        );
        assert_eq!(
            authorizer
                .authorize_send("root-a", "unknown-agent", "must not leak")
                .unwrap_err(),
            CollaborationAuthorizationError::PermissionDenied
        );
        assert!(authorizer
            .authorize_management(
                "root-a",
                &child.agent_id,
                AgentManagementOperation::Interrupt,
            )
            .is_ok());
        assert!(authorizer
            .authorize_management(
                "root-a",
                &grandchild.agent_id,
                AgentManagementOperation::Wait,
            )
            .is_ok());
        assert!(authorizer
            .authorize_management(
                &child.agent_id,
                &grandchild.agent_id,
                AgentManagementOperation::FollowUp,
            )
            .is_ok());
        assert_eq!(
            authorizer
                .authorize_management(
                    &child.agent_id,
                    "root-a",
                    AgentManagementOperation::FollowUp,
                )
                .unwrap_err(),
            CollaborationAuthorizationError::PermissionDenied
        );

        let constrained = CollaborationAuthorizer::with_policy(
            Arc::clone(&storage),
            AgentAccessPolicy {
                max_tree_depth: 8,
                max_nodes_per_tree: 2,
                max_message_bytes: 4,
            },
        )
        .unwrap();
        assert_eq!(
            constrained.authorize_spawn("root-a").unwrap().agent_id,
            "root-a"
        );
        assert_eq!(
            constrained
                .authorize_send("root-a", &child.agent_id, "12345")
                .unwrap_err(),
            CollaborationAuthorizationError::ResourceLimit {
                resource: "message_bytes",
                limit: 4,
            }
        );
    }
}
