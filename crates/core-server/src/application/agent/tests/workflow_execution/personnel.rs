use super::*;

fn set_admin(storage: &StorageService, role: &str) {
    let response = storage
        .workflow_request(mycopilot_core::workflow::Request::Manage(
            mycopilot_core::workflow_management::Request::ListInstances {},
        ))
        .unwrap();
    let instance = &response.instances[0];
    let mut definition = serde_json::to_value(&instance.definition).unwrap();
    definition["nodes"][0]["rank"] = json!(90);
    definition["nodes"][0]["managementRole"] = json!(role);
    if definition["departments"]
        .as_array()
        .is_none_or(|items| items.is_empty())
    {
        definition["departments"] = json!([{"id":"quality","name":"Quality","parentId":null,"x":0,"y":0,"width":600,"height":400}]);
        definition["nodes"][1]["departmentId"] = json!("quality");
        definition["nodes"][1]["managementRole"] = json!("department_admin");
        definition["nodes"][1]["rank"] = json!(20);
    }
    storage.workflow_request(serde_json::from_value(json!({"operation":"saveInstance","id":instance.id,
        "templateId":instance.template_id,"name":instance.name,"color":instance.color,"projectId":instance.project_id,
        "definition":definition,"bindings":instance.bindings,"expectedRevision":instance.revision,
        "expectedTemplateRevision":instance.template_revision})).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_organization_edit_real_host_preserves_roles_enforces_actual_authority_and_refreshes_state(
) {
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("personnel.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let mut other_model = settings.models[0].clone();
    other_model.id = "other-model".into();
    other_model.display_name = "Other model".into();
    settings.models.push(other_model);
    storage.save_model_settings(settings).unwrap();
    let mut preferences = storage.load_ui_preferences().unwrap();
    preferences.full_permission_enabled = true;
    storage.save_ui_preferences(preferences).unwrap();
    let (source, target) = workflow_fixture(&storage);
    set_admin(&storage, "organization_admin");
    let service = AgentService::new_authorized_for_test(storage.clone());
    // Role/task edits must remain available even while this member's model is maintained.
    service
        .provider_transitions
        .lock()
        .unwrap()
        .insert(target.clone(), "transition".into());
    let provider_service = service.clone();
    let (captured, mut requests) = unbounded_channel();
    let provider_storage = storage.clone();
    let provider_source = source.clone();
    let target_for_assert = target.clone();
    let provider = tokio::spawn(async move {
        for sample in 0..7 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_body(&mut stream).await;
            captured.send(request).unwrap();
            if sample == 0 {
                // A composer change is a future selection, not authority for this admitted run.
                let mut draft = provider_storage
                    .load_composer_draft(&provider_source)
                    .unwrap()
                    .unwrap();
                draft.model_id = Some("other-model".into());
                draft.permission_mode = "full".into();
                provider_storage.save_composer_draft(draft).unwrap();
                let arguments = json!({"reason":"Add a review department and refine responsibilities","changes":[
                    {"action":"add_department","name":"Evidence review","parent":"Quality"},
                    {"action":"add_member","name":"Independent reviewer","task":"Review delivery evidence"},
                    {"action":"update_member","member":"人事负责人","task":"Own the quality review and report evidence"}
                ]});
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"edit-one","type":"function","function":{"name":"organization_edit","arguments":arguments.to_string()}}]}),"tool_calls").await;
            } else if sample == 1 {
                provider_service
                    .provider_transitions
                    .lock()
                    .unwrap()
                    .clear();
                let arguments = json!({"reason":"Attempt permission above this run's grant","changes":[{"action":"update_member","member":"人事负责人","permissionMode":"full"}]});
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"edit-permission","type":"function","function":{"name":"organization_edit","arguments":arguments.to_string()}}]}),"tool_calls").await;
            } else if sample == 2 {
                provider_service
                    .provider_transitions
                    .lock()
                    .unwrap()
                    .insert(target.clone(), "transition".into());
                let arguments = json!({"reason":"Change model during provider maintenance","changes":[{"action":"update_member","member":"人事负责人","model":"Other model"}]});
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"edit-busy-model","type":"function","function":{"name":"organization_edit","arguments":arguments.to_string()}}]}),"tool_calls").await;
            } else if sample == 3 {
                provider_service
                    .provider_transitions
                    .lock()
                    .unwrap()
                    .clear();
                provider_storage
                    .claim_manual_context_compaction(
                        &mycopilot_core::storage::models::ManualContextCompactionOperation {
                            operation_id: "manual-operation".into(),
                            request_id: "manual-request".into(),
                            conversation_id: target.clone(),
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
                        },
                    )
                    .unwrap();
                let arguments = json!({"reason":"Change permission during context maintenance","changes":[{"action":"update_member","member":"人事负责人","permissionMode":"default"}]});
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"edit-busy-permission","type":"function","function":{"name":"organization_edit","arguments":arguments.to_string()}}]}),"tool_calls").await;
            } else if sample == 4 {
                let mut operation = provider_storage
                    .get_manual_context_compaction(&target, Some("manual-operation"))
                    .unwrap()
                    .unwrap();
                operation.status = "cancelled".into();
                operation.completed_at = Some(2);
                operation.updated_at = 2;
                provider_storage
                    .update_manual_context_compaction(&operation)
                    .unwrap();
                let mut draft = provider_storage
                    .load_composer_draft(&target)
                    .unwrap()
                    .unwrap();
                draft.permission_mode = "full".into();
                provider_storage.save_composer_draft(draft).unwrap();
                let arguments = json!({"reason":"Set the next model with permitted default authority","changes":[{"action":"update_member","member":"人事负责人","model":"Other model","permissionMode":"default"}]});
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"edit-model-and-permission","type":"function","function":{"name":"organization_edit","arguments":arguments.to_string()}}]}),"tool_calls").await;
            } else if sample == 5 {
                // Revoke after the request exposes the tool, before its result is dispatched.
                set_admin(&provider_storage, "member");
                let arguments = json!({"reason":"Try a stale organization edit","changes":[{"action":"update_member","member":"人事负责人","rank":30}]});
                respond(&mut stream,json!({"role":"assistant","tool_calls":[{"index":0,"id":"edit-after-revoke","type":"function","function":{"name":"organization_edit","arguments":arguments.to_string()}}]}),"tool_calls").await;
            } else {
                respond(&mut stream,json!({"role":"assistant","content":"The review department and reviewer are ready. My management permission changed."}),"stop").await;
            }
        }
    });
    let (notifications, mut events) = crate::transport::outbound_channel();
    let mut input = root_input(&source, "Organize this team");
    input.permissions.write = mycopilot_core::AgentWritePermission::WorkspaceOnly;
    service
        .start_conversation_turn(input, notifications)
        .unwrap();
    let (samples, notifications) = tokio::time::timeout(Duration::from_secs(30), async {
        let mut samples = vec![];
        for _ in 0..7 {
            samples.push(requests.recv().await.unwrap());
        }
        let mut notifications = vec![];
        loop {
            let event = events.recv().await.unwrap();
            let done = event["params"]["type"] == "done";
            notifications.push(event);
            if done {
                break;
            }
        }
        (samples, notifications)
    })
    .await
    .unwrap();
    provider.await.unwrap();
    let has_management = |sample: &Value| {
        sample["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "organization_edit")
    };
    assert!(has_management(&samples[0]));
    assert!(has_management(&samples[1]));
    assert!(has_management(&samples[2]));
    assert!(has_management(&samples[3]));
    assert!(has_management(&samples[4]));
    assert!(has_management(&samples[5]));
    assert!(!has_management(&samples[6]));
    assert!(samples[3]
        .to_string()
        .contains("workflow_configuration_busy"));
    assert!(samples[4]
        .to_string()
        .contains("workflow_configuration_busy"));
    assert!(samples.iter().all(|sample| !sample["tools"]
        .to_string()
        .contains("organization_manage_members")));
    assert!(samples[2]
        .to_string()
        .contains("organization_permission_ceiling"));
    assert!(samples[1].to_string().contains("Independent reviewer"));
    let semantic_edit = awareness::tool_result(&samples[1], "organization_edit");
    let edits = semantic_edit["changes"].as_array().unwrap();
    assert!(edits
        .iter()
        .any(|change| change["action"] == "update_member" && change["member"] == "人事负责人"));
    for change in edits {
        assert!(change.get("entityId").is_none());
        assert!(change.get("conversationId").is_none());
    }

    assert!(
        samples[6].to_string().contains("revision_conflict")
            || samples[6].to_string().contains("management_denied")
    );
    assert!(notifications
        .iter()
        .any(|event| event["method"] == "agent.workflows.runtime.changed"
            && event["params"]["events"]
                .as_array()
                .is_some_and(|events| events
                    .iter()
                    .any(|event| event["kind"] == "members_changed"))));
    let fresh_updates: Vec<&Value> = notifications
        .iter()
        .filter_map(|event| event["params"].get("preferenceUpdates"))
        .collect();
    assert_eq!(
        fresh_updates.len(),
        1,
        "only new configuration commits publish defaults"
    );
    assert_eq!(fresh_updates[0].as_array().unwrap().len(), 1);
    assert_eq!(fresh_updates[0][0]["nodeId"], "b");
    assert_eq!(fresh_updates[0][0]["conversationId"], target_for_assert);
    assert_eq!(fresh_updates[0][0]["modelId"], "other-model");
    assert_eq!(fresh_updates[0][0]["permissionMode"], "default");
    assert!(
        fresh_updates[0][0]["organizationRevision"]
            .as_u64()
            .unwrap()
            > 0
    );
    let response = storage
        .workflow_request(mycopilot_core::workflow::Request::Manage(
            mycopilot_core::workflow_management::Request::ListInstances {},
        ))
        .unwrap();
    let instance = &response.instances[0];
    let new = instance
        .definition
        .nodes
        .iter()
        .find(|node| node.name == "Independent reviewer")
        .unwrap();
    assert_eq!(
        new.management_role,
        mycopilot_core::workflow::ManagementRole::Member
    );
    assert_eq!(
        instance
            .definition
            .nodes
            .iter()
            .find(|node| node.id == "b")
            .unwrap()
            .rank,
        20
    );
    let existing = instance
        .definition
        .nodes
        .iter()
        .find(|node| node.id == "b")
        .unwrap();
    assert_eq!(
        existing.management_role,
        mycopilot_core::workflow::ManagementRole::DepartmentAdmin
    );
    assert_eq!(existing.department_id.as_deref(), Some("quality"));
    assert_eq!(existing.name, "人事负责人");
    assert_eq!(
        instance
            .bindings
            .iter()
            .find(|binding| binding.node_id == existing.id)
            .unwrap()
            .conversation_id,
        target_for_assert
    );

    let mycopilot_core::workflow::NodeConfig::Agent(config) = &existing.config;
    assert_eq!(config.task, "Own the quality review and report evidence");
    assert_eq!(config.receives, "Receive artifacts");
    assert_eq!(config.model_config_id.as_deref(), Some("other-model"));
    assert_eq!(
        config.permission_mode,
        mycopilot_core::workflow::WorkflowPermissionMode::Default
    );
    let mycopilot_core::workflow::NodeConfig::Agent(new_config) = &new.config;
    assert_eq!(new_config.model_config_id.as_deref(), Some("model-1"));
    assert_eq!(new.rank, 1);
    assert!(instance
        .definition
        .departments
        .iter()
        .any(|department| department.name == "Evidence review"
            && department.parent_id.as_deref() == Some("quality")));
    let binding = instance
        .bindings
        .iter()
        .find(|binding| binding.node_id == new.id)
        .unwrap();
    assert!(storage
        .load_conversation(&binding.conversation_id)
        .unwrap()
        .unwrap()
        .messages
        .is_empty());
    assert_eq!(
        storage
            .load_composer_draft(&binding.conversation_id)
            .unwrap()
            .unwrap()
            .permission_mode,
        "default"
    );
    assert!(storage
        .workflow_execution_snapshot(&source)
        .unwrap()
        .is_some());
}
