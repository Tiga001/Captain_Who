//! Application boundary for persistent Agent collaboration facts.
//!
//! This service deliberately does not own or call the Agent Loop. The unified Turn executor and
//! Dispatcher compose beside this boundary; the graph repository never becomes a Runtime
//! dependency.

use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    AcknowledgeAgentTaskAndWakeInput, AgentGraphError, AgentLifecycle, AgentMailboxMessageRecord,
    AgentNodeRecord, AgentSamplingBoundaryDelivery, AgentSamplingBoundaryInbox,
    AgentSamplingBoundaryMessage, AgentSamplingBoundaryRequest, AgentTemplateError,
    AgentTemplateRecord, AgentWakeRequestRecord, AgentWakeStatus, BindAgentSafeBoundaryInput,
    ChildAgentSpawnError, ChildAgentSpawnRecord, ConversationMessageOrigin,
    CreateAgentTemplateInput, CreateChildAgentInput, EnqueueAgentMessageInput,
    EnqueueAgentWakeInput, EnsureRootAgentInput, IdempotentCreate, ResolvedAgentTemplateForSpawn,
    TrustedActiveChildWakeBundle, UpdateAgentTemplateInput,
};
use std::sync::Arc;

/// Narrow use-case service shared by future Harness and transport adapters.
///
/// Keeping this separate from `AgentService` is intentional: an Agent identity and its durable
/// coordination facts outlive every individual Runtime/Turn hosted by `AgentService`.
#[derive(Clone)]
pub(crate) struct AgentCollaborationService {
    storage: Arc<StorageService>,
}

/// Storage-backed implementation of Core's single pre-sampling collaboration port. Installing it
/// for every Runtime segment is harmless for legacy Conversations: unbound Conversations return
/// `None`, while graph-bound root and child Turns share the same exact admission path.
pub(crate) struct PersistentAgentSamplingBoundaryInbox {
    storage: Arc<StorageService>,
}

impl PersistentAgentSamplingBoundaryInbox {
    pub(crate) fn new(storage: Arc<StorageService>) -> Self {
        Self { storage }
    }
}

impl AgentSamplingBoundaryInbox for PersistentAgentSamplingBoundaryInbox {
    fn bind_for_model_batch(
        &self,
        request: AgentSamplingBoundaryRequest,
    ) -> mycopilot_core::AgentResult<Option<AgentSamplingBoundaryDelivery>> {
        let delivery = self
            .storage
            .bind_agent_safe_boundary(&BindAgentSafeBoundaryInput {
                conversation_id: request.conversation_id,
                run_id: request.run_id,
                assistant_message_id: request.assistant_message_id,
                model_batch_index: request.model_batch_index,
                expected_next_trace_sequence: request.expected_next_trace_sequence,
                maximum: 1_024,
            })
            .map_err(|error| mycopilot_core::AgentError::new(error.to_string()))?;
        delivery
            .map(|delivery| {
                let messages = delivery
                    .messages
                    .into_iter()
                    .map(|message| {
                        Ok(AgentSamplingBoundaryMessage {
                            trace_sequence: message.trace_sequence.ok_or_else(|| {
                                mycopilot_core::AgentError::new(
                                    "safe-boundary receipt item is missing trace identity",
                                )
                            })?,
                            message_id: message.message_id,
                            sender_agent_id: message.sender_agent_id,
                            sender_task_name: message.sender_task_name,
                            sender_task_path: message.sender_task_path,
                            kind: message.kind,
                            content: message.content,
                            created_at: message.created_at,
                        })
                    })
                    .collect::<mycopilot_core::AgentResult<Vec<_>>>()?;
                Ok(AgentSamplingBoundaryDelivery {
                    receipt_id: delivery.receipt.receipt_id,
                    messages,
                })
            })
            .transpose()
    }
}

/// The only application entry point allowed to create child Agent identities.
///
/// It intentionally has no Runtime or transport dependency. A dispatcher can later consume the
/// returned queued Wake through the trusted resolve method without gaining access to low-level
/// node binding APIs.
#[derive(Clone)]
pub(crate) struct ChildAgentFactory {
    storage: Arc<StorageService>,
    authorizer: crate::application::collaboration_authorization::CollaborationAuthorizer,
}

/// Narrow Host/application boundary for durable Agent-to-Agent communication. Transport and model
/// tools are intentionally absent until Round 4.
#[derive(Clone)]
pub(crate) struct AgentMessagingService {
    storage: Arc<StorageService>,
    authorizer: crate::application::collaboration_authorization::CollaborationAuthorizer,
    wait_notifications: crate::application::agent_wait::AgentWaitNotifications,
}

