//! Host coordinator for independent conversation workflow deliveries.
//! The existing root Turn owns execution; this module only claims durable inputs and supplies
//! collaborator text at a safe sampling boundary. It never turns that text into human authority.
use super::*;
use mycopilot_core::storage::models::ChatMessageRecord;
use mycopilot_core::workflow_execution::{
    ConversationSnapshot, Input, RuntimeSnapshot, SendReceipt, SendRequest,
};
use mycopilot_core::{
    AgentWorkflowDelivery, AgentWorkflowInbox, WorkflowRuntimeHost, WorkflowSendInvocation,
};
use serde_json::json;

pub(super) struct WorkflowTurnOwner<'a> {
    pub run_id: &'a str,
    pub conversation_id: &'a str,
    pub assistant_message_id: &'a str,
    pub token: &'a AgentCancellationToken,
}

struct StoredWorkflowRuntime {
    service: AgentService,
    conversation_id: String,
    run_id: String,
    assistant_message_id: String,
    cancellation: AgentCancellationToken,
    notifications: CoreServerNotificationSender,
    initial_input_id: Option<String>,
}
impl StoredWorkflowRuntime {
    fn validate_owner(&self) -> AgentResult<()> {
        self.cancellation.check()?;
        let tokens = self
            .service
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !tokens
            .get(&self.run_id)
            .is_some_and(|token| token.shares_state_with(&self.cancellation))
        {
            return Err(AgentError::new("Workflow execution segment was retired."));
        }
        let root = self
            .service
            .storage
            .get_agent_node_by_conversation(&self.conversation_id)
            .map_err(|e| AgentError::new(e.to_string()))?;
        if root.is_some_and(|root| root.parent_agent_id.is_some()) {
            return Err(AgentError::new(
                "Workflow tools are only available to independent root conversations.",
            ));
        }
        self.service
            .ensure_conversation_turn_owner(
                &self.conversation_id,
                &self.run_id,
                &self.assistant_message_id,
            )
            .map_err(AgentError::new)
    }
}
impl WorkflowRuntimeHost for StoredWorkflowRuntime {
    fn snapshot(&self) -> AgentResult<Option<ConversationSnapshot>> {
        self.validate_owner()?;
        self.service
            .storage
            .workflow_execution_snapshot_for_run(&self.conversation_id, &self.run_id)
            .map_err(AgentError::new)
    }
    fn send(&self, invocation: WorkflowSendInvocation) -> AgentResult<SendReceipt> {
        if invocation.conversation_id != self.conversation_id
            || invocation.run_id != self.run_id
            || invocation.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "Workflow sender does not match the Host-bound conversation.",
            ));
        }
        self.validate_owner()?;
        // Stop and send share the cancellation lock: a stopped source cannot publish new work.
        let tokens = self
            .service
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.cancellation.check()?;
        if !tokens
            .get(&self.run_id)
            .is_some_and(|token| token.shares_state_with(&self.cancellation))
        {
            return Err(AgentError::new("Workflow execution segment was retired."));
        }
        let receipt = self
            .service
            .storage
            .workflow_execution_send(&SendRequest {
                conversation_id: self.conversation_id.clone(),
                source_run_id: self.run_id.clone(),
                tool_call_id: invocation.tool_call_id,
                execution_version: invocation.execution_version,
                outputs: invocation.outputs,
            })
            .map_err(AgentError::new)?;
        drop(tokens);
        self.service
            .publish_workflow_runtime(&receipt.instance_id, &self.notifications);
        self.service
            .schedule_workflow_deliveries(self.notifications.clone());
        Ok(receipt)
    }
}
impl AgentWorkflowInbox for StoredWorkflowRuntime {
    fn bind_for_model_batch(
        &self,
        request: mycopilot_core::AgentSamplingBoundaryRequest,
    ) -> AgentResult<Vec<AgentWorkflowDelivery>> {
        if request.conversation_id != self.conversation_id
            || request.run_id != self.run_id
            || request.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "Workflow input boundary does not match its Host owner.",
            ));
        }
        self.validate_owner()?;
        // A fresh boundary can claim injections which arrived after the previous scan. This runs
        // only after all tools/approvals/human waits have reached the normal safe boundary.
        self.service
            .claim_workflow_injections(&self.conversation_id, &self.run_id)
            .map_err(AgentError::new)?;
        let inputs = self
            .service
            .storage
            .workflow_execution_bound_inputs(&self.run_id)
            .map_err(AgentError::new)?;
        if let Some(id) = &self.initial_input_id {
            let admitted = self
                .service
                .storage
                .workflow_execution_load_input(id)
                .map_err(AgentError::new)?;
            if admitted.as_ref().is_none_or(|input| {
                input.status != mycopilot_core::workflow_execution::InputStatus::Applied
            }) && !inputs.iter().any(|input| &input.id == id)
            {
                return Err(AgentError::new(
                    "Workflow input was paused or invalidated before the first model request.",
                ));
            }
        }
        let trace = self
            .service
            .storage
            .get_conversation_turn_trace(&self.assistant_message_id)
            .map_err(AgentError::new)?;
        let mut deliveries = Vec::new();
        for input in inputs {
            if trace.as_ref().is_some_and(|trace| trace.items.iter().any(|item| {
                matches!(item, mycopilot_core::ConversationTurnTraceItem::WorkflowDelivery { input_id, .. } if input_id == &input.id)
            })) {
                self.service.storage.workflow_execution_mark_applied(&input.id).map_err(AgentError::new)?;
                continue;
            }
            let conversation = self
                .service
                .storage
                .load_conversation(&self.conversation_id)
                .map_err(AgentError::new)?
                .ok_or_else(|| AgentError::new("Workflow recipient conversation was removed."))?;
            let delivery_id = input
                .delivery_id
                .clone()
                .ok_or_else(|| AgentError::new("Workflow input has no delivery identity."))?;
            if !conversation
                .messages
                .iter()
                .any(|message| message.id == delivery_id)
            {
                self.service
                    .storage
                    .upsert_chat_messages(
                        &self.conversation_id,
                        vec![ChatMessageRecord {
                            human_interaction_response: None,
                            id: delivery_id,
                            role: "user".into(),
                            content: input.content.clone(),
                            created_at: input.created_at,
                            status: Some("sent".into()),
                            attachments: Vec::new(),
                            folder_references_json: None,
                            agent_run_json: None,
                            ui_state_json: None,
                        }],
                        conversation.messages.len() as i64,
                    )
                    .map_err(AgentError::new)?;
            }
            deliveries.push(AgentWorkflowDelivery {
                trace_sequence: request.expected_next_trace_sequence + deliveries.len() as u64,
                input_id: input.id,
                instance_id: input.instance_id,
                workflow_name: input
                    .messages
                    .first()
                    .map(|m| m.workflow_name.clone())
                    .unwrap_or_default(),
                content: input.content,
                created_at: input.created_at,
            });
        }
        Ok(deliveries)
    }
}

