use super::mcp_fixtures::{
    seed_durable_mcp_pending_owner, test_mcp_pending_action, test_mcp_resume_checkpoint,
    RecoverableApprovalRaceInvoker, StaticMcpStartupInspector,
};
use super::*;

struct AutoJournalObservingInvoker {
    storage: Arc<StorageService>,
    invocations: std::sync::atomic::AtomicUsize,
    saw_executing_before_invoke: std::sync::atomic::AtomicBool,
    return_outcome_unknown: bool,
}

impl AutoJournalObservingInvoker {
    fn new(storage: Arc<StorageService>) -> Arc<Self> {
        Self::with_outcome_unknown(storage, false)
    }

    fn with_outcome_unknown(
        storage: Arc<StorageService>,
        return_outcome_unknown: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            storage,
            invocations: std::sync::atomic::AtomicUsize::new(0),
            saw_executing_before_invoke: std::sync::atomic::AtomicBool::new(false),
            return_outcome_unknown,
        })
    }
}

impl McpToolInvoker for AutoJournalObservingInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<mycopilot_core::McpAgentToolDescriptor>> {
        Ok(Vec::new())
    }

    fn revalidate_approved(
        &self,
        _approval: &mycopilot_core::AgentMcpToolApproval,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn invalidate_prepared_approval(
        &self,
        _identity: &mycopilot_core::AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn invoke_approved<'a>(
        &'a self,
        invocation: McpApprovedToolInvocation,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::McpToolInvocationFuture<'a> {
        Box::pin(async move {
            let storage_id = pending_action_storage_id(
                &invocation.approval.identity.run_id,
                &invocation.approval.identity.action_id,
            );
            let saw_executing = self
                .storage
                .list_recoverable_agent_actions_after_reconciliation()?
                .into_iter()
                .any(|row| row.action_id == storage_id && row.status == "executing");
            self.saw_executing_before_invoke
                .store(saw_executing, std::sync::atomic::Ordering::SeqCst);
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.return_outcome_unknown {
                return Err(AgentError::structured(
                    "mcp.tool_outcome_unknown",
                    "Test transport lost the authoritative response.",
                    json!({
                        "type": "mcp_tool",
                        "code": "outcomeUnknown",
                        "retryable": false,
                        "dispatchCertainty": "possibly_dispatched",
                    }),
                ));
            }
            Ok(mycopilot_core::McpToolInvocationResult {
                content: vec![mycopilot_core::McpToolContentBlock::Text {
                    text: "AUTO_RESULT_CANARY_NOT_DURABLE".to_string(),
                }],
                structured_content: None,
                is_error: false,
                truncated_at_source: false,
            })
        })
    }
}

fn auto_mcp_action(
    run_id: &str,
    action_id: &str,
    invocation_id: &str,
    created_at: i64,
) -> AgentProposedAction {
    let mut action = test_mcp_pending_action(run_id, action_id, invocation_id, created_at);
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        unreachable!("test helper always creates an MCP action");
    };
    approval.approval_mode = mycopilot_core::AgentMcpApprovalMode::Auto;
    approval.call.approval_status = AgentApprovalStatus::Approved;
    action
}

fn auto_mcp_agent_input(storage: &StorageService, run_id: &str, action_id: &str) -> AgentChatInput {
    let mut input = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "test-token",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    freeze_test_pending_provider_configuration(storage, &mut input);
    input.resume_checkpoint = Some(test_mcp_resume_checkpoint(storage, run_id, action_id));
    input
}

