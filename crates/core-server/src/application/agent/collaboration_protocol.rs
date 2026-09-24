use super::*;
use mycopilot_core::{
    AgentCollaborationActivitySemantic, AgentCollaborationEventKind, AgentCollaborationEventRecord,
    AgentCollaborationTransmissionKind, AgentDisplayStatus, AgentLifecycle, AgentNodeRecord,
    AgentTemplateRecord, ConversationMessageOrigin,
};
use mycopilot_protocol_rs::{
    AgentConversationLocatorDto, AgentConversationLocatorRequest, AgentConversationModeDto,
    AgentDetailDto, AgentDetailRequest, AgentDisplayStatusDto, AgentLifecycleDto,
    AgentModelDisplayDto, AgentObserverAttachmentDto, AgentObserverConversationDto,
    AgentObserverConversationRequest, AgentObserverInputOriginDto, AgentObserverInputOriginKindDto,
    AgentObserverMessageDto, AgentSummaryDto, AgentTemplateBindingDto, AgentTemplateCreateRequest,
    AgentTemplateDeleteRequest, AgentTemplateDto, AgentTemplateListDto, AgentTemplateListRequest,
    AgentTemplateProjectAssignmentRequest, AgentTemplateSetEnabledRequest,
    AgentTemplateUpdateRequest, AgentTreeLookupDto, AgentTreeRequest, AgentTreeSnapshotDto,
    CollaborationActivitySemanticDto, CollaborationActivitySnapshotDto,
    CollaborationApprovalDecisionDto, CollaborationApprovalDecisionRequest,
    CollaborationApprovalDecisionResultDto, CollaborationApprovalListDto,
    CollaborationApprovalListRequest, CollaborationApprovalProjectionDto,
    CollaborationApprovalStatusDto, CollaborationEventEnvelopeDto, CollaborationEventKindDto,
    CollaborationEventsPageDto, CollaborationEventsRequest, CollaborationTransmissionDto,
    CollaborationTransmissionKindDto, AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION,
    AGENT_COLLABORATION_EVENT_SCHEMA_VERSION, AGENT_COLLABORATION_SCHEMA_VERSION,
};

const TREE_LIMIT: usize = 1_024;

