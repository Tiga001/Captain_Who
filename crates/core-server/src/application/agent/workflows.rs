use super::*;
use mycopilot_core::storage::workflow_repository::Error;
use mycopilot_core::workflow::{Request, Response};
use mycopilot_core::workflow_management::Request as ManagementRequest;

impl AgentService {
    /// Binding changes only the next composer input. Serialize with ordinary model/context
    /// maintenance admission so an in-flight transition cannot overwrite the new defaults.
    pub(crate) fn workflow_request(&self, request: Request) -> Result<Response, Error> {
        if let Request::Manage(ManagementRequest::SaveInstance { id, bindings, .. }) = &request {
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
            for binding in bindings {
                let Some(conversation_id) = &binding.conversation_id else {
                    continue;
                };
                if current.as_ref().is_some_and(|instance| {
                    instance.bindings.iter().any(|previous| {
                        previous.node_id == binding.node_id
                            && previous.conversation_id == *conversation_id
                    })
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