impl AgentMessagingService {
    pub(crate) fn new(storage: Arc<StorageService>) -> Self {
        Self {
            authorizer:
                crate::application::collaboration_authorization::CollaborationAuthorizer::new(
                    Arc::clone(&storage),
                ),
            storage,
            wait_notifications: crate::application::agent_wait::shared_agent_wait_notifications(),
        }
    }

    pub(crate) fn send_message(
        &self,
        input: &mycopilot_core::SendAgentMessageRequest,
    ) -> Result<mycopilot_core::AgentMessageDispatch, AgentGraphError> {
        self.authorizer
            .authorize_send(
                &input.sender_agent_id,
                &input.recipient_agent_id,
                &input.content,
            )
            .map_err(map_authorization_error)?;
        let dispatch = self.storage.send_agent_message(input)?;
        self.wait_notifications
            .notify_caller(&dispatch.message.recipient_agent_id);
        Ok(dispatch)
    }

    pub(crate) fn follow_up(
        &self,
        input: &mycopilot_core::SendAgentMessageRequest,
    ) -> Result<mycopilot_core::AgentMessageDispatch, AgentGraphError> {
        self.authorizer
            .authorize_message_size(&input.content)
            .and_then(|()| {
                self.authorizer
                    .authorize_management(
                        &input.sender_agent_id,
                        &input.recipient_agent_id,
                        crate::application::collaboration_authorization::AgentManagementOperation::FollowUp,
                    )
                    .map(|_| ())
            })
            .map_err(map_authorization_error)?;
        let dispatch = self.storage.follow_up_agent(input)?;
        self.wait_notifications
            .notify_caller(&dispatch.message.recipient_agent_id);
        Ok(dispatch)
    }

    pub(crate) fn finish_turn_result(
        &self,
        input: &mycopilot_core::FinishAgentTurnResultInput,
    ) -> Result<mycopilot_core::AgentTurnResultSettlement, AgentGraphError> {
        let settlement = self.storage.finish_agent_turn_with_result(input)?;
        self.wait_notifications
            .notify_caller(&settlement.result_message.recipient_agent_id);
        Ok(settlement)
    }

    pub(crate) fn display_status(
        &self,
        agent_id: &str,
    ) -> Result<mycopilot_core::AgentDisplayStatusSnapshot, AgentGraphError> {
        self.storage.get_agent_display_status(agent_id)
    }
}

impl ChildAgentFactory {
    pub(crate) fn new(storage: Arc<StorageService>) -> Self {
        Self {
            authorizer:
                crate::application::collaboration_authorization::CollaborationAuthorizer::new(
                    Arc::clone(&storage),
                ),
            storage,
        }
    }

    pub(crate) fn with_policy(
        storage: Arc<StorageService>,
        policy: crate::application::collaboration_authorization::AgentAccessPolicy,
    ) -> Result<
        Self,
        crate::application::collaboration_authorization::CollaborationAuthorizationError,
    > {
        Ok(Self {
            authorizer:
                crate::application::collaboration_authorization::CollaborationAuthorizer::with_policy(
                    Arc::clone(&storage),
                    policy,
                )?,
            storage,
        })
    }

    pub(crate) fn create_child(
        &self,
        input: &CreateChildAgentInput,
    ) -> Result<ChildAgentSpawnRecord, ChildAgentSpawnError> {
        self.authorizer
            .authorize_spawn(&input.parent_agent_id)
            .map_err(map_spawn_authorization_error)?;
        let policy = self.authorizer.policy();
        self.storage.create_child_agent_with_limits(
            input,
            mycopilot_core::AgentTreeResourceLimits {
                max_depth: policy.max_tree_depth,
                max_nodes: policy.max_nodes_per_tree,
                max_task_bytes: policy.max_message_bytes,
            },
        )
    }

    pub(crate) fn resolve_trusted_running_wake(
        &self,
        agent_id: &str,
        wake_id: &str,
        source_agent_message_id: &str,
        claim_token: &str,
    ) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
        self.storage.resolve_running_child_agent_wake(
            agent_id,
            wake_id,
            source_agent_message_id,
            claim_token,
        )
    }

    pub(crate) fn resolve_trusted_claimed_wake(
        &self,
        agent_id: &str,
        wake_id: &str,
        source_agent_message_id: &str,
        claim_token: &str,
    ) -> Result<ChildAgentSpawnRecord, AgentGraphError> {
        self.storage.resolve_claimed_agent_wake(
            agent_id,
            wake_id,
            source_agent_message_id,
            claim_token,
        )
    }

    pub(crate) fn resolve_trusted_active_wake_by_identity(
        &self,
        identity: &mycopilot_core::AgentCollaborationIdentity,
    ) -> Result<TrustedActiveChildWakeBundle, AgentGraphError> {
        self.storage
            .resolve_active_child_agent_wake_by_identity(identity)
    }
}