impl AgentService {
    pub fn get_collaboration_tree(
        &self,
        input: AgentTreeRequest,
    ) -> Result<AgentTreeLookupDto, AgentServiceError> {
        let root = match self
            .storage
            .get_agent_node_by_conversation(&input.root_conversation_id)
            .map_err(safe_graph_error)?
        {
            None => {
                return Ok(AgentTreeLookupDto {
                    schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
                    materialized: false,
                    tree: None,
                });
            }
            Some(node) if node.parent_agent_id.is_none() => node,
            Some(_) => {
                return Err(AgentServiceError::from(
                    "Agent tree is unavailable.".to_string(),
                ))
            }
        };
        // Capture a conservative replay cursor before reading the multi-query tree snapshot. A
        // concurrent mutation may make the snapshot newer than this cursor, but can never make the
        // cursor acknowledge state that the snapshot did not observe.
        let last_sequence = self
            .storage
            .latest_agent_collaboration_event_sequence(&root.root_agent_id)
            .map_err(safe_event_error)?;
        let nodes = self
            .storage
            .list_agent_tree(&root.root_agent_id)
            .map_err(safe_graph_error)?;
        if nodes.len() > TREE_LIMIT {
            return Err(AgentServiceError::from(
                "Agent tree exceeds the supported response limit.".to_string(),
            ));
        }
        let agents = nodes
            .iter()
            .map(|node| self.agent_summary(node))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AgentTreeLookupDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            materialized: true,
            tree: Some(AgentTreeSnapshotDto {
                schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
                workspace_id: root.project_id.clone(),
                project_id: root.project_id,
                root_agent_id: root.root_agent_id,
                root_conversation_id: root.root_conversation_id,
                agents,
                last_sequence,
            }),
        })
    }

    pub fn get_collaboration_agent(
        &self,
        input: AgentDetailRequest,
    ) -> Result<AgentDetailDto, AgentServiceError> {
        let node = self.authorized_node(&input.root_conversation_id, &input.agent_id)?;
        let summary = self.agent_summary(&node)?;
        Ok(AgentDetailDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            summary,
            template: node
                .template_snapshot
                .as_ref()
                .map(|template| AgentTemplateBindingDto {
                    template_id: template.template_id.clone(),
                    machine_key: template.machine_key.clone(),
                    name: template.name.clone(),
                    description: template.description.clone(),
                    revision: template.template_revision,
                }),
            reasoning_effort: node.reasoning_effort_snapshot.map(|effort| {
                match effort {
                    mycopilot_core::ReasoningEffort::ProviderDefault => "provider_default",
                    mycopilot_core::ReasoningEffort::High => "high",
                    mycopilot_core::ReasoningEffort::Max => "max",
                }
                .to_string()
            }),
            revision: node.revision,
            created_at: node.created_at,
            updated_at: node.updated_at,
        })
    }

    pub fn locate_collaboration_conversation(
        &self,
        input: AgentConversationLocatorRequest,
    ) -> Result<AgentConversationLocatorDto, AgentServiceError> {
        let node = self.authorized_node(&input.root_conversation_id, &input.agent_id)?;
        Ok(AgentConversationLocatorDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            agent_id: node.agent_id,
            conversation_id: node.conversation_id,
            mode: if node.parent_agent_id.is_some() {
                AgentConversationModeDto::Observer
            } else {
                AgentConversationModeDto::Interactive
            },
        })
    }

    pub fn load_collaboration_observer_conversation(
        &self,
        input: AgentObserverConversationRequest,
    ) -> Result<Option<AgentObserverConversationDto>, AgentServiceError> {
        let node = self.authorize_exact_child_observer_read(
            &input.root_conversation_id,
            &input.conversation_id,
        )?;
        let observer_streams = self
            .observer_streams
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(snapshot) = self
            .storage
            .load_conversation_observer_snapshot(&input.conversation_id)
            .map_err(AgentServiceError::from)?
        else {
            return Ok(None);
        };
        let conversation = snapshot.conversation;
        let live_stream = observer_streams
            .get(&input.conversation_id)
            .filter(|state| {
                state.agent_id == node.agent_id
                    && state.root_agent_id == node.root_agent_id
                    && state.root_conversation_id == node.root_conversation_id
                    && conversation.messages.iter().any(|message| {
                        message.id == state.snapshot.assistant_message_id
                            && message.role == "assistant"
                    })
            })
            .map(|state| state.snapshot.clone());
        drop(observer_streams);
        let input_origins = snapshot.input_origins;
        let messages = conversation
            .messages
            .iter()
            .map(|message| {
                let input_origin = if message.role == "user" {
                    Some(observer_origin_dto(
                        input_origins.get(&message.id).cloned().ok_or_else(|| {
                            AgentServiceError::from(
                                "Agent observer input provenance is unavailable.".to_string(),
                            )
                        })?,
                    ))
                } else {
                    None
                };
                Ok(AgentObserverMessageDto {
                    message_id: message.id.clone(),
                    role: message.role.clone(),
                    content: message.content.clone(),
                    created_at: message.created_at,
                    status: message.status.clone(),
                    input_origin,
                    human_interaction_response: message
                        .human_interaction_response
                        .as_ref()
                        .map(observer_human_response_dto),
                    attachments: message
                        .attachments
                        .iter()
                        .map(|attachment| AgentObserverAttachmentDto {
                            attachment_id: attachment.id.clone(),
                            kind: attachment.kind.clone(),
                            name: attachment.name.clone(),
                            mime_type: attachment.mime_type.clone(),
                            size_bytes: attachment.size_bytes,
                            preview_data: attachment.preview_data.clone(),
                            preview_mime_type: attachment.preview_mime_type.clone(),
                            created_at: attachment.created_at,
                        })
                        .collect(),
                    agent_run_json: message.agent_run_json.clone(),
                    ui_state_json: message.ui_state_json.clone(),
                })
            })
            .collect::<Result<Vec<_>, AgentServiceError>>()?;
        Ok(Some(AgentObserverConversationDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            agent_id: node.agent_id,
            root_conversation_id: node.root_conversation_id,
            conversation_id: conversation.id,
            project_id: conversation.project_id,
            model_id: conversation.model_id,
            title: conversation.title,
            created_at: conversation.created_at,
            updated_at: conversation.updated_at,
            messages,
            live_stream,
        }))
    }

    pub fn list_collaboration_events(
        &self,
        input: CollaborationEventsRequest,
    ) -> Result<CollaborationEventsPageDto, AgentServiceError> {
        let root = self.authorized_root(&input.root_conversation_id)?;
        if input.limit == 0 || input.limit > 512 {
            return Err(AgentServiceError::from(
                "Collaboration event page limit must be between 1 and 512.".to_string(),
            ));
        }
        let requested = usize::try_from(input.limit).unwrap_or(512);
        let fetched = self
            .storage
            .list_agent_collaboration_events(&root.root_agent_id, input.after_sequence, requested)
            .map_err(safe_event_error)?;
        let latest_sequence = self
            .storage
            .latest_agent_collaboration_event_sequence(&root.root_agent_id)
            .map_err(safe_event_error)?;
        let events = fetched.into_iter().map(event_dto).collect::<Vec<_>>();
        let last_sequence = events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(input.after_sequence);
        let has_more = last_sequence < latest_sequence;
        Ok(CollaborationEventsPageDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            root_agent_id: root.root_agent_id,
            root_conversation_id: root.root_conversation_id,
            events,
            last_sequence,
            has_more,
        })
    }

    pub fn list_collaboration_templates(
        &self,
        input: AgentTemplateListRequest,
    ) -> Result<AgentTemplateListDto, AgentServiceError> {
        let models = safe_models(&self.storage)?;
        let templates = self
            .storage
            .list_agent_templates(input.include_disabled)
            .map_err(safe_template_error)?
            .into_iter()
            .map(|record| template_dto(record, &models))
            .collect();
        Ok(AgentTemplateListDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            templates,
        })
    }

    pub fn create_collaboration_template(
        &self,
        input: AgentTemplateCreateRequest,
    ) -> Result<AgentTemplateDto, AgentServiceError> {
        let record = self
            .storage
            .create_agent_template(&mycopilot_core::CreateAgentTemplateInput {
                template_id: input.template_id,
                machine_key: input.machine_key,
                name: input.name,
                description: input.description,
                instructions: input.instructions,
                model_config_id: input.model_config_id,
                enabled: input.enabled,
            })
            .map_err(safe_template_error)?;
        Ok(template_dto(record, &safe_models(&self.storage)?))
    }

    pub fn update_collaboration_template(
        &self,
        input: AgentTemplateUpdateRequest,
    ) -> Result<AgentTemplateDto, AgentServiceError> {
        let record = self
            .storage
            .update_agent_template(&mycopilot_core::UpdateAgentTemplateInput {
                template_id: input.template_id,
                expected_revision: input.expected_revision,
                name: input.name,
                description: input.description,
                instructions: input.instructions,
                model_config_id: input.model_config_id,
            })
            .map_err(safe_template_error)?;
        Ok(template_dto(record, &safe_models(&self.storage)?))
    }

    pub fn set_collaboration_template_enabled(
        &self,
        input: AgentTemplateSetEnabledRequest,
    ) -> Result<AgentTemplateDto, AgentServiceError> {
        let record = self
            .storage
            .set_agent_template_enabled(&input.template_id, input.expected_revision, input.enabled)
            .map_err(safe_template_error)?;
        Ok(template_dto(record, &safe_models(&self.storage)?))
    }

    pub fn delete_collaboration_template(
        &self,
        input: AgentTemplateDeleteRequest,
    ) -> Result<AgentTemplateDto, AgentServiceError> {
        let models = safe_models(&self.storage)?;
        let record = self
            .storage
            .delete_agent_template(&input.template_id, input.expected_revision)
            .map_err(safe_template_error)?;
        Ok(template_dto(record, &models))
    }

    pub fn set_collaboration_template_project_assignment(
        &self,
        input: AgentTemplateProjectAssignmentRequest,
    ) -> Result<AgentTemplateDto, AgentServiceError> {
        let record = self
            .storage
            .set_agent_template_project_assignment(
                &input.project_id,
                &input.template_id,
                input.assigned,
            )
            .map_err(safe_template_error)?;
        Ok(template_dto(record, &safe_models(&self.storage)?))
    }

    pub fn list_collaboration_approvals(
        &self,
        input: CollaborationApprovalListRequest,
    ) -> Result<CollaborationApprovalListDto, AgentServiceError> {
        let approvals = self
            .list_root_projected_approvals(&input.root_conversation_id)
            .map_err(AgentServiceError::from)?
            .into_iter()
            .map(|approval| {
                let durable = self
                    .storage
                    .get_pending_agent_action(&approval.approval_id)
                    .map_err(AgentServiceError::from)?;
                let public = serde_json::to_value(&approval.action).map_err(|_| {
                    AgentServiceError::from("Approval projection is unavailable.".to_string())
                })?;
                let action = public.get("action").cloned().ok_or_else(|| {
                    AgentServiceError::from("Approval projection is unavailable.".to_string())
                })?;
                Ok(CollaborationApprovalProjectionDto {
                    schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
                    approval_id: approval.approval_id,
                    root_agent_id: approval.root_agent_id,
                    root_conversation_id: approval.root_conversation_id,
                    source_agent_id: approval.source_agent_id,
                    source_task_path: approval.source_task_path,
                    source_conversation_id: approval.source_conversation_id,
                    run_id: approval.action.run_id,
                    action_id: approval.action.action_id,
                    action_type: approval.action.action_type,
                    tool_name: approval.action.tool_name,
                    action,
                    status: approval_status_dto(super::pending_status_label(
                        approval.action.status,
                    )),
                    created_at: approval.action.created_at,
                    updated_at: durable
                        .map(|row| row.updated_at)
                        .unwrap_or(approval.action.created_at),
                })
            })
            .collect::<Result<Vec<_>, AgentServiceError>>()?;
        Ok(CollaborationApprovalListDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            approvals,
        })
    }

    pub fn decide_collaboration_approval(
        &self,
        input: CollaborationApprovalDecisionRequest,
        notifications: CoreServerNotificationSender,
    ) -> Result<CollaborationApprovalDecisionResultDto, AgentServiceError> {
        let result = self
            .decide_root_projected_approval(
                &input.root_conversation_id,
                &input.approval_id,
                match input.decision {
                    CollaborationApprovalDecisionDto::Approve => ProjectedApprovalDecision::Approve,
                    CollaborationApprovalDecisionDto::Reject => ProjectedApprovalDecision::Reject,
                    CollaborationApprovalDecisionDto::Cancel => ProjectedApprovalDecision::Cancel,
                },
                input.message,
                notifications,
            )
            .map_err(AgentServiceError::from)?;
        Ok(CollaborationApprovalDecisionResultDto {
            schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
            approval_id: result.approval_id,
            accepted: result.accepted,
            already_settled: !result.accepted,
            status: approval_status_dto(&result.status),
        })
    }

    fn authorized_root(
        &self,
        root_conversation_id: &str,
    ) -> Result<AgentNodeRecord, AgentServiceError> {
        self.collaboration_authorizer
            .authorize_root_conversation(root_conversation_id)
            .map_err(safe_authorization_error)
    }

    fn authorized_node(
        &self,
        root_conversation_id: &str,
        agent_id: &str,
    ) -> Result<AgentNodeRecord, AgentServiceError> {
        let root = self.authorized_root(root_conversation_id)?;
        let node = self
            .storage
            .get_agent_node(agent_id)
            .map_err(safe_graph_error)?
            .ok_or_else(|| AgentServiceError::from("Agent is unavailable.".to_string()))?;
        if node.root_agent_id != root.agent_id
            || node.root_conversation_id != root.root_conversation_id
            || node.project_id != root.project_id
        {
            return Err(AgentServiceError::from("Agent is unavailable.".to_string()));
        }
        Ok(node)
    }

    fn agent_summary(&self, node: &AgentNodeRecord) -> Result<AgentSummaryDto, AgentServiceError> {
        let display = self
            .storage
            .get_agent_display_status(&node.agent_id)
            .map_err(safe_graph_error)?;
        let model = match &node.model_snapshot {
            Some(model) => Some(AgentModelDisplayDto {
                model_config_id: model.model_config_id.clone(),
                display_name: model.display_name.clone(),
            }),
            None => {
                let conversation = self
                    .storage
                    .load_conversation(&node.conversation_id)
                    .map_err(AgentServiceError::from)?
                    .ok_or_else(|| {
                        AgentServiceError::from("Conversation is unavailable.".to_string())
                    })?;
                let models = safe_models(&self.storage)?;
                conversation.model_id.and_then(|model_id| {
                    models
                        .iter()
                        .find(|model| model.id == model_id)
                        .map(|model| AgentModelDisplayDto {
                            model_config_id: model.id.clone(),
                            display_name: model.display_label(),
                        })
                })
            }
        };
        let latest_activity_at = self
            .storage
            .latest_agent_collaboration_activity_at(&node.root_agent_id, &node.agent_id)
            .map_err(safe_event_error)?
            .unwrap_or(node.updated_at);
        Ok(AgentSummaryDto {
            agent_id: node.agent_id.clone(),
            root_agent_id: node.root_agent_id.clone(),
            root_conversation_id: node.root_conversation_id.clone(),
            parent_agent_id: node.parent_agent_id.clone(),
            conversation_id: node.conversation_id.clone(),
            project_id: node.project_id.clone(),
            task_name: node.task_name.clone(),
            task_path: node.task_path.clone(),
            lifecycle: lifecycle_dto(node.lifecycle),
            display_status: display_status_dto(display.status),
            latest_activity_at,
            model,
        })
    }
}