impl AgentService {
    pub(super) fn attach_workflow_runtime(
        &self,
        services: AgentRuntimeHostServices,
        input: &AgentChatInput,
        owner: WorkflowTurnOwner<'_>,
        notifications: CoreServerNotificationSender,
    ) -> AgentRuntimeHostServices {
        let WorkflowTurnOwner {
            run_id,
            conversation_id,
            assistant_message_id,
            token,
        } = owner;
        if input
            .context
            .as_ref()
            .is_none_or(|context| context.collaboration_identity.is_some())
            || input
                .prompt_preferences
                .as_ref()
                .is_some_and(|preferences| preferences.automation_execution_context.is_some())
        {
            return services;
        }
        let initial_input_id = self
            .storage
            .load_conversation(conversation_id)
            .ok()
            .flatten()
            .and_then(|chat| {
                chat.messages
                    .iter()
                    .position(|message| message.id == assistant_message_id)
                    .and_then(|position| position.checked_sub(1))
                    .map(|position| chat.messages[position].id.clone())
            })
            .and_then(|previous| {
                self.storage
                    .workflow_execution_inputs_for_conversation(conversation_id)
                    .ok()
                    .and_then(|inputs| {
                        inputs
                            .into_iter()
                            .find(|input| {
                                input.run_id.as_deref() == Some(run_id)
                                    && input.delivery_id.as_deref() == Some(previous.as_str())
                            })
                            .map(|input| input.id)
                    })
            });
        let host = Arc::new(StoredWorkflowRuntime {
            service: self.clone(),
            conversation_id: conversation_id.into(),
            run_id: run_id.into(),
            assistant_message_id: assistant_message_id.into(),
            cancellation: token.clone(),
            notifications,
            initial_input_id,
        });
        services
            .with_workflow_runtime(host.clone())
            .with_workflow_inbox(host)
    }