fn map_authorization_error(
    error: crate::application::collaboration_authorization::CollaborationAuthorizationError,
) -> AgentGraphError {
    use crate::application::collaboration_authorization::CollaborationAuthorizationError;
    match error {
        CollaborationAuthorizationError::ResourceLimit { resource, limit } => {
            AgentGraphError::ResourceLimit { resource, limit }
        }
        CollaborationAuthorizationError::StorageUnavailable(reason) => {
            AgentGraphError::StorageUnavailable(reason)
        }
        CollaborationAuthorizationError::InvalidPolicy(reason) => AgentGraphError::InvalidInput {
            field: "collaboration_policy",
            reason,
        },
        CollaborationAuthorizationError::ReadOnlyChildConversation
        | CollaborationAuthorizationError::CallerUnavailable
        | CollaborationAuthorizationError::PermissionDenied => {
            AgentGraphError::Conflict("Agent collaboration operation is not authorized".to_string())
        }
    }
}

fn map_spawn_authorization_error(
    error: crate::application::collaboration_authorization::CollaborationAuthorizationError,
) -> ChildAgentSpawnError {
    use crate::application::collaboration_authorization::CollaborationAuthorizationError;
    match error {
        CollaborationAuthorizationError::ResourceLimit { resource, limit } => {
            ChildAgentSpawnError::ResourceLimit { resource, limit }
        }
        CollaborationAuthorizationError::StorageUnavailable(reason) => {
            ChildAgentSpawnError::StorageUnavailable(reason)
        }
        CollaborationAuthorizationError::InvalidPolicy(reason) => {
            ChildAgentSpawnError::InvalidInput {
                field: "collaboration_policy",
                reason,
            }
        }
        CollaborationAuthorizationError::ReadOnlyChildConversation
        | CollaborationAuthorizationError::CallerUnavailable
        | CollaborationAuthorizationError::PermissionDenied => ChildAgentSpawnError::Conflict(
            "Agent collaboration operation is not authorized".to_string(),
        ),
    }
}

impl AgentCollaborationService {
    pub(crate) fn new(storage: Arc<StorageService>) -> Self {
        Self { storage }
    }

    pub(crate) fn ensure_root(
        &self,
        input: &EnsureRootAgentInput,
    ) -> Result<IdempotentCreate<AgentNodeRecord>, AgentGraphError> {
        self.storage.ensure_root_agent(input)
    }

    pub(crate) fn get_node(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
        self.storage.get_agent_node(agent_id)
    }

    pub(crate) fn get_node_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<AgentNodeRecord>, AgentGraphError> {
        self.storage.get_agent_node_by_conversation(conversation_id)
    }