fn observer_origin_dto(value: ConversationMessageOrigin) -> AgentObserverInputOriginDto {
    match value {
        ConversationMessageOrigin::Human => AgentObserverInputOriginDto {
            kind: AgentObserverInputOriginKindDto::Human,
            sender_agent_id: None,
            source_agent_message_id: None,
            snapshot_source_conversation_id: None,
            snapshot_source_message_id: None,
        },
        ConversationMessageOrigin::Agent {
            sender_agent_id,
            source_agent_message_id,
        } => AgentObserverInputOriginDto {
            kind: AgentObserverInputOriginKindDto::Agent,
            sender_agent_id: Some(sender_agent_id),
            source_agent_message_id: Some(source_agent_message_id),
            snapshot_source_conversation_id: None,
            snapshot_source_message_id: None,
        },
        ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id,
            source_message_id,
            original,
        } => {
            let original = observer_origin_dto(*original);
            AgentObserverInputOriginDto {
                kind: AgentObserverInputOriginKindDto::HistoricalSnapshot,
                sender_agent_id: original.sender_agent_id,
                source_agent_message_id: original.source_agent_message_id,
                snapshot_source_conversation_id: Some(source_conversation_id),
                snapshot_source_message_id: Some(source_message_id),
            }
        }
    }
}

fn lifecycle_dto(value: AgentLifecycle) -> AgentLifecycleDto {
    match value {
        AgentLifecycle::Active => AgentLifecycleDto::Active,
        AgentLifecycle::Archived => AgentLifecycleDto::Archived,
        AgentLifecycle::Disabled => AgentLifecycleDto::Disabled,
    }
}

