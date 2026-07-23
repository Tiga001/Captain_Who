use super::*;
use mycopilot_core::{
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimePackageRequirement,
    AgentCommandRuntimeProfile, AgentCommandRuntimeProvider, AgentCommandRuntimeRequest,
    AgentCommandRuntimeResolvedPackage, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
};

#[test]
fn pending_command_round_trip_keeps_the_host_frozen_runtime_binding() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": true },
        "messages": []
    }))
    .unwrap();
    let binding = AgentCommandRuntimeBinding {
        schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        profile: AgentCommandRuntimeProfile::Presentations,
        profile_revision: "artifact-runtime-profile-sha256-v1:test".to_string(),
        provider_id: "mycopilot.artifact-runtime".to_string(),
        bundle_version: "2026.07.3".to_string(),
        bundle_revision: "artifact-runtime-bundle-sha256-v1:test".to_string(),
        kind: AgentCommandRuntimeKind::Node,
        runtime_version: "22.23.1".to_string(),
        runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
        resolved_packages: vec![AgentCommandRuntimeResolvedPackage {
            name: "pptxgenjs".to_string(),
            version: "4.0.1".to_string(),
        }],
    };
    let action = AgentProposedAction::Command {
        command: AgentCommandRequest {
            id: "managed-runtime-profile-pending".to_string(),
            command: "node scripts/build.mjs".to_string(),
            cwd: Some(".".to_string()),
            timeout_ms: Some(30_000),
            approval_status: AgentApprovalStatus::Required,
            risk_level: None,
            reason: Some("build an Office artifact".to_string()),
            observe: None,
            runtime: None,
            runtime_binding: Some(Box::new(binding.clone())),
        },
    };

    assert!(service
        .store_pending_action(
            "managed-runtime-profile-run",
            "managed-runtime-profile-conversation",
            "managed-runtime-profile-assistant",
            action,
            agent_input,
        )
        .unwrap());
    drop(service);

    let reloaded = AgentService::new(Arc::clone(&storage));
    let pending = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let stored = pending
        .get(&pending_action_storage_id(
            "managed-runtime-profile-run",
            "managed-runtime-profile-pending",
        ))
        .unwrap();
    assert!(stored.agent_input.model_capabilities.image_input);
    let AgentProposedAction::Command { command } = &stored.snapshot.action else {
        panic!("persisted action must remain a command")
    };
    assert_eq!(command.runtime_binding.as_deref(), Some(&binding));
    assert!(command.runtime.is_none());
    let resumed_call = tool_call_for_action(&stored.snapshot.action);
    assert_eq!(resumed_call.args["runtimeProfile"], "presentations");
    assert!(resumed_call.args["runtime"].is_null());
}

#[test]
fn pending_command_round_trip_keeps_the_frozen_runtime_request() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    let action = AgentProposedAction::Command {
        command: AgentCommandRequest {
            id: "managed-runtime-pending".to_string(),
            command: "python scripts/build.py".to_string(),
            cwd: Some(".".to_string()),
            timeout_ms: Some(30_000),
            approval_status: AgentApprovalStatus::Required,
            risk_level: None,
            reason: Some("build an Office artifact".to_string()),
            observe: None,
            runtime: Some(AgentCommandRuntimeRequest {
                provider: AgentCommandRuntimeProvider::ManagedArtifact,
                kind: AgentCommandRuntimeKind::Python,
                required_packages: vec![AgentCommandRuntimePackageRequirement {
                    name: "openpyxl".to_string(),
                    version: "3.1.5".to_string(),
                }],
            }),
            runtime_binding: None,
        },
    };

    assert!(service
        .store_pending_action(
            "managed-runtime-run",
            "managed-runtime-conversation",
            "managed-runtime-assistant",
            action,
            agent_input,
        )
        .unwrap());
    drop(service);

    let reloaded = AgentService::new(Arc::clone(&storage));
    let pending = reloaded
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let stored = pending
        .get(&pending_action_storage_id(
            "managed-runtime-run",
            "managed-runtime-pending",
        ))
        .expect("persisted command action must reload");
    let AgentProposedAction::Command { command } = &stored.snapshot.action else {
        panic!("persisted action must remain a command");
    };
    let runtime = command
        .runtime
        .as_ref()
        .expect("managed runtime request must survive persistence");
    assert_eq!(
        runtime.provider,
        AgentCommandRuntimeProvider::ManagedArtifact
    );
    assert_eq!(runtime.kind, AgentCommandRuntimeKind::Python);
    assert_eq!(runtime.required_packages.len(), 1);
    assert_eq!(runtime.required_packages[0].name, "openpyxl");
    assert_eq!(runtime.required_packages[0].version, "3.1.5");
    let resumed_call = tool_call_for_action(&stored.snapshot.action);
    assert_eq!(resumed_call.args["runtime"]["provider"], "managedArtifact");
    assert_eq!(resumed_call.args["runtime"]["kind"], "python");
    assert_eq!(
        resumed_call.args["runtime"]["requiredPackages"][0]["version"],
        "3.1.5"
    );
}

