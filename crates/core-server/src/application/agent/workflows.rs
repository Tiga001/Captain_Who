use super::*;
use mycopilot_core::storage::workflow_repository::Error;
use mycopilot_core::workflow::{Request, Response};
use mycopilot_core::workflow_management::Request as ManagementRequest;

impl AgentService {
    /// Binding changes only the next composer input. Serialize with ordinary model/context
    /// maintenance admission so an in-flight transition cannot overwrite the new defaults.
    pub(crate) fn workflow_request(&self, request: Request) -> Result<Response, Error> {
        let changes_readiness = matches!(
            &request,
            Request::Save { .. }
                | Request::SaveWithDraft { .. }
                | Request::ImportTemplateMarkdown { .. }
                | Request::Delete { .. }
                | Request::Manage(
                    ManagementRequest::SaveInstance { .. }
                        | ManagementRequest::SetInstanceEnabled { .. }
                        | ManagementRequest::DeleteInstance { .. }
                )
        );
        let result = self.workflow_request_inner(request);
        if result.is_ok() && changes_readiness {
            self.workflow_readiness_changed(None);
        }
        result
    }

    fn workflow_request_inner(&self, request: Request) -> Result<Response, Error> {
        if let Request::Manage(ManagementRequest::SaveInstance {
            id,
            bindings,
            definition,
            ..
        }) = &request
        {
            let _admission = self
                .conversation_admission
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let current = self
                .storage
                .workflow_request(Request::Manage(ManagementRequest::ListInstances {}))?
                .instances
                .into_iter()
                .find(|instance| instance.id == *id);
            // Omitted bindings retain the current conversation in storage. Include those here
            // so a configuration-only edit cannot bypass ongoing model/context maintenance.
            let mut effective_bindings = bindings.clone();
            if let (Some(instance), Some(definition)) = (&current, definition) {
                for previous in &instance.bindings {
                    if definition
                        .nodes
                        .iter()
                        .any(|node| node.id == previous.node_id)
                        && !bindings
                            .iter()
                            .any(|binding| binding.node_id == previous.node_id)
                    {
                        effective_bindings.push(
                            mycopilot_core::workflow_management::BindingInput {
                                node_id: previous.node_id.clone(),
                                conversation_id: Some(previous.conversation_id.clone()),
                            },
                        );
                    }
                }
            }
            for binding in &effective_bindings {
                let Some(conversation_id) = &binding.conversation_id else {
                    continue;
                };
                if current.as_ref().is_some_and(|instance| {
                    let same_binding = instance.bindings.iter().any(|previous| {
                        previous.node_id == binding.node_id
                            && previous.conversation_id == *conversation_id
                    });
                    let old_config = instance
                        .definition
                        .nodes
                        .iter()
                        .find(|node| node.id == binding.node_id)
                        .and_then(member_composer_configuration);
                    let new_config = definition
                        .as_ref()
                        .and_then(|definition| {
                            definition
                                .nodes
                                .iter()
                                .find(|node| node.id == binding.node_id)
                        })
                        .and_then(member_composer_configuration);
                    same_binding && old_config == new_config
                }) {
                    continue;
                }
                if self
                    .provider_transitions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(conversation_id)
                {
                    return Err(Error::Conflict("workflow_configuration_busy".into()));
                }
                self.ensure_no_manual_context_compaction(conversation_id)
                    .map_err(|_| Error::Conflict("workflow_configuration_busy".into()))?;
            }
            return self.storage.workflow_request(request);
        }
        self.storage.workflow_request(request)
    }
}