#[tokio::test]
async fn auto_mcp_invokes_only_after_hidden_durable_executing_journal_and_scrubs_terminal_row() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-journal.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = AutoJournalObservingInvoker::new(Arc::clone(&storage));
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let run_id = "auto-mcp-journal-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = auto_mcp_action(
        run_id,
        &action_id,
        &invocation_id,
        mycopilot_core::storage::now_ms(),
    );
    let projection_created_at = mycopilot_core::storage::now_ms();
    storage
        .save_conversation(ChatConversationRecord {
            id: "auto-mcp-conversation".to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Backend-owned automatic MCP".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "auto-mcp-assistant".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: projection_created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: projection_created_at,
            updated_at: projection_created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let AgentProposedAction::McpToolCall { approval } = action else {
        unreachable!();
    };
    let frozen_approval = approval.clone();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let context = AutoApprovedActionContext::new(
        auto_mcp_agent_input(&storage, run_id, &action_id),
        run_id.to_string(),
        Some("auto-mcp-conversation".to_string()),
        Some("auto-mcp-assistant".to_string()),
        None,
    )
    .with_notifications(notifications);

    let result = service
        .execute_auto_mcp_tool_action(&context, approval, AgentCancellationToken::new())
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(invoker
        .saw_executing_before_invoke
        .load(std::sync::atomic::Ordering::SeqCst));
    assert!(service.list_pending_actions().is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());
    while let Ok(notification) = receiver.try_recv() {
        assert_ne!(notification["params"]["type"], "approval_required");
        assert!(
            notification["params"]["status"] != "waiting_for_approval",
            "Auto journal must never become a Renderer approval"
        );
    }

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let audit = storage
        .get_agent_action_audit(&storage_id)
        .unwrap()
        .expect("automatic external MCP journal keeps its durable audit");
    assert_eq!(audit.decision_source.as_deref(), Some("auto"));
    let terminal: (String, String, String) = rusqlite::Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT status, action_json, agent_input_json
             FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(terminal.0, "completed");
    assert_eq!(terminal.1, "{}");
    assert_eq!(terminal.2, "{}");
    let durable = format!("{}{}", terminal.1, terminal.2);
    assert!(!durable.contains("AUTO_RESULT_CANARY_NOT_DURABLE"));

    let durable_result = mycopilot_core::mcp_tool_result_persistence_projection(&result);
    let trace = mycopilot_core::ConversationTurnTrace {
        schema_version: mycopilot_core::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: "auto-mcp-conversation".to_string(),
        assistant_message_id: "auto-mcp-assistant".to_string(),
        terminal_status: mycopilot_core::ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: true,
        items: vec![
            mycopilot_core::ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: frozen_approval.identity.call_id.clone(),
                tool: frozen_approval.identity.provenance.model_tool_name.clone(),
                provenance: AgentToolIdentity::Mcp {
                    provenance: frozen_approval.identity.provenance.clone(),
                },
                operation: json!({}),
                approval_status: AgentApprovalStatus::Approved,
                truncated: true,
            },
            mycopilot_core::ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: frozen_approval.identity.call_id.clone(),
                tool: frozen_approval.identity.provenance.model_tool_name.clone(),
                status: mycopilot_core::ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: durable_result.result.clone().unwrap(),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: true,
                archive: Default::default(),
            },
        ],
    };
    let mut in_progress = trace.clone();
    in_progress.terminal_status = mycopilot_core::ConversationTurnTraceTerminalStatus::InProgress;
    storage
        .append_in_progress_conversation_turn_trace(
            &in_progress,
            projection_created_at,
            projection_created_at,
        )
        .unwrap();
    storage
        .replace_conversation_turn_trace(
            &trace,
            projection_created_at,
            projection_created_at.saturating_add(1),
        )
        .unwrap();
    let reloaded = storage
        .load_conversation("auto-mcp-conversation")
        .unwrap()
        .unwrap();
    let run: serde_json::Value = serde_json::from_str(
        reloaded.messages[0]
            .agent_run_json
            .as_deref()
            .expect("terminal MCP projection survives journal scrubbing"),
    )
    .unwrap();
    assert_eq!(run["mcpInvocations"][0]["actionId"], action_id);
    assert_eq!(run["mcpInvocations"][0]["invocationId"], invocation_id);
    assert_eq!(run["mcpInvocations"][0]["state"], "completed");
    assert_eq!(run["mcpInvocations"][0]["outcome"], "succeeded");
    assert_eq!(
        run["mcpInvocations"][0]["dispatchCertainty"],
        "response_received"
    );
    assert_eq!(run["timeline"][0]["type"], "mcp_tool_call");
    let encoded = run.to_string();
    assert!(!encoded.contains("AUTO_RESULT_CANARY_NOT_DURABLE"));
}

