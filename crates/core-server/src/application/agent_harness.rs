//! Host adapter for the six model-facing Agent collaboration tools.

use crate::application::agent::{AgentService, CoreServerNotificationSender};
use crate::application::agent_collaboration::{AgentMessagingService, ChildAgentFactory};
use crate::application::agent_dispatcher::{
    AgentDispatcher, AgentDispatcherConfig, AgentInterruptDisposition,
    SharedAgentTurnExecutionPort, SqliteAgentDispatcherStore,
};
use crate::application::agent_wait::{AgentWaitKernel, AgentWaitOutcome, AgentWaitStopReason};
use crate::application::collaboration_authorization::{
    AgentManagementOperation, CollaborationAuthorizationError, CollaborationAuthorizer,
};
use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    bounded_root_agent_task_name, root_agent_creation_request_id, root_agent_id_for_conversation,
    AgentCollaborationAction, AgentCollaborationCaller, AgentCollaborationDeliveryState,
    AgentCollaborationExecutionControl, AgentCollaborationExecutionFuture,
    AgentCollaborationExecutionOutput, AgentCollaborationExecutor, AgentCollaborationInvocation,
    AgentCollaborationModelSelector, AgentCollaborationResultPersistence,
    AgentCollaborationRuntimeServices, AgentCollaborationSelectorDirectory,
    AgentCollaborationTemplateSelector, AgentCollaborationToolResult, AgentDisplayStatus,
    AgentError, AgentGraphError, AgentHarnessSummary, AgentInterruptStatus, AgentNodeRecord,
    AgentRuntimeHostServices, AgentTemplateError, ChildAgentSpawnError, CreateChildAgentInput,
    EnsureRootAgentInput, PollAgentWaitInput, SendAgentMessageRequest,
};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct AgentCollaborationHarnessAdapter {
    storage: Arc<StorageService>,
    service: AgentService,
    authorizer: CollaborationAuthorizer,
    dispatcher: Arc<Mutex<Option<AgentDispatcher>>>,
    notifications: CoreServerNotificationSender,
}

impl AgentCollaborationHarnessAdapter {
    pub(crate) fn new(
        storage: Arc<StorageService>,
        service: AgentService,
        authorizer: CollaborationAuthorizer,
        dispatcher: Arc<Mutex<Option<AgentDispatcher>>>,
        notifications: CoreServerNotificationSender,
    ) -> Self {
        Self {
            storage,
            service,
            authorizer,
            dispatcher,
            notifications,
        }
    }

    pub(crate) fn runtime_services_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<AgentCollaborationRuntimeServices, AgentError> {
        let claimed = self.caller_snapshot(conversation_id)?;
        // Tool exposure must work for pre-collaboration root Conversations. Materialize their
        // stable root identity while the trusted Host constructs the Turn, before the first model
        // sample can invoke spawn_agent.
        let caller = caller_from_node(&self.ensure_and_validate_caller(&claimed)?);
        let selector_directory = self.selector_directory(caller.project_id.as_deref())?;
        Ok(AgentCollaborationRuntimeServices::new(
            Arc::new(self.clone()),
            caller,
            selector_directory,
        ))
    }

    pub(crate) fn preview_runtime_services_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<AgentCollaborationRuntimeServices, AgentError> {
        // Context-window inspection is read-only. It still projects the same six schemas and safe
        // selector snapshot, but defers root materialization to actual Turn construction.
        let caller = self.caller_snapshot(conversation_id)?;
        let selector_directory = self.selector_directory(caller.project_id.as_deref())?;
        Ok(AgentCollaborationRuntimeServices::new(
            Arc::new(self.clone()),
            caller,
            selector_directory,
        ))
    }

    pub(crate) fn attach_to_host_services(
        &self,
        host_services: AgentRuntimeHostServices,
        conversation_id: &str,
    ) -> Result<AgentRuntimeHostServices, AgentError> {
        Ok(host_services
            .with_agent_collaboration(self.runtime_services_for_conversation(conversation_id)?))
    }

    pub(crate) fn attach_preview_to_host_services(
        &self,
        host_services: AgentRuntimeHostServices,
        conversation_id: &str,
    ) -> Result<AgentRuntimeHostServices, AgentError> {
        Ok(host_services.with_agent_collaboration(
            self.preview_runtime_services_for_conversation(conversation_id)?,
        ))
    }

    fn caller_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<AgentCollaborationCaller, AgentError> {
        if let Some(node) = self
            .storage
            .get_agent_node_by_conversation(conversation_id)
            .map_err(graph_error)?
        {
            return Ok(caller_from_node(&node));
        }
        let conversation = self
            .storage
            .load_conversation(conversation_id)
            .map_err(storage_error)?
            .ok_or_else(|| unavailable("Root Conversation is unavailable."))?;
        let root_agent_id = root_agent_id_for_conversation(conversation_id);
        Ok(AgentCollaborationCaller {
            agent_id: root_agent_id.clone(),
            root_agent_id,
            root_conversation_id: conversation_id.to_string(),
            parent_agent_id: None,
            conversation_id: conversation_id.to_string(),
            project_id: conversation.project_id,
            task_name: bounded_root_agent_task_name(&conversation.title),
            task_path: "/root".to_string(),
        })
    }

