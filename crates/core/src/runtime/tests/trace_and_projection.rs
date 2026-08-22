use super::*;

#[test]
fn safe_boundary_delivery_uses_one_byte_exact_authenticated_envelope() {
    let delivery = AgentSamplingBoundaryDelivery {
        receipt_id: "receipt-safe".into(),
        messages: vec![AgentSamplingBoundaryMessage {
            trace_sequence: 0,
            message_id: "message-safe".into(),
            sender_agent_id: "agent-child".into(),
            sender_task_name: "child".into(),
            sender_task_path: "/root/child".into(),
            kind: crate::AgentMailboxKind::Result,
            content: "  exact payload  ".into(),
            created_at: 1,
        }],
    };
    let mut context = ContextFrame::new(Vec::new());
    let recorder = Arc::new(Mutex::new(ConversationTraceRecorder::default()));
    apply_agent_mailbox_delivery(&delivery, &mut context, &recorder, None, "assistant-safe")
        .unwrap();
    let snapshot = recorder
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .snapshot();
    let trace_content = match &snapshot.items[0] {
        ConversationTurnTraceItem::AgentMailboxDelivery { content, .. } => content.clone(),
        item => panic!("unexpected trace item: {item:?}"),
    };
    let model_content = snapshot.model_context_items[0].content.clone();
    let live_content = context.to_messages()[0].content().to_string();
    assert_eq!(trace_content, model_content);
    assert_eq!(model_content, live_content);
    let envelope: serde_json::Value = serde_json::from_str(&live_content).unwrap();
    assert_eq!(envelope["type"], "agent_collaboration_input");
    assert_eq!(envelope["payload"], "exact payload");
    assert!(envelope["payload"]
        .as_str()
        .unwrap()
        .find("agent_collaboration_input")
        .is_none());
}

