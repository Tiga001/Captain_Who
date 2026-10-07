//! Host coordinator for independent conversation organization mail deliveries.
//! The existing root Turn owns execution; this module only claims durable inputs and supplies
//! collaborator text at a safe sampling boundary. It never turns that text into human authority.
use super::*;
use mycopilot_core::storage::models::ChatMessageRecord;
use mycopilot_core::workflow_awareness::{MailboxQuery, StateQuery};
use mycopilot_core::workflow_execution::{
    ConversationSnapshot, Input, MutationRequest, RuntimeSnapshot, SendReceipt, SendRequest,
};
use mycopilot_core::{
    AgentWorkflowDelivery, AgentWorkflowInbox, OrganizationEditInvocation,
    OrganizationEditReceiptQuery, WorkflowMailReceipt, WorkflowMailReceiptQuery,
    WorkflowMutationInvocation, WorkflowRuntimeHost, WorkflowSendInvocation,
};
use serde_json::json;

mod awareness;

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
    current_model_id: Option<String>,
    effective_permissions: mycopilot_core::AgentPermissions,
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
            return Err(AgentError::new(
                "Organization execution segment was retired.",
            ));
        }
        let root = self
            .service
            .storage
            .get_agent_node_by_conversation(&self.conversation_id)
            .map_err(|e| AgentError::new(e.to_string()))?;
        if root.is_some_and(|root| root.parent_agent_id.is_some()) {
            return Err(AgentError::new(
                "Organization tools are only available to independent root conversations.",
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
    fn request_observation(&self) -> AgentResult<Option<(ConversationSnapshot, Value)>> {
        self.validate_owner()?;
        self.service
            .storage
            .workflow_execution_request_observation(&self.conversation_id, Some(&self.run_id))
            .map_err(AgentError::new)
    }
    fn mail_receipt(
        &self,
        query: WorkflowMailReceiptQuery,
    ) -> AgentResult<Option<WorkflowMailReceipt>> {
        if query.conversation_id != self.conversation_id
            || query.run_id != self.run_id
            || query.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "Organization mail receipt does not match its Host owner.",
            ));
        }
        self.validate_owner()?;
        let receipt = self
            .service
            .storage
            .workflow_mail_receipt(
                &self.conversation_id,
                &self.run_id,
                &query.tool_call_id,
                &query.call,
            )
            .map_err(AgentError::new)?;
        let instance = match &receipt {
            Some(WorkflowMailReceipt::Send(receipt)) => Some(receipt.instance_id.as_str()),
            Some(WorkflowMailReceipt::Mutation(receipt)) => receipt["instanceId"].as_str(),
            None => None,
        };
        if let Some(instance) = instance {
            self.service
                .publish_workflow_runtime(instance, &self.notifications);
        }
        Ok(receipt)
    }
    fn organization_edit_receipt(
        &self,
        query: OrganizationEditReceiptQuery,
    ) -> AgentResult<Option<mycopilot_core::organization_personnel::Receipt>> {
        if query.conversation_id != self.conversation_id
            || query.run_id != self.run_id
            || query.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "Organization receipt does not match its Host owner.",
            ));
        }
        self.validate_owner()?;
        let receipt = self
            .service
            .storage
            .organization_edit_receipt(
                &self.conversation_id,
                &self.run_id,
                &query.tool_call_id,
                &query.input,
            )
            .map_err(AgentError::new)?;
        if let Some(receipt) = &receipt {
            // The original commit may have survived a crash before its UI notification.
            self.service
                .publish_workflow_runtime(&receipt.instance_id, &self.notifications);
        }
        Ok(receipt)
    }
    fn edit_organization(
        &self,
        invocation: OrganizationEditInvocation,
    ) -> AgentResult<mycopilot_core::organization_personnel::Receipt> {
        if invocation.conversation_id != self.conversation_id
            || invocation.run_id != self.run_id
            || invocation.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "Organization edit does not match its Host owner.",
            ));
        }
        self.validate_owner()?;
        // Model/permission edits update the target's next-turn defaults. Share admission with
        // provider transitions and manual compaction, but leave ordinary roster edits unblocked.
        let admission = self
            .service
            .conversation_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Two dispatches may both observe the first receipt lookup as empty. Serialize this
        // second lookup with commit so only a genuinely new edit publishes preference updates.
        if let Some(receipt) = self
            .service
            .storage
            .organization_edit_receipt(
                &self.conversation_id,
                &self.run_id,
                &invocation.tool_call_id,
                &invocation.model_input.clone().unwrap_or(
                    serde_json::to_value(&invocation.input)
                        .map_err(|e| AgentError::new(e.to_string()))?,
                ),
            )
            .map_err(AgentError::new)?
        {
            self.service
                .publish_workflow_runtime(&receipt.instance_id, &self.notifications);
            return Ok(receipt);
        }
        // These values belong to the admitted execution segment. Composer selections can change
        // while the model is thinking and must never grant this call more authority.
        let context = organization_edit_context(
            self.current_model_id.as_deref(),
            self.effective_permissions,
            &self
                .service
                .storage
                .load_ui_preferences()
                .map_err(AgentError::new)?,
        )?;
        self.service.check_organization_edit_configuration(
            &self.conversation_id,
            &self.run_id,
            &invocation.input,
        )?;
        // Serialize with stop through the same lock as mail mutations. The storage transaction
        // independently checks current membership, administrator scope, rank and revision.
        let tokens = self
            .service
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.cancellation.check()?;
        if !tokens
            .get(&self.run_id)
            .is_some_and(|token| token.shares_state_with(&self.cancellation))
        {
            return Err(AgentError::new(
                "Organization execution segment was retired.",
            ));
        }
        let receipt = self
            .service
            .storage
            .organization_edit(&mycopilot_core::organization_personnel::Request {
                conversation_id: self.conversation_id.clone(),
                source_run_id: self.run_id.clone(),
                tool_call_id: invocation.tool_call_id,
                execution_version: invocation.execution_version,
                expected_revision: invocation.expected_revision,
                input: invocation.input,
                model_input: invocation.model_input,
                context,
            })
            .map_err(AgentError::new)?;
        drop(tokens);
        self.service.publish_workflow_runtime_with_preferences(
            &receipt.instance_id,
            organization_preference_updates(&receipt),
            &self.notifications,
        );
        drop(admission);
        self.service.workflow_readiness_changed(None);
        Ok(receipt)
    }
    fn snapshot(&self) -> AgentResult<Option<ConversationSnapshot>> {
        self.validate_owner()?;
        self.service
            .storage
            .workflow_execution_snapshot_for_run(&self.conversation_id, &self.run_id)
            .map_err(AgentError::new)
    }
    fn state(&self, query: StateQuery) -> AgentResult<Value> {
        self.validate_owner()?;
        let mut state = self
            .service
            .storage
            .workflow_execution_state_for_run(&self.conversation_id, &self.run_id, &query)
            .map_err(AgentError::new)?;
        if query.view == mycopilot_core::workflow_awareness::StateView::Configuration {
            // Storage has already authorized this explicit administrator query. Reuse the same
            // executable-model projection as settings and organization edit, never expose keys.
            let models = self
                .service
                .storage
                .load_model_projection()
                .map_err(AgentError::new)?;
            state["configuration"]["availableModels"] = json!(models
                .into_iter()
                .flat_map(|projection| projection.models)
                .filter(|entry| entry.execution.is_available())
                .map(
                    |entry| json!({"modelConfigId":entry.model.id,"name":entry.model.display_name})
                )
                .collect::<Vec<_>>());
            let context = organization_edit_context(
                self.current_model_id.as_deref(),
                self.effective_permissions,
                &self
                    .service
                    .storage
                    .load_ui_preferences()
                    .map_err(AgentError::new)?,
            )?;
            state["configuration"]["callerCurrentRun"] = json!({"modelConfigId":context.current_model_id,
                "inheritedPermissionMode":context.default_permission_mode,"allowedPermissionModes":context.allowed_permission_modes});
        } else {
            self.service
                .enrich_workflow_awareness(&mut state)
                .map_err(AgentError::new)?;
        }
        Ok(state)
    }
    fn mailbox(&self, query: MailboxQuery) -> AgentResult<Value> {
        self.validate_owner()?;
        self.service
            .storage
            .workflow_execution_mailbox_for_run(&self.conversation_id, &self.run_id, &query)
            .map_err(AgentError::new)
    }
    fn awareness(&self) -> AgentResult<Value> {
        self.validate_owner()?;
        let mut awareness = self
            .service
            .storage
            .workflow_execution_awareness_for_run(&self.conversation_id, &self.run_id)
            .map_err(AgentError::new)?;
        self.service
            .enrich_workflow_awareness(&mut awareness)
            .map_err(AgentError::new)?;
        Ok(awareness)
    }
    fn mutate(&self, invocation: WorkflowMutationInvocation) -> AgentResult<Value> {
        if invocation.conversation_id != self.conversation_id
            || invocation.run_id != self.run_id
            || invocation.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "Organization mail operation does not match its Host owner.",
            ));
        }
        self.validate_owner()?;
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
            return Err(AgentError::new(
                "Organization execution segment was retired.",
            ));
        }
        let receipt = self
            .service
            .storage
            .workflow_execution_mutate(&MutationRequest {
                conversation_id: self.conversation_id.clone(),
                source_run_id: self.run_id.clone(),
                tool_call_id: invocation.tool_call_id,
                execution_version: invocation.execution_version,
                action: invocation.action,
                message_ids: invocation.message_ids,
            })
            .map_err(AgentError::new)?;
        drop(tokens);
        if let Some(instance_id) = receipt["instanceId"].as_str() {
            self.service
                .publish_workflow_runtime(instance_id, &self.notifications);
        }
        // accept already owns its chosen letters; the next safe sampling boundary publishes
        // their WorkflowDelivery. Never wake a separate run or auto-claim other pending mail.
        Ok(receipt)
    }

    fn send(&self, invocation: WorkflowSendInvocation) -> AgentResult<SendReceipt> {
        if invocation.conversation_id != self.conversation_id
            || invocation.run_id != self.run_id
            || invocation.assistant_message_id != self.assistant_message_id
        {
            return Err(AgentError::new(
                "Organization sender does not match the Host-bound conversation.",
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
            return Err(AgentError::new(
                "Organization execution segment was retired.",
            ));
        }
        let receipt = self
            .service
            .storage
            .workflow_execution_send(&SendRequest {
                conversation_id: self.conversation_id.clone(),
                source_run_id: self.run_id.clone(),
                tool_call_id: invocation.tool_call_id,
                execution_version: invocation.execution_version,
                messages: invocation.messages,
                model_input: invocation.model_input,
                recipient_versions: invocation.recipient_versions,
            })
            .map_err(AgentError::new)?;
        drop(tokens);
        self.service
            .publish_workflow_runtime(&receipt.instance_id, &self.notifications);
        self.service.wake_workflow_deliveries();
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
                "Organization input boundary does not match its Host owner.",
            ));
        }
        self.validate_owner()?;
        // Only the startup letter and explicitly accepted letters have been bound to this run.
        // Running recipients never auto-claim additional pending mail at sampling boundaries.
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
                !matches!(
                    input.status,
                    mycopilot_core::workflow_execution::InputStatus::Applied
                        | mycopilot_core::workflow_execution::InputStatus::Completed
                ) || input.run_id.as_deref() != Some(self.run_id.as_str())
            }) && !inputs.iter().any(|input| &input.id == id)
            {
                return Err(AgentError::new(
                    "Organization mail was stopped or recalled before the first model request.",
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
                .ok_or_else(|| {
                    AgentError::new("Organization recipient conversation was removed.")
                })?;
            let delivery_id = input
                .delivery_id
                .clone()
                .ok_or_else(|| AgentError::new("Organization input has no delivery identity."))?;
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

fn organization_preference_updates(
    receipt: &mycopilot_core::organization_personnel::Receipt,
) -> Vec<mycopilot_core::workflow_execution::PreferenceUpdate> {
    receipt
        .changes
        .iter()
        .filter_map(|change| {
            if change.action != "update_member" || change.entity_type != "member" {
                return None;
            }
            let conversation_id = change.conversation_id.as_ref()?;
            let model_id = change
                .fields
                .iter()
                .find(|field| field.field == "modelConfigId" && field.before != field.after)
                .and_then(|field| field.after.as_str())
                .map(str::to_owned);
            let permission_mode = change
                .fields
                .iter()
                .find(|field| field.field == "permissionMode" && field.before != field.after)
                .and_then(|field| serde_json::from_value(field.after.clone()).ok());
            if model_id.is_none() && permission_mode.is_none() {
                return None;
            }
            Some(mycopilot_core::workflow_execution::PreferenceUpdate {
                node_id: change.entity_id.clone(),
                conversation_id: conversation_id.clone(),
                organization_revision: receipt.organization_revision,
                model_id,
                permission_mode,
            })
        })
        .collect()
}

fn organization_edit_context(
    model_id: Option<&str>,
    effective_permissions: mycopilot_core::AgentPermissions,
    preferences: &mycopilot_core::storage::models::UiPreferencesRecord,
) -> AgentResult<mycopilot_core::organization_personnel::TrustedEditContext> {
    use mycopilot_core::workflow::WorkflowPermissionMode;
    use mycopilot_protocol_rs::AutomationPermissionModeDto;
    let current_model_id = model_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| AgentError::new("organization_current_model_unavailable"))?
        .to_owned();
    let mut allowed_permission_modes = Vec::new();
    let mut default_permission_mode = None;
    for (mode, wire) in [
        (
            WorkflowPermissionMode::Default,
            AutomationPermissionModeDto::Default,
        ),
        (
            WorkflowPermissionMode::Custom,
            AutomationPermissionModeDto::Custom,
        ),
        (
            WorkflowPermissionMode::Full,
            AutomationPermissionModeDto::Full,
        ),
    ] {
        let Ok(resolved) =
            crate::application::automation::permissions::resolve_automation_permissions(
                wire,
                mycopilot_protocol_rs::AUTOMATION_PERMISSION_MODE_VERSION,
                preferences,
            )
        else {
            continue;
        };
        if resolved.permissions.meet(effective_permissions) == resolved.permissions {
            if default_permission_mode.is_none() && resolved.permissions == effective_permissions {
                default_permission_mode = Some(mode.clone());
            }
            allowed_permission_modes.push(mode);
        }
    }
    Ok(mycopilot_core::organization_personnel::TrustedEditContext {
        current_model_id,
        default_permission_mode,
        allowed_permission_modes,
        permission_settings_fingerprint: serde_json::to_string(&(
            preferences.full_permission_enabled,
            preferences.custom_permission_enabled,
            preferences.custom_permissions,
        ))
        .map_err(|error| AgentError::new(error.to_string()))?,
    })
}

impl AgentService {
    // Caller holds conversation_admission until the organization transaction commits.
    fn check_organization_edit_configuration(
        &self,
        conversation_id: &str,
        run_id: &str,
        input: &mycopilot_core::organization_personnel::Input,
    ) -> AgentResult<()> {
        use mycopilot_core::organization_personnel::Action;
        let targets: Vec<&str> = input
            .changes
            .iter()
            .filter_map(|change| match change {
                Action::UpdateMember {
                    member_id,
                    model_config_id,
                    permission_mode,
                    ..
                } if model_config_id.is_some() || permission_mode.is_some() => {
                    Some(member_id.as_str())
                }
                _ => None,
            })
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        let snapshot = self
            .storage
            .workflow_execution_snapshot_for_run(conversation_id, run_id)
            .map_err(AgentError::new)?
            .ok_or_else(|| AgentError::new("organization_management_denied"))?;
        let instance = self
            .storage
            .workflow_request(mycopilot_core::workflow::Request::Manage(
                mycopilot_core::workflow_management::Request::ListInstances {},
            ))
            .map_err(|error| AgentError::new(error.to_string()))?
            .instances
            .into_iter()
            .find(|instance| instance.id == snapshot.instance_id)
            .ok_or_else(|| AgentError::new("organization_management_denied"))?;
        for binding in instance
            .bindings
            .iter()
            .filter(|binding| targets.contains(&binding.node_id.as_str()))
        {
            if self
                .provider_transitions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains_key(&binding.conversation_id)
            {
                return Err(AgentError::new("workflow_configuration_busy"));
            }
            self.ensure_no_manual_context_compaction(&binding.conversation_id)
                .map_err(|_| AgentError::new("workflow_configuration_busy"))?;
        }
        Ok(())
    }

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
            current_model_id: input.model_config_id.clone(),
            effective_permissions: input.context.as_ref().expect("checked above").permissions,
        });
        services
            .with_workflow_runtime(host.clone())
            .with_workflow_inbox(host)
    }

    pub(crate) fn workflow_runtime_snapshot(
        &self,
        instance_id: &str,
        after_sequence: Option<u64>,
        summary_only: bool,
    ) -> Result<RuntimeSnapshot, String> {
        if summary_only {
            self.storage
                .workflow_execution_runtime_summary(instance_id, after_sequence)
        } else {
            self.storage
                .workflow_execution_runtime_since(instance_id, after_sequence)
        }
    }
    pub(super) fn publish_workflow_runtime(
        &self,
        instance_id: &str,
        notifications: &CoreServerNotificationSender,
    ) {
        self.publish_workflow_runtime_with_preferences(instance_id, Vec::new(), notifications);
    }
    fn publish_workflow_runtime_with_preferences(
        &self,
        instance_id: &str,
        preference_updates: Vec<mycopilot_core::workflow_execution::PreferenceUpdate>,
        notifications: &CoreServerNotificationSender,
    ) {
        let Some(preference_updates) = self
            .workflow_runtime_publications
            .enqueue(instance_id, preference_updates)
        else {
            return;
        };
        match self
            .storage
            .workflow_execution_runtime_summary(instance_id, None)
        {
            Ok(mut snapshot) => {
                snapshot.preference_updates = preference_updates;
                let _ = notifications.send(json!({"jsonrpc":"2.0","method":"agent.workflows.runtime.changed","params":snapshot}));
            }
            Err(error) => eprintln!("organization runtime projection failed: {error}"),
        }
    }
    pub(super) fn publish_workflow_delivery_timeline(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        after_sequence: Option<u64>,
        notifications: &CoreServerNotificationSender,
    ) {
        match self
            .storage
            .workflow_execution_delivery_presentations_since(
                conversation_id,
                assistant_message_id,
                after_sequence,
            ) {
            Ok(deliveries) => {
                for delivery in deliveries {
                    let _ = notifications.send(agent_event_notification(delivery.into_event()));
                }
            }
            // The durable trace still supports exact recovery on the next conversation reload.
            Err(error) => eprintln!("organization delivery timeline projection failed: {error}"),
        }
    }

    pub(super) fn acknowledge_workflow_trace<'a>(
        &self,
        items: impl IntoIterator<Item = &'a mycopilot_core::ConversationTurnTraceItem>,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        let mut changed = HashSet::new();
        for item in items {
            if let mycopilot_core::ConversationTurnTraceItem::WorkflowDelivery {
                input_id,
                instance_id,
                ..
            } = item
            {
                if let Some(input) = self
                    .storage
                    .workflow_execution_load_input(input_id)?
                    .filter(|input| {
                        matches!(
                            input.status,
                            mycopilot_core::workflow_execution::InputStatus::Claimed
                                | mycopilot_core::workflow_execution::InputStatus::Failed
                        )
                    })
                {
                    self.storage.workflow_execution_mark_applied(input_id)?;
                    self.workflow_readiness_changed(input.conversation_id.as_deref());
                    changed.insert(instance_id.clone());
                }
            }
        }
        for id in changed {
            self.publish_workflow_runtime(&id, notifications);
        }
        Ok(())
    }
    /// Called only by the single scheduler worker; durable claims retain FIFO/idempotency.
    pub(super) fn dispatch_workflow_deliveries(&self, notifications: CoreServerNotificationSender) {
        #[cfg(test)]
        self.workflow_scheduler_wake
            .scans
            .fetch_add(1, Ordering::Relaxed);
        if self.workflow_dispatch_stopped.load(Ordering::Acquire) {
            return;
        }
        use super::workflow_retry::RetryReason;
        let (cursor, generation) = {
            let mut state = self
                .workflow_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.scanning = true;
            (state.cursor, state.generation)
        };
        let candidates = match self
            .storage
            .workflow_execution_pending_candidates(cursor, 128)
        {
            Ok(inputs) => inputs,
            Err(error) => {
                eprintln!("organization scan failed: {error}");
                self.workflow_retry
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .scan_failed(Instant::now());
                return;
            }
        };
        self.workflow_retry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .scan_succeeded();
        for candidate in &candidates {
            if self.workflow_dispatch_stopped.load(Ordering::Acquire) {
                break;
            }
            if !self
                .workflow_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_due(candidate, Instant::now())
            {
                continue;
            }
            let attempt = || -> Result<Option<RetryReason>, AgentServiceError> {
                let Some(conversation_id) = candidate.conversation_id.as_deref() else {
                    return Ok(Some(RetryReason::Unavailable));
                };
                if self.has_conversation_turn_occupancy(conversation_id)? {
                    return Ok(Some(RetryReason::Busy));
                }
                if self.check_automation_execution_access().is_err() {
                    return Ok(Some(RetryReason::Access));
                }
                let Some(input) = self
                    .storage
                    .workflow_execution_eligible_pending_input(&candidate.id)?
                else {
                    return Ok(Some(RetryReason::Unavailable));
                };
                // Version and bindings may have changed since the lightweight page was read.
                if input.execution_version != candidate.execution_version {
                    return Ok(None);
                }
                match self.start_workflow_input(&input, notifications.clone()) {
                    Ok(result) => Ok(result),
                    Err(error) => {
                        if error
                            .data()
                            .is_some_and(|data| data["code"] == "workflow_capacity_exhausted")
                        {
                            return Ok(Some(RetryReason::Capacity));
                        }
                        if self
                            .storage
                            .workflow_execution_load_input(&input.id)?
                            .is_some_and(|current| current.status != input.status)
                        {
                            eprintln!("organization delivery deferred: {error}");
                            self.publish_workflow_runtime(&input.instance_id, &notifications);
                            Ok(None)
                        } else {
                            Err(error)
                        }
                    }
                }
            };
            let reason = match attempt() {
                Ok(reason) => reason,
                Err(error) => {
                    eprintln!(
                        "organization input {} deferred (transient): {error}",
                        candidate.id
                    );
                    Some(RetryReason::Transient)
                }
            };
            // Queued recipients can prepare their stable history while waiting for capacity or
            // configuration. An immediately startable letter goes straight to admission instead;
            // preparation never claims pending mail or becomes a scheduler dependency.
            if matches!(
                reason,
                Some(RetryReason::Capacity | RetryReason::Configuration)
            ) {
                if let Some(conversation_id) = candidate.conversation_id.as_deref() {
                    self.enqueue_history_warmup(conversation_id);
                }
            }
            let mut state = self
                .workflow_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(reason) = reason {
                state.defer(candidate, reason, generation, Instant::now());
            } else {
                state.forget(&candidate.id);
            }
        }
        let continue_scan = {
            let mut state = self
                .workflow_retry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.scanning = false;
            if candidates.len() == 128 {
                state.cursor = candidates.last().unwrap().sequence;
                true
            } else {
                state.cursor = 0;
                state.complete_cycle();
                std::mem::take(&mut state.restart_requested)
            }
        };
        // Yield between bounded pages, but finish the complete population promptly. Advancing
        // despite disabled/busy heads prevents the first 128 failures from hiding other owners.
        if continue_scan {
            self.wake_workflow_deliveries();
        }
    }
    fn start_workflow_input(
        &self,
        workflow: &Input,
        notifications: CoreServerNotificationSender,
    ) -> Result<Option<super::workflow_retry::RetryReason>, AgentServiceError> {
        use super::workflow_retry::RetryReason;
        let conversation_id = workflow
            .conversation_id
            .as_deref()
            .ok_or_else(|| "Organization recipient is not a conversation.".to_string())?;
        let conversation = self
            .storage
            .load_conversation_meta(conversation_id)?
            .ok_or_else(|| "Organization recipient was removed.".to_string())?;
        if conversation.archived_at.is_some() {
            return Err("Organization recipient is archived.".to_string().into());
        }
        let Some(draft) = self.storage.load_composer_draft(conversation_id)? else {
            return Ok(Some(RetryReason::Configuration));
        };
        let Some(model_id) = draft.model_id.or(conversation.model_id) else {
            return Ok(Some(RetryReason::Configuration));
        };
        // Missing model configuration is a common recoverable wait. Avoid history preflight
        // until the authoritative settings snapshot actually exists; admission rechecks it.
        let settings = match self
            .storage
            .load_model_settings_snapshot_for_model(&model_id, false)
        {
            Ok(Some(settings)) => settings,
            Ok(None) | Err(_) => return Ok(Some(RetryReason::Configuration)),
        };
        let Some(model) = settings
            .settings
            .models
            .iter()
            .find(|model| model.id == model_id && model.enabled)
        else {
            return Ok(Some(RetryReason::Configuration));
        };
        if settings.settings.effective_connection_for(model).is_err() {
            return Ok(Some(RetryReason::Configuration));
        }
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
                for_send: true,
                conversation_id: conversation_id.into(),
                target_model_id: model_id.clone(),
            })?;
        if transition.decision == AgentProviderTransitionDecision::RequiresCompaction {
            self.start_workflow_provider_transition(
                AgentProviderTransitionStartInput {
                    conversation_id: conversation_id.into(),
                    target_model_id: model_id,
                    transition_token: transition
                        .transition_token
                        .ok_or_else(|| "Provider transition token missing.".to_string())?,
                },
                notifications,
            )?;
            return Ok(Some(RetryReason::Transition));
        }
        if transition.decision == AgentProviderTransitionDecision::Blocked {
            return Ok(Some(RetryReason::Transition));
        }
        // ReadyForSend is the metadata-only same-protocol fast path. Compatible is returned
        // only after a necessary full compatibility preflight. Real adaptations above retain
        // their transition token and compaction admission; warming history cannot bypass them.
        if !matches!(
            transition.decision,
            AgentProviderTransitionDecision::ReadyForSend
                | AgentProviderTransitionDecision::Compatible
        ) {
            return Ok(Some(RetryReason::Transition));
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
        Ok(None)
    }
}