    pub(crate) fn list_children(
        &self,
        root_agent_id: &str,
        parent_agent_id: &str,
    ) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
        self.storage
            .list_agent_children(root_agent_id, parent_agent_id)
    }

    pub(crate) fn list_tree(
        &self,
        root_agent_id: &str,
    ) -> Result<Vec<AgentNodeRecord>, AgentGraphError> {
        self.storage.list_agent_tree(root_agent_id)
    }

    pub(crate) fn transition_lifecycle(
        &self,
        agent_id: &str,
        expected_revision: u64,
        expected_lifecycle: AgentLifecycle,
        requested_lifecycle: AgentLifecycle,
    ) -> Result<AgentNodeRecord, AgentGraphError> {
        self.storage.transition_agent_lifecycle(
            agent_id,
            expected_revision,
            expected_lifecycle,
            requested_lifecycle,
        )
    }

    pub(crate) fn enqueue_message(
        &self,
        input: &EnqueueAgentMessageInput,
    ) -> Result<IdempotentCreate<AgentMailboxMessageRecord>, AgentGraphError> {
        self.storage.enqueue_agent_message(input)
    }

    pub(crate) fn get_message(
        &self,
        message_id: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
        self.storage.get_agent_message(message_id)
    }

    pub(crate) fn claim_next_message(
        &self,
        recipient_agent_id: &str,
        claim_token: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentGraphError> {
        self.storage
            .claim_next_agent_message(recipient_agent_id, claim_token)
    }

    pub(crate) fn renew_message_lease(
        &self,
        message_id: &str,
        claim_token: &str,
    ) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
        self.storage
            .renew_agent_message_lease(message_id, claim_token)
    }

    pub(crate) fn acknowledge_message_with_projection(
        &self,
        message_id: &str,
        claim_token: &str,
    ) -> Result<AgentMailboxMessageRecord, AgentGraphError> {
        self.storage
            .acknowledge_agent_message_with_projection(message_id, claim_token)
    }

    pub(crate) fn acknowledge_task_with_projection_and_wake(
        &self,
        input: &AcknowledgeAgentTaskAndWakeInput,
    ) -> Result<(AgentMailboxMessageRecord, AgentWakeRequestRecord), AgentGraphError> {
        self.storage
            .acknowledge_agent_task_with_projection_and_wake(input)
    }

    pub(crate) fn message_origin(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<ConversationMessageOrigin, AgentGraphError> {
        self.storage
            .conversation_message_origin(conversation_id, message_id)
    }

    pub(crate) fn enqueue_wake(
        &self,
        input: &EnqueueAgentWakeInput,
    ) -> Result<IdempotentCreate<AgentWakeRequestRecord>, AgentGraphError> {
        self.storage.enqueue_agent_wake(input)
    }

    pub(crate) fn get_wake(
        &self,
        wake_id: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        self.storage.get_agent_wake(wake_id)
    }

    pub(crate) fn claim_next_wake(
        &self,
        agent_id: &str,
        claim_token: &str,
    ) -> Result<Option<AgentWakeRequestRecord>, AgentGraphError> {
        self.storage.claim_next_agent_wake(agent_id, claim_token)
    }

    pub(crate) fn renew_wake_lease(
        &self,
        wake_id: &str,
        claim_token: &str,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        self.storage.renew_agent_wake_lease(wake_id, claim_token)
    }

    pub(crate) fn transition_wake(
        &self,
        wake_id: &str,
        expected_status: AgentWakeStatus,
        requested_status: AgentWakeStatus,
        claim_token: Option<&str>,
    ) -> Result<AgentWakeRequestRecord, AgentGraphError> {
        self.storage
            .transition_agent_wake(wake_id, expected_status, requested_status, claim_token)
    }

    pub(crate) fn create_template(
        &self,
        input: &CreateAgentTemplateInput,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        self.storage.create_agent_template(input)
    }

    pub(crate) fn list_templates(
        &self,
        project_id: &str,
        include_disabled: bool,
    ) -> Result<Vec<AgentTemplateRecord>, AgentTemplateError> {
        self.storage
            .list_agent_templates(project_id, include_disabled)
    }

    pub(crate) fn get_template(
        &self,
        project_id: &str,
        template_id: &str,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        self.storage.get_agent_template(project_id, template_id)
    }

    pub(crate) fn update_template(
        &self,
        input: &UpdateAgentTemplateInput,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        self.storage.update_agent_template(input)
    }

    pub(crate) fn set_template_enabled(
        &self,
        project_id: &str,
        template_id: &str,
        expected_revision: u64,
        enabled: bool,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        self.storage
            .set_agent_template_enabled(project_id, template_id, expected_revision, enabled)
    }

    pub(crate) fn delete_template(
        &self,
        project_id: &str,
        template_id: &str,
        expected_revision: u64,
    ) -> Result<AgentTemplateRecord, AgentTemplateError> {
        self.storage
            .delete_agent_template(project_id, template_id, expected_revision)
    }

    pub(crate) fn resolve_template_for_spawn(
        &self,
        project_id: &str,
        machine_key: &str,
    ) -> Result<ResolvedAgentTemplateForSpawn, AgentTemplateError> {
        self.storage
            .resolve_template_for_spawn(project_id, machine_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::{ChatConversationMetaRecord, ProjectRecord};

    #[test]
    fn application_boundary_materializes_a_root_and_manages_project_templates_without_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("collaboration.sqlite")).unwrap());
        storage
            .save_project(ProjectRecord {
                id: "project-a".to_string(),
                name: "Project A".to_string(),
                path: None,
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        storage
            .save_conversation_meta(ChatConversationMetaRecord {
                id: "root-conversation".to_string(),
                project_id: Some("project-a".to_string()),
                model_id: Some("model-a".to_string()),
                title: "Root".to_string(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentCollaborationService::new(storage);

        let input = EnsureRootAgentInput {
            agent_id: "agent-root".to_string(),
            conversation_id: "root-conversation".to_string(),
            creation_request_id: "ensure-root-1".to_string(),
            task_name: "Root".to_string(),
        };
        let created = service.ensure_root(&input).unwrap();
        assert!(matches!(created, IdempotentCreate::Created(_)));
        let existing = service.ensure_root(&input).unwrap();
        assert!(matches!(existing, IdempotentCreate::Existing(_)));
        assert_eq!(service.list_tree("agent-root").unwrap().len(), 1);

        service
            .create_template(&CreateAgentTemplateInput {
                template_id: "template-review".to_string(),
                project_id: "project-a".to_string(),
                machine_key: "reviewer".to_string(),
                name: "Reviewer".to_string(),
                description: "Read-only reviewer".to_string(),
                instructions: "Review the assigned scope and return evidence.".to_string(),
                model_config_id: "model-a".to_string(),
                enabled: true,
            })
            .unwrap();
        assert_eq!(service.list_templates("project-a", false).unwrap().len(), 1);
    }
}