#[test]
fn resume_accepts_only_tool_provenance_from_the_frozen_registry() {
    let registry = ToolRegistry::defaults_with_search(None);
    let registered_call = AgentToolCall {
        id: "registered-call".to_string(),
        tool: "read_file".to_string(),
        args: json!({ "path": "README.md" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let mut registered = ConversationTraceRecorder::default();
    registered.record_tool_call_with_identity(
        &registered_call,
        registry
            .identity(&registered_call.tool)
            .expect("default registry identity")
            .clone(),
    );
    validate_resumed_tool_provenance(&registered, &registry).unwrap();

    let unregistered_call = AgentToolCall {
        id: "unregistered-call".to_string(),
        tool: "hallucinated_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let mut unregistered = ConversationTraceRecorder::default();
    unregistered.record_tool_call_with_identity(
        &unregistered_call,
        AgentToolIdentity::Unregistered {
            tool_name: unregistered_call.tool.clone(),
        },
    );
    assert!(validate_resumed_tool_provenance(&unregistered, &registry).is_err());

    let rejected_unknown_result = AgentToolResult {
        exact_archive_file: None,
        call_id: unregistered_call.id.clone(),
        tool: unregistered_call.tool.clone(),
        ok: false,
        result: None,
        error: Some("tool is not registered".to_string()),
    };
    unregistered.record_tool_result(&unregistered_call, &rejected_unknown_result);
    validate_resumed_tool_provenance(&unregistered, &registry)
        .expect("a settled unknown call is audit history, not resumed execution authority");

    let rejected_but_not_failed = ConversationTraceRecorder::from_checkpoint_with_model_context(
        vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: unregistered_call.id.clone(),
                tool: unregistered_call.tool.clone(),
                provenance: AgentToolIdentity::Unregistered {
                    tool_name: unregistered_call.tool.clone(),
                },
                operation: json!({}),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 1,
                call_id: unregistered_call.id.clone(),
                tool: unregistered_call.tool.clone(),
                status: crate::conversation_trace::ConversationTraceToolResultStatus::Rejected,
                success: false,
                observation: json!({}),
                approval_status: AgentApprovalStatus::NotRequired,
                error: Some("rejected".to_string()),
                truncated: false,
                archive: Default::default(),
            },
        ],
        Vec::new(),
        2,
        false,
    );
    assert!(validate_resumed_tool_provenance(&rejected_but_not_failed, &registry).is_err());

    let non_adjacent_failure = ConversationTraceRecorder::from_checkpoint_with_model_context(
        vec![
            ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: unregistered_call.id.clone(),
                tool: unregistered_call.tool.clone(),
                provenance: AgentToolIdentity::Unregistered {
                    tool_name: unregistered_call.tool.clone(),
                },
                operation: json!({}),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            },
            ConversationTurnTraceItem::AssistantNarration {
                sequence: 1,
                content: "unexpected gap".to_string(),
                truncated: false,
            },
            ConversationTurnTraceItem::ToolResult {
                sequence: 2,
                call_id: unregistered_call.id.clone(),
                tool: unregistered_call.tool.clone(),
                status: crate::conversation_trace::ConversationTraceToolResultStatus::Failed,
                success: false,
                observation: json!({}),
                approval_status: AgentApprovalStatus::NotRequired,
                error: Some("failed too late".to_string()),
                truncated: false,
                archive: Default::default(),
            },
        ],
        Vec::new(),
        3,
        false,
    );
    assert!(validate_resumed_tool_provenance(&non_adjacent_failure, &registry).is_err());

    let mut legacy_builtin_capability = ConversationTraceRecorder::default();
    legacy_builtin_capability.record_tool_call_with_identity(
        &registered_call,
        AgentToolIdentity::LegacyBuiltinCapability {
            capability_id: "browser_automation".into(),
            managed_mcp_id: "builtin.browser_automation.mcp".into(),
            manifest_digest: format!("sha256:{}", "a".repeat(64)).into(),
            tool_id: registered_call.tool.clone().into(),
            model_name: registered_call.tool.clone().into(),
        },
    );
    assert!(validate_resumed_tool_provenance(&legacy_builtin_capability, &registry).is_err());

    let mut spoofed = ConversationTraceRecorder::default();
    spoofed.record_tool_call_with_identity(
        &registered_call,
        AgentToolIdentity::RuntimeExtension {
            extension_id: "extension-spoof".to_string(),
            tool_name: registered_call.tool.clone(),
        },
    );
    assert!(validate_resumed_tool_provenance(&spoofed, &registry).is_err());
}

#[test]
fn provider_profile_context_boundaries_fail_closed_without_translating_private_state() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::provider_continuation_store::ProviderContinuationBinding;
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey,
    };
    use tempfile::tempdir;

    const CONVERSATION_ID: &str = "conversation-provider-boundary";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-provider-boundary";
    const RUN_ID: &str = "run-provider-boundary";
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("boundary.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some("deepseek-boundary-a".to_string()),
            title: "Provider boundary".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("completed".to_string()),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault =
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap();
    let deepseek_profile = ProviderProfileConfig::deepseek_v4_default();
    let deepseek_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &deepseek_profile,
        "deepseek-boundary-a",
        Some("configuration-a".to_string()),
    )
    .unwrap();
    let provider_call = LlmToolCall {
        id: "provider-boundary-call".to_string(),
        name: "read_file".to_string(),
        args: json!({ "path": "README.md" }),
    };
    let runtime_call = LlmToolCall {
        id: crate::llm::model_response_tool_call_id(RUN_ID, 0, 0, &provider_call.id),
        name: provider_call.name.clone(),
        args: provider_call.args.clone(),
    };
    let turn = crate::llm::LlmAssistantTurn::from_provider(
        deepseek_protocol.clone(),
        "",
        vec![provider_call.clone()],
    )
    .unwrap()
    .with_runtime_tool_bindings(vec![crate::llm::LlmRuntimeToolCallBinding::new(
        0,
        &provider_call,
        runtime_call.clone(),
    )])
    .unwrap();
    let turn_id = turn.stable_id();
    let turn_digest = turn.stable_digest();
    let binding = ProviderContinuationBinding {
        conversation_id: CONVERSATION_ID,
        assistant_message_id: ASSISTANT_MESSAGE_ID,
        run_id: RUN_ID,
        request_index: 0,
        assistant_turn_id: &turn_id,
        assistant_turn_digest: &turn_digest,
        provider_protocol: &deepseek_protocol,
    };
    let continuation_ref = vault
        .persist_staged(binding, &turn)
        .unwrap()
        .expect("DeepSeek tool-bearing turn must receive a private replay ref");
    vault
        .promote_staged(&continuation_ref, binding, now_ms())
        .unwrap();

    let generic_profile =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions);
    let generic_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &generic_profile,
        "generic-model",
        Some("generic-configuration".to_string()),
    )
    .unwrap();
    let mut clean_context = ContextFrame::new(Vec::new());
    let generic_error = hydrate_provider_continuation_history(
        &mut clean_context,
        &generic_profile,
        &generic_protocol,
        Some(CONVERSATION_ID),
        Some(storage.as_ref()),
        None,
        ProviderContinuationResumeRequirement {
            required_refs: None,
            current_assistant_turn_id: None,
        },
    )
    .unwrap_err();
    assert_eq!(
        generic_error.code(),
        Some("provider_context_boundary_required")
    );

    let grouped_context = || {
        ContextFrame::new(vec![ContextItem::new(
            LlmMessage::assistant("", vec![runtime_call.clone()]),
            ContextMetadata::new(
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        )])
    };
    let mut missing_context = grouped_context();
    let missing_error = hydrate_provider_continuation_history(
        &mut missing_context,
        &deepseek_profile,
        &deepseek_protocol,
        Some("conversation-without-provider-state"),
        Some(storage.as_ref()),
        Some(&vault),
        ProviderContinuationResumeRequirement {
            required_refs: None,
            current_assistant_turn_id: None,
        },
    )
    .unwrap_err();
    assert_eq!(
        missing_error.code(),
        Some("provider_context_boundary_required")
    );

    let mismatched_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &deepseek_profile,
        "deepseek-boundary-b",
        Some("configuration-b".to_string()),
    )
    .unwrap();
    let mut mismatched_context = grouped_context();
    let mismatch = hydrate_provider_continuation_history(
        &mut mismatched_context,
        &deepseek_profile,
        &mismatched_protocol,
        Some(CONVERSATION_ID),
        Some(storage.as_ref()),
        Some(&vault),
        ProviderContinuationResumeRequirement {
            required_refs: None,
            current_assistant_turn_id: None,
        },
    )
    .unwrap_err();
    assert!(matches!(
        mismatch.code(),
        Some("provider_context_boundary_required") | Some("provider_continuation_corrupt")
    ));

    // Recoverable staged rows are promoted after the first durable ToolCall. If the process
    // crashes before its ToolResult becomes durable, the exact provider turn can still be
    // decrypted, but it must never become a model request with an orphan ToolCall.
    let mut incomplete_context = ContextFrame::new(vec![ContextItem::new(
        LlmMessage::assistant("", vec![runtime_call]),
        ContextMetadata::new(
            ContextSource::ConversationTrace,
            ContextScope::Conversation,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::conversation_trace_item(
            ASSISTANT_MESSAGE_ID,
            0,
        )),
    )]);
    hydrate_provider_continuation_history(
        &mut incomplete_context,
        &deepseek_profile,
        &deepseek_protocol,
        Some(CONVERSATION_ID),
        Some(storage.as_ref()),
        Some(&vault),
        ProviderContinuationResumeRequirement {
            required_refs: None,
            current_assistant_turn_id: None,
        },
    )
    .unwrap();
    let incomplete_error = incomplete_context
        .validate_complete_tool_protocol()
        .unwrap_err();
    assert!(incomplete_error.to_string().contains("工具调用缺少结果"));
}