struct WorkflowPreview {
    service: AgentService,
    conversation_id: String,
    run_id: Option<String>,
}
impl WorkflowRuntimeHost for WorkflowPreview {
    fn request_observation(&self) -> AgentResult<Option<(ConversationSnapshot, Value)>> {
        self.service
            .storage
            .workflow_execution_request_observation(&self.conversation_id, self.run_id.as_deref())
            .map_err(AgentError::new)
    }
    fn snapshot(&self) -> AgentResult<Option<ConversationSnapshot>> {
        match &self.run_id {
            Some(run_id) => self
                .service
                .storage
                .workflow_execution_snapshot_for_run(&self.conversation_id, run_id),
            None => self
                .service
                .storage
                .workflow_execution_snapshot(&self.conversation_id),
        }
        .map_err(AgentError::new)
    }
    fn state(&self, _query: StateQuery) -> AgentResult<Value> {
        Err(AgentError::new(
            "An organization preview cannot execute model tools.",
        ))
    }
    fn mailbox(&self, _query: MailboxQuery) -> AgentResult<Value> {
        Err(AgentError::new(
            "An organization preview cannot execute model tools.",
        ))
    }
    fn awareness(&self) -> AgentResult<Value> {
        let mut awareness = match &self.run_id {
            Some(run_id) => self
                .service
                .storage
                .workflow_execution_awareness_for_run(&self.conversation_id, run_id),
            None => self
                .service
                .storage
                .workflow_execution_awareness_for_conversation(&self.conversation_id),
        }
        .map_err(AgentError::new)?;
        self.service
            .enrich_workflow_awareness(&mut awareness)
            .map_err(AgentError::new)?;
        Ok(awareness)
    }
    fn send(&self, _invocation: WorkflowSendInvocation) -> AgentResult<SendReceipt> {
        Err(AgentError::new(
            "A read-only organization preview cannot send messages.",
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
            service: self.clone(),
            conversation_id: conversation_id.into(),
            run_id: run_id.map(str::to_string),
        })
    }
}