fn display_status_dto(value: AgentDisplayStatus) -> AgentDisplayStatusDto {
    match value {
        AgentDisplayStatus::Idle => AgentDisplayStatusDto::Idle,
        AgentDisplayStatus::Queued => AgentDisplayStatusDto::Queued,
        AgentDisplayStatus::Running => AgentDisplayStatusDto::Running,
        AgentDisplayStatus::WaitingApproval => AgentDisplayStatusDto::WaitingApproval,
        AgentDisplayStatus::LatestCompleted => AgentDisplayStatusDto::LatestCompleted,
        AgentDisplayStatus::LatestFailed => AgentDisplayStatusDto::LatestFailed,
        AgentDisplayStatus::LatestInterrupted => AgentDisplayStatusDto::LatestInterrupted,
        AgentDisplayStatus::LatestOutcomeUnknown => AgentDisplayStatusDto::LatestOutcomeUnknown,
        AgentDisplayStatus::Archived => AgentDisplayStatusDto::Archived,
        AgentDisplayStatus::Disabled => AgentDisplayStatusDto::Disabled,
    }
}

pub(crate) fn event_dto(record: AgentCollaborationEventRecord) -> CollaborationEventEnvelopeDto {
    debug_assert_eq!(
        record.schema_version,
        AGENT_COLLABORATION_EVENT_SCHEMA_VERSION
    );
    CollaborationEventEnvelopeDto {
        schema_version: record.schema_version,
        event_id: record.event_id,
        sequence: record.root_sequence,
        workspace_id: record.workspace_id,
        project_id: record.project_id,
        root_agent_id: record.root_agent_id,
        root_conversation_id: record.root_conversation_id,
        agent_id: record.agent_id,
        conversation_id: record.conversation_id,
        turn_id: record.turn_id,
        run_id: record.run_id,
        message_id: record.message_id,
        kind: match record.kind {
            AgentCollaborationEventKind::AgentCreated => CollaborationEventKindDto::AgentCreated,
            AgentCollaborationEventKind::AgentUpdated => CollaborationEventKindDto::AgentUpdated,
            AgentCollaborationEventKind::MailboxEnqueued => {
                CollaborationEventKindDto::MailboxEnqueued
            }
            AgentCollaborationEventKind::MailboxUpdated => {
                CollaborationEventKindDto::MailboxUpdated
            }
            AgentCollaborationEventKind::WakeCreated => CollaborationEventKindDto::WakeCreated,
            AgentCollaborationEventKind::WakeUpdated => CollaborationEventKindDto::WakeUpdated,
            AgentCollaborationEventKind::TurnStarted => CollaborationEventKindDto::TurnStarted,
            AgentCollaborationEventKind::TurnUpdated => CollaborationEventKindDto::TurnUpdated,
            AgentCollaborationEventKind::ApprovalProjected => {
                CollaborationEventKindDto::ApprovalProjected
            }
            AgentCollaborationEventKind::ApprovalUpdated => {
                CollaborationEventKindDto::ApprovalUpdated
            }
        },
        resource_revision: record.resource_revision,
        activity: record
            .activity
            .map(|activity| CollaborationActivitySnapshotDto {
                schema_version: AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION,
                semantic: match activity.semantic {
                    AgentCollaborationActivitySemantic::Started => {
                        CollaborationActivitySemanticDto::Started
                    }
                    AgentCollaborationActivitySemantic::Updated => {
                        CollaborationActivitySemanticDto::Updated
                    }
                    AgentCollaborationActivitySemantic::WaitingApproval => {
                        CollaborationActivitySemanticDto::WaitingApproval
                    }
                    AgentCollaborationActivitySemantic::Completed => {
                        CollaborationActivitySemanticDto::Completed
                    }
                    AgentCollaborationActivitySemantic::Failed => {
                        CollaborationActivitySemanticDto::Failed
                    }
                    AgentCollaborationActivitySemantic::Interrupted => {
                        CollaborationActivitySemanticDto::Interrupted
                    }
                },
                agent_id: activity.agent_id,
                task_name_snapshot: activity.task_name_snapshot,
                parent_agent_id: activity.parent_agent_id,
                parent_conversation_id: activity.parent_conversation_id,
                anchor_message_id: activity.anchor_message_id,
                trace_boundary_sequence: activity.trace_boundary_sequence,
            }),
        transmission: record
            .transmission
            .map(|transmission| CollaborationTransmissionDto {
                id: transmission.id,
                kind: match transmission.kind {
                    AgentCollaborationTransmissionKind::Message => {
                        CollaborationTransmissionKindDto::Message
                    }
                    AgentCollaborationTransmissionKind::Task => {
                        CollaborationTransmissionKindDto::Task
                    }
                    AgentCollaborationTransmissionKind::UserMessage => {
                        CollaborationTransmissionKindDto::UserMessage
                    }
                    AgentCollaborationTransmissionKind::Completion => {
                        CollaborationTransmissionKindDto::Completion
                    }
                },
                source_agent_id: transmission.source_agent_id,
                target_agent_id: transmission.target_agent_id,
            }),
        occurred_at: record.created_at,
    }
}