    fn ensure_and_validate_caller(
        &self,
        claimed: &AgentCollaborationCaller,
    ) -> Result<AgentNodeRecord, AgentError> {
        let node = match self
            .storage
            .get_agent_node_by_conversation(&claimed.conversation_id)
            .map_err(graph_error)?
        {
            Some(node) => node,
            None if claimed.parent_agent_id.is_none() => self
                .storage
                .ensure_root_agent(&EnsureRootAgentInput {
                    agent_id: root_agent_id_for_conversation(&claimed.conversation_id),
                    conversation_id: claimed.conversation_id.clone(),
                    creation_request_id: root_agent_creation_request_id(&claimed.conversation_id),
                    task_name: claimed.task_name.clone(),
                })
                .map_err(graph_error)?
                .record()
                .clone(),
            None => return Err(permission_denied()),
        };
        let actual = caller_from_node(&node);
        if &actual != claimed {
            return Err(permission_denied());
        }
        Ok(node)
    }

    fn selector_directory(
        &self,
        project_id: Option<&str>,
    ) -> Result<AgentCollaborationSelectorDirectory, AgentError> {
        let settings = self.storage.load_model_settings().map_err(storage_error)?;
        let models = settings
            .as_ref()
            .map(|settings| {
                settings
                    .models
                    .iter()
                    .filter(|model| {
                        model.enabled
                            && settings.effective_connection_for(model).is_ok()
                            && model.provider_profile_config.validate().is_ok()
                    })
                    .map(|model| AgentCollaborationModelSelector {
                        model_config_id: model.id.clone(),
                        display_name: model.display_label(),
                        capabilities: mycopilot_core::ModelCapabilities {
                            image_input: model.supports_image,
                        },
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let templates = match project_id {
            Some(project_id) => self
                .storage
                .list_project_agent_templates(project_id, false)
                .map_err(template_error)?
                .into_iter()
                .filter_map(|template| {
                    self.storage
                        .resolve_template_for_spawn(project_id, &template.machine_key)
                        .ok()
                        .map(|resolved| AgentCollaborationTemplateSelector {
                            agent_type: template.machine_key,
                            template_id: resolved.template.template_id,
                            template_revision: resolved.template.template_revision,
                            name: template.name,
                            description: template.description,
                            model_display_name: resolved.model.display_name,
                            default_model_capabilities: mycopilot_core::ModelCapabilities {
                                image_input: resolved.model.supports_image,
                            },
                        })
                })
                .collect(),
            None => Vec::new(),
        };
        Ok(AgentCollaborationSelectorDirectory::bounded(
            templates, models,
        ))
    }

    fn with_dispatcher<T>(
        &self,
        operation: impl FnOnce(&AgentDispatcher) -> Result<T, AgentError>,
    ) -> Result<T, AgentError> {
        let mut slot = self
            .dispatcher
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot.is_none() {
            let gate = self.service.turn_concurrency_gate();
            *slot = Some(
                AgentDispatcher::start(
                    Arc::new(SqliteAgentDispatcherStore::new(Arc::clone(&self.storage))),
                    Arc::new(SharedAgentTurnExecutionPort::new(
                        self.service.clone(),
                        Arc::clone(&self.storage),
                        self.notifications.clone(),
                    )),
                    gate.clone(),
                    AgentDispatcherConfig {
                        global_concurrency_limit: gate.limit(),
                        ..AgentDispatcherConfig::default()
                    },
                )
                .map_err(|error| unavailable(error.to_string()))?,
            );
        }
        operation(slot.as_ref().expect("dispatcher was initialized"))
    }

    fn notify_work_available(&self) -> Result<(), AgentError> {
        self.with_dispatcher(|dispatcher| {
            dispatcher.notify_work_available();
            Ok(())
        })
    }

    pub(crate) fn start_dispatcher(&self) -> Result<(), AgentError> {
        self.notify_work_available()
    }

    async fn execute_inner(
        &self,
        invocation: AgentCollaborationInvocation,
        control: AgentCollaborationExecutionControl,
    ) -> Result<AgentCollaborationExecutionOutput, AgentError> {
        if invocation.conversation_id != invocation.caller.conversation_id {
            return Err(permission_denied());
        }
        let caller = self.ensure_and_validate_caller(&invocation.caller)?;
        // Every collaboration action first refreshes the caller's latest effective permissions
        // from the exact Host-authenticated ToolExecutionContext. This makes a tightened root Turn
        // visible before follow-up/interrupt/send can schedule descendant work, while the current
        // caller Run remains frozen in its own context/checkpoint.
        self.storage
            .record_agent_effective_permissions_for_active_turn(
                &caller.agent_id,
                &invocation.conversation_id,
                &invocation.run_id,
                &invocation.assistant_message_id,
                invocation.effective_permissions,
            )
            .map_err(graph_error)?;
        let request_id = stable_request_id(&invocation.run_id, &invocation.tool_call_id);
        let output = match invocation.action {
            AgentCollaborationAction::Spawn(request) => {
                validate_spawn_selector_authorization(
                    &invocation.selector_authorization,
                    &request,
                )?;
                let expected_model_capabilities = invocation
                    .selector_authorization
                    .expected_model_capabilities(
                        request.agent_type.as_deref(),
                        request.model_config_id.as_deref(),
                    )
                    .or_else(|| {
                        (request.agent_type.is_none() && request.model_config_id.is_none())
                            .then_some(invocation.caller_model_capabilities)
                    });
                let expected_template_identity =
                    request.agent_type.as_deref().and_then(|agent_type| {
                        invocation
                            .selector_authorization
                            .expected_template_identity(agent_type)
                    });
                self.authorizer
                    .authorize_spawn(&caller.agent_id)
                    .and_then(|_| self.authorizer.authorize_message_size(&request.message))
                    .map_err(authorizer_error)?;
                let child = ChildAgentFactory::new(Arc::clone(&self.storage))
                    .create_child_with_expected_selector(
                        &CreateChildAgentInput {
                            parent_agent_id: caller.agent_id.clone(),
                            creation_request_id: request_id,
                            task_name: request.task_name,
                            task: request.message,
                            template_machine_key: request.agent_type,
                            explicit_model_id: request.model_config_id,
                            reasoning_effort: request.reasoning_effort,
                            fork_turns: request.fork_turns,
                        },
                        expected_model_capabilities,
                        expected_template_identity,
                    )
                    .map_err(spawn_error)?;
                self.notify_work_available()?;
                let model = child.agent.model_snapshot.ok_or_else(|| {
                    collaboration_error(
                        "conflict",
                        false,
                        "Spawned child is missing its frozen model snapshot.".to_string(),
                    )
                })?;
                AgentCollaborationToolResult::Spawned {
                    child_agent_id: child.agent.agent_id,
                    task_path: child.agent.task_path,
                    model_display_name: model.display_name,
                    model_capabilities: mycopilot_core::ModelCapabilities {
                        image_input: model.supports_image,
                    },
                    status: AgentDisplayStatus::Queued,
                }
            }
            AgentCollaborationAction::SendMessage(request) => {
                self.authorizer
                    .authorize_send(&caller.agent_id, &request.target_agent_id, &request.message)
                    .map_err(authorizer_error)?;
                let dispatch = AgentMessagingService::new(Arc::clone(&self.storage))
                    .send_message(&SendAgentMessageRequest {
                        sender_agent_id: caller.agent_id,
                        recipient_agent_id: request.target_agent_id,
                        request_id,
                        content: request.message,
                    })
                    .map_err(graph_error)?;
                AgentCollaborationToolResult::MessageQueued {
                    message_id: dispatch.message.message_id,
                    delivery_state: AgentCollaborationDeliveryState::Queued,
                }
            }
            AgentCollaborationAction::FollowupTask(request) => {
                self.authorizer
                    .authorize_send(&caller.agent_id, &request.target_agent_id, &request.message)
                    .and_then(|_| {
                        self.authorizer.authorize_management(
                            &caller.agent_id,
                            &request.target_agent_id,
                            AgentManagementOperation::FollowUp,
                        )
                    })
                    .map_err(authorizer_error)?;
                let dispatch = AgentMessagingService::new(Arc::clone(&self.storage))
                    .follow_up(&SendAgentMessageRequest {
                        sender_agent_id: caller.agent_id,
                        recipient_agent_id: request.target_agent_id,
                        request_id,
                        content: request.message,
                    })
                    .map_err(graph_error)?;
                self.notify_work_available()?;
                AgentCollaborationToolResult::MessageQueued {
                    message_id: dispatch.message.message_id,
                    delivery_state: AgentCollaborationDeliveryState::Queued,
                }
            }
            AgentCollaborationAction::Wait(request) => {
                for target in &request.target_agent_ids {
                    self.authorizer
                        .authorize_management(
                            &caller.agent_id,
                            target,
                            AgentManagementOperation::Wait,
                        )
                        .map_err(authorizer_error)?;
                }
                let outcome = AgentWaitKernel::production(Arc::clone(&self.storage))
                    .wait(
                        PollAgentWaitInput {
                            caller_agent_id: caller.agent_id,
                            conversation_id: invocation.conversation_id,
                            run_id: invocation.run_id,
                            assistant_message_id: invocation.assistant_message_id,
                            model_batch_index: invocation.model_batch_index,
                            target_agent_ids: request.target_agent_ids,
                            maximum_messages: 64,
                        },
                        Duration::from_millis(request.timeout_ms),
                        control.cancellation(),
                        mycopilot_core::AgentCancellationToken::new(),
                        control.steer(),
                    )
                    .await
                    .map_err(graph_error)?;
                match outcome {
                    AgentWaitOutcome::Ready(snapshot) => {
                        let snapshot = *snapshot;
                        return Ok(AgentCollaborationExecutionOutput {
                            result: AgentCollaborationToolResult::WaitReady {
                                receipt_id: snapshot.receipt.receipt_id,
                                source_receipt_id: snapshot.source_receipt_id,
                                targets: snapshot.targets,
                            },
                            persistence:
                                AgentCollaborationResultPersistence::PrecommittedWaitToolResult,
                        });
                    }
                    AgentWaitOutcome::Stopped(reason) => {
                        AgentCollaborationToolResult::WaitStopped {
                            reason: wait_stop_reason(reason).to_string(),
                        }
                    }
                }
            }
            AgentCollaborationAction::List => {
                let tree = self
                    .authorizer
                    .visible_tree(&caller.agent_id)
                    .map_err(authorizer_error)?;
                let mut agents = Vec::with_capacity(tree.len());
                for node in tree {
                    let display = self
                        .storage
                        .get_agent_display_status(&node.agent_id)
                        .map_err(graph_error)?;
                    let latest_activity_at = self
                        .storage
                        .latest_agent_collaboration_activity_at(&node.root_agent_id, &node.agent_id)
                        .map_err(storage_error)?
                        .unwrap_or(node.updated_at);
                    agents.push(AgentHarnessSummary {
                        agent_id: node.agent_id,
                        task_name: node.task_name,
                        task_path: node.task_path,
                        parent_agent_id: node.parent_agent_id,
                        display_status: display.status,
                        model_display_name: node.model_snapshot.map(|model| model.display_name),
                        latest_activity_at,
                    });
                }
                AgentCollaborationToolResult::Agents { agents }
            }
            AgentCollaborationAction::Interrupt { target_agent_id } => {
                self.authorizer
                    .authorize_management(
                        &caller.agent_id,
                        &target_agent_id,
                        AgentManagementOperation::Interrupt,
                    )
                    .map_err(authorizer_error)?;
                let disposition = self.with_dispatcher(|dispatcher| {
                    dispatcher
                        .interrupt_agent(&caller.agent_id, &target_agent_id, &request_id)
                        .map_err(|error| unavailable(error.to_string()))
                })?;
                let status = match disposition {
                    AgentInterruptDisposition::NoPendingExecution => {
                        AgentInterruptStatus::NoActiveTurn
                    }
                    AgentInterruptDisposition::QueuedWakeCancelled { .. }
                    | AgentInterruptDisposition::ActiveTurn { .. } => {
                        AgentInterruptStatus::InterruptRequested
                    }
                };
                AgentCollaborationToolResult::Interrupted {
                    target_agent_id,
                    status,
                }
            }
        };
        Ok(AgentCollaborationExecutionOutput {
            result: output,
            persistence: AgentCollaborationResultPersistence::RuntimeCommits,
        })
    }
}

impl AgentCollaborationExecutor for AgentCollaborationHarnessAdapter {
    fn execute(
        &self,
        invocation: AgentCollaborationInvocation,
        control: AgentCollaborationExecutionControl,
    ) -> AgentCollaborationExecutionFuture {
        let adapter = self.clone();
        Box::pin(async move { adapter.execute_inner(invocation, control).await })
    }
}

fn caller_from_node(node: &AgentNodeRecord) -> AgentCollaborationCaller {
    AgentCollaborationCaller {
        agent_id: node.agent_id.clone(),
        root_agent_id: node.root_agent_id.clone(),
        root_conversation_id: node.root_conversation_id.clone(),
        parent_agent_id: node.parent_agent_id.clone(),
        conversation_id: node.conversation_id.clone(),
        project_id: node.project_id.clone(),
        task_name: node.task_name.clone(),
        task_path: node.task_path.clone(),
    }
}

fn stable_digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn stable_request_id(run_id: &str, tool_call_id: &str) -> String {
    let digest = stable_digest(&format!("{run_id}\0{tool_call_id}"));
    format!("harness-{}", &digest[..48])
}

fn wait_stop_reason(reason: AgentWaitStopReason) -> &'static str {
    match reason {
        AgentWaitStopReason::TimedOut => "timeout",
        AgentWaitStopReason::Cancelled => "cancelled",
        AgentWaitStopReason::Shutdown => "shutdown",
        AgentWaitStopReason::InterruptedBySteer => "steered",
    }
}

fn validate_spawn_selector_authorization(
    authorization: &mycopilot_core::AgentCollaborationSelectorAuthorization,
    request: &mycopilot_core::AgentSpawnRequest,
) -> Result<(), AgentError> {
    if request
        .agent_type
        .as_deref()
        .is_some_and(|agent_type| !authorization.allows_agent_type(agent_type))
    {
        return Err(collaboration_error(
            "unavailable",
            false,
            "Selected agent_type is unavailable in this Turn's authorized selector snapshot."
                .to_string(),
        ));
    }
    if request
        .model_config_id
        .as_deref()
        .is_some_and(|model_config_id| !authorization.allows_model_config_id(model_config_id))
    {
        return Err(collaboration_error(
            "unavailable",
            false,
            "Selected model is unavailable in this Turn's authorized selector snapshot."
                .to_string(),
        ));
    }
    Ok(())
}

fn authorizer_error(error: CollaborationAuthorizationError) -> AgentError {
    AgentError::structured(
        format!("agent.collaboration.{}", error.code()),
        error.to_string(),
        serde_json::json!({
            "type": "agent_collaboration",
            "category": error.code(),
            "retryable": error.retryable(),
        }),
    )
}

fn graph_error(error: AgentGraphError) -> AgentError {
    let (category, retryable) = match &error {
        AgentGraphError::InvalidInput { .. } => ("invalid_arguments", false),
        AgentGraphError::ResourceLimit { .. } => ("resource_limit", false),
        AgentGraphError::StorageUnavailable(_) => ("unavailable", true),
        AgentGraphError::ConversationNotFound(_)
        | AgentGraphError::AgentNotFound(_)
        | AgentGraphError::MessageNotFound(_)
        | AgentGraphError::WakeNotFound(_)
        | AgentGraphError::RevisionConflict { .. }
        | AgentGraphError::Conflict(_)
        | AgentGraphError::IllegalLifecycleTransition { .. }
        | AgentGraphError::IllegalTransition { .. }
        | AgentGraphError::BoundConversation(_)
        | AgentGraphError::BoundProject(_)
        | AgentGraphError::CorruptRecord(_) => ("conflict", false),
    };
    collaboration_error(category, retryable, error.to_string())
}

fn spawn_error(error: ChildAgentSpawnError) -> AgentError {
    let (category, retryable) = match &error {
        ChildAgentSpawnError::InvalidInput { .. } => ("invalid_arguments", false),
        ChildAgentSpawnError::ProjectRequiredForTemplate
        | ChildAgentSpawnError::TemplateNotFound(_)
        | ChildAgentSpawnError::TemplateDisabled(_)
        | ChildAgentSpawnError::ModelUnavailable { .. } => ("unavailable", false),
        ChildAgentSpawnError::UnsupportedReasoningEffort(_) => ("unsupported", false),
        ChildAgentSpawnError::ResourceLimit { .. } => ("resource_limit", false),
        ChildAgentSpawnError::StorageUnavailable(_) => ("unavailable", true),
        ChildAgentSpawnError::ParentNotFound(_)
        | ChildAgentSpawnError::ParentUnavailable(_)
        | ChildAgentSpawnError::IdempotencyConflict(_)
        | ChildAgentSpawnError::Conflict(_)
        | ChildAgentSpawnError::SnapshotUnavailable(_)
        | ChildAgentSpawnError::CorruptRecord(_) => ("conflict", false),
    };
    collaboration_error(category, retryable, error.to_string())
}

fn collaboration_error(category: &'static str, retryable: bool, message: String) -> AgentError {
    AgentError::structured(
        format!("agent.collaboration.{category}"),
        message,
        serde_json::json!({
            "type": "agent_collaboration",
            "category": category,
            "retryable": retryable,
        }),
    )
}

fn storage_error(error: impl std::fmt::Display) -> AgentError {
    unavailable(error.to_string())
}

fn template_error(error: AgentTemplateError) -> AgentError {
    match error {
        AgentTemplateError::StorageUnavailable(message) => unavailable(message),
        other => collaboration_error("unavailable", false, other.to_string()),
    }
}

fn unavailable(message: impl Into<String>) -> AgentError {
    AgentError::structured(
        "agent.collaboration.unavailable",
        message.into(),
        serde_json::json!({
            "type": "agent_collaboration",
            "category": "unavailable",
            "retryable": true,
        }),
    )
}

fn permission_denied() -> AgentError {
    AgentError::structured(
        "agent.collaboration.permission_denied",
        "Agent collaboration operation is not authorized.",
        serde_json::json!({
            "type": "agent_collaboration",
            "category": "permission_denied",
            "retryable": false,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::{
        ChatConversationRecord, ChatMessageRecord, ModelConfigRecord, ModelSettingsRecord,
        ProjectRecord,
    };
    use mycopilot_core::{
        AgentCancellationToken, AgentForkTurns, AgentModelUnavailableReason, AgentSpawnRequest,
        CreateAgentTemplateInput, ProviderProfileConfig, ProviderProtocolDialect, ReasoningEffort,
    };

    fn assert_category(error: AgentError, category: &str, retryable: bool) {
        assert_eq!(
            error.code(),
            Some(format!("agent.collaboration.{category}").as_str())
        );
        let details = error.details().unwrap();
        assert_eq!(details["category"], category);
        assert_eq!(details["retryable"], retryable);
    }

    #[test]
    fn typed_spawn_errors_have_stable_machine_categories() {
        assert_category(
            spawn_error(ChildAgentSpawnError::ModelUnavailable {
                model_config_id: Some("model-stale".to_string()),
                reason: AgentModelUnavailableReason::Disabled,
            }),
            "unavailable",
            false,
        );
        assert_category(
            spawn_error(ChildAgentSpawnError::UnsupportedReasoningEffort(
                ReasoningEffort::Max,
            )),
            "unsupported",
            false,
        );
        assert_category(
            spawn_error(ChildAgentSpawnError::ResourceLimit {
                resource: "tree_nodes",
                limit: 64,
            }),
            "resource_limit",
            false,
        );
        assert_category(
            spawn_error(ChildAgentSpawnError::StorageUnavailable(
                "database busy".to_string(),
            )),
            "unavailable",
            true,
        );
    }

    #[test]
    fn typed_graph_errors_distinguish_bad_input_capacity_and_storage() {
        assert_category(
            graph_error(AgentGraphError::InvalidInput {
                field: "target",
                reason: "empty".to_string(),
            }),
            "invalid_arguments",
            false,
        );
        assert_category(
            graph_error(AgentGraphError::ResourceLimit {
                resource: "mailbox_messages",
                limit: 1_024,
            }),
            "resource_limit",
            false,
        );
        assert_category(
            graph_error(AgentGraphError::StorageUnavailable(
                "database busy".to_string(),
            )),
            "unavailable",
            true,
        );
    }

    #[test]
    fn bounded_model_visible_directory_is_the_exact_spawn_allow_list() {
        let directory = AgentCollaborationSelectorDirectory::bounded(
            (0..34)
                .map(|index| AgentCollaborationTemplateSelector {
                    agent_type: format!("type-{index:02}"),
                    template_id: format!("template-{index:02}"),
                    template_revision: 1,
                    name: format!("Type {index:02}"),
                    description: "Fixture".to_string(),
                    model_display_name: "Model".to_string(),
                    default_model_capabilities: mycopilot_core::ModelCapabilities {
                        image_input: index % 2 == 0,
                    },
                })
                .collect(),
            (0..34)
                .map(|index| AgentCollaborationModelSelector {
                    model_config_id: format!("model-{index:02}"),
                    display_name: format!("Model {index:02}"),
                    capabilities: mycopilot_core::ModelCapabilities {
                        image_input: index % 2 == 0,
                    },
                })
                .collect(),
        );
        assert!(directory.truncated);
        assert_eq!(directory.templates.len(), 32);
        assert_eq!(directory.models.len(), 32);
        let authorization = directory_authorization(&directory);
        let request = |agent_type: Option<&str>, model: Option<&str>| AgentSpawnRequest {
            task_name: "selector_test".to_string(),
            message: "Check selector authorization.".to_string(),
            agent_type: agent_type.map(str::to_string),
            model_config_id: model.map(str::to_string),
            reasoning_effort: None,
            fork_turns: AgentForkTurns::None,
        };
        validate_spawn_selector_authorization(
            &authorization,
            &request(Some("type-31"), Some("model-31")),
        )
        .unwrap();
        assert_category(
            validate_spawn_selector_authorization(&authorization, &request(Some("type-32"), None))
                .unwrap_err(),
            "unavailable",
            false,
        );
        assert_category(
            validate_spawn_selector_authorization(&authorization, &request(None, Some("model-32")))
                .unwrap_err(),
            "unavailable",
            false,
        );
    }

    fn directory_authorization(
        directory: &AgentCollaborationSelectorDirectory,
    ) -> mycopilot_core::AgentCollaborationSelectorAuthorization {
        AgentCollaborationRuntimeServices::new(
            Arc::new(UnreachableExecutor),
            AgentCollaborationCaller {
                agent_id: "agent-root".to_string(),
                root_agent_id: "agent-root".to_string(),
                root_conversation_id: "conversation-root".to_string(),
                parent_agent_id: None,
                conversation_id: "conversation-root".to_string(),
                project_id: Some("project-root".to_string()),
                task_name: "Root".to_string(),
                task_path: "/root".to_string(),
            },
            directory.clone(),
        )
        .selector_authorization()
    }

    struct UnreachableExecutor;

    impl AgentCollaborationExecutor for UnreachableExecutor {
        fn execute(
            &self,
            _invocation: AgentCollaborationInvocation,
            _control: AgentCollaborationExecutionControl,
        ) -> AgentCollaborationExecutionFuture {
            Box::pin(async { panic!("selector authorization fixture never executes Host work") })
        }
    }

    #[tokio::test]
    async fn selector_catalog_refreshes_each_turn_and_stale_exact_keys_fail_closed() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open(&fixture.path().join("agent-harness-catalog.sqlite")).unwrap(),
        );
        storage
            .save_project(ProjectRecord {
                id: "project-catalog".to_string(),
                name: "Catalog".to_string(),
                path: Some(fixture.path().to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        storage
            .save_model_settings(ModelSettingsRecord {
                api_url: "https://provider.example/v1/chat/completions".to_string(),
                api_token: "private-catalog-token".to_string(),
                search_mode: "disabled".to_string(),
                tavily_api_key: String::new(),
                models: vec![ModelConfigRecord {
                    id: "model-catalog".to_string(),
                    provider_model_id: "model-catalog".to_string(),
                    display_name: "Catalog Model".to_string(),
                    api_url_override: None,
                    api_token_override: None,
                    supports_image: false,
                    context_window_tokens: Some(64_000),
                    provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                        ProviderProtocolDialect::OpenAiChatCompletions,
                    ),
                    input_price: "0.01".to_string(),
                    cached_input_price: String::new(),
                    output_price: "0.02".to_string(),
                    enabled: true,
                }],
            })
            .unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-catalog".to_string(),
                project_id: Some("project-catalog".to_string()),
                model_id: Some("model-catalog".to_string()),
                title: "Catalog root".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
            Arc::clone(&storage),
            None,
            2,
        )
        .unwrap();
        let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let adapter = AgentCollaborationHarnessAdapter::new(
            Arc::clone(&storage),
            service.clone(),
            service.collaboration_authorizer(),
            Arc::new(Mutex::new(None)),
            notifications,
        );

        let before = adapter
            .preview_runtime_services_for_conversation("conversation-catalog")
            .unwrap();
        assert!(before.selector_directory.templates.is_empty());
        assert_eq!(before.selector_directory.models.len(), 1);
        assert!(!before.selector_directory.models[0].capabilities.image_input);

        let template = storage
            .create_agent_template(&CreateAgentTemplateInput {
                template_id: "template-catalog-reviewer".to_string(),
                machine_key: "reviewer".to_string(),
                name: "Reviewer".to_string(),
                description: "Review one delegated boundary".to_string(),
                instructions: "PRIVATE_CATALOG_INSTRUCTIONS".to_string(),
                model_config_id: "model-catalog".to_string(),
                enabled: true,
            })
            .unwrap();
        storage
            .set_agent_template_project_assignment("project-catalog", &template.template_id, true)
            .unwrap();
        let enabled = adapter
            .runtime_services_for_conversation("conversation-catalog")
            .unwrap();
        assert_eq!(enabled.selector_directory.templates.len(), 1);
        assert_eq!(
            enabled.selector_directory.templates[0].agent_type,
            "reviewer"
        );
        assert!(
            !enabled.selector_directory.templates[0]
                .default_model_capabilities
                .image_input
        );
        let encoded = serde_json::to_string(&enabled.selector_directory).unwrap();
        assert!(!encoded.contains("PRIVATE_CATALOG_INSTRUCTIONS"));
        assert!(!encoded.contains("private-catalog-token"));
        let frozen_caller = enabled.caller.clone();
        let frozen_authorization = enabled.selector_authorization();

        let disabled = storage
            .set_agent_template_enabled(&template.template_id, template.revision, false)
            .unwrap();
        let refreshed = adapter
            .preview_runtime_services_for_conversation("conversation-catalog")
            .unwrap();
        assert!(refreshed.selector_directory.templates.is_empty());

        let (conversation, revision) = storage
            .load_conversation_for_turn("conversation-catalog")
            .unwrap();
        let mut conversation = conversation.unwrap();
        conversation.messages.push(ChatMessageRecord {
            id: "assistant-stale-after-sampling".to_string(),
            role: "assistant".to_string(),
            content: "Thinking...".to_string(),
            created_at: 2,
            status: Some("pending".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        });
        conversation.updated_at = 2;
        let active_trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
            "run-stale-after-sampling",
            "conversation-catalog",
            "assistant-stale-after-sampling",
        );
        storage
            .save_conversation_and_begin_turn(
                conversation,
                revision,
                None,
                mycopilot_core::AgentTurnPermissionSource::HostAuthenticatedRoot(
                    mycopilot_core::AgentPermissions::default(),
                ),
                &active_trace,
                2,
                2,
            )
            .unwrap();

        // The model-visible snapshot remains the admission authority for this logical Turn, but
        // it does not resurrect a template disabled after sampling: Factory revalidation fails
        // closed instead of silently inheriting another model.
        let stale_after_sampling = adapter
            .execute(
                AgentCollaborationInvocation {
                    caller: frozen_caller.clone(),
                    selector_authorization: frozen_authorization.clone(),
                    caller_model_capabilities: mycopilot_core::ModelCapabilities {
                        image_input: false,
                    },
                    effective_permissions: mycopilot_core::AgentPermissions::default(),
                    conversation_id: "conversation-catalog".to_string(),
                    run_id: "run-stale-after-sampling".to_string(),
                    assistant_message_id: "assistant-stale-after-sampling".to_string(),
                    model_batch_index: 1,
                    tool_call_id: "call-stale-after-sampling".to_string(),
                    action: AgentCollaborationAction::Spawn(AgentSpawnRequest {
                        task_name: "stale_after_sampling".to_string(),
                        message: "The template was visible but is now disabled.".to_string(),
                        agent_type: Some("reviewer".to_string()),
                        model_config_id: None,
                        reasoning_effort: None,
                        fork_turns: AgentForkTurns::None,
                    }),
                },
                AgentCollaborationExecutionControl::new(AgentCancellationToken::new(), None),
            )
            .await
            .unwrap_err();
        assert_eq!(
            stale_after_sampling.code(),
            Some("agent.collaboration.unavailable")
        );
        assert_eq!(stale_after_sampling.details().unwrap()["retryable"], false);

        // A selector-less spawn inherits the caller Conversation's model. Its capability is still
        // frozen in the caller's Run/World State, so changing image support before Host execution
        // must fail inside the atomic spawn transaction rather than create a different child.
        let mut changed_settings = storage.load_model_settings().unwrap().unwrap();
        changed_settings.models[0].supports_image = true;
        storage.save_model_settings(changed_settings).unwrap();
        let changed_capabilities = adapter
            .execute(
                AgentCollaborationInvocation {
                    caller: frozen_caller,
                    selector_authorization: frozen_authorization,
                    caller_model_capabilities: mycopilot_core::ModelCapabilities {
                        image_input: false,
                    },
                    effective_permissions: mycopilot_core::AgentPermissions::default(),
                    conversation_id: "conversation-catalog".to_string(),
                    run_id: "run-stale-after-sampling".to_string(),
                    assistant_message_id: "assistant-stale-after-sampling".to_string(),
                    model_batch_index: 2,
                    tool_call_id: "call-capability-changed-after-sampling".to_string(),
                    action: AgentCollaborationAction::Spawn(AgentSpawnRequest {
                        task_name: "capability_changed".to_string(),
                        message: "Use the exact sampled model capability.".to_string(),
                        agent_type: None,
                        model_config_id: None,
                        reasoning_effort: None,
                        fork_turns: AgentForkTurns::None,
                    }),
                },
                AgentCollaborationExecutionControl::new(AgentCancellationToken::new(), None),
            )
            .await
            .unwrap_err();
        assert_eq!(
            changed_capabilities.code(),
            Some("agent.collaboration.unavailable")
        );
        assert!(changed_capabilities
            .to_string()
            .contains("CapabilitiesChanged"));
        let root = storage
            .get_agent_node_by_conversation("conversation-catalog")
            .unwrap()
            .expect("the attempted root collaboration call materializes only the root");
        assert_eq!(storage.list_agent_tree(&root.agent_id).unwrap().len(), 1);

        let runtime = adapter
            .runtime_services_for_conversation("conversation-catalog")
            .unwrap();
        assert!(
            runtime.selector_directory.models[0]
                .capabilities
                .image_input
        );
        let selector_authorization = runtime.selector_authorization();
        let error = adapter
            .execute(
                AgentCollaborationInvocation {
                    caller: runtime.caller,
                    selector_authorization,
                    caller_model_capabilities: mycopilot_core::ModelCapabilities {
                        image_input: true,
                    },
                    effective_permissions: mycopilot_core::AgentPermissions::default(),
                    conversation_id: "conversation-catalog".to_string(),
                    run_id: "run-stale-after-sampling".to_string(),
                    assistant_message_id: "assistant-stale-after-sampling".to_string(),
                    model_batch_index: 1,
                    tool_call_id: "call-stale-template".to_string(),
                    action: AgentCollaborationAction::Spawn(AgentSpawnRequest {
                        task_name: "stale_template".to_string(),
                        message: "This exact selector is no longer available.".to_string(),
                        agent_type: Some("reviewer".to_string()),
                        model_config_id: None,
                        reasoning_effort: None,
                        fork_turns: AgentForkTurns::None,
                    }),
                },
                AgentCollaborationExecutionControl::new(AgentCancellationToken::new(), None),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("agent.collaboration.unavailable"));
        assert_eq!(error.details().unwrap()["retryable"], false);

        storage
            .set_agent_template_enabled(&template.template_id, disabled.revision, true)
            .unwrap();
        let mut settings = storage.load_model_settings().unwrap().unwrap();
        settings.models[0].enabled = false;
        storage.save_model_settings(settings).unwrap();
        let model_disabled = adapter
            .preview_runtime_services_for_conversation("conversation-catalog")
            .unwrap();
        assert!(model_disabled.selector_directory.models.is_empty());
        assert!(model_disabled.selector_directory.templates.is_empty());
    }
}
