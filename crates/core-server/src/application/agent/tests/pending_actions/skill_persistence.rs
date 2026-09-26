use super::*;

#[test]
fn terminal_pending_action_persistence_redacts_run_scoped_skill_bodies() {
    const MARKER: &str = "PENDING_SKILL_INSTRUCTION_BODY_MUST_NOT_SURVIVE";
    const CATALOG_MARKER: &str = "PENDING_SKILL_CATALOG_BODY_MUST_NOT_SURVIVE";
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    let provider_configuration_revision = format!("provider-protocol-v1:{}", uuid::Uuid::new_v4());
    let provider_profile_config = crate::test_provider_profile_config();
    let provider_protocol_key = mycopilot_core::ProviderProtocolKey::new(
        mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile_config,
        "test-model",
        Some(provider_configuration_revision.clone()),
    )
    .unwrap();
    agent_input.provider_configuration_revision = Some(provider_configuration_revision);
    agent_input.provider_connection_revision =
        Some(format!("provider-connection-v1:{}", uuid::Uuid::new_v4()));
    agent_input.search_connection_revision =
        Some(format!("search-connection-v1:{}", uuid::Uuid::new_v4()));
    agent_input.model_config_id = Some("test-model-config".to_string());
    agent_input.provider_profile_config = Some(provider_profile_config.clone());
    agent_input.provider_protocol_key = Some(provider_protocol_key.clone());
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-revision".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application:test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-revision".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    agent_input.skill_discovery = Some(mycopilot_core::skills::AgentSkillDiscoverySnapshot {
        schema_version: mycopilot_core::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
        catalog_revision: "skill-enabled-catalog-sha256-v1:redaction".to_string(),
        prompt_token_budget: mycopilot_core::skills::DEFAULT_SKILL_DISCOVERY_PROMPT_TOKENS,
        skills: vec![mycopilot_core::skills::AgentDiscoverableSkill {
            activation_ref: mycopilot_core::skills::derive_skill_activation_ref(
                "skill-enabled-catalog-sha256-v1:redaction",
                "bundled:application:test-bundled-skill",
                "package-revision",
            ),
            id: "bundled:application:test-bundled-skill".to_string(),
            revision: "package-revision".to_string(),
            name: "test-bundled-skill".to_string(),
            description: CATALOG_MARKER.to_string(),
            source_kind: "bundled".to_string(),
        }],
        max_activated_skills: 8,
        max_total_source_bytes: 512 * 1024,
    });
    let checkpoint_discovery = agent_input.skill_discovery.clone().unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        pause_reason: mycopilot_core::AgentRunCheckpointPauseReason::Approval,
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-skill-redaction".to_string(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        context_items: vec![
            mycopilot_core::AgentContextCheckpointItem {
                context_image_refs: Vec::new(),
                role: "user".to_string(),
                content: format!("<backend_activated_skill>{MARKER}</backend_activated_skill>"),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["skill_instructions".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                request_order: None,
                group: None,
                origin: Some(mycopilot_core::AgentContextCheckpointOrigin {
                    kind: "skill".to_string(),
                    id: "bundled:application:test-bundled-skill".to_string(),
                }),
            },
            mycopilot_core::AgentContextCheckpointItem {
                context_image_refs: Vec::new(),
                role: "user".to_string(),
                content: format!(
                    "<backend_available_skills>{CATALOG_MARKER}</backend_available_skills>"
                ),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["skill_catalog".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                request_order: None,
                group: None,
                origin: None,
            },
            mycopilot_core::AgentContextCheckpointItem {
                context_image_refs: Vec::new(),
                role: "system".to_string(),
                content: "NON_SKILL_CHECKPOINT_CONTENT".to_string(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["runtime_guard".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                request_order: None,
                group: None,
                origin: None,
            },
        ],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: vec![mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: 3,
            state: json!({
                "discovery": checkpoint_discovery,
                "skills": [{
                    "id": "bundled:application:test-bundled-skill",
                    "name": "test-bundled-skill",
                    "revision": "package-revision",
                    "source": "bundled:application",
                    "sourceBytes": MARKER.len(),
                    "hasResources": false,
                    "resourceKinds": [],
                    "activatedBy": "user"
                }]
            }),
        }],
        tool_set: crate::test_tool_set_checkpoint(),
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: ModelCapabilities::default(),
        provider_profile_config,
        provider_protocol_key,
        assistant_turn_identity: crate::test_assistant_turn_identity(&["action-skill-redaction"]),
        provider_continuation_refs: Vec::new(),
        conversation_world_state_records: Vec::new(),
        run_world_state: crate::test_run_world_state(),
        pending_tool_call_id: "action-skill-redaction".to_string(),
        conversation_model_context_items: Vec::new(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
    });
    let mut record = PendingActionRecord {
        storage_id: pending_action_storage_id("run-skill-redaction", "action-skill-redaction"),
        snapshot: PendingAgentActionSnapshot {
            action_id: "action-skill-redaction".to_string(),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("action-skill-redaction".to_string()),
            run_id: "run-skill-redaction".to_string(),
            conversation_id: Some("conversation-skill-redaction".to_string()),
            assistant_message_id: Some("assistant-skill-redaction".to_string()),
            action: AgentProposedAction::ToolCall {
                call: AgentToolCall {
                    id: "action-skill-redaction".to_string(),
                    tool: "approval_tool".to_string(),
                    args: json!({}),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            },
            created_at: 1,
            status: PendingActionStatus::Pending,
        },
        agent_input,
    };

    for status in [
        PendingActionStatus::Pending,
        PendingActionStatus::Approved,
        PendingActionStatus::Executing,
    ] {
        record.snapshot.status = status;
        let persisted = pending_storage_record(&record, 2).unwrap();
        assert!(persisted.agent_input_json.contains(MARKER));
        assert!(persisted.agent_input_json.contains(CATALOG_MARKER));
    }

    for status in [
        PendingActionStatus::Completed,
        PendingActionStatus::Rejected,
        PendingActionStatus::Cancelled,
        PendingActionStatus::Failed,
    ] {
        record.snapshot.status = status;
        let persisted = pending_storage_record(&record, 3).unwrap();
        assert!(!persisted.agent_input_json.contains(MARKER));
        assert!(!persisted.agent_input_json.contains(CATALOG_MARKER));
        assert!(!persisted.agent_input_json.contains("secret"));
        let restored = PersistedAgentResumeInput::decode(&persisted.agent_input_json)
            .unwrap()
            .agent_input;
        let activation = restored.skill_activation.unwrap();
        assert!(restored.skill_discovery.is_none());
        assert_eq!(activation.activation_revision, "activation-revision");
        assert_eq!(activation.skills.len(), 1);
        assert_eq!(
            activation.skills[0].id,
            "bundled:application:test-bundled-skill"
        );
        assert_eq!(activation.skills[0].revision, "package-revision");
        assert_eq!(activation.skills[0].source, "bundled:application");
        assert!(activation.skills[0].instructions.is_empty());
        let checkpoint = restored.resume_checkpoint.unwrap();
        assert!(checkpoint.context_items[0].content.is_empty());
        assert_eq!(
            checkpoint.context_items[0].sources,
            vec!["skill_instructions"]
        );
        assert_eq!(
            checkpoint.context_items[0].origin.as_ref().unwrap().id,
            "bundled:application:test-bundled-skill"
        );
        assert_eq!(checkpoint.context_items[1].content, "");
        assert_eq!(checkpoint.context_items[1].sources, vec!["skill_catalog"]);
        assert_eq!(
            checkpoint.context_items[2].content,
            "NON_SKILL_CHECKPOINT_CONTENT"
        );
        assert_eq!(
            checkpoint.extension_snapshots[0].state["discovery"],
            serde_json::Value::Null
        );
    }

    record.snapshot.status = PendingActionStatus::Pending;
    assert!(pending_storage_record(&record, 4)
        .unwrap()
        .agent_input_json
        .contains(MARKER));
    assert!(pending_storage_record(&record, 4)
        .unwrap()
        .agent_input_json
        .contains(CATALOG_MARKER));
}