    pub(crate) fn workflow_runtime_snapshot(
        &self,
        instance_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<RuntimeSnapshot, String> {
        self.storage
            .workflow_execution_runtime_since(instance_id, after_sequence)
    }
    pub(crate) fn complete_workflow_user_input(
        &self,
        instance_id: &str,
        input_id: &str,
        notifications: &CoreServerNotificationSender,
    ) -> Result<RuntimeSnapshot, String> {
        let input = self
            .storage
            .workflow_execution_load_input(input_id)?
            .ok_or_else(|| "Workflow input was removed.".to_string())?;
        if input.instance_id != instance_id {
            return Err("Workflow input belongs to another workflow.".into());
        }
        self.storage.workflow_execution_complete_user(input_id)?;
        let snapshot = self.storage.workflow_execution_runtime(instance_id)?;
        let _ = notifications.send(
            json!({"jsonrpc":"2.0","method":"agent.workflows.runtime.changed","params":snapshot}),
        );
        Ok(snapshot)
    }
    pub(crate) fn discard_failed_workflow_input(
        &self,
        instance_id: &str,
        input_id: &str,
        notifications: &CoreServerNotificationSender,
    ) -> Result<RuntimeSnapshot, String> {
        let input = self
            .storage
            .workflow_execution_load_input(input_id)?
            .ok_or_else(|| "Workflow input was removed.".to_string())?;
        if input.instance_id != instance_id {
            return Err("Workflow input belongs to another workflow.".into());
        }
        self.storage.workflow_execution_discard_failed(input_id)?;
        self.publish_workflow_runtime(instance_id, notifications);
        self.schedule_workflow_deliveries(notifications.clone());
        self.storage.workflow_execution_runtime(instance_id)
    }
    pub(super) fn publish_workflow_runtime(
        &self,
        instance_id: &str,
        notifications: &CoreServerNotificationSender,
    ) {
        match self.storage.workflow_execution_runtime(instance_id) {
            Ok(snapshot) => {
                let _ = notifications.send(json!({"jsonrpc":"2.0","method":"agent.workflows.runtime.changed","params":snapshot}));
            }
            Err(error) => eprintln!("workflow runtime projection failed: {error}"),
        }
    }
    pub(super) fn acknowledge_workflow_trace(
        &self,
        snapshot: &mycopilot_core::ConversationTraceSnapshot,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        let mut changed = HashSet::new();
        for item in &snapshot.items {
            if let mycopilot_core::ConversationTurnTraceItem::WorkflowDelivery {
                input_id,
                instance_id,
                ..
            } = item
            {
                if self
                    .storage
                    .workflow_execution_load_input(input_id)?
                    .is_some_and(|input| {
                        matches!(
                            input.status,
                            mycopilot_core::workflow_execution::InputStatus::Claimed
                                | mycopilot_core::workflow_execution::InputStatus::Failed
                        )
                    })
                {
                    self.storage.workflow_execution_mark_applied(input_id)?;
                    changed.insert(instance_id.clone());
                }
            }
        }
        for id in changed {
            self.publish_workflow_runtime(&id, notifications);
        }
        Ok(())
    }
    fn claim_workflow_injections(&self, conversation_id: &str, run_id: &str) -> Result<(), String> {
        let Some(identity) = self
            .storage
            .workflow_execution_snapshot_for_run(conversation_id, run_id)?
        else {
            return Ok(());
        };
        for input in self.storage.workflow_execution_pending_inputs()? {
            if input.execution_version == identity.execution_version
                && input.node_id == identity.node_id
                && input.conversation_id.as_deref() == Some(conversation_id)
                && input.busy_policy == mycopilot_core::workflow::BusyPolicy::Inject
            {
                self.storage.workflow_execution_bind_input(
                    &input.id,
                    run_id,
                    &format!("workflow-message-{}", input.id),
                )?;
            }
        }
        Ok(())
    }
    /// Events accelerate delivery; the startup-owned periodic wake handles capacity/configuration
    /// recovery without depending on an open Renderer page. A busy scan is simply coalesced.
    pub(crate) fn schedule_workflow_deliveries(&self, notifications: CoreServerNotificationSender) {
        if self.workflow_dispatch_stopped.load(Ordering::Acquire) {
            return;
        }
        let Ok(_dispatch) = self.workflow_delivery_dispatch.try_lock() else {
            return;
        };
        let inputs = match self.storage.workflow_execution_pending_inputs() {
            Ok(inputs) => inputs,
            Err(error) => {
                eprintln!("workflow scan failed: {error}");
                return;
            }
        };
        for input in inputs {
            let Some(conversation_id) = input.conversation_id.as_deref() else {
                continue;
            };
            let occupied = match self.has_conversation_turn_occupancy(conversation_id) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if occupied {
                continue;
            } // active inputs are claimed by their own safe-boundary inbox
            if let Err(error) = self.start_workflow_input(&input, notifications.clone()) {
                // Readiness, account access, or model transition can recover; the unclaimed input
                // stays durable. Once claimed, start_root_turn marks a failure without replay.
                if self
                    .storage
                    .workflow_execution_load_input(&input.id)
                    .ok()
                    .flatten()
                    .is_some_and(|current| current.status != input.status)
                {
                    eprintln!("workflow delivery deferred: {error}");
                    self.publish_workflow_runtime(&input.instance_id, &notifications);
                }
            }
        }
    }
    fn start_workflow_input(
        &self,
        workflow: &Input,
        notifications: CoreServerNotificationSender,
    ) -> Result<(), AgentServiceError> {
        let conversation_id = workflow
            .conversation_id
            .as_deref()
            .ok_or_else(|| "Workflow recipient is not a conversation.".to_string())?;
        let conversation = self
            .storage
            .load_conversation(conversation_id)?
            .ok_or_else(|| "Workflow recipient was removed.".to_string())?;
        if conversation.archived_at.is_some() {
            return Err("Workflow recipient is archived.".to_string().into());
        }
        let draft = self
            .storage
            .load_composer_drafts()?
            .into_iter()
            .find(|draft| draft.scope_id == conversation_id)
            .ok_or_else(|| "Workflow recipient has no composer configuration.".to_string())?;
        let model_id = draft
            .model_id
            .or(conversation.model_id)
            .ok_or_else(|| "Workflow recipient has no selected model.".to_string())?;
        let mode = match draft.permission_mode.as_str() {
            "full" => mycopilot_protocol_rs::AutomationPermissionModeDto::Full,
            "custom" => mycopilot_protocol_rs::AutomationPermissionModeDto::Custom,
            _ => mycopilot_protocol_rs::AutomationPermissionModeDto::Default,
        };
        let permissions =
            crate::application::automation::permissions::resolve_automation_permissions(
                mode,
                mycopilot_protocol_rs::AUTOMATION_PERMISSION_MODE_VERSION,
                &self.storage.load_ui_preferences()?,
            )
            .map_err(|error| error.to_string())?
            .permissions;
        let transition =
            self.preflight_provider_transition(AgentProviderTransitionPreflightInput {
                conversation_id: conversation_id.into(),
                target_model_id: model_id.clone(),
            })?;
        if transition.decision == AgentProviderTransitionDecision::RequiresCompaction {
            self.start_provider_transition(
                AgentProviderTransitionStartInput {
                    conversation_id: conversation_id.into(),
                    target_model_id: model_id,
                    transition_token: transition
                        .transition_token
                        .ok_or_else(|| "Provider transition token missing.".to_string())?,
                },
                notifications,
            )?;
            return Ok(());
        }
        if transition.decision == AgentProviderTransitionDecision::Blocked {
            return Ok(());
        }
        self.start_root_turn_with_workflow(
            AgentConversationTurnInput {
                conversation_id: Some(conversation_id.into()),
                project_id: conversation.project_id,
                model_id,
                context_window_indicator_enabled: true,
                content: workflow.content.clone(),
                attachments: Vec::new(),
                folder_references: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some(format!("workflow-message-{}", workflow.id)),
                assistant_message_id: None,
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions,
            },
            None,
            None,
            None,
            Some(workflow.clone()),
            notifications.clone(),
        )?;
        self.publish_workflow_runtime(&workflow.instance_id, &notifications);
        Ok(())
    }
}

struct WorkflowPreview {
    storage: Arc<StorageService>,
    conversation_id: String,
    run_id: Option<String>,
}
impl WorkflowRuntimeHost for WorkflowPreview {
    fn snapshot(&self) -> AgentResult<Option<ConversationSnapshot>> {
        match &self.run_id {
            Some(run_id) => self
                .storage
                .workflow_execution_snapshot_for_run(&self.conversation_id, run_id),
            None => self
                .storage
                .workflow_execution_snapshot(&self.conversation_id),
        }
        .map_err(AgentError::new)
    }
    fn send(&self, _invocation: WorkflowSendInvocation) -> AgentResult<SendReceipt> {
        Err(AgentError::new(
            "A read-only workflow preview cannot send messages.",
        ))
    }
}
impl AgentService {
    pub(super) fn workflow_preview_host(
        &self,
        conversation_id: &str,
        run_id: Option<&str>,
    ) -> Arc<dyn WorkflowRuntimeHost> {
        Arc::new(WorkflowPreview {
            storage: self.storage.clone(),
            conversation_id: conversation_id.into(),
            run_id: run_id.map(str::to_string),
        })
    }
}
