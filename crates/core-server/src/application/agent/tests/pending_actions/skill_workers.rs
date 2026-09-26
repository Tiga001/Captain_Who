use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

#[tokio::test]
async fn skill_script_worker_setup_failure_persists_receipt_and_runs_continuation() {
    const SCRIPT_URI: &str = "skill://package/installed%3Auser%3Afixture/revision/scripts/build.py";
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let run_id = "skill-worker-setup-failure-run";
    let conversation_id = "skill-worker-setup-failure-conversation";
    let assistant_message_id = "skill-worker-setup-failure-assistant";
    let call = AgentToolCall {
        id: "skill-worker-setup-failure-call".to_string(),
        tool: "skills_run_script".to_string(),
        args: json!({ "scriptUri": SCRIPT_URI }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::SkillScript {
        script: Box::new(AgentSkillScriptRequest {
            id: call.id.clone(),
            script_uri: SCRIPT_URI.to_string(),
            skill_id: "installed:user:fixture".to_string(),
            skill_revision: "revision".to_string(),
            resource_path: "scripts/build.py".to_string(),
            resource_digest: "sha256:fixture".to_string(),
            source: mycopilot_core::AgentSkillScriptSourceProof {
                source_id: "installed:user".to_string(),
                source_kind: mycopilot_core::AgentSkillScriptSourceKind::Installed,
                trust: mycopilot_core::AgentSkillScriptTrust::Untrusted,
            },
            interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
            args: Vec::new(),
            requirements: mycopilot_core::AgentSkillScriptRequirements::default(),
            preflight: mycopilot_core::AgentSkillScriptPreflightReport {
                status: mycopilot_core::AgentSkillScriptPreflightStatus::Ready,
                interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
                interpreter_version: Some("Python 3".to_string()),
                dependencies: Vec::new(),
                runtime_fingerprint: "fixture-runtime".to_string(),
                error_code: None,
                message: None,
            },
            timeout_ms: Some(mycopilot_core::skills::DEFAULT_SKILL_SCRIPT_TIMEOUT_MS),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "http://127.0.0.1:1/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);

    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Executing)
        .unwrap();
    let executing = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    // The closed loopback endpoint fails deterministically after the Skill result receipt commits,
    // proving the common runner exit enters the real continuation without a paid Provider.
    service.register_usage_context(
        run_id,
        AgentRunUsageContext {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            project_id: None,
            model_id: "test-model".to_string(),
            model_name: "Test Model".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    let setup_failure = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: Some(json!({
            "type": "skill_script",
            "status": "failed",
            "outcome": "definitely_not_executed",
            "code": "executionSetupFailed",
            "effectsMayHaveOccurred": false,
        })),
        error: Some("fixture Skill worker setup failed".to_string()),
    };
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .finish_skill_script_execution(
            executing,
            call.clone(),
            setup_failure,
            notifications,
            None,
            None,
        )
        .await;
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();

    let (status, target_status): (String, Option<String>) =
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(status, "failed", "events: {events:#?}");
    assert_eq!(target_status.as_deref(), Some("failed"));
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
    )));
    assert!(!service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    assert!(events
        .iter()
        .any(|event| event["params"]["type"] == "error"));
}

#[derive(Clone, Copy)]
enum InjectedSkillScriptWorkerPanic {
    AfterEffectBoundary,
    AfterDurableReceipt,
}

async fn assert_queued_skill_script_worker_panic_is_supervised(
    injected: InjectedSkillScriptWorkerPanic,
) {
    const SCRIPT_URI: &str = "skill://package/installed%3Auser%3Afixture/revision/scripts/build.py";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let (run_id, conversation_id, assistant_message_id, call_id) = match injected {
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary => (
            "skill-worker-supervised-panic-run",
            "skill-worker-supervised-panic-conversation",
            "skill-worker-supervised-panic-assistant",
            "skill-worker-supervised-panic-call",
        ),
        InjectedSkillScriptWorkerPanic::AfterDurableReceipt => (
            "skill-worker-post-receipt-panic-run",
            "skill-worker-post-receipt-panic-conversation",
            "skill-worker-post-receipt-panic-assistant",
            "skill-worker-post-receipt-panic-call",
        ),
    };
    let call = AgentToolCall {
        id: call_id.to_string(),
        tool: "skills_run_script".to_string(),
        args: json!({ "scriptUri": SCRIPT_URI }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::SkillScript {
        script: Box::new(AgentSkillScriptRequest {
            id: call.id.clone(),
            script_uri: SCRIPT_URI.to_string(),
            skill_id: "installed:user:fixture".to_string(),
            skill_revision: "revision".to_string(),
            resource_path: "scripts/build.py".to_string(),
            resource_digest: "sha256:fixture".to_string(),
            source: mycopilot_core::AgentSkillScriptSourceProof {
                source_id: "installed:user".to_string(),
                source_kind: mycopilot_core::AgentSkillScriptSourceKind::Installed,
                trust: mycopilot_core::AgentSkillScriptTrust::Untrusted,
            },
            interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
            args: Vec::new(),
            requirements: mycopilot_core::AgentSkillScriptRequirements::default(),
            preflight: mycopilot_core::AgentSkillScriptPreflightReport {
                status: mycopilot_core::AgentSkillScriptPreflightStatus::Ready,
                interpreter: mycopilot_core::AgentSkillScriptInterpreter::Python3,
                interpreter_version: Some("Python 3".to_string()),
                dependencies: Vec::new(),
                runtime_fingerprint: "fixture-runtime".to_string(),
                error_code: None,
                message: None,
            },
            timeout_ms: Some(mycopilot_core::skills::DEFAULT_SKILL_SCRIPT_TIMEOUT_MS),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        }),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "http://127.0.0.1:1/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);

    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Executing)
        .unwrap();
    let executing = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service.register_usage_context(
        run_id,
        AgentRunUsageContext {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            project_id: None,
            model_id: "test-model".to_string(),
            model_name: "Test Model".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    match injected {
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary => {
            service.inject_skill_script_worker_panic_once(&call.id)
        }
        InjectedSkillScriptWorkerPanic::AfterDurableReceipt => {
            service.inject_skill_script_post_receipt_panic_once(&call.id)
        }
    }
    let process_guard = service.process_runs.register(&storage_id, run_id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let queued = service
        .queue_skill_script_execution(executing, call.clone(), process_guard, notifications)
        .unwrap();
    assert_eq!(queued.status, "approved");
    assert_eq!(
        storage
            .get_pending_agent_action(&storage_id)
            .unwrap()
            .unwrap()
            .status,
        "executing"
    );

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let current = storage
                .get_pending_agent_action(&storage_id)
                .unwrap()
                .unwrap();
            let trace_terminal = storage
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .is_some_and(|trace| {
                    trace.terminal_status == ConversationTurnTraceTerminalStatus::Failed
                });
            if current.status == "failed"
                && current.target_status.as_deref() == Some("failed")
                && trace_terminal
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the supervised panic must converge without an executing ghost");

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    let observation = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id,
                observation,
                ..
            } if call_id == &call.id => Some(observation),
            _ => None,
        })
        .expect("the panic outcome must have a durable ToolResult receipt");
    if matches!(
        injected,
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary
    ) {
        assert_eq!(observation["status"], "outcome_unknown");
        assert_eq!(observation["outcome"], "outcome_unknown");
        assert_eq!(observation["effectsMayHaveOccurred"], true);
        assert_eq!(observation["retryable"], false);
    }
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
            ))
            .count(),
        1
    );
    assert!(service
        .file_effects
        .unsettled_effect_ids_for_conversation(conversation_id)
        .is_empty());
    assert!(service
        .file_effects
        .active_run_ids_for_conversation(conversation_id)
        .is_empty());
    assert!(!service.process_runs.has_active_run(run_id));
    assert!(!service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
}

#[tokio::test]
async fn queued_skill_script_worker_panic_is_supervised_to_durable_outcome_unknown() {
    assert_queued_skill_script_worker_panic_is_supervised(
        InjectedSkillScriptWorkerPanic::AfterEffectBoundary,
    )
    .await;
}

#[tokio::test]
async fn queued_skill_script_post_receipt_panic_terminalizes_without_replay() {
    assert_queued_skill_script_worker_panic_is_supervised(
        InjectedSkillScriptWorkerPanic::AfterDurableReceipt,
    )
    .await;
}