#[test]
fn provider_action_id_is_scoped_by_run_and_same_run_reuse_is_strict() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    let original = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "first_tool".to_string(),
            args: json!({ "value": "original" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    assert!(service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            original.clone(),
            agent_input.clone(),
        )
        .unwrap());
    assert!(!service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            original,
            agent_input.clone(),
        )
        .unwrap());

    let cross_run = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "second_tool".to_string(),
            args: json!({ "value": "replacement" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    assert!(service
        .store_pending_action(
            "run-second",
            "conversation-second",
            "assistant-second",
            cross_run,
            agent_input.clone(),
        )
        .unwrap());
    let same_run_collision = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "shared-action-id".to_string(),
            tool: "conflicting_tool".to_string(),
            args: json!({ "value": "conflict" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    let error = service
        .store_pending_action(
            "run-original",
            "conversation-original",
            "assistant-original",
            same_run_collision,
            agent_input,
        )
        .unwrap_err();
    assert!(error.contains("冻结快照冲突"));

    let in_memory = service
        .pending_actions
        .lock()
        .unwrap_or_else(|lock_error| lock_error.into_inner());
    let frozen = in_memory
        .get(&pending_action_storage_id(
            "run-original",
            "shared-action-id",
        ))
        .unwrap();
    assert_eq!(frozen.snapshot.run_id, "run-original");
    assert_eq!(frozen.snapshot.tool_name, "first_tool");
    drop(in_memory);
    let durable = storage.list_pending_agent_actions().unwrap();
    assert_eq!(durable.len(), 2);
    assert!(durable.iter().any(|record| {
        record.run_id == "run-original" && record.action_json.contains("first_tool")
    }));
    assert!(durable.iter().any(|record| {
        record.run_id == "run-second" && record.action_json.contains("second_tool")
    }));
    assert!(durable
        .iter()
        .all(|record| !record.action_json.contains("conflicting_tool")));
}

#[test]
fn legacy_pending_row_keeps_raw_storage_key_but_is_resolved_by_run_identity() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let action = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "legacy-call-id".to_string(),
            tool: "legacy_tool".to_string(),
            args: json!({}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    let agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: "legacy-call-id".to_string(),
            run_id: "legacy-run".to_string(),
            conversation_id: None,
            assistant_message_id: None,
            action_type: "tool_call".to_string(),
            tool_name: "legacy_tool".to_string(),
            tool_call_id: Some("legacy-call-id".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: serialize_json(&action),
            agent_input_json: serialize_json(&agent_input),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();

    let service = AgentService::new(storage);
    assert!(service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key("legacy-call-id"));
    assert!(service
        .cancel_action("legacy-run", "legacy-call-id")
        .unwrap());
}

#[test]
fn startup_reconciliation_failure_prevents_agent_service_startup() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-bad-reconciliation".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "bad reconciliation".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-bad-reconciliation".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some("not-json".to_string()),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let action = AgentProposedAction::ToolCall {
        call: AgentToolCall {
            id: "bad-reconciliation-call".to_string(),
            tool: "approval_tool".to_string(),
            args: json!({}),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    };
    let agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: pending_action_storage_id(
                "bad-reconciliation-run",
                "bad-reconciliation-call",
            ),
            run_id: "bad-reconciliation-run".to_string(),
            conversation_id: Some("conversation-bad-reconciliation".to_string()),
            assistant_message_id: Some("assistant-bad-reconciliation".to_string()),
            action_type: "tool_call".to_string(),
            tool_name: "approval_tool".to_string(),
            tool_call_id: Some("bad-reconciliation-call".to_string()),
            status: "approved".to_string(),
            target_status: None,
            action_json: serialize_json(&action),
            agent_input_json: serialize_json(&agent_input),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();

    let error = match AgentService::try_new(storage) {
        Ok(_) => panic!("reconciliation failure must prevent startup"),
        Err(error) => error,
    };
    assert!(error.contains("failed to reconcile interrupted pending actions"));
    assert!(error.contains("无法解析中断操作"));
}

#[test]
fn agent_service_startup_retires_an_orphaned_cancelled_conversation_trace() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-orphaned-trace".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "orphaned trace".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-orphaned-trace".to_string(),
                role: "assistant".to_string(),
                content: "partial response".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(
                    json!({
                        "runId": "run-orphaned-trace",
                        "status": "cancelled",
                        "completedAt": 20,
                        "state": {
                            "status": "running",
                            "activeRunId": null,
                            "updatedAt": 10
                        }
                    })
                    .to_string(),
                ),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-orphaned-trace".to_string(),
        conversation_id: "conversation-orphaned-trace".to_string(),
        assistant_message_id: "assistant-orphaned-trace".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: "Reading the image.".to_string(),
            truncated: false,
        }],
    };
    storage
        .append_in_progress_conversation_turn_trace(&trace, 10, 15)
        .unwrap();

    let _service = AgentService::new(storage.clone());

    let repaired = storage
        .get_conversation_turn_trace("assistant-orphaned-trace")
        .unwrap()
        .unwrap();
    assert_eq!(
        repaired.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(repaired.items, trace.items);
    let conversation = storage
        .load_conversation("conversation-orphaned-trace")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.updated_at, 2);
    let run: Value =
        serde_json::from_str(conversation.messages[0].agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "cancelled");
    assert_eq!(run["state"]["status"], "cancelled");
}

