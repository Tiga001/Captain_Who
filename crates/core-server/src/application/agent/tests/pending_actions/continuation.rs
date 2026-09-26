use super::fixtures::test_pending_resume_checkpoint_for_call;
use super::mcp_fixtures::test_mcp_call_id;
use super::*;

#[tokio::test]
async fn projected_child_skill_approval_atomically_resumes_wake_before_worker_runs() {
    const SCRIPT_URI: &str =
        "skill://package/installed%3Auser%3Aapproval-fixture/revision/scripts/build.py";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "http://127.0.0.1:1/v1/chat/completions",
        "secret",
        "disabled",
        "",
    );
    let root_conversation_id = "projected-skill-approval-root-conversation";
    let root_agent_id = "projected-skill-approval-root-agent";
    storage
        .save_conversation(ChatConversationRecord {
            id: root_conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Projected Skill approval root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: root_agent_id.to_string(),
            conversation_id: root_conversation_id.to_string(),
            creation_request_id: "projected-skill-approval-root-ensure".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        root_agent_id,
        root_conversation_id,
        "projected-skill-approval",
        AgentPermissions::default(),
    );
    let child = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: root_agent_id.to_string(),
            creation_request_id: "projected-skill-approval-child-spawn".to_string(),
            task_name: "projected_skill_approval_child".to_string(),
            task: "Wait for Skill script approval.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let admitted_at = mycopilot_core::storage::now_ms();
    let claimed = storage
        .claim_next_dispatchable_agent_wake_at("projected-skill-approval-host", admitted_at)
        .unwrap()
        .unwrap();
    let run_id = "projected-skill-approval-run";
    let assistant_message_id = "projected-skill-approval-assistant";
    let call = AgentToolCall {
        id: "projected-skill-approval-call".to_string(),
        tool: "skills_run_script".to_string(),
        args: json!({ "scriptUri": SCRIPT_URI }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let action = AgentProposedAction::SkillScript {
        script: Box::new(AgentSkillScriptRequest {
            id: call.id.clone(),
            script_uri: SCRIPT_URI.to_string(),
            skill_id: "installed:user:approval-fixture".to_string(),
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
    let context = AgentRunContext {
        conversation_id: Some(child.agent.conversation_id.clone()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: Some(child.collaboration_identity.clone()),
    };
    agent_input.context = Some(context.clone());
    agent_input.assistant_message_id = Some(assistant_message_id.to_string());
    let mut checkpoint = test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Builtin {
            tool_name: call.tool.clone(),
        },
    );
    checkpoint.run_context = Some(context);
    checkpoint.collaboration_run_snapshot =
        Some(mycopilot_core::AgentCollaborationRunSnapshot::default());
    agent_input.resume_checkpoint = Some(checkpoint.clone());

    let (conversation, revision) = storage
        .load_conversation_for_turn(&child.agent.conversation_id)
        .unwrap();
    let mut conversation = conversation.unwrap();
    let waiting_run = json!({
        "runId": run_id,
        "status": "waiting_for_approval",
        "startedAt": admitted_at + 1,
        "toolDefinitions": [],
        "toolCalls": [{
            "id": call.id,
            "tool": call.tool,
            "args": call.args,
            "approvalStatus": "required",
            "reason": null
        }],
        "toolResults": [],
        "approvals": [serde_json::to_value(&action).unwrap()],
        "fileChangeProposals": [],
        "fileChanges": [],
        "webSearchActivities": [],
        "readActivities": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {},
        "timeline": [],
        "state": {
            "status": "waiting_for_approval",
            "activeRunId": run_id,
            "lastError": null,
            "updatedAt": admitted_at + 1
        }
    });
    conversation.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: String::new(),
        created_at: admitted_at + 1,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: Some(waiting_run.to_string()),
        ui_state_json: None,
    });
    conversation.updated_at = admitted_at + 1;
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: child.agent.conversation_id.clone(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: checkpoint.conversation_trace_items.clone(),
    };
    storage
        .save_conversation_and_begin_turn_with_preloaded_agent_messages(
            conversation,
            revision,
            Some(&mycopilot_core::TrustedAgentWakeTurnAdmission {
                agent_id: child.agent.agent_id.clone(),
                wake_id: claimed.wake_id.clone(),
                claim_token: claimed.claim_token.clone().unwrap(),
                source_agent_message_id: claimed.source_agent_message_id.clone().unwrap(),
            }),
            mycopilot_core::AgentTurnPermissionSource::InheritTrustedAncestors,
            &[claimed.source_agent_message_id.clone().unwrap()],
            &trace,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &checkpoint.conversation_model_context_items,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    let waiting = storage
        .transition_agent_wake(
            &claimed.wake_id,
            mycopilot_core::AgentWakeStatus::Running,
            mycopilot_core::AgentWakeStatus::WaitingForApproval,
            claimed.claim_token.as_deref(),
        )
        .unwrap();
    storage
        .upsert_agent_usage(AgentUsageRecordInsert {
            id: "projected-skill-approval-usage".to_string(),
            conversation_id: child.agent.conversation_id.clone(),
            message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            project_id: None,
            model_id: "test-model".to_string(),
            model_name: "Test Model".to_string(),
            started_at: Some(admitted_at + 1),
            completed_at: None,
            status: Some("waiting_for_approval".to_string()),
            error: None,
            created_at: admitted_at + 1,
            input_tokens: None,
            output_tokens: None,
            output_thinking_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: 0,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            estimated_cost: None,
        })
        .unwrap();

    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    service
        .store_pending_action(
            run_id,
            &child.agent.conversation_id,
            assistant_message_id,
            action,
            agent_input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service.inject_skill_script_post_receipt_panic_once(&call.id);
    let decision = service
        .decide_root_projected_approval(
            root_conversation_id,
            &storage_id,
            ProjectedApprovalDecision::Approve,
            None,
            notifications,
        )
        .unwrap();

    // This current-thread Tokio test has not yielded since queueing the worker, so these facts
    // prove the approval bridge itself committed before any Skill execution could settle them.
    assert!(decision.accepted);
    assert_eq!(decision.status, "approved");
    let pending = storage
        .get_pending_agent_action(&storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "executing");
    let resumed = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(resumed.status, mycopilot_core::AgentWakeStatus::Running);
    assert_eq!(resumed.status_revision, waiting.status_revision + 1);
    let observer = storage
        .load_conversation_observer_snapshot(&child.agent.conversation_id)
        .unwrap()
        .unwrap();
    let assistant = observer
        .conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .unwrap();
    assert_eq!(assistant.status.as_deref(), Some("pending"));
    let projected_run: serde_json::Value =
        serde_json::from_str(assistant.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(projected_run["status"], "running");
    assert_eq!(projected_run["state"]["status"], "running");
    assert_eq!(projected_run["state"]["activeRunId"], run_id);
    assert_eq!(projected_run["toolCalls"][0]["approvalStatus"], "approved");
    assert_eq!(
        projected_run["approvals"][0]["script"]["approvalStatus"],
        "approved"
    );
    let usage = storage
        .load_agent_usage_for_owner(run_id, &child.agent.conversation_id, assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(usage.status.as_deref(), Some("running"));
    assert_eq!(
        storage
            .get_agent_display_status(&child.agent.agent_id)
            .unwrap()
            .status,
        mycopilot_core::AgentDisplayStatus::Running
    );
    assert!(service
        .list_root_projected_approvals(root_conversation_id)
        .unwrap()
        .is_empty());

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let pending = storage
                .get_pending_agent_action(&storage_id)
                .unwrap()
                .unwrap();
            let wake = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
            let trace_is_failed = storage
                .get_conversation_turn_trace(assistant_message_id)
                .unwrap()
                .is_some_and(|trace| {
                    trace.terminal_status == ConversationTurnTraceTerminalStatus::Failed
                });
            if pending.status == "failed"
                && pending.target_status.as_deref() == Some("failed")
                && wake.status == mycopilot_core::AgentWakeStatus::Failed
                && trace_is_failed
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("post-receipt panic must settle the child Turn without restart");
    let settled_wake = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(settled_wake.status, mycopilot_core::AgentWakeStatus::Failed);
    assert!(settled_wake.completed_at.is_some());
    assert!(settled_wake.result_message_id.is_some());
    assert!(service
        .file_effects
        .unsettled_effect_ids_for_conversation(&child.agent.conversation_id)
        .is_empty());
    assert!(service
        .file_effects
        .active_run_ids_for_conversation(&child.agent.conversation_id)
        .is_empty());
    assert!(!service.process_runs.has_active_run(run_id));
    assert!(!service
        .has_conversation_turn_occupancy(&child.agent.conversation_id)
        .unwrap());
    assert_eq!(
        storage
            .get_agent_display_status(&child.agent.agent_id)
            .unwrap()
            .status,
        mycopilot_core::AgentDisplayStatus::LatestFailed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_approval_continuation_persists_waiting_to_running_before_runtime() {
    struct ApprovalRecoveryClock(std::sync::atomic::AtomicI64);

    impl crate::application::agent_dispatcher::AgentDispatcherClock for ApprovalRecoveryClock {
        fn now_ms(&self) -> i64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_address = listener.local_addr().unwrap();
    let (provider_accepted, provider_entered) = tokio::sync::oneshot::channel();
    let provider = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        let _ = provider_accepted.send(());
        std::future::pending::<()>().await;
    });
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": format!("http://{provider_address}/v1/chat/completions"),
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-child-approval-root".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Approval root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "agent-child-approval-root".to_string(),
            conversation_id: "conversation-child-approval-root".to_string(),
            creation_request_id: "ensure-child-approval-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        "agent-child-approval-root",
        "conversation-child-approval-root",
        "child-approval-root",
        AgentPermissions::default(),
    );
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-foreign-approval-root".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Foreign approval root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "agent-foreign-approval-root".to_string(),
            conversation_id: "conversation-foreign-approval-root".to_string(),
            creation_request_id: "ensure-foreign-approval-root".to_string(),
            task_name: "Foreign root".to_string(),
        })
        .unwrap();
    let child = storage
        .create_child_agent(&mycopilot_core::CreateChildAgentInput {
            parent_agent_id: "agent-child-approval-root".to_string(),
            creation_request_id: "spawn-child-approval".to_string(),
            task_name: "approval_review".to_string(),
            task: "Pause for approval, then continue.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: mycopilot_core::AgentForkTurns::None,
        })
        .unwrap();
    let admitted_at = mycopilot_core::storage::now_ms();
    let claimed = storage
        .claim_next_dispatchable_agent_wake_at("approval-host", admitted_at)
        .unwrap()
        .unwrap();
    let run_id = "child-approval-continuation-run";
    let assistant_message_id = "child-approval-continuation-assistant";
    let call = AgentToolCall {
        id: test_mcp_call_id("child-approval-continuation-call"),
        tool: "todo_update".to_string(),
        args: json!({ "items": [] }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::RuntimeExtension {
        extension_id: "todo".to_string(),
        tool_name: call.tool.clone(),
    };
    let context = AgentRunContext {
        conversation_id: Some(child.agent.conversation_id.clone()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: Some(child.collaboration_identity.clone()),
    };
    agent_input.context = Some(context.clone());
    agent_input.assistant_message_id = Some(assistant_message_id.to_string());
    // Freeze the exact Tool authority through the same Host projection used by Runtime instead
    // of copying revision hashes into this durable approval fixture.
    let checkpoint_tool_set = {
        let projection_service =
            AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
                Arc::clone(&storage),
                None,
                1,
            )
            .unwrap();
        projection_service
            .context_window_tool_projection(&agent_input, None)
            .unwrap()
            .tool_set_checkpoint()
    };
    let (conversation, revision) = storage
        .load_conversation_for_turn(&child.agent.conversation_id)
        .unwrap();
    let mut conversation = conversation.unwrap();
    conversation.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: assistant_message_id.to_string(),
        role: "assistant".to_string(),
        content: String::new(),
        created_at: admitted_at + 1,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
    });
    conversation.updated_at = admitted_at + 1;
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: child.agent.conversation_id.clone(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            operation: call.args.clone(),
            provenance: provenance.clone(),
            approval_status: call.approval_status,
            truncated: false,
        }],
    };
    storage
        .save_conversation_and_begin_turn_with_preloaded_agent_messages(
            conversation,
            revision,
            Some(&mycopilot_core::TrustedAgentWakeTurnAdmission {
                agent_id: child.agent.agent_id.clone(),
                wake_id: claimed.wake_id.clone(),
                claim_token: claimed.claim_token.clone().unwrap(),
                source_agent_message_id: claimed.source_agent_message_id.clone().unwrap(),
            }),
            mycopilot_core::AgentTurnPermissionSource::InheritTrustedAncestors,
            &[claimed.source_agent_message_id.clone().unwrap()],
            &trace,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    let mut durable_checkpoint =
        test_pending_resume_checkpoint_for_call(&storage, run_id, None, &call, provenance.clone());
    durable_checkpoint.tool_set = checkpoint_tool_set.clone();
    durable_checkpoint.collaboration_run_snapshot =
        Some(mycopilot_core::AgentCollaborationRunSnapshot::default());
    let checkpoint_model_context = durable_checkpoint.conversation_model_context_items;
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &checkpoint_model_context,
            admitted_at + 1,
            admitted_at + 1,
        )
        .unwrap();
    let waiting = storage
        .transition_agent_wake(
            &claimed.wake_id,
            mycopilot_core::AgentWakeStatus::Running,
            mycopilot_core::AgentWakeStatus::WaitingForApproval,
            claimed.claim_token.as_deref(),
        )
        .unwrap();

    let mut checkpoint =
        test_pending_resume_checkpoint_for_call(&storage, run_id, None, &call, provenance);
    checkpoint.run_context = Some(context.clone());
    checkpoint.tool_set = checkpoint_tool_set;
    checkpoint.collaboration_run_snapshot =
        Some(mycopilot_core::AgentCollaborationRunSnapshot::default());
    agent_input.resume_checkpoint = Some(checkpoint);
    agent_input.approval_decision = Some(AgentApprovalDecision {
        action_id: call.id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    agent_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({ "revision": 1, "items": [], "updatedAt": admitted_at + 1 })),
            error: None,
        },
    });

    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    service
        .store_pending_action(
            run_id,
            &child.agent.conversation_id,
            assistant_message_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let (root_card_notifications, _root_card_receiver) = tokio::sync::mpsc::unbounded_channel();
    let direct_child_write =
        service.approve_action(run_id, &call.id, root_card_notifications.clone());
    assert!(
        direct_child_write
            .unwrap_err()
            .contains("子 Agent Conversation 是只读观察视图"),
        "a user approval must not use the child Conversation write API"
    );
    assert!(!service.cancel_run(run_id));
    assert!(service
        .reject_action(
            run_id,
            &call.id,
            Some("forged rejection".to_string()),
            root_card_notifications.clone(),
        )
        .unwrap_err()
        .contains("子 Agent Conversation 是只读观察视图"));
    assert!(service
        .cancel_action(run_id, &call.id)
        .unwrap_err()
        .contains("子 Agent Conversation 是只读观察视图"));
    let projected = service
        .list_root_projected_approvals("conversation-child-approval-root")
        .unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].approval_id, storage_id);
    assert_eq!(projected[0].source_agent_id, child.agent.agent_id);
    assert_eq!(projected[0].action.run_id, run_id);
    let unknown_error = service
        .decide_root_projected_approval(
            "conversation-child-approval-root",
            "missing-projected-approval",
            ProjectedApprovalDecision::Cancel,
            None,
            root_card_notifications.clone(),
        )
        .unwrap_err();
    let foreign_error = service
        .decide_root_projected_approval(
            "conversation-foreign-approval-root",
            &storage_id,
            ProjectedApprovalDecision::Cancel,
            None,
            root_card_notifications.clone(),
        )
        .unwrap_err();
    assert_eq!(unknown_error, "Approval is unavailable.");
    assert_eq!(foreign_error, unknown_error);
    let restarted_projection =
        AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
            Arc::clone(&storage),
            None,
            1,
        )
        .unwrap();
    let recovered = restarted_projection
        .list_root_projected_approvals("conversation-child-approval-root")
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].approval_id, storage_id);
    assert_eq!(recovered[0].source_agent_id, child.agent.agent_id);
    drop(restarted_projection);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .commit_trace_snapshot_with_continuation(&approved, &agent_input, &notifications)
        .unwrap();
    let gate = service.turn_concurrency_gate();
    assert_eq!(gate.active(), 1);
    let (dispatcher_notifications, _dispatcher_receiver) = tokio::sync::mpsc::unbounded_channel();
    let dispatcher_store: Arc<dyn crate::application::agent_dispatcher::AgentDispatcherStore> =
        Arc::new(
            crate::application::agent_dispatcher::SqliteAgentDispatcherStore::new(Arc::clone(
                &storage,
            )),
        );
    let dispatcher_executor: Arc<
        dyn crate::application::agent_dispatcher::AgentWakeTurnExecutionPort,
    > = Arc::new(
        crate::application::agent_dispatcher::SharedAgentTurnExecutionPort::new(
            service.clone(),
            Arc::clone(&storage),
            dispatcher_notifications,
        )
        .with_fallback_poll_interval(Duration::from_millis(5)),
    );
    let dispatcher = crate::application::agent_dispatcher::AgentDispatcher::start_with_clock(
        dispatcher_store,
        dispatcher_executor,
        Arc::new(ApprovalRecoveryClock(std::sync::atomic::AtomicI64::new(
            waiting.lease_expires_at.unwrap(),
        ))),
        gate.clone(),
        crate::application::agent_dispatcher::AgentDispatcherConfig {
            global_concurrency_limit: 1,
            wake_lease_renew_interval: Duration::from_millis(20),
            idle_poll_interval: Duration::from_millis(5),
            shutdown_grace: Duration::from_millis(250),
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if dispatcher.observed_waiting_for_approval(&child.agent.agent_id) == Some(true) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Dispatcher must recover and observe the durable approval wait");

    // The approved tool result is already durable before the continuation enters Runtime. This
    // mirrors the production action runner and lets the observer distinguish a live continuation
    // from a still-pending approval after the Wake CAS.
    service
        .transition_pending_status(&approved, PendingActionStatus::Completed)
        .unwrap();
    let duplicate = service
        .decide_root_projected_approval(
            "conversation-child-approval-root",
            &storage_id,
            ProjectedApprovalDecision::Approve,
            None,
            root_card_notifications,
        )
        .unwrap();
    assert!(!duplicate.accepted);
    assert_eq!(duplicate.status, "completed");
    assert_eq!(duplicate.run_id, run_id);
    let continuation_service = service.clone();
    let continuation = tokio::spawn(async move {
        continuation_service
            .run_action_continuation(
                approved,
                agent_input,
                notifications,
                PendingActionStatus::Completed,
                None,
            )
            .await;
    });
    match tokio::time::timeout(Duration::from_secs(2), provider_entered).await {
        Ok(entered) => entered.unwrap(),
        Err(error) => {
            let mut events = Vec::new();
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
            panic!(
                "continuation must reach the deterministic Provider boundary: {error:?}; events={events:#?}"
            );
        }
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if dispatcher.observed_waiting_for_approval(&child.agent.agent_id) == Some(false) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("durable Running transition must make shutdown see a live continuation");
    let resumed = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(resumed.status, mycopilot_core::AgentWakeStatus::Running);
    assert_eq!(resumed.status_revision, waiting.status_revision + 1);
    assert_eq!(resumed.run_id.as_deref(), Some(run_id));
    assert_eq!(
        resumed.assistant_message_id.as_deref(),
        Some(assistant_message_id)
    );

    let shutdown = dispatcher.shutdown().await.unwrap();
    assert_eq!(
        shutdown.cancellation_requested, 1,
        "shutdown must interrupt the resumed live Runtime, not preserve it as approval waiting"
    );
    tokio::time::timeout(Duration::from_secs(2), continuation)
        .await
        .expect("cancelled continuation must converge")
        .unwrap();
    let terminal = storage.get_agent_wake(&claimed.wake_id).unwrap().unwrap();
    assert_eq!(
        terminal.status,
        mycopilot_core::AgentWakeStatus::Interrupted
    );
    assert!(terminal.result_message_id.is_some());
    assert_eq!(gate.active(), 0);
    provider.abort();
}