#[test]
fn deepseek_cancellation_closes_current_and_queued_calls_in_original_order() {
    let registry = ToolRegistry::defaults_with_search(None);
    let first_call_id =
        crate::llm::model_response_tool_call_id("run-deepseek-cancel", 0, 0, "cancel-call-1");
    let second_call_id =
        crate::llm::model_response_tool_call_id("run-deepseek-cancel", 0, 1, "cancel-call-2");
    let calls = vec![
        LlmToolCall {
            id: first_call_id.clone(),
            name: "read_file".to_string(),
            args: json!({ "path": "first.txt" }),
        },
        LlmToolCall {
            id: second_call_id.clone(),
            name: "read_file".to_string(),
            args: json!({ "path": "second.txt" }),
        },
    ];
    let mut batch = ToolCallBatch::from_model_response(
        "run-deepseek-cancel",
        0,
        String::new(),
        calls,
        false,
        |call| (call.clone(), registry.checkpoint_persistence(&call.name)),
    );
    let mut pending_assistant_context = take_pending_assistant_tool_context(&mut batch).unwrap();
    let current = batch.pop_front().expect("first grouped call");
    let current_call = AgentToolCall {
        id: current.call.id.clone(),
        tool: current.call.name.clone(),
        args: current.call.args.clone(),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut context = ContextFrame::new(Vec::new());
    let recorder = Arc::new(Mutex::new(ConversationTraceRecorder::default()));
    let mut events = AgentEventStream::new(None);
    let gate = ContextCapacityDetector::for_model(
        "deepseek-cancel-test",
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &registry.definitions(),
    )
    .model_tool_result_gate();

    settle_cancelled_grouped_tool_batch(
        Some(TerminalToolCallSettlement {
            queued: current,
            call: Some(current_call),
            announced: false,
            dispatch_started: false,
            outcome: TerminalToolCallOutcome::Synthetic,
        }),
        &mut batch,
        &mut pending_assistant_context,
        &mut context,
        &recorder,
        None,
        &mut events,
        &registry,
        &gate,
        Some("assistant-deepseek-cancel"),
        "run-deepseek-cancel",
    )
    .unwrap();

    assert!(batch.is_empty());
    assert!(pending_assistant_context.is_none());
    context.validate_complete_tool_protocol().unwrap();

    let trace = recorder.lock().unwrap().finish(
        "run-deepseek-cancel",
        "conversation-deepseek-cancel",
        "assistant-deepseek-cancel",
        ConversationTurnTraceTerminalStatus::Cancelled,
        None,
    );
    let ordered = trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => {
                Some(("call", call_id.as_str(), None))
            }
            ConversationTurnTraceItem::ToolResult {
                call_id, status, ..
            } => Some(("result", call_id.as_str(), Some(*status))),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ordered,
        vec![
            ("call", first_call_id.as_str(), None),
            (
                "result",
                first_call_id.as_str(),
                Some(ConversationTraceToolResultStatus::Cancelled),
            ),
            ("call", second_call_id.as_str(), None),
            (
                "result",
                second_call_id.as_str(),
                Some(ConversationTraceToolResultStatus::Cancelled),
            ),
        ]
    );

    let emitted = events.into_events();
    let event_order = emitted
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCall { call, .. } => Some(("call", call.id.as_str())),
            AgentEvent::ToolResult { result, .. } => Some(("result", result.call_id.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        event_order,
        vec![
            ("call", first_call_id.as_str()),
            ("result", first_call_id.as_str()),
            ("call", second_call_id.as_str()),
            ("result", second_call_id.as_str()),
        ]
    );
}

#[test]
fn exact_history_archive_precedes_model_and_checkpoint_projection() {
    use crate::conversation_trace::ConversationTraceRecorder;
    use crate::protocol::{AgentApprovalStatus, AgentToolCall, AgentToolResult};
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus};
    use std::sync::Arc;
    use tempfile::tempdir;

    let fixture = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&fixture.path().join("exact-history.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-archive".to_string(),
            project_id: None,
            model_id: None,
            title: "archive".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-archive".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
    let call = AgentToolCall {
        id: "call-archive".to_string(),
        tool: "web_fetch".to_string(),
        args: json!({ "url": "https://example.com" }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(json!({
            "url": "https://example.com",
            "content": "精确正文".repeat(10_000),
            "truncated": true
        })),
        error: None,
    };
    let model_result = AgentToolResult {
        result: Some(json!({
            "summary": "bounded model projection ".repeat(4_000),
            "truncated": true
        })),
        ..raw.clone()
    };
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_tool_call(&call);
    let sequence = recorder.pending_tool_result_sequence(&call.id).unwrap();
    let gate = ContextCapacityDetector::for_model(
        "test-model",
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &[],
    )
    .model_tool_result_gate();
    let metadata = super::archive_tool_result(super::ToolResultArchiveRequest {
        storage: Some(&storage),
        conversation_id: Some("conversation-archive"),
        assistant_message_id: Some("assistant-archive"),
        sequence: Some(sequence),
        raw_result: &raw,
        archive_result: &raw,
        model_result: &model_result,
        model_tool_result_gate: &gate,
    })
    .expect("archive tool result");
    assert_eq!(metadata.archived_completely, Some(true));
    assert!(metadata.truncated_at_source);
    assert!(metadata.model_projection_truncated);
    let (observation, checkpoint_observation) = finalize_tool_observations(
        &gate,
        &call.id,
        false,
        &model_result,
        &raw,
        &metadata,
        false,
    )
    .unwrap();
    let projected: Value = serde_json::from_str(&observation).unwrap();
    let checkpoint_projected: Value = serde_json::from_str(&checkpoint_observation).unwrap();
    for projection in [&projected, &checkpoint_projected] {
        assert_eq!(projection["truncated"], true);
        assert_eq!(projection["truncatedAtSource"], true);
        assert!(projection["originalBytes"].as_u64().unwrap() > 0);
        assert_eq!(projection["continueWith"]["tool"], "conversation_history");
        assert!(projection["historyOpen"]
            .as_str()
            .unwrap()
            .starts_with("hist_v1_"));
    }
    assert_eq!(
        checkpoint_projected, projected,
        "persisted model history must replay the exact live Model projection"
    );
    assert!(
        gate.would_truncate_with_source(&call.id, false, &model_result, false),
        "the model fixture must exercise the central length gate"
    );
    assert!(
        gate.would_truncate_with_source(&call.id, false, &raw, false),
        "the checkpoint fixture must exercise the central length gate"
    );
    recorder.record_tool_result_with_archive(&call, &raw, metadata.clone());
    let trace = recorder.finish(
        "run-archive",
        "conversation-archive",
        "assistant-archive",
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    );
    trace.validate().unwrap();
    let ConversationTurnTraceItem::ToolResult {
        observation,
        archive,
        ..
    } = &trace.items[1]
    else {
        panic!("expected durable tool result");
    };
    assert!(observation["summary"].as_str().unwrap().chars().count() <= 4_100);
    assert!(archive.history_projection_truncated);
    assert_eq!(archive.content_hash, metadata.content_hash);

    let page = storage
        .read_conversation_history_archive_page(
            "conversation-archive",
            archive.archive_ref.as_deref().unwrap(),
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            0,
            u64::MAX,
        )
        .unwrap()
        .unwrap();
    let restored: AgentToolResult = serde_json::from_str(&page.content).unwrap();
    assert_eq!(
        restored.result.unwrap()["content"],
        raw.result.unwrap()["content"]
    );
}

#[test]
fn process_spool_is_archived_exactly_and_forces_a_recovery_route() {
    use crate::command::{
        join_process_output_capture, materialize_process_tool_result_archive,
        process_output_spool_substitutions, spawn_process_output_capture,
        ProcessOutputCaptureBudget, ProcessOutputCapturePolicy,
    };
    use crate::protocol::AgentToolResult;
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use std::io::Cursor;
    use std::sync::Arc;
    use tempfile::tempdir;

    let fixture = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&fixture.path().join("process-spool.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-process".to_string(),
            project_id: None,
            model_id: None,
            title: "process".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-process".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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

    let full_stdout = "完整进程输出-".repeat(200);
    let policy = ProcessOutputCapturePolicy::with_limits(16, 64 * 1024);
    let budget = ProcessOutputCaptureBudget::new(policy.max_capture_bytes());
    let stdout = join_process_output_capture(
        spawn_process_output_capture(
            Cursor::new(full_stdout.as_bytes().to_vec()),
            budget.clone(),
            policy,
        ),
        "stdout",
    )
    .unwrap();
    let stderr = join_process_output_capture(
        spawn_process_output_capture(Cursor::new(Vec::<u8>::new()), budget, policy),
        "stderr",
    )
    .unwrap();
    let mut raw = AgentToolResult {
        exact_archive_file: None,
        call_id: "call-process".to_string(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "exitCode": 0,
            "stdout": stdout.preview(),
            "stderr": stderr.preview(),
            "stdoutTruncated": stdout.preview_truncated(),
            "stderrTruncated": stderr.preview_truncated(),
            "stdoutPreviewTruncated": stdout.preview_truncated(),
            "stderrPreviewTruncated": stderr.preview_truncated(),
            "originalBytes": stdout.original_bytes(),
            "capturedBytes": stdout.captured_bytes(),
            "omittedBytes": stdout.omitted_bytes(),
            "truncatedAtSource": stdout.truncated_at_source()
        })),
        error: None,
    };
    raw.exact_archive_file = materialize_process_tool_result_archive(
        &raw,
        &process_output_spool_substitutions(&stdout.spool(), &stderr.spool()),
    )
    .unwrap();
    let registry = ToolRegistry::defaults_with_search(None);
    let archive_result = registry.archive_projection(&raw);
    let model_result = registry.model_projection(&raw);
    let gate = ContextCapacityDetector::for_model(
        "test-model",
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &[],
    )
    .model_tool_result_gate();
    let metadata = super::archive_tool_result(super::ToolResultArchiveRequest {
        storage: Some(&storage),
        conversation_id: Some("conversation-process"),
        assistant_message_id: Some("assistant-process"),
        sequence: Some(1),
        raw_result: &raw,
        archive_result: &archive_result,
        model_result: &model_result,
        model_tool_result_gate: &gate,
    })
    .expect("archive tool result");

    assert_eq!(metadata.archived_completely, Some(true));
    assert!(metadata.model_projection_truncated);
    assert!(!metadata.truncated_at_source);
    let observation =
        finalize_model_tool_observation(&gate, &raw.call_id, false, &model_result, &metadata)
            .unwrap();
    let projected: Value = serde_json::from_str(&observation).unwrap();
    assert_eq!(projected["truncated"], true);
    assert_eq!(projected["continueWith"]["tool"], "conversation_history");

    let page = storage
        .read_conversation_history_archive_page(
            "conversation-process",
            metadata.archive_ref.as_deref().unwrap(),
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            0,
            u64::MAX,
        )
        .unwrap()
        .unwrap();
    let restored: AgentToolResult = serde_json::from_str(&page.content).unwrap();
    assert_eq!(
        restored.result.unwrap()["stdout"].as_str().unwrap(),
        full_stdout
    );
}

#[test]
fn command_session_archive_route_is_reused_without_preview_rearchive() {
    use crate::command::CommandAuthorizationSource;
    use crate::protocol::{
        AgentCommandSessionSnapshot, AgentCommandSessionStatus, AgentToolResult,
    };
    use crate::storage::agent_command_session_repository::{
        AgentCommandSessionCreate, AgentCommandSessionTerminalUpdate,
        AGENT_COMMAND_SESSION_SCHEMA_VERSION,
    };
    use crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput;
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use rusqlite::Connection;
    use std::sync::Arc;
    use tempfile::tempdir;

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("command-authoritative.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-command-authoritative".to_string(),
            project_id: None,
            model_id: None,
            title: "command archive".to_string(),
            messages: vec![ChatMessageRecord {
                id: "assistant-command-authoritative".to_string(),
                role: "assistant".to_string(),
                content: String::new(),
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
    let session_id = "cmd_1234567890abcdef1234567890abcdef";
    let call_id = "call-command-authoritative";
    storage
        .create_agent_command_session(&AgentCommandSessionCreate {
            snapshot: AgentCommandSessionSnapshot {
                schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                session_id: session_id.to_string(),
                conversation_id: "conversation-command-authoritative".to_string(),
                assistant_message_id: "assistant-command-authoritative".to_string(),
                origin_run_id: "run-command-authoritative".to_string(),
                call_id: call_id.to_string(),
                project_id: None,
                command: "emit-large-output".to_string(),
                cwd: ".".to_string(),
                command_digest: format!("sha256:{}", "a".repeat(64)),
                status: AgentCommandSessionStatus::Starting,
                started_at: 2,
                ended_at: None,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                outputs: Vec::new(),
                artifact_observation: None,
                archive_ref: None,
            },
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({"decision": "approved"}),
            permission_provenance: json!({"mode": "default"}),
            created_at: 2,
        })
        .unwrap();
    storage
        .mark_agent_command_session_running("conversation-command-authoritative", session_id, 3)
        .unwrap();

    let full_stdout = (0..20_000)
        .map(|line| format!("authoritative-line-{line:05}\n"))
        .collect::<String>();
    let archived_result = AgentToolResult {
        exact_archive_file: None,
        call_id: format!("command-session:{session_id}"),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "status": "exited",
            "exitCode": 0,
            "stdout": full_stdout,
            "stderr": ""
        })),
        error: None,
    };
    let descriptor = storage
        .archive_conversation_tool_result(ConversationHistoryArchiveInput {
            conversation_id: "conversation-command-authoritative".to_string(),
            assistant_message_id: "assistant-command-authoritative".to_string(),
            sequence: 9_999,
            call_id: archived_result.call_id.clone(),
            tool: "run_command".to_string(),
            content_type: "application/vnd.mycopilot.agent-tool-result+json".to_string(),
            content: serde_json::to_string(&archived_result).unwrap(),
            truncated_at_source: false,
            model_projection_truncated: true,
            archive_projection_truncated: false,
            created_at: 4,
        })
        .unwrap();
    storage
        .settle_agent_command_session(&AgentCommandSessionTerminalUpdate {
            conversation_id: "conversation-command-authoritative",
            session_id,
            status: AgentCommandSessionStatus::Exited,
            ended_at: 5,
            exit_code: Some(0),
            latest_sequence: 0,
            transcript_truncated: true,
            output_capture_truncated: false,
            archive_ref: Some(&descriptor.archive_ref),
            terminal_reason: None,
            published_outputs: &[],
            artifact_observation: None,
            committed_at: 5,
        })
        .unwrap();

    let history_open = crate::storage::conversation_history_open::encode_archive_history_open(
        &descriptor.archive_ref,
        0,
    )
    .unwrap();
    let raw = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: "run_command".to_string(),
        ok: true,
        result: Some(json!({
            "status": "exited",
            "exitCode": 0,
            "stdout": "bounded preview",
            "stdoutPreviewTruncated": true,
            "historyOpen": history_open,
            "continueWith": {
                "tool": "conversation_history",
                "args": { "open": history_open }
            }
        })),
        error: None,
    };
    let registry = ToolRegistry::defaults_with_search(None);
    let archive_result = registry.archive_projection(&raw);
    let model_result = registry.model_projection(&raw);
    let gate = ContextCapacityDetector::for_model(
        "test-model",
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &[],
    )
    .model_tool_result_gate();
    let metadata = super::archive_tool_result(super::ToolResultArchiveRequest {
        storage: Some(&storage),
        conversation_id: Some("conversation-command-authoritative"),
        assistant_message_id: Some("assistant-command-authoritative"),
        sequence: Some(1),
        raw_result: &raw,
        archive_result: &archive_result,
        model_result: &model_result,
        model_tool_result_gate: &gate,
    })
    .expect("archive tool result");
    assert_eq!(
        metadata.archive_ref.as_deref(),
        Some(descriptor.archive_ref.as_str())
    );
    assert_eq!(
        storage
            .resolve_authoritative_command_archive_metadata(
                "conversation-command-authoritative",
                "assistant-command-authoritative",
                &raw,
            )
            .unwrap(),
        Some(metadata.clone())
    );
    let mut wrong_call = raw.clone();
    wrong_call.call_id = "different-call".to_string();
    assert!(storage
        .resolve_authoritative_command_archive_metadata(
            "conversation-command-authoritative",
            "assistant-command-authoritative",
            &wrong_call,
        )
        .unwrap_err()
        .contains("does not belong"));
    let mut malformed_route = raw.clone();
    malformed_route.result.as_mut().unwrap()["historyOpen"] = json!("not-a-history-route");
    assert!(storage
        .resolve_authoritative_command_archive_metadata(
            "conversation-command-authoritative",
            "assistant-command-authoritative",
            &malformed_route,
        )
        .unwrap_err()
        .contains("historyOpen is invalid"));
    let mut nonzero_route = raw.clone();
    nonzero_route.result.as_mut().unwrap()["historyOpen"] = json!(
        crate::storage::conversation_history_open::encode_archive_history_open(
            &descriptor.archive_ref,
            1,
        )
        .unwrap()
    );
    assert!(storage
        .resolve_authoritative_command_archive_metadata(
            "conversation-command-authoritative",
            "assistant-command-authoritative",
            &nonzero_route,
        )
        .unwrap_err()
        .contains("beginning of an Exact Archive"));
    let mut rejected_durable_route = raw.clone();
    let rejected_body = rejected_durable_route
        .result
        .as_mut()
        .unwrap()
        .as_object_mut()
        .unwrap();
    rejected_body.remove("historyOpen");
    rejected_body.insert("historyOpenInvalid".to_string(), json!(true));
    assert!(storage
        .resolve_authoritative_command_archive_metadata(
            "conversation-command-authoritative",
            "assistant-command-authoritative",
            &rejected_durable_route,
        )
        .unwrap_err()
        .contains("refusing fallback archival"));
    let archive_count: u64 = Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            ["conversation-command-authoritative"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        archive_count, 1,
        "bounded preview must not be archived again"
    );

    let observation =
        finalize_model_tool_observation(&gate, call_id, false, &model_result, &metadata).unwrap();
    let projected: Value = serde_json::from_str(&observation).unwrap();
    let model_open = projected["historyOpen"].as_str().unwrap();
    let exact = storage
        .read_conversation_history_archive_page_from_open(
            "conversation-command-authoritative",
            model_open,
            u64::MAX,
        )
        .unwrap()
        .unwrap();
    assert!(exact.content.contains("authoritative-line-19999"));
}