fn member_composer_configuration(
    node: &mycopilot_core::workflow::Node,
) -> Option<(
    &Option<String>,
    &mycopilot_core::workflow::WorkflowPermissionMode,
)> {
    match &node.config {
        mycopilot_core::workflow::NodeConfig::Agent(config) => {
            Some((&config.model_config_id, &config.permission_mode))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::{
        ChatConversationMetaRecord, ManualContextCompactionOperation,
    };
    use serde_json::json;

    fn binding_request() -> Request {
        serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Workflow","color":"#4A82E8","bindings":[{"nodeId":"a","conversationId":"chat"}],"expectedRevision":0,"expectedTemplateRevision":1})).unwrap()
    }

    #[test]
    fn live_member_configuration_obeys_maintenance_lock_but_rank_changes_do_not() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("live.sqlite")).unwrap());
        let settings = serde_json::from_value(json!({
            "apiUrl":"https://example.test/v1/chat/completions", "apiToken":"test-token",
            "searchMode":"disabled", "tavilyApiKey":"", "models":[{
                "id":"model", "providerModelId":"model", "displayName":"Model",
                "apiUrlOverride":null,"apiTokenOverride":null,"supportsImage":false,
                "contextWindowTokens":128000,
                "providerProfileConfig":mycopilot_core::ProviderProfileConfig::generic_for_dialect(
                    mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions),
                "inputPrice":"0","cachedInputPrice":"","outputPrice":"0","enabled":true
            }]
        }))
        .unwrap();
        storage.save_model_settings(settings).unwrap();
        let definition = json!({"schemaVersion":1,"id":"instance","name":"Team",
            "description":"","background":"", "viewport":{"x":0,"y":0,"zoom":1},
            "nodes":[{"kind":"agent","id":"a","name":"Member","x":0,"y":0,
                "permissionMode":"default","modelConfigId":"model",
                "receives":"","task":"Review","delivers":""}]});
        let created = storage
            .workflow_request(
                serde_json::from_value(json!({
                    "operation":"saveInstance","id":"instance","name":"Team","color":"#123456",
                    "definition":definition,"bindings":[],"expectedRevision":0
                }))
                .unwrap(),
            )
            .unwrap();
        let instance = &created.instances[0];
        let conversation_id = instance.bindings[0].conversation_id.clone();
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
        service
            .provider_transitions
            .lock()
            .unwrap()
            .insert(conversation_id.clone(), "transition".into());
        let mut update = json!({"operation":"saveInstance","id":"instance","name":"Team","color":"#123456",
            "definition":instance.definition,"bindings":instance.bindings,"expectedRevision":1});
        update["definition"]["nodes"][0]["permissionMode"] = json!("full");
        assert!(
            matches!(service.workflow_request(serde_json::from_value(update.clone()).unwrap()),
            Err(Error::Conflict(message)) if message == "workflow_configuration_busy")
        );
        update["bindings"] = json!([]);
        assert!(
            matches!(service.workflow_request(serde_json::from_value(update.clone()).unwrap()),
            Err(Error::Conflict(message)) if message == "workflow_configuration_busy")
        );
        update["definition"]["nodes"][0]["permissionMode"] = json!("default");
        update["definition"]["nodes"][0]["rank"] = json!(5);
        let saved = service
            .workflow_request(serde_json::from_value(update).unwrap())
            .unwrap();
        assert_eq!(saved.instances[0].definition.nodes[0].rank, 5);
        assert_eq!(
            saved.instances[0].bindings[0].conversation_id,
            conversation_id
        );
    }

    #[test]
    fn workflow_binding_respects_the_normal_model_and_context_maintenance_locks() {
        let directory = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&directory.path().join("workflow.sqlite")).unwrap());
        storage
            .save_conversation_meta(ChatConversationMetaRecord {
                id: "chat".into(),
                project_id: None,
                model_id: None,
                title: "Chat".into(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
        service
            .provider_transitions
            .lock()
            .unwrap()
            .insert("chat".into(), "transition".into());
        assert!(
            matches!(service.workflow_request(binding_request()),Err(Error::Conflict(message)) if message=="workflow_configuration_busy")
        );
        service.provider_transitions.lock().unwrap().clear();
        storage
            .claim_manual_context_compaction(&ManualContextCompactionOperation {
                operation_id: "manual-operation".into(),
                request_id: "manual-request".into(),
                conversation_id: "chat".into(),
                status: "running".into(),
                phase: "preparing".into(),
                assistant_message_id: None,
                covered_through_message_id: None,
                model_id: None,
                summary_id: None,
                source_input_tokens: None,
                replacement_input_tokens: None,
                error: None,
                started_at: 1,
                updated_at: 1,
                completed_at: None,
            })
            .unwrap();
        assert!(
            matches!(service.workflow_request(binding_request()),Err(Error::Conflict(message)) if message=="workflow_configuration_busy")
        );
        assert!(storage
            .workflow_request(Request::Manage(ManagementRequest::ListInstances {}))
            .unwrap()
            .instances
            .is_empty());
    }
}