#[test]
fn terminal_pending_action_persistence_redacts_run_scoped_skill_bodies() {
    const MARKER: &str = "PENDING_SKILL_INSTRUCTION_BODY_MUST_NOT_SURVIVE";
    const CATALOG_MARKER: &str = "PENDING_SKILL_CATALOG_BODY_MUST_NOT_SURVIVE";
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
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
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-skill-redaction".to_string(),
        context_items: vec![
            mycopilot_core::AgentContextCheckpointItem {
                role: "user".to_string(),
                content: format!("<backend_activated_skill>{MARKER}</backend_activated_skill>"),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["skill_instructions".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                group: None,
                origin: Some(mycopilot_core::AgentContextCheckpointOrigin {
                    kind: "skill".to_string(),
                    id: "bundled:application:test-bundled-skill".to_string(),
                }),
            },
            mycopilot_core::AgentContextCheckpointItem {
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
                group: None,
                origin: None,
            },
            mycopilot_core::AgentContextCheckpointItem {
                role: "system".to_string(),
                content: "NON_SKILL_CHECKPOINT_CONTENT".to_string(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["runtime_guard".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                group: None,
                origin: None,
            },
        ],
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        suppressed_narration: false,
        extension_snapshots: vec![mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: 2,
            state: json!({
                "discovery": checkpoint_discovery,
                "skills": [{
                    "id": "bundled:application:test-bundled-skill",
                    "name": "test-bundled-skill",
                    "revision": "package-revision",
                    "source": "bundled:application",
                    "sourceBytes": MARKER.len(),
                    "hasResources": false,
                    "activatedBy": "user"
                }]
            }),
        }],
        pending_tool_call_id: "action-skill-redaction".to_string(),
        conversation_trace_items: Vec::new(),
        next_conversation_trace_sequence: 0,
        conversation_trace_truncated: false,
        model_visible_trace_item_count: 0,
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
        let persisted = pending_storage_record(&record, 2);
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
        let persisted = pending_storage_record(&record, 3);
        assert!(!persisted.agent_input_json.contains(MARKER));
        assert!(!persisted.agent_input_json.contains(CATALOG_MARKER));
        assert!(!persisted.agent_input_json.contains("secret"));
        let restored: AgentChatInput = serde_json::from_str(&persisted.agent_input_json).unwrap();
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
        .agent_input_json
        .contains(MARKER));
    assert!(pending_storage_record(&record, 4)
        .agent_input_json
        .contains(CATALOG_MARKER));
}

#[test]
fn missing_pending_transition_row_fails_closed_without_terminal_success() {
    const MARKER: &str = "MISSING_TRANSITION_SKILL_BODY";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-missing-row".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application/test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-missing-row".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    let call = AgentToolCall {
        id: "action-missing-transition-row".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    service
        .store_pending_action(
            "run-missing-transition-row",
            "conversation-missing-transition-row",
            "assistant-missing-transition-row",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();
    assert!(storage.list_pending_agent_actions().unwrap()[0]
        .agent_input_json
        .contains(MARKER));

    // Reproduce a cross-boundary missing-row race: durable conversation deletion has removed
    // the pending row while this service instance still owns its pre-deletion memory snapshot.
    storage
        .delete_conversation("conversation-missing-transition-row")
        .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = service
        .approve_action("run-missing-transition-row", &call.id, notifications)
        .unwrap_err();
    assert!(error.contains("实际更新 0 条"));
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            [&pending_action_storage_id("run-missing-transition-row", &call.id)]
            .snapshot
            .status,
        PendingActionStatus::Pending
    );

    let reloaded = AgentService::new(storage);
    assert!(reloaded.list_pending_actions().is_empty());
}

#[test]
fn cancel_finalize_failure_atomically_restores_pending_payload() {
    const MARKER: &str = "CANCEL_ROLLBACK_SKILL_BODY";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(Arc::clone(&storage));
    let call = AgentToolCall {
        id: "action-cancel-rollback".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    agent_input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-cancel-rollback".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "bundled:application/test-bundled-skill".to_string(),
            name: "test-bundled-skill".to_string(),
            revision: "package-cancel-rollback".to_string(),
            source: "bundled:application".to_string(),
            instructions: MARKER.to_string(),
            source_bytes: u64::try_from(MARKER.len()).unwrap(),
            resources: None,
        }],
    });
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-cancel-rollback".to_string(),
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        pending_tool_call_id: call.id.clone(),
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: json!({}),
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
        model_visible_trace_item_count: 0,
    });
    // These durable owner rows intentionally do not exist, forcing final trace persistence to
    // fail after the action has first transitioned to cancelled.
    service
        .store_pending_action(
            "run-cancel-rollback",
            "conversation-cancel-rollback-missing",
            "assistant-cancel-rollback-missing",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();

    assert!(service
        .cancel_action("run-cancel-rollback", &call.id)
        .is_err());
    assert_eq!(
        service.list_pending_actions()[0].status,
        PendingActionStatus::Pending
    );
    let persisted = storage.list_pending_agent_actions().unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, "pending");
    assert_eq!(persisted[0].target_status, None);
    assert!(persisted[0].agent_input_json.contains(MARKER));

    // The compensating transition must make the action reusable. A stale cancelled marker
    // would make this second attempt fail before finalization and strand the row.
    let second_error = service
        .cancel_action("run-cancel-rollback", &call.id)
        .unwrap_err();
    assert!(!second_error.contains("无法写入 cancelled 目标终态"));
    let persisted = storage.list_pending_agent_actions().unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, "pending");
    assert_eq!(persisted[0].target_status, None);
}

