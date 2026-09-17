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
    root_agent_creation_request_id, root_agent_id_for_conversation, AgentCollaborationAction,
    AgentCollaborationCaller, AgentCollaborationDeliveryState, AgentCollaborationExecutionControl,
    AgentCollaborationExecutionFuture, AgentCollaborationExecutionOutput,
    AgentCollaborationExecutor, AgentCollaborationInvocation, AgentCollaborationModelSelector,
    AgentCollaborationResultPersistence, AgentCollaborationRuntimeServices,
    AgentCollaborationSelectorDirectory, AgentCollaborationTemplateSelector,
    AgentCollaborationToolResult, AgentDisplayStatus, AgentError, AgentGraphError,
    AgentHarnessSummary, AgentInterruptStatus, AgentNodeRecord, AgentRuntimeHostServices,
    AgentTemplateError, ChildAgentSpawnError, CreateChildAgentInput, EnsureRootAgentInput,
    PollAgentWaitInput, SendAgentMessageRequest, ROOT_AGENT_TASK_NAME,
    ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE,
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
        // stable root identity while the trusted Host constructs the Turn. Human interaction
        // ownership and durable inbox delivery also use this identity, independently of tools.
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
        run_id: &str,
    ) -> Result<AgentRuntimeHostServices, AgentError> {
        let policy = self.run_policy(run_id)?;
        // A disabled run has neither collaboration tools nor checkpoint authority. Keep its
        // frozen policy for world state, without materializing or restoring a selector directory.
        let host_services = if policy.enabled {
            host_services.with_agent_collaboration(self.freeze_run_directory(
                self.runtime_services_for_conversation(conversation_id)?,
                run_id,
                None,
            )?)
        } else {
            // Root identity also owns human interaction and inbox records. Disabling model-facing
            // collaboration must not remove that independent Host authority or its validation.
            self.ensure_and_validate_caller(&self.caller_snapshot(conversation_id)?)?;
            host_services
        };
        Ok(host_services.with_agent_collaboration_policy(Arc::new(
            mycopilot_core::FrozenAgentCollaborationPolicySource::new(policy),
        )))
    }

    pub(crate) fn attach_preview_to_host_services(
        &self,
        host_services: AgentRuntimeHostServices,
        conversation_id: &str,
        run_id: Option<&str>,
        resume_checkpoint: Option<&mycopilot_core::AgentRunCheckpoint>,
    ) -> Result<AgentRuntimeHostServices, AgentError> {
        let policy = match run_id {
            Some(run_id) => self.run_policy(run_id)?,
            None => match self
                .storage
                .load_active_agent_collaboration_run_policy(conversation_id)
                .map_err(storage_error)?
            {
                Some(policy) => policy,
                None => self
                    .storage
                    .load_agent_collaboration_settings()
                    .map_err(storage_error)?,
            },
        };
        let host_services = if policy.enabled {
            let services = self.preview_runtime_services_for_conversation(conversation_id)?;
            let services = match run_id {
                Some(run_id) => self.freeze_run_directory(services, run_id, resume_checkpoint)?,
                None => services,
            };
            host_services.with_agent_collaboration(services)
        } else {
            host_services
        };
        Ok(host_services.with_agent_collaboration_policy(Arc::new(
            mycopilot_core::FrozenAgentCollaborationPolicySource::new(policy),
        )))
    }

    fn run_policy(
        &self,
        run_id: &str,
    ) -> Result<mycopilot_core::AgentCollaborationSettings, AgentError> {
        self.storage
            .load_agent_collaboration_run_policy(run_id)
            .map_err(storage_error)?
            .ok_or_else(|| unavailable("The run's frozen Agent collaboration policy is missing."))
    }

    fn freeze_run_directory(
        &self,
        services: AgentCollaborationRuntimeServices,
        run_id: &str,
        resume_checkpoint: Option<&mycopilot_core::AgentRunCheckpoint>,
    ) -> Result<AgentCollaborationRuntimeServices, AgentError> {
        let mut snapshot = services.run_snapshot();
        snapshot.selector_directory = self.service.collaboration_directory_for_run(
            run_id,
            &snapshot.selector_directory,
            resume_checkpoint,
        )?;
        services.with_run_snapshot(&snapshot)
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
            task_name: ROOT_AGENT_TASK_NAME.to_string(),
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
        // The model list is the exact spawn allow-list. Only models that can execute in this
        // Host right now are advertised: enabled, complete connection, a credential that
        // resolves in the active backend, and a spawn-viable provider identity. Never go back
        // to the presence-only catalog here; a model whose key cannot be read must not be
        // advertised to the model.
        let settings = self
            .storage
            .load_executable_model_catalog()
            .map_err(storage_error)?;
        let models = settings
            .as_ref()
            .map(|settings| {
                settings
                    .models
                    .iter()
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
        let executable_model_ids = models
            .iter()
            .map(|model| model.model_config_id.clone())
            .collect::<std::collections::HashSet<_>>();
        let templates = match project_id {
            Some(project_id) => self
                .storage
                .list_project_agent_templates(project_id, false)
                .map_err(template_error)?
                .into_iter()
                .filter_map(|template| {
                    let resolved = self
                        .storage
                        .resolve_template_for_spawn(project_id, &template.machine_key)
                        .ok()?;
                    // A template whose model cannot execute must not be advertised; spawning
                    // it would fail closed once execution resolves the credential.
                    if !executable_model_ids.contains(resolved.template.model_config_id.as_str()) {
                        return None;
                    }
                    Some(AgentCollaborationTemplateSelector {
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
                .map_err(|_| unavailable("The Agent dispatcher is unavailable."))?,
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
        let cancellation = control.cancellation();
        // Reject work which reached the Host after its owning Turn was already stopped. This is
        // deliberately before caller materialization and permission refresh so a cancelled Tool
        // invocation cannot create new durable collaboration state.
        cancellation.check()?;
        if invocation.conversation_id != invocation.caller.conversation_id {
            return Err(permission_denied());
        }
        if !self.run_policy(&invocation.run_id)?.enabled {
            return Err(collaboration_error(
                "disabled",
                false,
                "本轮任务未启用子智能体协作能力。".to_string(),
            ));
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
        cancellation.check()?;
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
                self.check_scheduling_precommit(&invocation.run_id, &cancellation)?;
                let child = match ChildAgentFactory::new(Arc::clone(&self.storage))
                    .create_child_with_expected_selector_from_run(
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
                        &invocation.run_id,
                    ) {
                    Ok(child) => child,
                    Err(error) => {
                        if cancellation.is_cancelled()
                            || self
                                .service
                                .agent_tree_run_is_stopped(&invocation.run_id)
                                .map_err(|_| {
                                    unavailable(
                                        "Agent collaboration execution state could not be read.",
                                    )
                                })?
                        {
                            return Err(AgentError::cancelled());
                        }
                        return Err(spawn_error(error));
                    }
                };
                self.finish_scheduling_commit(&invocation.run_id, &cancellation)?;
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
                    task_name: child.agent.task_name,
                    task_path: child.agent.task_path,
                    model_display_name: model.display_name,
                    model_capabilities: mycopilot_core::ModelCapabilities {
                        image_input: model.supports_image,
                    },
                    status: AgentDisplayStatus::Queued,
                }
            }
            AgentCollaborationAction::SendMessage(request) => {
                let target =
                    self.resolve_target_task_name(&caller.agent_id, &request.target_task_name)?;
                self.authorizer
                    .authorize_send(&caller.agent_id, &target.agent_id, &request.message)
                    .map_err(authorizer_error)?;
                self.check_scheduling_precommit(&invocation.run_id, &cancellation)?;
                let dispatch = match AgentMessagingService::new(Arc::clone(&self.storage))
                    .send_message_from_run(
                        &SendAgentMessageRequest {
                            sender_agent_id: caller.agent_id,
                            recipient_agent_id: target.agent_id,
                            request_id,
                            content: request.message,
                        },
                        &invocation.run_id,
                    ) {
                    Ok(dispatch) => dispatch,
                    Err(error) => {
                        if cancellation.is_cancelled()
                            || self
                                .service
                                .agent_tree_run_is_stopped(&invocation.run_id)
                                .map_err(|_| {
                                    unavailable(
                                        "Agent collaboration execution state could not be read.",
                                    )
                                })?
                        {
                            return Err(AgentError::cancelled());
                        }
                        return Err(graph_error(error));
                    }
                };
                self.finish_scheduling_commit(&invocation.run_id, &cancellation)?;
                AgentCollaborationToolResult::MessageQueued {
                    message_id: dispatch.message.message_id,
                    task_name: target.task_name,
                    delivery_state: AgentCollaborationDeliveryState::Queued,
                }
            }
            AgentCollaborationAction::FollowupTask(request) => {
                let target =
                    self.resolve_target_task_name(&caller.agent_id, &request.target_task_name)?;
                self.authorizer
                    .authorize_send(&caller.agent_id, &target.agent_id, &request.message)
                    .and_then(|_| {
                        self.authorizer.authorize_management(
                            &caller.agent_id,
                            &target.agent_id,
                            AgentManagementOperation::FollowUp,
                        )
                    })
                    .map_err(authorizer_error)?;
                self.check_scheduling_precommit(&invocation.run_id, &cancellation)?;
                let dispatch = match AgentMessagingService::new(Arc::clone(&self.storage))
                    .follow_up_from_run(
                        &SendAgentMessageRequest {
                            sender_agent_id: caller.agent_id,
                            recipient_agent_id: target.agent_id,
                            request_id,
                            content: request.message,
                        },
                        &invocation.run_id,
                    ) {
                    Ok(dispatch) => dispatch,
                    Err(error) => {
                        if cancellation.is_cancelled()
                            || self
                                .service
                                .agent_tree_run_is_stopped(&invocation.run_id)
                                .map_err(|_| {
                                    unavailable(
                                        "Agent collaboration execution state could not be read.",
                                    )
                                })?
                        {
                            return Err(AgentError::cancelled());
                        }
                        return Err(graph_error(error));
                    }
                };
                self.finish_scheduling_commit(&invocation.run_id, &cancellation)?;
                self.notify_work_available()?;
                AgentCollaborationToolResult::MessageQueued {
                    message_id: dispatch.message.message_id,
                    task_name: target.task_name,
                    delivery_state: AgentCollaborationDeliveryState::Queued,
                }
            }
            AgentCollaborationAction::Wait(request) => {
                let mut target_agent_ids = Vec::with_capacity(request.target_task_names.len());
                for task_name in &request.target_task_names {
                    let target = self.resolve_target_task_name(&caller.agent_id, task_name)?;
                    self.authorizer
                        .authorize_management(
                            &caller.agent_id,
                            &target.agent_id,
                            AgentManagementOperation::Wait,
                        )
                        .map_err(authorizer_error)?;
                    target_agent_ids.push(target.agent_id);
                }
                let outcome = AgentWaitKernel::production(Arc::clone(&self.storage))
                    .wait(
                        PollAgentWaitInput {
                            caller_agent_id: caller.agent_id,
                            conversation_id: invocation.conversation_id,
                            run_id: invocation.run_id,
                            assistant_message_id: invocation.assistant_message_id,
                            model_batch_index: invocation.model_batch_index,
                            target_agent_ids,
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
                let task_names = tree
                    .iter()
                    .map(|node| (node.agent_id.clone(), node.task_name.clone()))
                    .collect::<std::collections::HashMap<_, _>>();
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
                        parent_task_name: node
                            .parent_agent_id
                            .as_ref()
                            .and_then(|id| task_names.get(id))
                            .cloned(),
                        parent_agent_id: node.parent_agent_id,
                        display_status: display.status,
                        model_display_name: node.model_snapshot.map(|model| model.display_name),
                        latest_activity_at,
                    });
                }
                AgentCollaborationToolResult::Agents { agents }
            }
            AgentCollaborationAction::Interrupt { target_task_name } => {
                let target = self.resolve_target_task_name(&caller.agent_id, &target_task_name)?;
                self.authorizer
                    .authorize_management(
                        &caller.agent_id,
                        &target.agent_id,
                        AgentManagementOperation::Interrupt,
                    )
                    .map_err(authorizer_error)?;
                let disposition = self.with_dispatcher(|dispatcher| {
                    dispatcher
                        .interrupt_agent(&caller.agent_id, &target.agent_id, &request_id)
                        .map_err(|_| unavailable("The Agent dispatcher is unavailable."))
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
                    target_agent_id: target.agent_id,
                    task_name: target.task_name,
                    status,
                }
            }
        };
        Ok(AgentCollaborationExecutionOutput {
            result: output,
            persistence: AgentCollaborationResultPersistence::RuntimeCommits,
        })
    }

    fn resolve_target_task_name(
        &self,
        caller_agent_id: &str,
        task_name: &str,
    ) -> Result<AgentNodeRecord, AgentError> {
        self.authorizer
            .resolve_task_name(caller_agent_id, task_name)
            .map_err(authorizer_error)?
            .ok_or_else(|| {
                collaboration_error(
                    "target_unavailable",
                    false,
                    "No accessible Agent has this exact task name. Use list_agents and copy taskName; Agent IDs and task paths are not accepted.".to_string(),
                )
            })
    }

    fn check_scheduling_precommit(
        &self,
        run_id: &str,
        cancellation: &mycopilot_core::AgentCancellationToken,
    ) -> Result<(), AgentError> {
        cancellation.check()?;
        if self
            .service
            .agent_tree_run_is_stopped(run_id)
            .map_err(|_| unavailable("Agent collaboration execution state could not be read."))?
        {
            return Err(AgentError::cancelled());
        }
        Ok(())
    }

    fn finish_scheduling_commit(
        &self,
        run_id: &str,
        cancellation: &mycopilot_core::AgentCancellationToken,
    ) -> Result<(), AgentError> {
        finish_scheduling_commit(
            cancellation,
            self.service
                .agent_tree_run_is_stopped(run_id)
                .map_err(|_| {
                    unavailable("Agent collaboration execution state could not be read.")
                })?,
            || self.service.reinforce_agent_tree_stop_for_run(run_id),
        )
    }
}

fn finish_scheduling_commit(
    cancellation: &mycopilot_core::AgentCancellationToken,
    tree_stop_requested: bool,
    reinforce_tree_stop: impl FnOnce() -> bool,
) -> Result<(), AgentError> {
    if tree_stop_requested {
        // Spawn/follow-up commits are authoritative once SQLite accepts them. If a user root-stop
        // crossed that commit boundary, immediately repeat the durable tree sweep so the newly
        // visible Wake cannot escape and restart after the process is relaunched.
        let _ = reinforce_tree_stop();
        return Err(AgentError::cancelled());
    }
    cancellation.check()
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
    let message = match &error {
        CollaborationAuthorizationError::InvalidPolicy(_) => {
            "Agent collaboration policy is invalid.".to_string()
        }
        CollaborationAuthorizationError::StorageUnavailable(_) => {
            "Agent collaboration authorization storage is unavailable.".to_string()
        }
        _ => error.to_string(),
    };
    AgentError::structured(
        format!("agent.collaboration.{}", error.code()),
        message,
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
    let message = match &error {
        AgentGraphError::InvalidInput { field: "task_name", reason } if reason == ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE => ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
        AgentGraphError::InvalidInput { field, .. } => format!("Invalid collaboration {field}. Check the tool arguments and use exact taskName values from list_agents."),
        AgentGraphError::ResourceLimit { .. }
        | AgentGraphError::IllegalLifecycleTransition { .. }
        | AgentGraphError::IllegalTransition { .. } => error.to_string(),
        AgentGraphError::AgentNotFound(_) => "The target Agent is unavailable. Refresh list_agents and address it by taskName.".to_string(),
        AgentGraphError::ConversationNotFound(_) => "The Agent conversation is unavailable.".to_string(),
        AgentGraphError::MessageNotFound(_) | AgentGraphError::WakeNotFound(_) => "The Agent task or message is no longer available. Refresh list_agents before continuing.".to_string(),
        AgentGraphError::StorageUnavailable(_) => "Agent collaboration storage is unavailable.".to_string(),
        AgentGraphError::CorruptRecord(_) => "Agent collaboration state is inconsistent; the operation could not be completed.".to_string(),
        _ => "Agent collaboration state changed. Refresh list_agents before continuing.".to_string(),
    };
    collaboration_error(category, retryable, message)
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
    let message = match &error {
        ChildAgentSpawnError::InvalidInput { field: "task_name", reason } if reason == ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE => ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
        ChildAgentSpawnError::InvalidInput { field, .. } => format!("Invalid spawn_agent {field}. Check the tool schema and the authorized selector directory."),
        ChildAgentSpawnError::ParentNotFound(_) | ChildAgentSpawnError::ParentUnavailable(_) => "The parent Agent is unavailable; a child could not be created.".to_string(),
        ChildAgentSpawnError::TemplateNotFound(_) | ChildAgentSpawnError::TemplateDisabled(_) => "The selected agent_type is unavailable in this project. Use an authorized selector from the collaboration directory.".to_string(),
        ChildAgentSpawnError::Conflict(reason) if reason == "task name or path is already in use in this Agent tree" => "This task_name is already in use in the Agent tree. Use list_agents and followup_task with the existing taskName to continue that Agent, or choose a unique task_name for a new Agent.".to_string(),
        ChildAgentSpawnError::IdempotencyConflict(_) => "This child creation request was already used with different inputs; no additional child was created.".to_string(),
        ChildAgentSpawnError::SnapshotUnavailable(_) => "The requested child context snapshot is unavailable.".to_string(),
        ChildAgentSpawnError::CorruptRecord(_) => "Child Agent state is inconsistent; a child could not be created.".to_string(),
        ChildAgentSpawnError::Conflict(_) => "Child Agent state changed. Refresh list_agents before requesting more work.".to_string(),
        ChildAgentSpawnError::StorageUnavailable(_) => "Child Agent storage is unavailable.".to_string(),
        ChildAgentSpawnError::ProjectRequiredForTemplate
        | ChildAgentSpawnError::ModelUnavailable { .. }
        | ChildAgentSpawnError::UnsupportedReasoningEffort(_)
        | ChildAgentSpawnError::ResourceLimit { .. } => error.to_string(),
    };
    collaboration_error(category, retryable, message)
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

fn storage_error(_error: impl std::fmt::Display) -> AgentError {
    unavailable("Agent collaboration storage is unavailable.")
}

fn template_error(error: AgentTemplateError) -> AgentError {
    match error {
        AgentTemplateError::StorageUnavailable(_) => {
            unavailable("The Agent template directory is unavailable.")
        }
        _ => collaboration_error(
            "unavailable",
            false,
            "The authorized Agent template directory could not be loaded.".to_string(),
        ),
    }
}

fn unavailable(message: &'static str) -> AgentError {
    AgentError::structured(
        "agent.collaboration.unavailable",
        message,
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
    fn model_facing_collaboration_errors_do_not_expose_backend_identities() {
        const PRIVATE: &str = "agent-c110510f-5338-4d97-9336-0232c53ec142 /root/private-agent";
        let mut errors = vec![
            storage_error(PRIVATE),
            template_error(AgentTemplateError::TemplateNotFound(PRIVATE.to_string())),
            template_error(AgentTemplateError::StorageUnavailable(PRIVATE.to_string())),
            authorizer_error(CollaborationAuthorizationError::InvalidPolicy(
                PRIVATE.to_string(),
            )),
            authorizer_error(CollaborationAuthorizationError::StorageUnavailable(
                PRIVATE.to_string(),
            )),
        ];
        errors.extend(
            [
                AgentGraphError::AgentNotFound(PRIVATE.to_string()),
                AgentGraphError::ConversationNotFound(PRIVATE.to_string()),
                AgentGraphError::MessageNotFound(PRIVATE.to_string()),
                AgentGraphError::WakeNotFound(PRIVATE.to_string()),
                AgentGraphError::BoundConversation(PRIVATE.to_string()),
                AgentGraphError::BoundProject(PRIVATE.to_string()),
                AgentGraphError::Conflict(PRIVATE.to_string()),
                AgentGraphError::CorruptRecord(PRIVATE.to_string()),
                AgentGraphError::StorageUnavailable(PRIVATE.to_string()),
                AgentGraphError::InvalidInput {
                    field: "target",
                    reason: PRIVATE.to_string(),
                },
            ]
            .into_iter()
            .map(graph_error),
        );
        errors.extend(
            [
                ChildAgentSpawnError::ParentNotFound(PRIVATE.to_string()),
                ChildAgentSpawnError::ParentUnavailable(PRIVATE.to_string()),
                ChildAgentSpawnError::TemplateNotFound(PRIVATE.to_string()),
                ChildAgentSpawnError::TemplateDisabled(PRIVATE.to_string()),
                ChildAgentSpawnError::IdempotencyConflict(PRIVATE.to_string()),
                ChildAgentSpawnError::Conflict(PRIVATE.to_string()),
                ChildAgentSpawnError::SnapshotUnavailable(PRIVATE.to_string()),
                ChildAgentSpawnError::CorruptRecord(PRIVATE.to_string()),
                ChildAgentSpawnError::StorageUnavailable(PRIVATE.to_string()),
                ChildAgentSpawnError::InvalidInput {
                    field: "task_name",
                    reason: PRIVATE.to_string(),
                },
            ]
            .into_iter()
            .map(spawn_error),
        );
        for error in errors {
            let model_visible = format!("{error} {:?}", error.details());
            assert!(!model_visible.contains("agent-c110510f"), "{model_visible}");
            assert!(
                !model_visible.contains("/root/private-agent"),
                "{model_visible}"
            );
            assert!(error.code().unwrap().starts_with("agent.collaboration."));
        }
        let duplicate = spawn_error(ChildAgentSpawnError::Conflict(
            "task name or path is already in use in this Agent tree".to_string(),
        ));
        assert!(duplicate
            .to_string()
            .contains("task_name is already in use"));
        assert!(duplicate.to_string().contains("followup_task"));
        assert_category(duplicate, "conflict", false);
    }

    #[test]
    fn reserved_child_name_errors_explain_how_to_recover_without_backend_identity() {
        for error in [
            graph_error(AgentGraphError::InvalidInput {
                field: "task_name",
                reason: ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
            }),
            spawn_error(ChildAgentSpawnError::InvalidInput {
                field: "task_name",
                reason: ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE.to_string(),
            }),
        ] {
            assert!(error
                .to_string()
                .contains(ROOT_AGENT_TASK_NAME_RESERVED_MESSAGE));
            assert_category(error, "invalid_arguments", false);
        }
    }

    #[tokio::test]
    async fn root_task_name_is_independent_of_titles_and_renaming() {
        let fixture = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("root-name.sqlite")).unwrap());
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
        for (index, title) in [
            "让子智能体发送“已完成”给父亲".to_string(),
            "长中文会话标题".repeat(80),
            "读取 [页面](http://127.0.0.1:18765/path)\n标题".to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let conversation_id = format!("root-name-{index}");
            let mut conversation = ChatConversationRecord {
                id: conversation_id.clone(),
                project_id: None,
                model_id: None,
                title: title.clone(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            };
            storage.save_conversation(conversation.clone()).unwrap();
            let preview = adapter
                .preview_runtime_services_for_conversation(&conversation_id)
                .unwrap();
            assert_eq!(preview.caller.task_name, "主智能体");
            assert!(storage
                .get_agent_node_by_conversation(&conversation_id)
                .unwrap()
                .is_none());
            let live = adapter
                .runtime_services_for_conversation(&conversation_id)
                .unwrap();
            assert_eq!(live.caller, preview.caller);
            assert_eq!(
                storage
                    .load_conversation(&conversation_id)
                    .unwrap()
                    .unwrap()
                    .title,
                title
            );

            conversation.title = "重命名后的新标题：“继续工作”".to_string();
            storage.save_conversation(conversation.clone()).unwrap();
            let renamed = adapter
                .runtime_services_for_conversation(&conversation_id)
                .unwrap();
            assert_eq!(renamed.caller, live.caller);
            assert_eq!(
                adapter
                    .preview_runtime_services_for_conversation(&conversation_id)
                    .unwrap()
                    .caller,
                live.caller
            );
            assert_eq!(
                adapter
                    .resolve_target_task_name(&live.caller.agent_id, ROOT_AGENT_TASK_NAME)
                    .unwrap()
                    .agent_id,
                live.caller.agent_id
            );
            assert!(adapter
                .resolve_target_task_name(&live.caller.agent_id, &title)
                .is_err());
            assert!(adapter
                .resolve_target_task_name(&live.caller.agent_id, &conversation.title)
                .is_err());
        }
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
    fn scheduling_post_commit_reinforces_only_an_explicit_tree_stop() {
        let cancellation = AgentCancellationToken::new();
        let reinforcements = std::cell::Cell::new(0);
        let error = finish_scheduling_commit(&cancellation, true, || {
            reinforcements.set(reinforcements.get() + 1);
            true
        })
        .unwrap_err();

        assert!(error.is_cancelled());
        assert_eq!(reinforcements.get(), 1);
    }

    #[test]
    fn scheduling_post_commit_does_not_cascade_a_single_turn_interrupt() {
        let cancellation = AgentCancellationToken::new();
        cancellation.cancel();
        let error = finish_scheduling_commit(&cancellation, false, || {
            panic!("a single-Agent interrupt must not reinforce a tree stop")
        })
        .unwrap_err();

        assert!(error.is_cancelled());
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
            .save_project(ProjectRecord::with_primary_folder(
                "project-catalog".to_string(),
                "Catalog".to_string(),
                fixture.path().to_string_lossy().into_owned(),
                1,
            ))
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

        let cancelled_before_host_entry = AgentCancellationToken::new();
        cancelled_before_host_entry.cancel();
        let cancelled_error = adapter
            .execute(
                AgentCollaborationInvocation {
                    caller: before.caller.clone(),
                    selector_authorization: before.selector_authorization(),
                    caller_model_capabilities: mycopilot_core::ModelCapabilities {
                        image_input: false,
                    },
                    effective_permissions: mycopilot_core::AgentPermissions::default(),
                    conversation_id: "conversation-catalog".to_string(),
                    run_id: "run-cancelled-before-host-entry".to_string(),
                    assistant_message_id: "assistant-cancelled-before-host-entry".to_string(),
                    model_batch_index: 1,
                    tool_call_id: "call-cancelled-before-host-entry".to_string(),
                    action: AgentCollaborationAction::List,
                },
                AgentCollaborationExecutionControl::new(cancelled_before_host_entry, None),
            )
            .await
            .unwrap_err();
        assert!(cancelled_error.is_cancelled());
        assert!(storage
            .get_agent_node_by_conversation("conversation-catalog")
            .unwrap()
            .is_none());

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
            human_interaction_response: None,
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

    #[tokio::test]
    async fn selector_directory_advertises_only_models_whose_credentials_resolve() {
        use mycopilot_core::image_generation::{
            CredentialReference, CredentialStore, InMemoryCredentialStore,
        };

        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("agent-harness-credentials.sqlite");
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let storage = Arc::new(
            StorageService::open_with_model_credentials(&database_path, credentials.clone())
                .unwrap(),
        );
        storage
            .save_project(ProjectRecord::with_primary_folder(
                "project-credentials".to_string(),
                "Credentials".to_string(),
                fixture.path().to_string_lossy().into_owned(),
                1,
            ))
            .unwrap();
        let model_config =
            |id: &str, url_override: Option<&str>, token: Option<&str>| ModelConfigRecord {
                id: id.to_string(),
                provider_model_id: id.to_string(),
                display_name: id.to_string(),
                api_url_override: url_override.map(ToString::to_string),
                api_token_override: token.map(ToString::to_string),
                supports_image: false,
                context_window_tokens: Some(64_000),
                provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".to_string(),
                cached_input_price: String::new(),
                output_price: "0".to_string(),
                enabled: true,
            };
        storage
            .save_model_settings(ModelSettingsRecord {
                api_url: "https://provider.example/v1/chat/completions".to_string(),
                api_token: "private-ready-token".to_string(),
                search_mode: "disabled".to_string(),
                tavily_api_key: String::new(),
                models: vec![
                    model_config("model-ready", None, None),
                    model_config(
                        "model-broken",
                        Some("https://provider-broken.example/v1/chat/completions"),
                        Some("private-broken-token"),
                    ),
                    model_config(
                        "model-partial",
                        Some("https://provider-partial.example/v1/chat/completions"),
                        None,
                    ),
                ],
            })
            .unwrap();
        let assign_template = |template_id: &str, model_id: &str| {
            storage
                .create_agent_template(&CreateAgentTemplateInput {
                    template_id: template_id.to_string(),
                    machine_key: template_id.to_string(),
                    name: template_id.to_string(),
                    description: "Fixture".to_string(),
                    instructions: "Fixture instructions".to_string(),
                    model_config_id: model_id.to_string(),
                    enabled: true,
                })
                .unwrap();
            storage
                .set_agent_template_project_assignment("project-credentials", template_id, true)
                .unwrap();
        };
        assign_template("ready-template", "model-ready");
        assign_template("broken-template", "model-broken");
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-credentials".to_string(),
                project_id: Some("project-credentials".to_string()),
                model_id: Some("model-ready".to_string()),
                title: "Credentials root".to_string(),
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
            .preview_runtime_services_for_conversation("conversation-credentials")
            .unwrap();
        let mut models_before = before
            .selector_directory
            .models
            .iter()
            .map(|model| model.model_config_id.as_str())
            .collect::<Vec<_>>();
        models_before.sort_unstable();
        assert_eq!(models_before, vec!["model-broken", "model-ready"]);
        let mut templates_before = before
            .selector_directory
            .templates
            .iter()
            .map(|template| template.agent_type.as_str())
            .collect::<Vec<_>>();
        templates_before.sort_unstable();
        assert_eq!(templates_before, vec!["broken-template", "ready-template"]);

        // Break the dedicated credential: the reference stays in place, but its secret is gone.
        let broken_ref: String = rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT api_token_override_ref FROM models WHERE id = 'model-broken'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        credentials
            .delete(&CredentialReference::parse(&broken_ref).unwrap())
            .unwrap();

        let after = adapter
            .preview_runtime_services_for_conversation("conversation-credentials")
            .unwrap();
        let models_after = after
            .selector_directory
            .models
            .iter()
            .map(|model| model.model_config_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(models_after, vec!["model-ready"]);
        let templates_after = after
            .selector_directory
            .templates
            .iter()
            .map(|template| template.agent_type.as_str())
            .collect::<Vec<_>>();
        assert_eq!(templates_after, vec!["ready-template"]);
    }
}