#[tokio::test]
async fn live_auto_mcp_outcome_unknown_is_durable_and_never_collapses_to_plain_failure() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-outcome-unknown.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let invoker = AutoJournalObservingInvoker::with_outcome_unknown(Arc::clone(&storage), true);
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let run_id = "auto-mcp-outcome-unknown-run";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = auto_mcp_action(
        run_id,
        &action_id,
        &invocation_id,
        mycopilot_core::storage::now_ms(),
    );
    let AgentProposedAction::McpToolCall { approval } = action else {
        unreachable!();
    };
    let context = AutoApprovedActionContext::new(
        auto_mcp_agent_input(&storage, run_id, &action_id),
        run_id.to_string(),
        None,
        None,
        None,
    );

    let result = service
        .execute_auto_mcp_tool_action(&context, approval, AgentCancellationToken::new())
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(
        result
            .result
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str),
        Some("mcp.tool_outcome_unknown")
    );
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());

    let storage_id = pending_action_storage_id(run_id, &action_id);
    let terminal: (String, String, String) = rusqlite::Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT pending.status, pending.action_json, audit.error
             FROM agent_pending_actions pending
             JOIN agent_action_audit audit ON audit.action_id = pending.action_id
             WHERE pending.action_id = ?1",
            [&storage_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(terminal.0, "failed");
    assert_eq!(terminal.1, "{}");
    assert_eq!(terminal.2, "mcp.tool_outcome_unknown");

    drop(service);
    let restarted = AgentService::try_new(Arc::clone(&storage)).unwrap();
    assert!(restarted.list_pending_actions().is_empty());
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a durably settled outcome-unknown invocation must never replay"
    );
}

#[test]
fn startup_adopts_a_terminal_auto_mcp_result_when_its_dispatch_journal_lagged() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-terminal-adoption.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let run_id = "auto-mcp-terminal-adoption-run";
    let conversation_id = "auto-mcp-terminal-adoption-conversation";
    let assistant_message_id = "auto-mcp-terminal-adoption-assistant";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let mut action = auto_mcp_action(run_id, &action_id, &invocation_id, now);
    let AgentProposedAction::McpToolCall { approval } = &mut action else {
        unreachable!("fixture always creates an external MCP action")
    };
    // External MCP arguments live only in the authenticated payload envelope. The durable
    // ToolCall operation is its safe projection and cannot be used to recompute this digest.
    approval.identity.arguments_digest =
        mycopilot_core::mcp_tool_arguments_digest(&json!({ "private": "omitted" })).unwrap();
    seed_durable_mcp_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    let mut journal = service
        .prepare_auto_mcp_action_journal(
            run_id,
            Some(conversation_id),
            Some(assistant_message_id),
            action.clone(),
            auto_mcp_agent_input(&storage, run_id, &action_id),
        )
        .unwrap();
    service.claim_auto_mcp_dispatch(&mut journal).unwrap();
    append_terminal_auto_mcp_outcome_unknown(
        &storage,
        &database_path,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    let trace_before = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace_before.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    drop(service);

    let invoker = Arc::new(RecoverableApprovalRaceInvoker::default());
    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 1);
    assert!(restarted.list_pending_actions().is_empty());
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "startup adoption must never replay the external invocation"
    );

    let durable = storage
        .get_pending_agent_action(&journal.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "failed");
    assert_eq!(durable.target_status.as_deref(), Some("failed"));
    assert_eq!(durable.action_json, "{}");
    assert_eq!(durable.agent_input_json, "{}");
    let trace_after = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(trace_after, trace_before);
    trace_after
        .validate_complete_model_context(
            &storage
                .get_conversation_model_context_log(assistant_message_id)
                .unwrap()
                .unwrap()
                .items,
        )
        .unwrap();
}