#[cfg(test)]
mod organization_permission_tests {
    use super::*;
    use mycopilot_core::workflow::WorkflowPermissionMode;

    #[test]
    fn organization_edit_context_uses_exact_run_grant_and_rejects_unrepresentable_inheritance() {
        let directory = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&directory.path().join("permissions.sqlite")).unwrap();
        let mut preferences = storage.load_ui_preferences().unwrap();
        preferences.full_permission_enabled = true;
        preferences.custom_permission_enabled = true;
        preferences.custom_permissions = mycopilot_core::AgentPermissions {
            read: mycopilot_core::AgentReadPermission::All,
            write: mycopilot_core::AgentWritePermission::All,
            ..mycopilot_core::AgentPermissions::default()
        };
        let actual = mycopilot_core::AgentPermissions {
            write: mycopilot_core::AgentWritePermission::WorkspaceOnly,
            ..mycopilot_core::AgentPermissions::default()
        };
        let context =
            organization_edit_context(Some("actual-local-config"), actual, &preferences).unwrap();
        assert_eq!(context.current_model_id, "actual-local-config");
        assert_eq!(
            context.default_permission_mode,
            Some(WorkflowPermissionMode::Default)
        );
        assert_eq!(
            context.allowed_permission_modes,
            vec![WorkflowPermissionMode::Default]
        );
        let restricted = organization_edit_context(
            Some("actual-local-config"),
            mycopilot_core::AgentPermissions::default(),
            &preferences,
        )
        .unwrap();
        assert!(restricted.default_permission_mode.is_none());
        assert!(restricted.allowed_permission_modes.is_empty());
        preferences.custom_permissions = actual;
        let same =
            organization_edit_context(Some("actual-local-config"), actual, &preferences).unwrap();
        assert!(same
            .allowed_permission_modes
            .contains(&WorkflowPermissionMode::Custom));
        assert_ne!(
            context.permission_settings_fingerprint,
            same.permission_settings_fingerprint
        );
        assert!(organization_edit_context(None, actual, &preferences).is_err());
    }
}