fn safe_models(
    storage: &StorageService,
) -> Result<Vec<mycopilot_core::storage::models::ModelConfigRecord>, AgentServiceError> {
    Ok(storage
        .load_model_settings_catalog()
        .map_err(AgentServiceError::from)?
        .map(|settings| settings.models)
        .unwrap_or_default())
}

fn template_dto(
    record: AgentTemplateRecord,
    models: &[mycopilot_core::storage::models::ModelConfigRecord],
) -> AgentTemplateDto {
    let model_display_name = models
        .iter()
        .find(|model| model.id == record.model_config_id)
        .map(|model| model.display_label());
    AgentTemplateDto {
        schema_version: AGENT_COLLABORATION_SCHEMA_VERSION,
        template_id: record.template_id,
        project_ids: record.project_ids,
        machine_key: record.machine_key,
        name: record.name,
        description: record.description,
        instructions: record.instructions,
        model_config_id: record.model_config_id,
        model_display_name,
        enabled: record.enabled,
        revision: record.revision,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn safe_authorization_error(
    _: crate::application::collaboration_authorization::CollaborationAuthorizationError,
) -> AgentServiceError {
    AgentServiceError::from("Agent collaboration operation is not authorized.".to_string())
}

fn observer_human_response_dto(
    display: &mycopilot_core::human_interaction::HumanInteractionResponseDisplay,
) -> mycopilot_protocol_rs::HumanInteractionResponseDisplayDto {
    use mycopilot_core::human_interaction::HumanInteractionAnswerDisplay as Core;
    use mycopilot_protocol_rs::HumanInteractionAnswerDisplayDto as Wire;
    mycopilot_protocol_rs::HumanInteractionResponseDisplayDto {
        result_type: display.result_type.clone(),
        schema_version: display.schema_version,
        request_id: display.request_id.clone(),
        response_id: display.response_id.clone(),
        answers: display
            .answers
            .iter()
            .map(|answer| match answer {
                Core::Option {
                    question_id,
                    question,
                    option_id,
                    answer,
                } => Wire::Option {
                    question_id: question_id.clone(),
                    question: question.clone(),
                    option_id: option_id.clone(),
                    answer: answer.clone(),
                },
                Core::Text {
                    question_id,
                    question,
                    answer,
                } => Wire::Text {
                    question_id: question_id.clone(),
                    question: question.clone(),
                    answer: answer.clone(),
                },
                Core::Skipped {
                    question_id,
                    question,
                    answer,
                } => Wire::Skipped {
                    question_id: question_id.clone(),
                    question: question.clone(),
                    answer: answer.clone(),
                },
            })
            .collect(),
    }
}

#[cfg(test)]
#[test]
fn observer_human_response_preserves_validated_history_display_without_reparsing_content() {
    let display: mycopilot_core::human_interaction::HumanInteractionResponseDisplay =
        serde_json::from_value(serde_json::json!({
            "type":"human_interaction_response", "schemaVersion":1,
            "requestId":"request-observer", "responseId":"response-observer",
            "answers":[
                {"questionId":"option", "question":"Choose format", "kind":"option", "optionId":"csv", "answer":"CSV"},
                {"questionId":"text", "question":"Complete question. ".repeat(150), "kind":"text", "answer":"Complete answer. ".repeat(1500)},
                {"questionId":"skip", "question":"Anything else?", "kind":"skipped", "answer":"已跳过"}
            ]
        })).unwrap();
    display.validate().unwrap();
    assert_eq!(
        serde_json::to_value(observer_human_response_dto(&display)).unwrap(),
        serde_json::to_value(display).unwrap()
    );
    let ordinary: AgentObserverMessageDto = serde_json::from_value(serde_json::json!({
        "messageId":"ordinary-json", "role":"user", "content":"{\"type\":\"human_interaction_response\"}",
        "createdAt":1, "status":"sent", "inputOrigin":null, "attachments":[], "agentRunJson":null, "uiStateJson":null
    })).unwrap();
    assert!(ordinary.human_interaction_response.is_none());
    assert!(serde_json::to_value(ordinary)
        .unwrap()
        .get("humanInteractionResponse")
        .is_none());
}

fn safe_graph_error(_: mycopilot_core::AgentGraphError) -> AgentServiceError {
    AgentServiceError::from("Agent collaboration state is unavailable.".to_string())
}

fn safe_event_error(_: mycopilot_core::AgentCollaborationEventError) -> AgentServiceError {
    AgentServiceError::from("Agent collaboration events are unavailable.".to_string())
}

fn safe_template_error(_: mycopilot_core::AgentTemplateError) -> AgentServiceError {
    AgentServiceError::from("Agent template operation failed.".to_string())
}

fn approval_status_dto(value: &str) -> CollaborationApprovalStatusDto {
    match value {
        "pending" => CollaborationApprovalStatusDto::Pending,
        "approved" => CollaborationApprovalStatusDto::Approved,
        "executing" => CollaborationApprovalStatusDto::Executing,
        "rejected" => CollaborationApprovalStatusDto::Rejected,
        "cancelled" | "unchanged" => CollaborationApprovalStatusDto::Cancelled,
        "completed" => CollaborationApprovalStatusDto::Completed,
        "expired" => CollaborationApprovalStatusDto::Expired,
        "interrupted" => CollaborationApprovalStatusDto::Interrupted,
        _ => CollaborationApprovalStatusDto::Failed,
    }
}