#[test]
fn startup_rejects_malformed_terminal_auto_mcp_result_instead_of_adopting_it() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-malformed-terminal.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let run_id = "auto-mcp-malformed-terminal-run";
    let conversation_id = "auto-mcp-malformed-terminal-conversation";
    let assistant_message_id = "auto-mcp-malformed-terminal-assistant";
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = auto_mcp_action(run_id, &action_id, &invocation_id, now);
    seed_durable_mcp_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    let mut journal = service
        .prepare_auto_mcp_action_journal(
            run_id,
            Some(conversation_id),
            Some(assistant_message_id),
            action.clone(),
            auto_mcp_agent_input(&storage, run_id, &action_id),
        )
        .unwrap();
    service.claim_auto_mcp_dispatch(&mut journal).unwrap();
    append_terminal_auto_mcp_outcome_unknown(
        &storage,
        &database_path,
        conversation_id,
        assistant_message_id,
        run_id,
        &action,
        now,
    );
    drop(service);

    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE conversation_turn_trace_items
             SET item_json = json_set(item_json, '$.observation.contentOmitted', false)
             WHERE assistant_message_id = ?1 AND item_kind = 'tool_result'",
            [assistant_message_id],
        )
        .unwrap();

    let restarted =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    assert!(restarted.reconcile_startup_mcp_actions().is_err());
    let durable = storage
        .get_pending_agent_action(&journal.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.status, "executing");
    assert!(durable.target_status.is_none());
}