#[test]
fn cancel_usage_failure_rolls_back_message_trace_and_action_together() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-cancel-usage-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Cancel usage failure".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-cancel-usage-failure".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    // The invalid usage owner is a deterministic fault injection: the usage insert violates
    // its foreign key only after message and trace writes have run inside the transaction.
    service.register_usage_context(
        "run-cancel-usage-failure",
        AgentRunUsageContext {
            conversation_id: "missing-usage-conversation".to_string(),
            assistant_message_id: "assistant-cancel-usage-failure".to_string(),
            run_id: "run-cancel-usage-failure".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let call = AgentToolCall {
        id: "call-cancel-usage-failure".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-cancel-usage-failure".to_string(),
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        pending_tool_call_id: call.id.clone(),
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: json!({}),
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
        model_visible_trace_item_count: 0,
    });
    service
        .store_pending_action(
            "run-cancel-usage-failure",
            "conversation-cancel-usage-failure",
            "assistant-cancel-usage-failure",
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input,
        )
        .unwrap();

    assert!(service
        .cancel_action("run-cancel-usage-failure", &call.id)
        .is_err());
    let pending = storage.list_pending_agent_actions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, "pending");
    assert_eq!(pending[0].target_status, None);
    assert!(storage
        .get_conversation_turn_trace("assistant-cancel-usage-failure")
        .unwrap()
        .is_none());
    let conversation = storage
        .load_conversation("conversation-cancel-usage-failure")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].content, THINKING_PLACEHOLDER);
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    assert!(storage
        .list_agent_tool_results_for_run("run-cancel-usage-failure", "approval_tool")
        .unwrap()
        .is_empty());
}