#[test]
fn major_tool_result_projections_match_consumer_contract_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/tool_result_projection_contract_v1.json"
    ))
    .unwrap();
    assert_eq!(fixture["schemaVersion"], 1);

    let registry = ToolRegistry::defaults_with_search(Some(&AgentSearchConfig {
        mode: AgentSearchMode::Tavily,
        tavily_api_key: Some("tvly-contract-fixture".to_string()),
    }));
    let cases = fixture["cases"].as_array().expect("contract cases");

    for case in cases {
        let name = case["name"].as_str().expect("contract case name");
        let raw: AgentToolResult = serde_json::from_value(case["toolResult"].clone())
            .unwrap_or_else(|error| {
                panic!("invalid raw ToolResult in consumer contract `{name}`: {error}")
            });
        assert!(
            registry.contains_tool(&raw.tool),
            "consumer contract `{name}` names an unregistered Tool `{}`",
            raw.tool
        );

        let projections = [
            ("model", registry.model_projection(&raw)),
            (
                "rendererEvent",
                redact_tool_result_for_event(&registry.event_projection(&raw)),
            ),
            ("durableTrace", registry.trace_projection(&raw)),
            ("exactArchive", registry.archive_projection(&raw)),
            ("checkpoint", registry.checkpoint_projection(&raw)),
        ];
        let raw_value = serde_json::to_value(&raw).unwrap();
        let stages = case["stages"]
            .as_object()
            .expect("consumer contract stages");

        for (stage_name, projection) in projections {
            let stage = stages
                .get(stage_name)
                .unwrap_or_else(|| panic!("`{name}` is missing stage `{stage_name}`"));
            let projection_value = serde_json::to_value(&projection).unwrap();

            if stage["equalsRaw"].as_bool() == Some(true) {
                assert_eq!(
                    projection_value, raw_value,
                    "`{name}` changed the `{stage_name}` snapshot"
                );
            }
            if let Some(required) = stage.get("required").and_then(Value::as_object) {
                for (pointer, expected) in required {
                    assert_eq!(
                        projection_value.pointer(pointer),
                        Some(expected),
                        "`{name}` lost `{pointer}` from `{stage_name}`"
                    );
                }
            }
            if let Some(absent) = stage.get("absent").and_then(Value::as_array) {
                for pointer in absent {
                    let pointer = pointer.as_str().expect("absent JSON pointer");
                    assert!(
                        projection_value.pointer(pointer).is_none(),
                        "`{name}` unexpectedly exposed `{pointer}` through `{stage_name}`"
                    );
                }
            }
        }
    }
}