#[test]
fn startup_auto_mcp_journals_never_replay_and_only_executing_becomes_outcome_unknown() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("auto-mcp-restart.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "test-model",
        "https://example.test/v1/chat/completions",
        "test-token",
        "disabled",
        "",
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let now = mycopilot_core::storage::now_ms();
    let mut identities = Vec::new();
    for should_claim in [false, true] {
        let run_id = format!("auto-mcp-restart-{should_claim}");
        let action_id = uuid::Uuid::new_v4().to_string();
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let action = auto_mcp_action(&run_id, &action_id, &invocation_id, now);
        let conversation_id = format!("auto-mcp-restart-{should_claim}-conversation");
        let assistant_message_id = format!("auto-mcp-restart-{should_claim}-assistant");
        seed_durable_mcp_pending_owner(
            &storage,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &action,
            now,
        );
        let mut record = service
            .prepare_auto_mcp_action_journal(
                &run_id,
                Some(&conversation_id),
                Some(&assistant_message_id),
                action,
                auto_mcp_agent_input(&storage, &run_id, &action_id),
            )
            .unwrap();
        if should_claim {
            service.claim_auto_mcp_dispatch(&mut record).unwrap();
        }
        assert!(service.list_pending_actions().is_empty());
        identities.push((record.storage_id, should_claim));
    }
    drop(service);

    let invoker = Arc::new(RecoverableApprovalRaceInvoker::default());
    let restarted = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>)
        .with_mcp_startup_inspector(Arc::new(StaticMcpStartupInspector(
            McpApprovalStartupPayloadState::DurableAvailable,
        )));
    assert_eq!(restarted.reconcile_startup_mcp_actions().unwrap(), 2);
    assert!(restarted.list_pending_actions().is_empty());
    assert!(storage
        .list_recoverable_agent_actions_after_reconciliation()
        .unwrap()
        .is_empty());
    assert_eq!(
        invoker
            .invocations
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    let connection = rusqlite::Connection::open(database_path).unwrap();
    for (storage_id, was_claimed) in identities {
        let terminal: (String, String, Option<String>) = connection
            .query_row(
                "SELECT pending.status, pending.action_json, audit.error
                 FROM agent_pending_actions pending
                 LEFT JOIN agent_action_audit audit ON audit.action_id = pending.action_id
                 WHERE pending.action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(terminal.0, "failed");
        assert_eq!(terminal.1, "{}");
        assert_eq!(
            terminal.2.as_deref(),
            if was_claimed {
                Some("mcp.tool_outcome_unknown")
            } else {
                Some("mcp.approval_policy_denied")
            }
        );
    }
}

fn append_terminal_auto_mcp_outcome_unknown(
    storage: &StorageService,
    database_path: &std::path::Path,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    action: &AgentProposedAction,
    created_at: i64,
) {
    let AgentProposedAction::McpToolCall { approval } = action else {
        panic!("test helper requires an MCP action");
    };
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    let model_context_items = storage
        .get_conversation_model_context_log(assistant_message_id)
        .unwrap()
        .unwrap()
        .items;
    let next_sequence = trace
        .items
        .last()
        .map(ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .checked_add(1)
        .unwrap();
    let snapshot = ConversationTraceSnapshot {
        items: trace.items,
        model_context_items,
        next_sequence,
        truncated: trace.truncated,
    };
    let result = mycopilot_core::mcp_tool_result_persistence_projection(&AgentToolResult {
        exact_archive_file: None,
        call_id: approval.identity.call_id.clone(),
        tool: approval.identity.provenance.model_tool_name.clone(),
        ok: false,
        result: Some(json!({
            "schemaVersion": 1,
            "type": "mcp_tool",
            "status": "outcome_unknown",
            "outcome": "outcome_unknown",
            "code": "mcp.tool_outcome_unknown",
            "retryable": false,
            "external": true,
            "dispatchCertainty": "possibly_dispatched",
            "isError": true
        })),
        error: Some("The external MCP tool reported an error.".to_string()),
    });
    let mut terminal = mycopilot_core::terminal_conversation_trace_from_snapshot_with_tool_result(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Completed,
        "",
        &result,
    )
    .unwrap();
    terminal.trace.terminal_error = None;
    terminal
        .trace
        .validate_complete_model_context(&terminal.model_context_items)
        .unwrap();
    let usage = AgentUsageRecordInsert {
        id: format!("usage-{assistant_message_id}"),
        conversation_id: conversation_id.to_string(),
        message_id: assistant_message_id.to_string(),
        run_id: run_id.to_string(),
        project_id: None,
        model_id: "test-model".to_string(),
        model_name: "Test Model".to_string(),
        started_at: Some(created_at),
        completed_at: Some(created_at.saturating_add(1)),
        status: Some("completed".to_string()),
        error: None,
        created_at,
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
    };
    storage
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            conversation_id,
            assistant_message_id,
            "",
            Some("sent"),
            "completed",
            &terminal.trace,
            Some(&terminal.model_context_items),
            created_at,
            created_at.saturating_add(1),
            Some(&usage),
            None,
        )
        .unwrap();
    let connection = rusqlite::Connection::open(database_path).unwrap();
    let raw: String = connection
        .query_row(
            "SELECT agent_run_json FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut run: Value = serde_json::from_str(&raw).unwrap();
    let invocation = run["mcpInvocations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|invocation| invocation["actionId"] == approval.identity.action_id)
        .unwrap();
    *invocation = json!({
        "actionId": approval.identity.action_id,
        "invocationId": approval.identity.invocation_id,
        "callId": approval.identity.call_id,
        "serverId": approval.identity.provenance.server_id,
        "serverDisplayName": approval.summary.server_display_name,
        "scope": approval.identity.provenance.scope,
        "rawToolName": approval.identity.provenance.raw_tool_name,
        "modelToolName": approval.identity.provenance.model_tool_name,
        "external": true,
        "state": "outcome_unknown",
        "dispatchCertainty": "possibly_dispatched",
        "outcome": "outcome_unknown",
        "errorCode": "mcp.tool_outcome_unknown",
        "durationMs": 1,
        "outputTruncated": false
    });
    connection
        .execute(
            "UPDATE messages SET agent_run_json = ?3
             WHERE conversation_id = ?1 AND id = ?2",
            rusqlite::params![conversation_id, assistant_message_id, run.to_string()],
        )
        .unwrap();
}