#[test]
fn cancelled_file_write_with_durable_rejection_never_rolls_back_to_pending() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-file-write-cancel-failure".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "File write cancel failure".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-file-write-cancel-failure".to_string(),
                role: "assistant".to_string(),
                content: THINKING_PLACEHOLDER.to_string(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let service = AgentService::new(Arc::clone(&storage));
    service.register_usage_context(
        "run-file-write-cancel-failure",
        AgentRunUsageContext {
            conversation_id: "missing-file-write-usage-owner".to_string(),
            assistant_message_id: "assistant-file-write-cancel-failure".to_string(),
            run_id: "run-file-write-cancel-failure".to_string(),
            project_id: None,
            model_id: "model-1".to_string(),
            model_name: "Model 1".to_string(),
            input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let action_id = "file-write-cancel-failure";
    storage
        .create_agent_file_draft(AgentFileDraftRecord {
            id: "draft-file-write-cancel-failure".to_string(),
            conversation_id: "conversation-file-write-cancel-failure".to_string(),
            project_id: None,
            run_id: "run-file-write-cancel-failure".to_string(),
            file_path: "cancelled.txt".to_string(),
            mode: "create".to_string(),
            status: "waiting_approval".to_string(),
            base_revision: None,
            base_content: String::new(),
            content: "hello".to_string(),
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: 5,
            chunk_count: 1,
            next_chunk_index: 1,
            stats_final: true,
            summary: None,
            final_action_id: Some(action_id.to_string()),
            created_at: 1,
            updated_at: 1,
            expires_at: i64::MAX,
        })
        .unwrap();
    let file_write = AgentFileWriteProposal {
        id: action_id.to_string(),
        draft_id: "draft-file-write-cancel-failure".to_string(),
        mode: AgentFileWriteMode::Create,
        file_path: "cancelled.txt".to_string(),
        base_revision: None,
        summary: None,
        additions: 1,
        deletions: 0,
        line_count: 1,
        byte_count: 5,
        approval_status: AgentApprovalStatus::Required,
    };
    let call = tool_call_for_action(&AgentProposedAction::FileWrite {
        file_write: file_write.clone(),
    });
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "messages": []
    }))
    .unwrap();
    agent_input.resume_checkpoint = Some(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: "run-file-write-cancel-failure".to_string(),
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        pending_tool_call_id: action_id.to_string(),
        conversation_trace_items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: action_id.to_string(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            approval_status: AgentApprovalStatus::Required,
            truncated: false,
        }],
        next_conversation_trace_sequence: 1,
        conversation_trace_truncated: false,
        model_visible_trace_item_count: 0,
    });
    service
        .store_pending_action(
            "run-file-write-cancel-failure",
            "conversation-file-write-cancel-failure",
            "assistant-file-write-cancel-failure",
            AgentProposedAction::FileWrite { file_write },
            agent_input,
        )
        .unwrap();

    let error = service
        .cancel_action("run-file-write-cancel-failure", action_id)
        .unwrap_err();
    assert!(error.contains("保持 executing"));
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|lock_error| lock_error.into_inner())
            [&pending_action_storage_id("run-file-write-cancel-failure", action_id,)]
            .snapshot
            .status,
        PendingActionStatus::Executing
    );
    assert_eq!(
        storage
            .get_agent_file_draft("draft-file-write-cancel-failure")
            .unwrap()
            .unwrap()
            .status,
        "rejected"
    );
    assert!(storage
        .get_conversation_turn_trace("assistant-file-write-cancel-failure")
        .unwrap()
        .is_none());
    let conversation = storage
        .load_conversation("conversation-file-write-cancel-failure")
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    let interrupted = storage
        .reconcile_interrupted_pending_agent_actions(42)
        .unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].status, "executing");
    assert_eq!(interrupted[0].target_status.as_deref(), Some("cancelled"));
    assert!(storage.list_pending_agent_actions().unwrap().is_empty());
}
