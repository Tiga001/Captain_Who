// Tests for runtime message construction and tool-flow helpers.
use super::*;
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::llm::LlmToolCall;
use crate::protocol::{
    AgentActivatedSkill, AgentAttachmentLibraryContext, AgentAttachmentReference,
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind,
    AgentPatchPermission, AgentRunContext, AgentSearchConfig, AgentSearchMode,
    AgentSkillActivation, AgentWorkspaceContext,
};
use crate::runtime::tool_flow::parse_tool_call_request;
use crate::tools::{EffectiveToolSet, ToolCapabilityId, OFFICE_DOCUMENTS_CAPABILITY};
use crate::{
    AnchoredWorldStateRecord, ConversationTraceToolResultStatus, ConversationTurnTrace,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus, WorldStateLifetime,
    WorldStateRecord, WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use base64::Engine;
use std::collections::BTreeSet;

fn message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        message_id: None,
        role: role.to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    }
}

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
        "../../tests/fixtures/tool_result_projection_contract_v1.json"
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

fn empty_attachment_context() -> AttachmentContext {
    AttachmentContext {
        text: String::new(),
        images: Vec::new(),
    }
}

fn assert_runtime_owned_tool_call_id(id: &str) {
    assert!(id.starts_with("tc1_"), "unexpected tool-call ID: {id}");
    assert_eq!(id.len(), 47, "unexpected canonical ID length: {id}");
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "tool-call ID contains provider-unsafe characters: {id}"
    );
}

fn activated_skill(instructions: &str) -> AgentSkillActivation {
    AgentSkillActivation {
        activation_revision: "activation-sha256-v1:test".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "workspace:workspace-1:review".to_string(),
            name: "repository-review".to_string(),
            revision: activated_skill_revision(),
            source: "workspace:workspace-1".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: None,
        }],
    }
}

fn activated_skill_revision() -> String {
    format!("skill-package-sha256-v3:{}", "d".repeat(64))
}

fn activated_skill_authority() -> Arc<crate::skills::SkillResourceSession> {
    let skill_id = crate::skills::SkillId::parse("workspace:workspace-1:review").unwrap();
    let source_id = skill_id.source_id().clone();
    Arc::new(
        crate::skills::memory_resource_session_for_test(
            skill_id,
            crate::skills::SkillRevision::parse(activated_skill_revision()).unwrap(),
            source_id,
            Vec::new(),
        )
        .unwrap(),
    )
}

fn discoverable_skill(description: &str) -> crate::skills::AgentSkillDiscoverySnapshot {
    const CATALOG_REVISION: &str = "skill-enabled-catalog-sha256-v1:test";
    const SKILL_ID: &str = "bundled:application:documents";
    let revision = format!("skill-package-sha256-v2:{}", "d".repeat(64));
    crate::skills::AgentSkillDiscoverySnapshot {
        schema_version: crate::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
        catalog_revision: CATALOG_REVISION.to_string(),
        prompt_token_budget: crate::skills::DEFAULT_SKILL_DISCOVERY_PROMPT_TOKENS,
        skills: vec![crate::skills::AgentDiscoverableSkill {
            activation_ref: crate::skills::derive_skill_activation_ref(
                CATALOG_REVISION,
                SKILL_ID,
                &revision,
            ),
            id: SKILL_ID.to_string(),
            revision,
            name: "documents".to_string(),
            description: description.to_string(),
            source_kind: "bundled".to_string(),
        }],
        max_activated_skills: 8,
        max_total_source_bytes: 512 * 1024,
    }
}

fn command_dispatch_fixture(command: &str) -> (AgentToolCall, AgentProposedAction) {
    let call = AgentToolCall {
        id: "command-dispatch".to_string(),
        tool: "run_command".to_string(),
        args: json!({ "command": command }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let action = AgentProposedAction::Command {
        command: crate::protocol::AgentCommandRequest {
            id: call.id.clone(),
            command: command.to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: AgentApprovalStatus::NotRequired,
            risk_level: None,
            reason: None,
            observe: None,
            inputs: Vec::new(),
            runtime_binding: None,
        },
    };
    (call, action)
}

fn command_permissions(command_safety: AgentCommandSafetyPolicy) -> AgentPermissions {
    AgentPermissions {
        command_safety,
        ..AgentPermissions::default()
    }
}

#[test]
fn command_dispatch_guarded_automatic_routes_high_impact_work_to_approval() {
    let (call, action) = command_dispatch_fixture("python3 -m pip install openpyxl");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::Guarded),
        None,
        true,
    );

    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));
}

#[test]
fn command_dispatch_applies_read_scope_before_automatic_execution() {
    let (call, action) = command_dispatch_fixture("cat /etc/passwd");
    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::Guarded),
        None,
        true,
    );
    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));

    let (call, action) = command_dispatch_fixture("cat /etc/passwd");
    let dispatch = prepare_command_dispatch(
        &call,
        action,
        AgentPermissions {
            read: crate::protocol::AgentReadPermission::All,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            ..AgentPermissions::default()
        },
        None,
        true,
    );
    assert!(matches!(dispatch, CommandDispatch::ExecuteAutomatically(_)));
}

#[test]
fn command_dispatch_routes_workspace_external_cwd_to_approval() {
    let (call, mut action) = command_dispatch_fixture("cat local.txt");
    let AgentProposedAction::Command { command } = &mut action else {
        unreachable!("fixture always produces a command")
    };
    command.cwd = Some("/tmp".to_string());

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        AgentPermissions {
            read: crate::protocol::AgentReadPermission::WorkspaceOnly,
            write: crate::protocol::AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            ..AgentPermissions::default()
        },
        Some(Path::new("/workspace")),
        true,
    );
    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));
}

#[test]
fn command_dispatch_full_access_automatic_executes_high_impact_work() {
    let (call, action) = command_dispatch_fixture("python3 -m pip install openpyxl");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::FullAccess),
        None,
        true,
    );

    assert!(matches!(dispatch, CommandDispatch::ExecuteAutomatically(_)));
}

#[test]
fn command_dispatch_full_access_returns_structured_rejection_for_catastrophic_command() {
    let (call, action) = command_dispatch_fixture("sudo rm -rf /");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::FullAccess),
        None,
        true,
    );

    let CommandDispatch::Reject(result) = dispatch else {
        panic!("catastrophic command must be rejected");
    };
    assert!(!result.ok);
    let payload = result.result.expect("policy rejection payload");
    assert_eq!(payload["type"], "command_policy");
    assert_eq!(payload["decision"], "deny");
    assert!(payload["code"]
        .as_str()
        .is_some_and(|code| !code.is_empty()));
    assert!(payload["findings"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
}

#[test]
fn command_dispatch_require_approval_never_auto_executes_an_allowed_command() {
    let (call, action) = command_dispatch_fixture("git status --short");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::FullAccess),
        None,
        false,
    );

    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));
}

#[test]
fn runtime_messages_add_backend_system_prompt() {
    let context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-1".to_string()),
        project_id: Some("project-1".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-1".to_string()),
            display_name: Some("Workspace".to_string()),
            root_path: Some("/private/path".to_string()),
        }),
        attachment_library: None,
        permissions: Default::default(),
    };
    let context = assemble_initial_context(
        None,
        vec![message("user", "Read src/main.rs")],
        None,
        empty_attachment_context(),
        Some(&context),
        None,
        &ToolRegistry::defaults_with_search(None).definitions(),
    )
    .unwrap();
    let messages = context.to_messages();

    assert_eq!(messages[0].role().as_str(), "system");
    assert!(messages[0].content().contains("Captain（船长）"));
    assert!(messages[0]
        .content()
        .contains("设置 → 配置 → 图片生成 → 添加水印"));
    assert!(!messages[0].content().contains("/private/path"));
    assert_eq!(messages[1].role().as_str(), "user");
}

#[test]
fn runtime_messages_include_text_attachment_content() {
    let context = assemble_initial_context(
        None,
        vec![message("user", "Summarize this attachment")],
        None,
        AttachmentContext {
            text: "用户输入框附件内容如下。\n\n### notes.txt\nhello from attachment".to_string(),
            images: Vec::new(),
        },
        None,
        None,
        &ToolRegistry::defaults_with_search(None).definitions(),
    )
    .unwrap();
    let messages = context.to_messages();

    assert!(messages
        .iter()
        .any(|message| message.role() == LlmMessageRole::User
            && message.content().contains("hello from attachment")));
}

#[test]
fn attachment_context_reads_text_with_registered_tool() {
    let context = build_attachment_context(
        &[AgentInputAttachment {
            id: "attachment-1".to_string(),
            kind: AgentInputAttachmentKind::File,
            name: "notes.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 16,
            encoding: AgentInputAttachmentEncoding::Utf8,
            data: "hello from file".to_string(),
            truncated: None,
        }],
        None,
    )
    .unwrap();

    assert!(context.text.contains("读取工具：read_file"));
    assert!(context.text.contains("hello from file"));
}

fn runtime_test_zip(entries: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;

    let mut output = std::io::Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut output);
        for (path, content) in entries {
            archive
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
    }
    output.into_inner()
}

fn runtime_test_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0_usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.into_iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn runtime_binary_attachment(
    id: &str,
    name: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> AgentInputAttachment {
    AgentInputAttachment {
        id: id.to_string(),
        kind: AgentInputAttachmentKind::File,
        name: name.to_string(),
        mime_type: Some(mime_type.to_string()),
        size_bytes: bytes.len() as u64,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        truncated: None,
    }
}

fn runtime_attachment_library(
    attachments: &[AgentInputAttachment],
) -> AgentAttachmentLibraryContext {
    AgentAttachmentLibraryContext {
        root_path: None,
        conversation_id: Some("conversation-attachments".to_string()),
        project_id: None,
        conversation_attachments: attachments
            .iter()
            .map(|attachment| AgentAttachmentReference {
                id: attachment.id.clone(),
                conversation_id: "conversation-attachments".to_string(),
                message_id: "message-attachments".to_string(),
                project_id: None,
                kind: attachment.kind,
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size_bytes: attachment.size_bytes,
                read_path: format!("@attachments/{}/{}", attachment.id, attachment.name),
                storage_rel_path: format!(
                    "conversations/conversation-attachments/message-attachments/{}/{}",
                    attachment.id, attachment.name
                ),
                created_at: 1,
            })
            .collect(),
        project_attachments: Vec::new(),
    }
}

#[test]
fn steer_attachment_library_updates_run_world_state_without_granting_conversation_identity() {
    let attachment =
        runtime_binary_attachment("steer-image", "steer.png", "image/png", vec![1, 2, 3]);
    let library = runtime_attachment_library(&[attachment]);
    let mut run_context = None;
    let mut tool_context = ToolExecutionContext::from_run_context(None);

    replace_runtime_attachment_library(&mut run_context, &mut tool_context, library.clone());

    let run_context = run_context.expect("steer library should create runtime projection");
    assert_eq!(run_context.conversation_id, None);
    assert_eq!(run_context.project_id, None);
    assert_eq!(run_context.attachment_library, Some(library));
    let mut input = conversation_context_input(vec![message("user", "inspect attachment")]);
    input.context = Some(run_context);
    let capabilities =
        prepare_runtime_capabilities(&input, "steer-attachment-world-state", &[], true, None)
            .unwrap();
    let tracker = RunWorldStateTracker::new(
        "steer-attachment-world-state",
        &input,
        &capabilities.initial_tool_set,
    )
    .unwrap();
    let rendered = tracker
        .snapshot()
        .model_projection(WorldStateLifetime::Run)
        .unwrap()
        .render_sanitized_text();
    assert!(rendered.contains("\"conversationAttachmentCount\":1"));
    assert!(rendered.contains("\"projectAttachmentCount\":0"));
    assert!(!rendered.contains("conversationAvailable"));
}

#[test]
fn attachment_context_defers_pdf_and_office_files_to_matching_skills() {
    let attachments = vec![
        runtime_binary_attachment(
            "attachment-pdf",
            "paper.pdf",
            "application/pdf",
            runtime_test_pdf("Guidance PDF"),
        ),
        runtime_binary_attachment(
            "attachment-docx",
            "document.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            runtime_test_zip(&[(
                "word/document.xml",
                r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Guidance DOCX</w:t></w:r></w:p></w:body></w:document>"#,
            )]),
        ),
        runtime_binary_attachment(
            "attachment-pptx",
            "slides.pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            runtime_test_zip(&[(
                "ppt/slides/slide1.xml",
                r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><a:t xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">Guidance PPTX</a:t></p:spTree></p:cSld></p:sld>"#,
            )]),
        ),
        runtime_binary_attachment(
            "attachment-xlsx",
            "table.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            runtime_test_zip(&[
                (
                    "xl/sharedStrings.xml",
                    r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Guidance XLSX</t></si></sst>"#,
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#,
                ),
            ]),
        ),
        runtime_binary_attachment(
            "attachment-csv",
            "table.csv",
            "text/csv",
            b"name,value\nGuidance CSV,1\n".to_vec(),
        ),
        runtime_binary_attachment(
            "attachment-tsv",
            "table.tsv",
            "text/tab-separated-values",
            b"name\tvalue\nGuidance TSV\t1\n".to_vec(),
        ),
        runtime_binary_attachment(
            "attachment-csv-mime",
            "renamed-table.txt",
            "text/csv",
            b"name,value\nGuidance MIME CSV,1\n".to_vec(),
        ),
    ];

    let library = runtime_attachment_library(&attachments);
    let context = build_attachment_context(&attachments, Some(&library)).unwrap();

    for hidden_content in [
        "Guidance PDF",
        "Guidance DOCX",
        "Guidance PPTX",
        "Guidance XLSX",
        "Guidance CSV",
        "Guidance TSV",
        "Guidance MIME CSV",
    ] {
        assert!(
            !context.text.contains(hidden_content),
            "skill-gated document content leaked during attachment preprocessing: {hidden_content}\n{}",
            context.text
        );
    }
    for attachment_id in [
        "attachment-pdf",
        "attachment-docx",
        "attachment-pptx",
        "attachment-xlsx",
        "attachment-csv",
        "attachment-tsv",
        "attachment-csv-mime",
    ] {
        assert!(
            context
                .text
                .contains(&format!("@attachments/{attachment_id}/")),
            "missing authoritative readPath for {attachment_id}\n{}",
            context.text
        );
    }
    assert_eq!(
        context
            .text
            .matches("正文未读取；先激活匹配该文件类型的 Skill")
            .count(),
        7
    );
    assert!(context.text.contains("bundled:application:pdf"));
    assert!(context.text.contains("run_command.inputs"));
    assert!(context.text.contains("read_image"));
    for hidden_contract_detail in [
        "read_word",
        "read_presentation",
        "read_spreadsheet",
        "office.documents",
        "office.presentations",
        "office.spreadsheets",
    ] {
        assert!(
            !context.text.contains(hidden_contract_detail),
            "dynamic Tool contract leaked before Skill activation: {hidden_contract_detail}"
        );
    }
}

#[test]
fn attachment_context_keeps_image_visual_input_and_authoritative_read_path() {
    let attachment = AgentInputAttachment {
        id: "attachment-image".to_string(),
        kind: AgentInputAttachmentKind::Image,
        name: "pixel.png".to_string(),
        mime_type: Some("image/png".to_string()),
        size_bytes: 3,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(b"png"),
        truncated: None,
    };
    let library = runtime_attachment_library(std::slice::from_ref(&attachment));

    let context = build_attachment_context(&[attachment], Some(&library)).unwrap();

    assert_eq!(context.images.len(), 1);
    assert!(context.text.contains("已作为视觉输入发送给模型"));
    assert!(context
        .text
        .contains("@attachments/attachment-image/pixel.png"));
}

#[test]
fn attachment_context_does_not_decode_skill_gated_office_payloads() {
    let attachment = AgentInputAttachment {
        id: "attachment-docx-invalid-payload".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "document.docx".to_string(),
        mime_type: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        ),
        size_bytes: 10,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: "not-base64".to_string(),
        truncated: None,
    };
    let library = runtime_attachment_library(std::slice::from_ref(&attachment));

    let context = build_attachment_context(&[attachment], Some(&library)).unwrap();

    assert!(context.text.contains("正文未读取"));
    assert!(context
        .text
        .contains("@attachments/attachment-docx-invalid-payload/document.docx"));
}

#[test]
fn attachment_context_routes_pdf_without_decoding_its_payload() {
    let attachment = AgentInputAttachment {
        id: "attachment-pdf-invalid-payload".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "manual.pdf".to_string(),
        mime_type: Some("application/pdf".to_string()),
        size_bytes: 10,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: "not-base64".to_string(),
        truncated: None,
    };
    let library = runtime_attachment_library(std::slice::from_ref(&attachment));

    let context = build_attachment_context(&[attachment], Some(&library)).unwrap();

    assert!(context.text.contains("正文未读取"));
    assert!(context.text.contains("bundled:application:pdf"));
    assert!(context.text.contains("run_command.inputs"));
    assert!(context
        .text
        .contains("@attachments/attachment-pdf-invalid-payload/manual.pdf"));
}

#[test]
fn activated_document_reader_can_read_the_same_authoritative_attachment_path() {
    let fixture = tempfile::tempdir().unwrap();
    let storage_rel_path = "conversations/conversation-1/message-1/attachment-docx/document.docx";
    let attachment_path = fixture.path().join(storage_rel_path);
    std::fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
    let bytes = runtime_test_zip(&[(
        "word/document.xml",
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Visible after activation</w:t></w:r></w:p></w:body></w:document>"#,
    )]);
    std::fs::write(&attachment_path, &bytes).unwrap();
    let read_path = "@attachments/attachment-docx/document.docx";
    let library = AgentAttachmentLibraryContext {
        root_path: Some(fixture.path().to_string_lossy().to_string()),
        conversation_id: Some("conversation-1".to_string()),
        project_id: None,
        conversation_attachments: vec![AgentAttachmentReference {
            id: "attachment-docx".to_string(),
            conversation_id: "conversation-1".to_string(),
            message_id: "message-1".to_string(),
            project_id: None,
            kind: AgentInputAttachmentKind::File,
            name: "document.docx".to_string(),
            mime_type: Some(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    .to_string(),
            ),
            size_bytes: u64::try_from(bytes.len()).unwrap(),
            read_path: read_path.to_string(),
            storage_rel_path: storage_rel_path.to_string(),
            created_at: 1,
        }],
        project_attachments: Vec::new(),
    };
    let registry = ToolRegistry::defaults_with_search(None);
    let active_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        OFFICE_DOCUMENTS_CAPABILITY,
    )]);
    let effective_tool_set = EffectiveToolSet::from_permitted_definitions(
        &registry,
        registry.definitions(),
        &active_capabilities,
    )
    .unwrap();
    assert!(effective_tool_set.contains("read_word"));

    let tool_context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-1".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: Some(library),
        permissions: Default::default(),
    }));
    let result = registry.execute(
        &tool_context,
        &AgentToolCall {
            id: "call-read-word".to_string(),
            tool: "read_word".to_string(),
            args: json!({ "path": read_path }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: Some("读取已激活文档技能可访问的附件".to_string()),
        },
    );

    assert!(result.ok, "{:?}", result.error);
    assert!(result
        .result
        .as_ref()
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .is_some_and(|text| text.contains("Visible after activation")));
}

#[test]
fn read_image_tool_result_is_redacted_but_creates_visual_message() {
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: "call-image".to_string(),
        tool: "read_image".to_string(),
        ok: true,
        result: Some(json!({
            "path": "@attachments/image1/pixel.png",
            "format": "png",
            "mimeType": "image/png",
            "sizeBytes": 3,
            "thumbnailDataUrl": "data:image/png;base64,dGh1bWI=",
            "image": {
                "mimeType": "image/png",
                "dataBase64": "YWJj"
            }
        })),
        error: None,
    };

    let registry = ToolRegistry::defaults_with_search(None);
    let event_result = redact_tool_result_for_event(&registry.event_projection(&result));
    let event_value = event_result.result.as_ref().unwrap();
    assert_eq!(
        event_value["thumbnailDataUrl"],
        "data:image/png;base64,dGh1bWI="
    );
    assert!(event_value.get("image").is_none());

    let image_message =
        llm_image_message_from_tool_result(&result, crate::ModelCapabilities { image_input: true })
            .unwrap();
    assert_eq!(image_message.role(), LlmMessageRole::User);
    assert_eq!(image_message.images().len(), 1);
    assert_eq!(image_message.images()[0].mime_type, "image/png");
    assert_eq!(image_message.images()[0].data_base64, "YWJj");
}

#[test]
fn failed_read_image_capability_result_never_creates_visual_input() {
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: "call-image-unsupported".to_string(),
        tool: "read_image".to_string(),
        ok: false,
        result: Some(json!({
            "type": "model_capability",
            "code": "modelCapabilityUnsupported",
            "errorCode": "agent.model_capability_unsupported",
            "capability": "imageInput"
        })),
        error: Some("当前模型不支持图片输入；文件尚未读取。".to_string()),
    };

    assert!(llm_image_message_from_tool_result(
        &result,
        crate::ModelCapabilities { image_input: false }
    )
    .is_none());
}

#[test]
fn generated_image_visual_input_is_capability_gated() {
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: "call-generated-image".to_string(),
        tool: "image_generation".to_string(),
        ok: true,
        result: Some(json!({
            "schemaVersion": 1,
            "status": "succeeded",
            "savedPath": "/managed/generated-image.png",
            "image": {
                "mimeType": "image/png",
                "dataBase64": "YWJj"
            }
        })),
        error: None,
    };

    assert!(llm_image_message_from_tool_result(
        &result,
        crate::ModelCapabilities { image_input: false }
    )
    .is_none());

    let image_message =
        llm_image_message_from_tool_result(&result, crate::ModelCapabilities { image_input: true })
            .expect("image-capable models receive generated pixels");
    assert_eq!(image_message.role(), LlmMessageRole::User);
    assert!(image_message
        .content()
        .contains("/managed/generated-image.png"));
    assert_eq!(image_message.images().len(), 1);
    assert_eq!(image_message.images()[0].mime_type, "image/png");
    assert_eq!(image_message.images()[0].data_base64, "YWJj");
}

#[test]
fn file_write_tail_is_available_to_llm_but_not_persisted_in_events() {
    let result = AgentToolResult {
        exact_archive_file: None,
        call_id: "call-write".to_string(),
        tool: "write_file".to_string(),
        ok: true,
        result: Some(json!({
            "draft": { "draftId": "draft-1" },
            "tail": "private generated content"
        })),
        error: None,
    };

    let llm_result = canonical_tool_result_for_context(&result);
    let event_result = redact_tool_result_for_event(&result);

    assert_eq!(
        llm_result.result.as_ref().unwrap()["tail"],
        "private generated content"
    );
    assert!(event_result.result.as_ref().unwrap().get("tail").is_none());
    assert_eq!(
        event_result.result.as_ref().unwrap()["draft"]["draftId"],
        "draft-1"
    );
}

#[test]
fn parses_plain_and_fenced_tool_calls() {
    let plain = parse_tool_call_request(
        r#"{"type":"tool_call","tool":"search_files","args":{"query":"main"}}"#,
    )
    .unwrap();
    let fenced = parse_tool_call_request(
            "```json\n{\"type\":\"tool_call\",\"tool\":\"read_file\",\"args\":{\"path\":\"src/lib.rs\"}}\n```",
        )
        .unwrap();

    assert_eq!(plain.tool, "search_files");
    assert_eq!(plain.args["query"], "main");
    assert_eq!(fenced.tool, "read_file");
    assert_eq!(fenced.args["path"], "src/lib.rs");
}

#[test]
fn transient_events_are_emitted_without_entering_output_history() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = captured.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let stream = AgentEventStream::new(Some(emitter));

    stream.emit_transient(AgentEvent::ToolInputProgress {
        run_id: "run-1".to_string(),
        stream_id: "stream-1".to_string(),
        attempt: 1,
        tool_call_index: 0,
        tool_call_id: Some("call-1".to_string()),
        tool: "write_file".to_string(),
        received_bytes: 128,
    });

    assert_eq!(captured.lock().unwrap().len(), 1);
    assert!(stream.into_events().is_empty());
}

#[test]
fn context_window_preview_is_available_independently_of_indicator_events() {
    let mut input = serde_json::from_value::<AgentChatInput>(serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": false,
        "messages": []
    }))
    .unwrap();
    input.provider_profile_config = Some(ProviderProfileConfig::generic_for_dialect(
        ProviderProtocolDialect::OpenAiChatCompletions,
    ));

    assert!(inspect_context_window(input).unwrap().is_some());
}

fn conversation_context_input(messages: Vec<AgentChatMessage>) -> AgentChatInput {
    AgentChatInput {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: String::new(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: Some(ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        )),
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(30_000),
        temperature: None,
        stream: Some(true),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages,
    }
}

fn freeze_runtime_test_generic_provider(input: &mut AgentChatInput, revision_label: &str) {
    let dialect = match input.api_style.expect("runtime test API style") {
        crate::protocol::AgentApiStyle::OpenAiCompatible => {
            ProviderProtocolDialect::OpenAiChatCompletions
        }
        crate::protocol::AgentApiStyle::AnthropicCompatible => {
            ProviderProtocolDialect::AnthropicMessages
        }
    };
    let profile = ProviderProfileConfig::generic_for_dialect(dialect);
    let revision = format!("provider-protocol-v1:{revision_label}");
    let key = ProviderProtocolKey::new(
        dialect,
        &profile,
        input.model.clone(),
        Some(revision.clone()),
    )
    .expect("current runtime test Provider protocol key");
    input.provider_configuration_revision = Some(revision);
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(key);
}

#[tokio::test]
async fn runtime_rejects_unknown_frozen_provider_registration_before_transport_or_tools() {
    use crate::storage::service::StorageService;
    use crate::{ProviderProfileConfig, ProviderProtocolDialect, ProviderProtocolKey};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    const MODEL_ID: &str = "unknown-provider-registration-model";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_requests = Arc::new(AtomicUsize::new(0));
    let model_requests_for_server = Arc::clone(&model_requests);
    let server = tokio::spawn(async move {
        if let Ok(Ok((mut stream, _))) =
            timeout(Duration::from_millis(300), listener.accept()).await
        {
            model_requests_for_server.fetch_add(1, Ordering::SeqCst);
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "must not be reached" },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    let mut protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        None,
    )
    .unwrap();
    profile.profile.version = 99;
    protocol.profile.version = 99;

    let tool_executions = Arc::new(AtomicUsize::new(0));
    let tool_executions_for_executor = Arc::clone(&tool_executions);
    let forbidden_executor: AgentHostActionExecutor = Arc::new(move |_, _, _| {
        tool_executions_for_executor.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::new(
            "tool executor must not run for an unknown Provider registration",
        ))
    });
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());

    let mut input = conversation_context_input(vec![message("user", "Do not execute this run.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.model = MODEL_ID.to_string();
    input.stream = Some(false);
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(protocol);

    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-unknown-provider-registration".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_host_actions(forbidden_executor, storage)),
        )
        .await
        .unwrap_err();
    server.await.unwrap();

    assert_eq!(
        error.to_string(),
        "Provider profile configuration is invalid: unsupported provider profile deepseek_v4_chat version 99"
    );
    assert_eq!(model_requests.load(Ordering::SeqCst), 0);
    assert_eq!(tool_executions.load(Ordering::SeqCst), 0);
}

async fn read_runtime_test_json_request(stream: &mut tokio::net::TcpStream) -> Value {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "connection closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                let start = header_end + 4;
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.expect("content length")])
        .unwrap()
}

async fn write_runtime_test_json_response(stream: &mut tokio::net::TcpStream, body: Value) {
    use tokio::io::AsyncWriteExt;

    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

fn runtime_steer_input(
    guidance_id: &str,
    client_message_id: &str,
    content: &str,
) -> crate::AgentSteerInput {
    crate::AgentSteerInput {
        guidance_id: guidance_id.to_string(),
        client_message_id: client_message_id.to_string(),
        content: content.to_string(),
        attachments: Vec::new(),
        attachment_library: None,
        created_at: 42,
    }
}

#[tokio::test]
async fn concurrent_steer_during_sampling_is_fifo_and_turns_a_terminal_response_into_narration() {
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            }
            let content = if request_index == 0 {
                "Initial answer before guidance."
            } else {
                "Final answer after guidance."
            };
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": content },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-before-stream",
                "client-before-stream",
                "Include the pre-stream constraint."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    let mut input = conversation_context_input(vec![message("user", "Start the task.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-steer".to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    first_request_seen_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-1",
                "client-1",
                "Also include the migration risk."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-2",
                "client-2",
                "Keep the rollout steps in chronological order."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    release_first_response_tx.send(()).unwrap();
    let output = runtime.await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Final answer after guidance.");
    let requests = requests.lock().unwrap();
    let second_messages = requests[1]["messages"].as_array().unwrap();
    let intermediate_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "assistant"
                && message["content"] == "Initial answer before guidance."
        })
        .unwrap();
    let before_stream_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user" && message["content"] == "Include the pre-stream constraint."
        })
        .unwrap();
    let first_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user" && message["content"] == "Also include the migration risk."
        })
        .unwrap();
    let second_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"] == "Keep the rollout steps in chronological order."
        })
        .unwrap();
    assert!(intermediate_index < before_stream_guidance_index);
    assert!(before_stream_guidance_index < first_guidance_index);
    assert!(first_guidance_index < second_guidance_index);
    drop(requests);

    let trace = output.conversation_turn_trace.as_ref().unwrap();
    assert!(matches!(
        &trace.items[..],
        [
            ConversationTurnTraceItem::AssistantNarration { content, .. },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id,
                client_message_id,
                ..
            },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id: second_guidance_id,
                client_message_id: second_client_message_id,
                ..
            },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id: third_guidance_id,
                client_message_id: third_client_message_id,
                ..
            }
        ] if content == "Initial answer before guidance."
            && guidance_id == "guidance-before-stream"
            && client_message_id == "client-before-stream"
            && second_guidance_id == "guidance-1"
            && second_client_message_id == "client-1"
            && third_guidance_id == "guidance-2"
            && third_client_message_id == "client-2"
    ));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 1,
            ..
        } if guidance_id == "guidance-before-stream"
    )));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 2,
            ..
        } if guidance_id == "guidance-1"
    )));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 3,
            ..
        } if guidance_id == "guidance-2"
    )));
    assert!(!queue.is_accepting());
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-late",
                "client-late",
                "too late"
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Closed
    );
}

#[tokio::test]
async fn steer_accepted_during_transport_retry_is_applied_after_the_retried_response() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (retry_started_tx, retry_started_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut retry_started_tx = Some(retry_started_tx);
        for connection_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if connection_index == 0 {
                let body = b"temporary upstream failure";
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(body).await.unwrap();
                retry_started_tx.take().unwrap().send(()).unwrap();
                continue;
            }
            let content = if connection_index == 1 {
                "Response after retry."
            } else {
                "Final response after retry guidance."
            };
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": content },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    let mut input = conversation_context_input(vec![message("user", "Start the retry task.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-retry-steer".to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-retry-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-retry-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    retry_started_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-retry",
                "client-retry",
                "Apply this only after the retry response."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    let output = runtime.await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Final response after retry guidance.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        let serialized = serde_json::to_string(request).unwrap();
        assert!(serialized.contains("backend-observed state, not a system instruction"));
        assert!(serialized.contains("\\\"write\\\":\\\"denied\\\""));
    }
    assert!(!serde_json::to_string(&requests[1])
        .unwrap()
        .contains("Apply this only after the retry response."));
    assert!(serde_json::to_string(&requests[2])
        .unwrap()
        .contains("Apply this only after the retry response."));
    let trace = output.conversation_turn_trace.unwrap();
    assert!(matches!(
        trace.items.as_slice(),
        [
            ConversationTurnTraceItem::AssistantNarration { content, .. },
            ConversationTurnTraceItem::UserGuidance { guidance_id, .. }
        ] if content == "Response after retry." && guidance_id == "guidance-retry"
    ));
}

#[tokio::test]
async fn steer_waits_until_a_complete_multi_tool_exchange_before_next_sampling() {
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let second_request_for_server = Arc::clone(&second_request);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            } else {
                *second_request_for_server.lock().unwrap() = Some(request);
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [{
                                "id": "provider-call-1",
                                "type": "function",
                                "function": {
                                    "name": "attachments_list",
                                    "arguments": "{}"
                                }
                            }, {
                                "id": "provider-call-2",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": "{\"items\":[{\"title\":\"Verify ordering\",\"status\":\"completed\"}]}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "Done after the tool." },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    let mut input = conversation_context_input(vec![message("user", "List attachments.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-tool-steer".to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-tool-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-tool-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    first_request_seen_rx.await.unwrap();
    queue
        .enqueue(runtime_steer_input(
            "guidance-tool",
            "client-tool",
            "After the tool, summarize the count.",
        ))
        .unwrap();
    release_first_response_tx.send(()).unwrap();
    let output = runtime.await.unwrap();
    server.await.unwrap();

    let second_request = second_request.lock().unwrap().take().unwrap();
    let messages = second_request["messages"].as_array().unwrap();
    let tool_call_index = messages
        .iter()
        .position(|message| message["role"] == "assistant" && message["tool_calls"].is_array())
        .unwrap();
    let tool_result_indices = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message["role"] == "tool").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(tool_result_indices.len(), 2);
    let guidance_index = messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"] == "After the tool, summarize the count."
        })
        .unwrap();
    assert!(tool_result_indices
        .iter()
        .all(|tool_result_index| tool_call_index < *tool_result_index
            && *tool_result_index < guidance_index));

    let trace = output.conversation_turn_trace.unwrap();
    trace.validate().unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::UserGuidance {
            guidance_id,
            ..
        }) if guidance_id == "guidance-tool"
    ));
    let tool_call_indices = trace
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matches!(item, ConversationTurnTraceItem::ToolCall { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    let tool_result_indices = trace
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matches!(item, ConversationTurnTraceItem::ToolResult { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_call_indices.len(), 2);
    assert_eq!(tool_result_indices.len(), 2);
    let exchange_start = *tool_call_indices.first().unwrap();
    let exchange_end = *tool_result_indices.last().unwrap();
    assert!(trace.items[exchange_start..=exchange_end]
        .iter()
        .all(|item| !matches!(item, ConversationTurnTraceItem::UserGuidance { .. })));
    assert!(exchange_end < trace.items.len() - 1);
}

#[tokio::test]
async fn empty_normal_completion_is_repaired_once_for_openai_and_anthropic() {
    use crate::model_request_observation::ModelRequestObservationStatus;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut body_start = None;
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_length = Some(start + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(
            &request[body_start.unwrap()..expected_length.expect("content length")],
        )
        .unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    async fn run_case(style: crate::protocol::AgentApiStyle) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_for_server = Arc::clone(&captured);
        let server = tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                captured_for_server.lock().unwrap().push(request);
                let response = match (style, request_index) {
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, 0) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 11,
                            "completion_tokens": 3,
                            "total_tokens": 14
                        }
                    }),
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "recovered response" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 12,
                            "completion_tokens": 4,
                            "total_tokens": 16
                        }
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, 0) => json!({
                        "content": [],
                        "stop_reason": "end_turn",
                        "usage": { "input_tokens": 11, "output_tokens": 3 }
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{ "type": "text", "text": "recovered response" }],
                        "stop_reason": "end_turn",
                        "usage": { "input_tokens": 12, "output_tokens": 4 }
                    }),
                };
                write_json_response(&mut stream, response).await;
            }
        });

        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_host = Arc::clone(&observations);
        let observer: AgentModelRequestObserver = Arc::new(move |observation| {
            observations_for_host.lock().unwrap().push(observation);
        });
        let mut input = conversation_context_input(vec![message("user", "complete the task")]);
        input.api_url = format!("http://{address}/v1/messages");
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        freeze_runtime_test_generic_provider(&mut input, "empty-normal-completion");
        input.stream = Some(false);
        input.assistant_message_id = Some("assistant-empty-repair".to_string());
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-empty-repair".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        });

        let output = AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-empty-repair".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_model_request_observer(observer)),
            )
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(output.content, "recovered response");
        assert_eq!(
            output
                .usage
                .as_ref()
                .and_then(|usage| usage.billable_request_count),
            Some(2)
        );
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let first = serde_json::to_string(&requests[0]).unwrap();
        let second = serde_json::to_string(&requests[1]).unwrap();
        assert!(!first.contains("preceding model response ended normally"));
        assert!(second.contains("preceding model response ended normally"));
        match style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                assert!(requests[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| {
                        message["role"] == "user"
                            && message["content"]
                                .as_str()
                                .is_some_and(|content| content.contains("do not repeat"))
                    }));
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                assert!(!requests[1]["system"]
                    .as_str()
                    .is_some_and(|system| system.contains("do not repeat")));
                assert!(requests[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|blocks| {
                                blocks.iter().any(|block| {
                                    block["text"]
                                        .as_str()
                                        .is_some_and(|text| text.contains("do not repeat"))
                                })
                            })
                    }));
            }
        }
        drop(requests);

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].request_index, 1);
        assert_eq!(
            observations[0].status,
            ModelRequestObservationStatus::Failed
        );
        assert_eq!(
            observations[0].error_code.as_deref(),
            Some("agent.empty_model_action")
        );
        assert_eq!(observations[1].request_index, 2);
        assert_eq!(
            observations[1].status,
            ModelRequestObservationStatus::Completed
        );
        let trace = serde_json::to_string(
            output
                .conversation_turn_trace
                .as_ref()
                .expect("completed trace"),
        )
        .unwrap();
        assert!(!trace.contains("preceding model response ended normally"));
    }

    run_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
}

#[tokio::test]
async fn empty_model_action_repair_stops_after_the_second_empty_response() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_for_server = Arc::clone(&request_count);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2_048];
            let expected_length = loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    let expected = header_end + 4 + content_length;
                    if request.len() >= expected {
                        break expected;
                    }
                }
            };
            assert!(request.len() >= expected_length);
            request_count_for_server.fetch_add(1, Ordering::SeqCst);
            let body = serde_json::to_vec(&json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "" },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 5,
                    "completion_tokens": 1,
                    "total_tokens": 6
                }
            }))
            .unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    });

    let mut input = conversation_context_input(vec![message("user", "complete the task")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();
    server.await.unwrap();

    assert_eq!(error.code(), Some("agent.empty_model_action"));
    assert_eq!(
        error.usage().and_then(|usage| usage.billable_request_count),
        Some(2)
    );
    assert_eq!(request_count.load(Ordering::SeqCst), 2);
}

#[test]
fn runtime_command_definition_is_fixed_while_dispatch_uses_current_permissions() {
    let definitions = [
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
        },
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::AutoApprove,
        },
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
        },
    ]
    .into_iter()
    .map(|permissions| {
        let mut input = conversation_context_input(vec![message("user", "run a command")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions,
        });
        let capabilities =
            prepare_runtime_capabilities(&input, "command-definition", &[], true, None).unwrap();
        capabilities
            .initial_tool_set
            .stable_definitions()
            .iter()
            .find(|definition| definition.name == "run_command")
            .cloned()
            .expect("stable run_command definition")
    })
    .collect::<Vec<_>>();

    assert!(definitions[0].requires_approval);
    assert_eq!(
        definitions[0].approval_mode,
        crate::protocol::AgentToolApprovalMode::Always
    );
    assert_eq!(
        serde_json::to_value(&definitions[0]).unwrap(),
        serde_json::to_value(&definitions[1]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&definitions[0]).unwrap(),
        serde_json::to_value(&definitions[2]).unwrap()
    );
}

#[test]
fn model_capabilities_do_not_change_tool_definitions_or_context_revision() {
    let mut input = conversation_context_input(vec![message("user", "Inspect the image")]);
    input.model_capabilities.image_input = false;
    let text_only =
        prepare_runtime_capabilities(&input, "model-capabilities-text-only", &[], true, None)
            .unwrap();
    let text_only_revision = conversation_context_configuration_revision(&input).unwrap();

    input.model_capabilities.image_input = true;
    let image_capable =
        prepare_runtime_capabilities(&input, "model-capabilities-image", &[], true, None).unwrap();
    let image_capable_revision = conversation_context_configuration_revision(&input).unwrap();

    assert!(text_only
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "read_image"));
    assert!(image_capable
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "read_image"));
    assert_eq!(
        serde_json::to_value(&text_only.tool_definitions).unwrap(),
        serde_json::to_value(&image_capable.tool_definitions).unwrap()
    );
    assert_eq!(text_only_revision, image_capable_revision);
}

#[test]
fn run_context_changes_only_world_state_while_prompt_preferences_change_configuration() {
    let mut baseline = conversation_context_input(vec![message("user", "Inspect the project")]);
    baseline.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
        },
    });
    let baseline_revision = conversation_context_configuration_revision(&baseline).unwrap();

    let mut changed_runtime = baseline.clone();
    changed_runtime.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-private".to_string()),
        project_id: Some("project-private".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-private".to_string()),
            display_name: Some("Runtime Workspace".to_string()),
            root_path: Some("/Users/example/runtime-workspace".to_string()),
        }),
        attachment_library: Some(AgentAttachmentLibraryContext {
            root_path: Some("/Users/example/runtime-attachments".to_string()),
            conversation_id: Some("conversation-private".to_string()),
            project_id: Some("project-private".to_string()),
            conversation_attachments: Vec::new(),
            project_attachments: Vec::new(),
        }),
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
        },
    });
    assert_eq!(
        baseline_revision,
        conversation_context_configuration_revision(&changed_runtime).unwrap()
    );

    let host_services = AgentRuntimeHostServices::new();
    let baseline_projection =
        prepare_context_window_tool_projection(&baseline, &host_services, true).unwrap();
    let changed_projection =
        prepare_context_window_tool_projection(&changed_runtime, &host_services, true).unwrap();
    let baseline_world_state = baseline_projection
        .initial_run_world_state()
        .model_projection(WorldStateLifetime::Run)
        .unwrap()
        .render_sanitized_text();
    let changed_world_state = changed_projection
        .initial_run_world_state()
        .model_projection(WorldStateLifetime::Run)
        .unwrap()
        .render_sanitized_text();
    assert_ne!(baseline_world_state, changed_world_state);
    assert!(changed_world_state.contains("\"read\":\"all\""));
    assert!(changed_world_state.contains("\"displayName\":\"Runtime Workspace\""));
    assert_eq!(
        changed_world_state.matches("permissions.effective").count(),
        1
    );
    assert_eq!(changed_world_state.matches("workspace.binding").count(), 1);
    assert!(!changed_world_state.contains("<backend_runtime_context>"));
    assert!(!changed_world_state.contains("<backend_dynamic_tool_availability>"));
    assert!(!changed_world_state.contains("/Users/example/runtime-workspace"));
    assert!(!changed_world_state.contains("/Users/example/runtime-attachments"));
    assert!(!changed_world_state.contains("conversation-private"));
    assert!(!changed_world_state.contains("project-private"));
    let projection_debug = format!("{changed_projection:?}");
    assert!(!projection_debug.contains("/Users/example/runtime-workspace"));
    assert!(!projection_debug.contains("/Users/example/runtime-attachments"));

    let baseline_snapshot =
        inspect_context_window_with_tool_projection(baseline.clone(), &baseline_projection)
            .unwrap()
            .unwrap();
    let changed_snapshot =
        inspect_context_window_with_tool_projection(changed_runtime.clone(), &changed_projection)
            .unwrap()
            .unwrap();
    assert!(baseline_snapshot.input_tokens > 0);
    assert!(changed_snapshot.input_tokens > 0);
    assert_ne!(
        baseline_snapshot.input_tokens,
        changed_snapshot.input_tokens
    );

    changed_runtime.prompt_preferences = Some(AgentPromptPreferences {
        work_mode: Some(crate::protocol::AgentPromptWorkMode::General),
        tone: Some(crate::protocol::AgentPromptTone::Friendly),
        detail_level: None,
        custom_instructions: Some("Use concise domain terminology.".to_string()),
        updated_at: Some(10),
    });
    assert_ne!(
        baseline_revision,
        conversation_context_configuration_revision(&changed_runtime).unwrap()
    );

    let mut timestamp_only_change = changed_runtime.clone();
    timestamp_only_change
        .prompt_preferences
        .as_mut()
        .expect("prompt preferences")
        .updated_at = Some(11);
    assert_eq!(
        conversation_context_configuration_revision(&changed_runtime).unwrap(),
        conversation_context_configuration_revision(&timestamp_only_change).unwrap(),
        "presentation-only settings timestamps must not open a new configuration epoch"
    );
}

#[test]
fn collaboration_identity_is_stable_prompt_configuration_not_run_world_state() {
    let mut root = conversation_context_input(vec![message("user", "Inspect the project")]);
    root.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-root".to_string()),
        project_id: Some("project-1".to_string()),
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    let root_revision = conversation_context_configuration_revision(&root).unwrap();

    let mut child = root.clone();
    child.context.as_mut().unwrap().collaboration_identity =
        Some(crate::AgentCollaborationIdentity {
            agent_id: "agent-child".to_string(),
            root_agent_id: "agent-root".to_string(),
            root_conversation_id: "conversation-root".to_string(),
            parent_agent_id: "agent-root".to_string(),
            parent_task_name: "root".to_string(),
            parent_task_path: "/root".to_string(),
            conversation_id: "conversation-child".to_string(),
            task_name: "review".to_string(),
            task_path: "/root/review".to_string(),
            source_agent_id: "agent-root".to_string(),
            source_kind: crate::AgentMailboxKind::Task,
            source_task_name: "root".to_string(),
            source_task_path: "/root".to_string(),
            source_agent_message_id: "mailbox-task-1".to_string(),
            entrusted_task: "Review the change and report evidence.".to_string(),
            template_instructions: Some("Prefer concrete file references.".to_string()),
        });

    assert_ne!(
        root_revision,
        conversation_context_configuration_revision(&child).unwrap(),
        "a trusted child identity changes the stable system-prompt prefix"
    );

    let mut runtime_only_change = child.clone();
    runtime_only_change
        .context
        .as_mut()
        .unwrap()
        .conversation_id = Some("different-runtime-conversation".to_string());
    assert_eq!(
        conversation_context_configuration_revision(&child).unwrap(),
        conversation_context_configuration_revision(&runtime_only_change).unwrap(),
        "ordinary runtime authority remains outside the stable prompt revision"
    );
}

#[test]
fn durable_conversation_sections_are_not_duplicated_in_run_world_state() {
    let mut input = conversation_context_input(vec![message("user", "Inspect the workspace")]);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-1".to_string()),
        project_id: Some("project-1".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-1".to_string()),
            display_name: Some("Workspace".to_string()),
            root_path: Some("/private/workspace".to_string()),
        }),
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    let conversation_snapshot = WorldStateSnapshot::new(
        "conversation-world-state",
        0,
        vec![
            crate::world_state::effective_permissions_section(
                AgentPermissions::default(),
                WorldStateLifetime::Conversation,
            )
            .unwrap(),
            crate::world_state::workspace_binding_section(
                input
                    .context
                    .as_ref()
                    .and_then(|context| context.workspace.as_ref()),
                WorldStateLifetime::Conversation,
            )
            .unwrap(),
            crate::world_state::interaction_profile_section(
                input.prompt_preferences.as_ref(),
                WorldStateLifetime::Conversation,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    input.world_state_records =
        vec![
            AnchoredWorldStateRecord::new(WorldStateRecord::Full(conversation_snapshot), None)
                .unwrap(),
        ];

    let capabilities =
        prepare_runtime_capabilities(&input, "no-duplicate-world-state", &[], true, None).unwrap();
    let tracker = RunWorldStateTracker::new(
        "no-duplicate-world-state",
        &input,
        &capabilities.initial_tool_set,
    )
    .unwrap();
    let section_ids = tracker
        .snapshot()
        .sections
        .iter()
        .map(|section| section.id.clone())
        .collect::<Vec<_>>();
    assert!(section_ids.contains(&WorldStateSectionId::EffectiveTools));
    assert!(section_ids.contains(&WorldStateSectionId::ModelCapabilities));
    assert!(!section_ids.contains(&WorldStateSectionId::EffectivePermissions));
    assert!(!section_ids.contains(&WorldStateSectionId::WorkspaceBinding));
    assert!(!section_ids.contains(&WorldStateSectionId::InteractionProfile));
}

#[test]
fn settings_capability_changes_open_a_stable_epoch_but_secret_rotation_does_not() {
    let input_with_search = |mode, key: Option<&str>| {
        let mut input = conversation_context_input(vec![message("user", "Find current evidence")]);
        input.search_config = Some(crate::protocol::AgentSearchConfig {
            mode,
            tavily_api_key: key.map(str::to_string),
        });
        input
    };
    let disabled_input = input_with_search(
        crate::protocol::AgentSearchMode::Disabled,
        Some("tvly-disabled"),
    );
    let enabled_a_input = input_with_search(
        crate::protocol::AgentSearchMode::Tavily,
        Some("tvly-secret-a"),
    );
    let enabled_b_input = input_with_search(
        crate::protocol::AgentSearchMode::Tavily,
        Some("tvly-secret-b"),
    );

    let prepare = |input: &AgentChatInput, host_actions_available| {
        prepare_runtime_capabilities(
            input,
            "settings-capability-epoch",
            &[],
            host_actions_available,
            None,
        )
        .unwrap()
    };
    let disabled = prepare(&disabled_input, true);
    let enabled_a = prepare(&enabled_a_input, true);
    let enabled_b = prepare(&enabled_b_input, false);

    assert!(!disabled.initial_tool_set.contains("web_search"));
    assert!(!disabled.initial_tool_set.contains("web_fetch"));
    assert!(enabled_a.initial_tool_set.contains("web_search"));
    assert!(enabled_a.initial_tool_set.contains("web_fetch"));
    assert_ne!(
        disabled.initial_tool_set.stable_revision(),
        enabled_a.initial_tool_set.stable_revision(),
        "enabling a model-visible settings capability must open a new stable epoch"
    );
    assert_ne!(
        conversation_context_configuration_revision(&disabled_input).unwrap(),
        conversation_context_configuration_revision(&enabled_a_input).unwrap()
    );

    assert_eq!(
        serde_json::to_vec(enabled_a.initial_tool_set.stable_definitions()).unwrap(),
        serde_json::to_vec(enabled_b.initial_tool_set.stable_definitions()).unwrap(),
        "rotating a ready capability secret must not rewrite the model-visible contract"
    );
    assert_eq!(
        enabled_a.initial_tool_set.stable_revision(),
        enabled_b.initial_tool_set.stable_revision(),
        "neither secret rotation nor Host executor availability may perturb stable tools"
    );
    assert_eq!(
        conversation_context_configuration_revision(&enabled_a_input).unwrap(),
        conversation_context_configuration_revision(&enabled_b_input).unwrap()
    );
    let serialized_definitions =
        serde_json::to_string(enabled_a.initial_tool_set.stable_definitions()).unwrap();
    assert!(!serialized_definitions.contains("tvly-secret-a"));
    assert!(!serialized_definitions.contains("tvly-secret-b"));
}

#[test]
fn composer_permissions_do_not_change_stable_tools_but_denied_writes_still_fail() {
    let input_with_permissions = |permissions| {
        let mut input = conversation_context_input(vec![message("user", "edit a file")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions,
        });
        input
    };
    let default_permissions = AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::RequireApproval,
    };
    let full_permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
    };
    let custom_permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::Denied,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::AutoApprove,
    };

    let default = prepare_runtime_capabilities(
        &input_with_permissions(default_permissions),
        "stable-default",
        &[],
        true,
        None,
    )
    .unwrap();
    let full = prepare_runtime_capabilities(
        &input_with_permissions(full_permissions),
        "stable-full",
        &[],
        true,
        None,
    )
    .unwrap();
    let denied_input = input_with_permissions(custom_permissions);
    let custom =
        prepare_runtime_capabilities(&denied_input, "stable-custom", &[], true, None).unwrap();

    let default_bytes = serde_json::to_vec(default.initial_tool_set.stable_definitions()).unwrap();
    assert_eq!(
        default_bytes,
        serde_json::to_vec(full.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        default_bytes,
        serde_json::to_vec(custom.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        default.initial_tool_set.stable_revision(),
        full.initial_tool_set.stable_revision()
    );
    assert_eq!(
        default.initial_tool_set.stable_revision(),
        custom.initial_tool_set.stable_revision()
    );
    for name in ["apply_patch", "write_file", "run_command"] {
        assert!(
            custom.initial_tool_set.contains(name),
            "{name} must remain in the stable prefix"
        );
    }

    let error = custom
        .tool_registry
        .proposed_action(
            &ToolExecutionContext::from_run_context(denied_input.context.as_ref()),
            &AgentToolCall {
                id: "denied-stable-write".to_string(),
                tool: "apply_patch".to_string(),
                args: json!({
                    "operation": "create",
                    "filePath": "denied.txt",
                    "content": "must not be written"
                }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("denied"));
}

#[test]
fn conversation_identity_does_not_change_the_stable_history_tool() {
    let input_with_conversation = |conversation_id: Option<&str>| {
        let mut input = conversation_context_input(vec![message("user", "find earlier evidence")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: conversation_id.map(str::to_string),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                patch: AgentPatchPermission::RequireApproval,
            },
        });
        input
    };
    let without_input = input_with_conversation(None);
    let without = prepare_runtime_capabilities(
        &without_input,
        "stable-without-conversation",
        &[],
        true,
        None,
    )
    .unwrap();
    let with_input = input_with_conversation(Some("conversation-1"));
    let with =
        prepare_runtime_capabilities(&with_input, "stable-with-conversation", &[], true, None)
            .unwrap();

    assert!(without.initial_tool_set.contains("conversation_history"));
    assert!(with.initial_tool_set.contains("conversation_history"));
    assert_eq!(
        serde_json::to_vec(without.initial_tool_set.stable_definitions()).unwrap(),
        serde_json::to_vec(with.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        without.initial_tool_set.stable_revision(),
        with.initial_tool_set.stable_revision()
    );

    let result = without.tool_registry.execute(
        &ToolExecutionContext::from_run_context(without_input.context.as_ref()),
        &AgentToolCall {
            id: "history-without-conversation".to_string(),
            tool: "conversation_history".to_string(),
            args: json!({ "action": "search", "query": "evidence" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        },
    );
    assert!(!result.ok);
    assert!(result.error.is_some());
}

#[test]
fn runtime_structured_writers_share_the_file_edit_approval_policy() {
    let definitions = |patch, host_actions_available| {
        let mut input = conversation_context_input(vec![message("user", "edit a workbook")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                write: crate::protocol::AgentWritePermission::WorkspaceOnly,
                patch,
                ..Default::default()
            },
        });
        let engine =
            crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
        prepare_runtime_capabilities(
            &input,
            "office-definition",
            &[],
            host_actions_available,
            Some(engine),
        )
        .unwrap()
        .tool_definitions
    };

    let manual = definitions(AgentPatchPermission::RequireApproval, true);
    let automatic = definitions(AgentPatchPermission::AutoApprove, true);
    let without_host = definitions(AgentPatchPermission::AutoApprove, false);

    for name in ["apply_patch", "write_file"] {
        let manual = manual
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        let automatic = automatic
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        let without_host = without_host
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        assert!(manual.requires_approval);
        assert_eq!(
            serde_json::to_value(manual).unwrap(),
            serde_json::to_value(automatic).unwrap()
        );
        assert_eq!(
            serde_json::to_value(manual).unwrap(),
            serde_json::to_value(without_host).unwrap()
        );
    }

    for name in [
        "skills_materialize_resource",
        "office_document",
        "office_spreadsheet",
        "office_presentation",
    ] {
        assert!(
            manual
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .requires_approval
        );
        let automatic = automatic
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        assert!(!automatic.requires_approval);
        assert_eq!(
            automatic.approval_mode,
            crate::protocol::AgentToolApprovalMode::Never
        );
        assert!(
            without_host
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .requires_approval
        );
    }
}

#[test]
fn write_denied_keeps_stable_writers_but_filters_dynamic_write_only_tools() {
    let mut input = conversation_context_input(vec![message("user", "inspect a workbook")]);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("workspace".to_string()),
            root_path: Some("/tmp/workspace".to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            write: crate::protocol::AgentWritePermission::Denied,
            patch: AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
    });
    let engine =
        crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
    let definitions = prepare_runtime_capabilities(&input, "write-denied", &[], true, Some(engine))
        .unwrap()
        .tool_definitions;

    for name in ["apply_patch", "write_file"] {
        assert!(definitions.iter().any(|definition| definition.name == name));
    }
    assert!(!definitions
        .iter()
        .any(|definition| definition.name == "skills_materialize_resource"));
    for name in [
        "office_document",
        "office_spreadsheet",
        "office_presentation",
    ] {
        assert!(definitions.iter().any(|definition| definition.name == name));
    }
}

#[test]
fn unavailable_tool_errors_distinguish_activation_permissions_and_runtime_capabilities() {
    let registry = ToolRegistry::defaults_with_search(None);
    let definitions = registry.definitions();

    let inactive = registry
        .effective_tool_set(definitions.clone(), &BTreeSet::new())
        .unwrap();
    let activation_error = unavailable_tool_error(&inactive, "office_document");
    assert_eq!(
        activation_error.code(),
        Some("agent.tool_requires_skill_activation")
    );
    assert_eq!(
        activation_error.details().unwrap()["recovery"],
        "activateSkill"
    );

    let document_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        crate::tools::OFFICE_DOCUMENTS_CAPABILITY,
    )]);
    let active_without_engine = registry
        .effective_tool_set(definitions.clone(), &document_capabilities)
        .unwrap();
    let runtime_error = unavailable_tool_error(&active_without_engine, "office_document");
    assert_eq!(
        runtime_error.code(),
        Some("agent.tool_runtime_capability_unavailable")
    );
    assert_eq!(
        runtime_error.details().unwrap()["code"],
        "toolRuntimeCapabilityUnavailable"
    );
    assert_eq!(
        runtime_error.details().unwrap()["recovery"],
        "configureCapability"
    );

    let permitted_without_script_execution = definitions
        .into_iter()
        .filter(|definition| definition.name != "skills_run_script")
        .collect::<Vec<_>>();
    let script_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        crate::tools::SKILL_SCRIPTS_CAPABILITY,
    )]);
    let permission_filtered = registry
        .effective_tool_set(permitted_without_script_execution, &script_capabilities)
        .unwrap();
    let permission_error = unavailable_tool_error(&permission_filtered, "skills_run_script");
    assert_eq!(
        permission_error.code(),
        Some("agent.tool_blocked_by_permissions")
    );
    assert_eq!(
        permission_error.details().unwrap()["recovery"],
        "changePermissions"
    );
    assert_eq!(permission_error.details().unwrap()["bypassAllowed"], false);

    let unknown_error = unavailable_tool_error(&inactive, "invented_tool");
    assert_eq!(unknown_error.code(), Some("agent.tool_not_registered"));
    assert_eq!(
        unknown_error.details().unwrap()["recovery"],
        "useAvailableTool"
    );
}

#[tokio::test]
async fn effective_tool_definitions_are_also_the_execution_allowlist() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                let body_start = request
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                return request[body_start..].to_vec();
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(Vec::new()));
    let captured_second_request = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_body(&mut stream).await;
            if request_index == 1 {
                *captured_second_request.lock().unwrap() = request;
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "hidden-tool-call",
                                "type": "function",
                                "function": {
                                    "name": "skills_preflight_script",
                                    "arguments": "{}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "done" },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let mut input = conversation_context_input(vec![message("user", "run the hidden tool")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            None,
            None,
            AgentCancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "done");
    let call = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "skills_preflight_script" => {
                Some(call)
            }
            _ => None,
        })
        .expect("the unavailable tool call remains observable");
    assert_runtime_owned_tool_call_id(&call.id);
    assert_eq!(call.reason, None);
    let request = String::from_utf8(second_request.lock().unwrap().clone()).unwrap();
    assert!(request.contains(&call.id));
    assert!(!request.contains("hidden-tool-call"));
    assert!(request.contains("agent.tool_requires_skill_activation"));
    assert!(request.contains("toolRequiresSkillActivation"));
    assert!(request.contains("activateSkill"));
    assert!(request.contains("skill.scripts"));
}

#[tokio::test]
async fn text_only_model_receives_paired_read_image_capability_failure_without_image_payload() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        serde_json::from_slice(&request[body_start..expected_length.unwrap()]).unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let captured_second_request = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            if request_index == 1 {
                *captured_second_request.lock().unwrap() = Some(request);
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I will inspect the image.",
                            "tool_calls": [{
                                "id": "read-image-unsupported",
                                "type": "function",
                                "function": {
                                    "name": "read_image",
                                    "arguments": serde_json::to_string(&json!({
                                        "path": "/path/that/must/not/be-read.png"
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "This model cannot inspect images." },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let mut input = conversation_context_input(vec![message("user", "Inspect this image")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.model_capabilities.image_input = false;
    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "This model cannot inspect images.");
    let (call_index, call_id) = output
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "read_image" => {
                Some((index, call.id.clone()))
            }
            _ => None,
        })
        .expect("read_image tool call event");
    assert_runtime_owned_tool_call_id(&call_id);
    let (result_index, result) = output
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ToolResult { result, .. } if result.call_id == call_id => {
                Some((index, result))
            }
            _ => None,
        })
        .expect("paired read_image tool result event");
    assert!(call_index < result_index);
    assert!(!result.ok);
    assert_eq!(result.tool, "read_image");
    let structured = result
        .result
        .as_ref()
        .expect("structured capability failure");
    assert_eq!(structured["code"], "modelCapabilityUnsupported");
    assert_eq!(
        structured["errorCode"],
        "agent.model_capability_unsupported"
    );

    let second_request = second_request
        .lock()
        .unwrap()
        .clone()
        .expect("second model request");
    let messages = second_request["messages"].as_array().unwrap();
    let tool_call_index = messages
        .iter()
        .position(|message| {
            message["role"] == "assistant" && message["tool_calls"][0]["id"] == call_id
        })
        .expect("assistant tool call in provider payload");
    let tool_result_index = messages
        .iter()
        .position(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
        .expect("paired tool result in provider payload");
    assert!(tool_call_index < tool_result_index);
    assert!(!serde_json::to_string(messages)
        .unwrap()
        .contains("read-image-unsupported"));

    let request = serde_json::to_string(&second_request).unwrap();
    assert!(request.contains("modelCapabilityUnsupported"));
    assert!(request.contains("agent.model_capability_unsupported"));
    assert!(!request.contains("modelCapabilities"));
    assert!(!request.contains("image_url"));
    assert!(!request.contains("data:image/"));
}

#[tokio::test]
async fn image_capable_read_image_round_trip_is_legal_for_openai_and_anthropic() {
    use base64::Engine;
    use image::ImageEncoder;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut body_start = None;
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_length = Some(start + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(
            &request[body_start.unwrap()..expected_length.expect("content length")],
        )
        .unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    async fn run_case(style: crate::protocol::AgentApiStyle) {
        let fixture = tempdir().unwrap();
        let image_path = fixture.path().join("pixel.png");
        let mut image_bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut image_bytes)
            .write_image(
                &[0x10, 0x40, 0x90, 0xff],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        std::fs::write(&image_path, &image_bytes).unwrap();
        let full_image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let second_request = Arc::new(Mutex::new(None::<Value>));
        let captured_second_request = Arc::clone(&second_request);
        let server = tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                if request_index == 1 {
                    *captured_second_request.lock().unwrap() = Some(request);
                }
                let response = match (style, request_index) {
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, 0) => json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "I will inspect the image.",
                                "tool_calls": [{
                                    "id": "read-image-success",
                                    "type": "function",
                                    "function": {
                                        "name": "read_image",
                                        "arguments": "{\"path\":\"pixel.png\"}"
                                    }
                                }]
                            },
                            "finish_reason": "tool_calls"
                        }]
                    }),
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "image inspected" },
                            "finish_reason": "stop"
                        }]
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, 0) => json!({
                        "content": [
                            { "type": "text", "text": "I will inspect the image." },
                            {
                                "type": "tool_use",
                                "id": "read-image-success",
                                "name": "read_image",
                                "input": { "path": "pixel.png" }
                            }
                        ],
                        "stop_reason": "tool_use"
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{ "type": "text", "text": "image inspected" }],
                        "stop_reason": "end_turn"
                    }),
                };
                write_json_response(&mut stream, response).await;
            }
        });

        let mut input = conversation_context_input(vec![message("user", "Inspect pixel.png")]);
        input.api_url = format!("http://{address}/v1/messages");
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        freeze_runtime_test_generic_provider(&mut input, "image-capable-round-trip");
        input.stream = Some(false);
        input.model_capabilities.image_input = true;
        input.assistant_message_id = Some("assistant-image".to_string());
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-image".to_string()),
            project_id: Some("project-image".to_string()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-image".to_string()),
                display_name: Some("Image workspace".to_string()),
                root_path: Some(fixture.path().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: Default::default(),
        });

        let output = AgentRuntime::default().send_chat(input).await.unwrap();
        server.await.unwrap();
        assert_eq!(output.content, "image inspected");

        let call_id = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolCall { call, .. } if call.tool == "read_image" => {
                    Some(call.id.clone())
                }
                _ => None,
            })
            .expect("read_image call event");
        assert_runtime_owned_tool_call_id(&call_id);
        let event_result = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolResult { result, .. } if result.call_id == call_id => Some(result),
                _ => None,
            })
            .expect("read_image result event");
        let event_value = event_result.result.as_ref().unwrap();
        assert!(event_value["thumbnailDataUrl"]
            .as_str()
            .is_some_and(|value| value.starts_with("data:image/png;base64,")));
        assert!(event_value.get("image").is_none());

        let trace = output
            .conversation_turn_trace
            .as_ref()
            .expect("terminal conversation trace");
        trace.validate().unwrap();
        assert!(trace.truncated);
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                call_id: trace_call_id,
                truncated: true,
                ..
            } if trace_call_id == &call_id
        )));
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall {
                call_id: trace_call_id,
                ..
            } if trace_call_id == &call_id
        )));
        let durable = serde_json::to_string(trace).unwrap();
        assert!(!durable.contains(&full_image_base64));
        assert!(!durable.contains("data:image"));
        assert!(!durable.contains("thumbnailDataUrl"));
        assert!(!durable.contains("dataBase64"));

        let request = second_request
            .lock()
            .unwrap()
            .clone()
            .expect("second provider request");
        let messages = request["messages"].as_array().unwrap();
        match style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                let tool_call_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "assistant" && message["tool_calls"][0]["id"] == call_id
                    })
                    .expect("OpenAI assistant tool call");
                let tool_result_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "tool" && message["tool_call_id"] == call_id
                    })
                    .expect("OpenAI paired tool result");
                let image_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| part["type"] == "image_url")
                            })
                    })
                    .expect("OpenAI visual input");
                assert!(tool_call_index < tool_result_index && tool_result_index < image_index);
                assert!(messages[image_index]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|part| {
                        part["image_url"]["url"]
                            == format!("data:image/png;base64,{full_image_base64}")
                    }));
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                let tool_call_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "assistant"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts
                                    .iter()
                                    .any(|part| part["type"] == "tool_use" && part["id"] == call_id)
                            })
                    })
                    .expect("Anthropic assistant tool_use");
                let result_and_image_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| {
                                    part["type"] == "tool_result" && part["tool_use_id"] == call_id
                                }) && parts.iter().any(|part| {
                                    part["type"] == "image"
                                        && part["source"]["data"] == full_image_base64
                                })
                            })
                    })
                    .expect("Anthropic paired tool_result and visual input");
                assert!(tool_call_index < result_and_image_index);
            }
        }
        assert!(!serde_json::to_string(messages)
            .unwrap()
            .contains("read-image-success"));
    }

    run_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
}

#[test]
fn runtime_skill_script_definition_respects_the_host_permission_matrix() {
    use crate::protocol::{
        AgentCommandPermission, AgentReadPermission, AgentToolApprovalMode, AgentWorkspaceContext,
        AgentWritePermission,
    };

    let definitions = |write, command, command_safety, host_actions_available| {
        let mut input = conversation_context_input(vec![message("user", "run a Skill script")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                read: if command_safety == AgentCommandSafetyPolicy::FullAccess {
                    AgentReadPermission::All
                } else {
                    AgentReadPermission::WorkspaceOnly
                },
                write,
                command,
                command_safety,
                ..Default::default()
            },
        });
        prepare_runtime_capabilities(
            &input,
            "skill-script-definition",
            &[],
            host_actions_available,
            None,
        )
        .unwrap()
        .tool_definitions
    };

    let guarded = definitions(
        AgentWritePermission::WorkspaceOnly,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::Guarded,
        true,
    );
    assert!(!guarded
        .iter()
        .any(|definition| definition.name == "skills_run_script"));
    assert!(!guarded
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));

    let write_denied = definitions(
        AgentWritePermission::Denied,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    assert!(!write_denied
        .iter()
        .any(|definition| definition.name == "skills_run_script"));
    assert!(!write_denied
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));

    let manual = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    assert!(manual
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));
    let manual = manual
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(manual.requires_approval);
    assert_eq!(manual.approval_mode, AgentToolApprovalMode::Always);

    let automatic = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::AutoApprove,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    let automatic = automatic
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(automatic.requires_approval);
    assert_eq!(automatic.approval_mode, AgentToolApprovalMode::Always);

    let without_host = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::AutoApprove,
        AgentCommandSafetyPolicy::FullAccess,
        false,
    );
    let without_host = without_host
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(without_host.requires_approval);
    assert_eq!(without_host.approval_mode, AgentToolApprovalMode::Always);
}

fn conversation_context_trace(
    terminal_status: ConversationTurnTraceTerminalStatus,
    items: Vec<ConversationTurnTraceItem>,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-context".to_string(),
        conversation_id: "conversation-context".to_string(),
        assistant_message_id: "assistant-context".to_string(),
        terminal_status,
        terminal_error: None,
        truncated: false,
        items,
    }
}

fn traced_assistant_message(content: &str, trace: ConversationTurnTrace) -> AgentChatMessage {
    let mut provider_tool_index = 0_u32;
    let conversation_model_context_items = trace
        .items
        .iter()
        .filter_map(|item| {
            let sequence = item.sequence();
            let projected = match item {
                ConversationTurnTraceItem::AssistantNarration { content, .. } => {
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "assistant".to_string(),
                        content: content.clone(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::UserGuidance { content, .. } => {
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "user".to_string(),
                        content: content.clone(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::AgentMailboxDelivery { content, .. } => {
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "user".to_string(),
                        content: content.clone(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
                    operation,
                    ..
                } => {
                    let identity = crate::AgentProviderToolCallIdentity {
                        provider_tool_index,
                        provider_call_id: call_id.clone(),
                        runtime_call_id: call_id.clone(),
                    };
                    provider_tool_index = provider_tool_index.saturating_add(1);
                    crate::ConversationModelContextItem {
                        sequence,
                        ordinal: 0,
                        role: "assistant".to_string(),
                        content: String::new(),
                        tool_call_id: None,
                        tool_calls: vec![crate::AgentContextCheckpointToolCall {
                            id: call_id.clone(),
                            name: tool.clone(),
                            args: operation.clone(),
                            provider_identity: identity,
                        }],
                        is_error: false,
                    }
                }
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    observation,
                    success,
                    ..
                } => crate::ConversationModelContextItem {
                    sequence,
                    ordinal: 0,
                    role: "tool".to_string(),
                    content: observation.to_string(),
                    tool_call_id: Some(call_id.clone()),
                    tool_calls: Vec::new(),
                    is_error: !success,
                },
                ConversationTurnTraceItem::CommandSessionLifecycle { .. }
                | ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
                | ConversationTurnTraceItem::RuntimeError { .. } => return None,
            };
            Some(projected)
        })
        .collect();
    AgentChatMessage {
        message_id: Some(trace.assistant_message_id.clone()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: Some(2_000),
        conversation_turn_trace: Some(trace),
        conversation_model_context_items,
    }
}

fn current_assistant_history_message(content: &str) -> AgentChatMessage {
    traced_assistant_message(
        content,
        conversation_context_trace(ConversationTurnTraceTerminalStatus::Completed, Vec::new()),
    )
}

fn full_conversation_context_snapshot(
    messages: Vec<AgentChatMessage>,
) -> AgentContextWindowSnapshot {
    create_conversation_context_state(conversation_context_input(messages))
        .unwrap()
        .snapshot()
}

#[test]
fn conversation_context_state_incremental_updates_match_full_rebuilds() {
    let mut first_user = message("user", "Inspect the project and update src/lib.rs");
    first_user.created_at = Some(1_000);
    let mut state =
        create_conversation_context_state(conversation_context_input(vec![first_user.clone()]))
            .unwrap();

    let narration = ConversationTurnTraceItem::AssistantNarration {
        sequence: 0,
        content: "I will inspect the current implementation first.".to_string(),
        truncated: false,
    };
    let narrated_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration.clone()],
    );
    let narrated_message = traced_assistant_message("", narrated_trace.clone());
    let cursor = state
        .append_trace_items(
            &narrated_trace,
            &narrated_message.conversation_model_context_items,
            0,
        )
        .unwrap();
    assert_eq!(cursor, 1);
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![first_user.clone(), narrated_message,])
    );

    let context_call_id =
        crate::llm::model_response_tool_call_id("run-context", 0, 0, "call-context");
    assert_runtime_owned_tool_call_id(&context_call_id);
    let call = ConversationTurnTraceItem::ToolCall {
        sequence: 1,
        call_id: context_call_id.clone(),
        tool: "read_file".to_string(),
        provenance: crate::AgentToolIdentity::Builtin {
            tool_name: "read_file".to_string(),
        },
        operation: json!({ "path": "src/lib.rs" }),
        approval_status: AgentApprovalStatus::NotRequired,
        truncated: false,
    };
    let result = ConversationTurnTraceItem::ToolResult {
        sequence: 2,
        call_id: context_call_id,
        tool: "read_file".to_string(),
        status: ConversationTraceToolResultStatus::Succeeded,
        success: true,
        observation: json!({ "path": "src/lib.rs", "endLine": 40 }),
        approval_status: AgentApprovalStatus::NotRequired,
        error: None,
        truncated: false,
        archive: Default::default(),
    };
    let closed_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration, call, result],
    );
    let closed_message = traced_assistant_message("", closed_trace.clone());
    let cursor = state
        .append_trace_items(
            &closed_trace,
            &closed_message.conversation_model_context_items,
            cursor,
        )
        .unwrap();
    assert_eq!(cursor, 3);
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![first_user.clone(), closed_message,])
    );

    let completed_trace = ConversationTurnTrace {
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        ..closed_trace
    };
    let final_content = "I updated the implementation and verified the tests.";
    state
        .finalize_conversation_turn(
            &completed_trace,
            &traced_assistant_message("", completed_trace.clone()).conversation_model_context_items,
            cursor,
            final_content,
            Some(2_000),
        )
        .unwrap();
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message(final_content, completed_trace.clone()),
        ])
    );

    let follow_up = "Now explain the change.";
    state
        .append_user_message(None, follow_up, Some(3_000))
        .unwrap();
    let mut follow_up_message = message("user", follow_up);
    follow_up_message.created_at = Some(3_000);
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![
            first_user,
            traced_assistant_message(final_content, completed_trace),
            follow_up_message,
        ])
    );
}

#[test]
fn runtime_shared_baseline_matches_full_context_assembly() {
    let input = conversation_context_input(vec![
        message("user", "First question"),
        current_assistant_history_message("First answer"),
        message("user", "Current question"),
    ]);
    let capabilities =
        prepare_runtime_capabilities(&input, "baseline-test", &[], true, None).unwrap();
    let full = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let mut durable_state = create_conversation_context_state(input.clone()).unwrap();
    let baseline = durable_state.shared_baseline().unwrap();
    let shared = build_llm_request(
        input,
        &capabilities.tool_definitions,
        None,
        Some(baseline),
        None,
    )
    .unwrap();

    assert_eq!(shared.context.to_messages(), full.context.to_messages());
}

#[test]
fn activated_skill_is_a_measured_dynamic_overlay_not_a_cache_input() {
    const INSTRUCTIONS: &str = "SKILL_DYNAMIC_MARKER: inspect evidence before editing.";
    let mut input = conversation_context_input(vec![
        message("user", "First question"),
        current_assistant_history_message("First answer"),
        message("user", "Current question"),
    ]);
    input.skill_activation = Some(activated_skill(INSTRUCTIONS));

    let mut changed_selection = input.clone();
    changed_selection.skill_activation = Some(activated_skill(
        "SKILL_CHANGED_MARKER: use a different workflow.",
    ));
    assert_eq!(
        conversation_context_configuration_revision(&input).unwrap(),
        conversation_context_configuration_revision(&changed_selection).unwrap()
    );

    let mut stable_input = input.clone();
    stable_input.skill_activation = None;
    stable_input.skill_discovery = None;
    let capabilities =
        prepare_runtime_capabilities(&stable_input, "skill-overlay", &[], true, None).unwrap();
    let mut full = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let manifest = full.context.manifest();
    let skill_entry = manifest
        .entries
        .iter()
        .find(|entry| entry.sources == vec!["skill_instructions"])
        .unwrap();
    assert_eq!(skill_entry.role, "user");
    assert_eq!(skill_entry.scope, "run");
    assert_eq!(skill_entry.retention, "retained");
    assert_eq!(skill_entry.origin_kind, Some("skill"));
    assert_eq!(skill_entry.origin_id, Some("workspace:workspace-1:review"));
    let messages = full.context.to_messages();
    let skill_index = messages
        .iter()
        .position(|message| message.content().contains(INSTRUCTIONS))
        .unwrap();
    let current_user_index = messages
        .iter()
        .position(|message| message.content().contains("Current question"))
        .unwrap();
    assert!(skill_index > current_user_index);
    assert!(!messages[0].content().contains(INSTRUCTIONS));

    let detector = ContextCapacityDetector::for_model(
        &input.model,
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &capabilities.tool_definitions,
    );
    let skill_report = detector.inspect(
        &mut full.context,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
    );
    assert_eq!(
        skill_report
            .usage
            .breakdown
            .run_transient
            .context_item_count,
        1
    );
    assert!(skill_report.usage.breakdown.run_transient.input_tokens > 0);

    let mut without_skill = input.clone();
    without_skill.skill_activation = None;
    let mut plain = build_llm_request(
        without_skill.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let plain_report = detector.inspect(
        &mut plain.context,
        without_skill.context_window_tokens,
        sanitize_max_tokens(without_skill.max_tokens),
    );
    assert_eq!(
        skill_report.usage.persistent_revision,
        plain_report.usage.persistent_revision
    );
    assert!(skill_report.usage.request_input_tokens() > plain_report.usage.request_input_tokens());

    let plain_preview = inspect_context_window(without_skill).unwrap().unwrap();
    let skill_preview = inspect_context_window(input.clone()).unwrap().unwrap();
    assert!(skill_preview.input_tokens > plain_preview.input_tokens);
    let mut dynamic_tool = capabilities
        .tool_definitions
        .iter()
        .find(|definition| definition.name == "read_file")
        .cloned()
        .unwrap();
    dynamic_tool.name = "skill_dynamic_test_tool".to_string();
    dynamic_tool.description =
        "A deliberately verbose Skill-gated Tool schema used for context capacity testing."
            .to_string();
    let tool_state = json!({
        "stableTools": ["read_file"],
        "dynamicTools": ["skill_dynamic_test_tool"],
    });
    let dynamic_run_world_state = WorldStateSnapshot::new(
        "dynamic-context-preview",
        0,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectiveTools,
            WorldStateLifetime::Run,
            tool_state.clone(),
            tool_state,
        )
        .unwrap()],
    )
    .unwrap();
    let dynamic_projection = AgentContextWindowToolProjection::new(
        crate::protocol::AgentRunToolSetCheckpoint {
            stable_revision: "stable-test-revision".to_string(),
            dynamic_revision: "dynamic-test-revision".to_string(),
            effective_revision: "effective-test-revision".to_string(),
            active_capability_ids: Vec::new(),
            exposed_tool_names: vec![
                "read_file".to_string(),
                "skill_dynamic_test_tool".to_string(),
            ],
        },
        dynamic_run_world_state,
        vec![dynamic_tool.clone()],
    );
    let dynamic_preview =
        inspect_context_window_with_tool_projection(input.clone(), &dynamic_projection)
            .unwrap()
            .unwrap();
    assert!(dynamic_preview.input_tokens > skill_preview.input_tokens);

    let mut durable_state = create_conversation_context_state(input.clone()).unwrap();
    let cached_plain = durable_state.snapshot();
    let cached_skill = durable_state
        .snapshot_with_skill_activation(input.skill_activation.as_ref())
        .unwrap();
    let cached_plain_after = durable_state.snapshot();
    assert_eq!(cached_plain, cached_plain_after);
    assert!(cached_skill.input_tokens > cached_plain.input_tokens);
    let cached_dynamic_skill = durable_state
        .snapshot_with_skill_overlays_and_tool_projection(
            input.skill_discovery.as_ref(),
            input.skill_activation.as_ref(),
            &dynamic_projection,
        )
        .unwrap();
    assert!(cached_dynamic_skill.input_tokens > cached_skill.input_tokens);
    let baseline = durable_state.shared_baseline().unwrap();
    let shared = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        Some(baseline),
        None,
    )
    .unwrap();
    assert_eq!(shared.context.to_messages(), full.context.to_messages());

    let debug = format!("{:?}", input.skill_activation);
    assert!(!debug.contains(INSTRUCTIONS));
}

#[test]
fn context_preview_counts_only_host_verified_initial_dynamic_tools() {
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillRevision, SkillSourceId,
    };

    let skill_id = SkillId::parse("workspace:workspace-1:preview-resources").unwrap();
    let revision =
        SkillRevision::parse(format!("skill-package-sha256-v3:{}", "d".repeat(64))).unwrap();
    let source_id = SkillSourceId::parse("workspace:workspace-1").unwrap();
    let resources = Arc::new(
        memory_resource_session_for_test(
            skill_id.clone(),
            revision.clone(),
            source_id,
            vec![(
                "references/guide.md".to_string(),
                SkillResourceKind::Reference,
                b"verified reference".to_vec(),
            )],
        )
        .unwrap(),
    );
    let package = SkillPackageUri::new(skill_id.clone(), revision.clone());
    let instructions = "Read the verified reference before answering.";
    let mut input = conversation_context_input(vec![message("user", "Inspect the reference")]);
    input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-sha256-v1:preview-resources".to_string(),
        skills: vec![AgentActivatedSkill {
            id: skill_id.to_string(),
            name: "preview-resources".to_string(),
            revision: revision.to_string(),
            source: "workspace:workspace-1".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: Some(crate::protocol::AgentActivatedSkillResources {
                root_uri: package.to_string(),
                resource_count: 1,
                kinds: vec!["reference".to_string()],
            }),
        }],
    });

    let without_authority =
        prepare_context_window_tool_projection(&input, &AgentRuntimeHostServices::new(), true)
            .unwrap_err();
    assert!(without_authority
        .to_string()
        .contains("no exact Host package authority"));

    let host_services =
        AgentRuntimeHostServices::new().with_skill_resources(Arc::clone(&resources));
    let projection = prepare_context_window_tool_projection(&input, &host_services, true).unwrap();
    let names = projection
        .dynamic_definitions()
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["skills_list_resources", "skills_read_resource"]);

    let conservative = inspect_context_window(input.clone()).unwrap().unwrap();
    let exact = inspect_context_window_with_tool_projection(input, &projection)
        .unwrap()
        .unwrap();
    assert!(exact.input_tokens > conservative.input_tokens);
}

#[test]
fn discoverable_skill_catalog_is_a_measured_dynamic_overlay_not_a_cache_input() {
    let mut input = conversation_context_input(vec![message("user", "Create a document")]);
    input.skill_discovery = Some(discoverable_skill("Create and verify Word documents."));
    let mut changed_catalog = input.clone();
    changed_catalog.skill_discovery = Some(discoverable_skill("Updated routing metadata."));

    assert_eq!(
        conversation_context_configuration_revision(&input).unwrap(),
        conversation_context_configuration_revision(&changed_catalog).unwrap()
    );

    let capabilities =
        prepare_runtime_capabilities(&input, "skill-discovery-overlay", &[], true, None).unwrap();
    let activation_ref = input.skill_discovery.as_ref().unwrap().skills[0]
        .activation_ref
        .clone();
    let mut with_catalog = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let catalog_entry = with_catalog
        .context
        .manifest()
        .entries
        .into_iter()
        .find(|entry| entry.sources == vec!["skill_catalog"])
        .unwrap();
    assert_eq!(catalog_entry.scope, "run");
    assert_eq!(catalog_entry.retention, "retained");

    let rendered_message = with_catalog
        .context
        .to_messages()
        .into_iter()
        .find(|message| message.content().contains("backend_available_skills"))
        .unwrap();
    let rendered = rendered_message.content();
    assert!(rendered.contains(&format!("\"ref\":\"{activation_ref}\"")));
    assert!(rendered.contains("Create and verify Word documents."));
    assert!(!rendered.contains("bundled:application:documents"));
    assert!(!rendered.contains("skill-package-sha256-v2:"));

    let detector = ContextCapacityDetector::for_model(
        &input.model,
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &capabilities.tool_definitions,
    );
    let catalog_report = detector.inspect(
        &mut with_catalog.context,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
    );
    let mut without_catalog = input.clone();
    without_catalog.skill_discovery = None;
    let mut plain = build_llm_request(
        without_catalog.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let plain_report = detector.inspect(
        &mut plain.context,
        without_catalog.context_window_tokens,
        sanitize_max_tokens(without_catalog.max_tokens),
    );
    assert_eq!(
        catalog_report.usage.persistent_revision,
        plain_report.usage.persistent_revision
    );
    assert!(
        catalog_report.usage.request_input_tokens() > plain_report.usage.request_input_tokens()
    );
}

#[tokio::test]
async fn model_activation_preserves_exposed_siblings_and_discloses_new_tools_next_request() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const INSTRUCTIONS: &str =
        "DYNAMIC_SKILL_INSTRUCTION_MARKER: verify the document before reporting success.";

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap();
        serde_json::from_slice(&request[body_start..expected_length.unwrap()]).unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn open_ai_tool_names(request: &Value) -> Vec<&str> {
        request["tools"]
            .as_array()
            .expect("OpenAI-compatible request tools")
            .iter()
            .map(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .expect("function tool name")
            })
            .collect()
    }

    let discovery = discoverable_skill("Create and verify Word documents.");
    let activation_ref = discovery.skills[0].activation_ref.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [
                                {
                                    "id": "todo-alongside-skill",
                                    "type": "function",
                                    "function": {
                                        "name": "todo_update",
                                        "arguments": serde_json::to_string(&json!({
                                            "items": [],
                                            "explanation": "Verify same-response calls remain executable"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": "activate-documents",
                                    "type": "function",
                                    "function": {
                                        "name": "skills_activate",
                                        "arguments": serde_json::to_string(&json!({
                                            "skillRef": activation_ref,
                                            "reason": "Create and verify the requested document"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": "new-tool-before-next-request",
                                    "type": "function",
                                    "function": {
                                        "name": "office_document",
                                        "arguments": serde_json::to_string(&json!({
                                            "operation": "inspect",
                                            "path": "draft.docx"
                                        })).unwrap()
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Skill loaded and applied."
                        },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let entry = discovery.skills[0].clone();
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls_for_host = Arc::clone(&resolver_calls);
    let resolver: AgentSkillActivationResolver = Arc::new(move |selection| {
        resolver_calls_for_host.fetch_add(1, Ordering::SeqCst);
        assert_eq!(selection.skill_id().as_str(), entry.id);
        assert_eq!(selection.expected_revision().as_str(), entry.revision);
        let skill_id = crate::skills::SkillId::parse(entry.id.clone()).unwrap();
        let source_id = skill_id.source_id().clone();
        let resources = crate::skills::memory_resource_session_for_test(
            skill_id,
            crate::skills::SkillRevision::parse(entry.revision.clone()).unwrap(),
            source_id,
            Vec::new(),
        )
        .unwrap();
        Ok(AgentResolvedSkillActivation {
            skill: AgentActivatedSkill {
                id: entry.id.clone(),
                name: entry.name.clone(),
                revision: entry.revision.clone(),
                source: "bundled:application".to_string(),
                instructions: INSTRUCTIONS.to_string(),
                source_bytes: u64::try_from(INSTRUCTIONS.len()).unwrap(),
                resources: None,
            },
            resources: Arc::new(resources),
        })
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_for_emitter = Arc::clone(&events);
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        events_for_emitter.lock().unwrap().push(event);
    });
    let context_window_snapshots = Arc::new(Mutex::new(Vec::<AgentContextWindowSnapshot>::new()));
    let context_window_snapshots_for_observer = Arc::clone(&context_window_snapshots);
    let context_window_observer: AgentContextWindowObserver = Arc::new(move |snapshot| {
        context_window_snapshots_for_observer
            .lock()
            .unwrap()
            .push(snapshot);
    });
    let mut input = conversation_context_input(vec![message("user", "Create a Word guide")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.skill_discovery = Some(discovery);
    let office_engine =
        crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-dynamic-skill".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_skill_activation_resolver(resolver)
                    .with_skill_resources(Arc::new(crate::skills::SkillResourceSession::empty()))
                    .with_office_engine(office_engine)
                    .with_context_window_observer(context_window_observer),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Skill loaded and applied.");
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    let context_window_snapshots = context_window_snapshots.lock().unwrap();
    assert_eq!(
        context_window_snapshots.len(),
        2,
        "the observer must receive one exact aggregate snapshot for each sendable request"
    );
    assert!(
        context_window_snapshots[1].input_tokens > context_window_snapshots[0].input_tokens,
        "the post-activation request must account for the paired ToolResult, full Skill instructions, tools.effective World State diff and unlocked Tool schemas"
    );
    drop(context_window_snapshots);
    let activation_call_id = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "skills_activate" => {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("canonical skills_activate call");
    assert_runtime_owned_tool_call_id(&activation_call_id);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first_serialized = serde_json::to_string(&requests[0]).unwrap();
    assert!(first_serialized.contains("backend_available_skills"));
    assert!(first_serialized.contains("skills_activate"));
    assert!(!first_serialized.contains(INSTRUCTIONS));
    let first_tool_names = open_ai_tool_names(&requests[0]);
    assert!(
        !first_tool_names.contains(&"read_word"),
        "documents tools must remain hidden before Skill activation"
    );
    assert!(
        !first_tool_names.contains(&"office_document"),
        "Office semantic tools must remain hidden before Skill activation"
    );

    let second_tool_names = open_ai_tool_names(&requests[1]);
    assert!(
        second_tool_names.starts_with(&first_tool_names),
        "stable tools must remain an exact prefix after dynamic activation"
    );
    assert!(second_tool_names.contains(&"read_word"));
    assert!(second_tool_names.contains(&"office_document"));
    assert_eq!(
        requests[0]["messages"][0], requests[1]["messages"][0],
        "the backend-owned stable system prompt must not change when a Skill unlocks tools"
    );

    let second_messages = requests[1]["messages"].as_array().unwrap();
    let tool_result_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "tool" && message["tool_call_id"] == activation_call_id
        })
        .unwrap();
    let skill_context_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(INSTRUCTIONS))
        })
        .unwrap();
    assert!(tool_result_index < skill_context_index);
    assert!(!second_messages[tool_result_index]["content"]
        .as_str()
        .unwrap()
        .contains(INSTRUCTIONS));
    assert_eq!(
        serde_json::to_string(&requests[1])
            .unwrap()
            .matches(INSTRUCTIONS)
            .count(),
        1
    );
    assert!(
        !serde_json::to_string(&requests[1])
            .unwrap()
            .contains("skillActivationBoundary"),
        "the next request must contain real sibling results, not a synthetic activation barrier"
    );

    let events = events.lock().unwrap();
    let result_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ToolResult { result, .. } if result.call_id == activation_call_id))
        .unwrap();
    let activated_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::SkillActivated { skill, .. } if skill.id == "bundled:application:documents"))
        .unwrap();
    assert!(result_index < activated_index);
    let sibling_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "todo_update" => Some(result),
            _ => None,
        })
        .expect("a tool exposed in the original request must execute alongside Skill activation");
    assert!(sibling_result.ok);
    let newly_unlocked_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "office_document" => {
                Some(result)
            }
            _ => None,
        })
        .expect("a same-response call to a newly unlocked tool must settle deterministically");
    assert!(!newly_unlocked_result.ok);
    assert_eq!(
        newly_unlocked_result
            .result
            .as_ref()
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("agent.tool_requires_skill_activation")
    );
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains("skillActivationBoundary"));
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains("activate-documents"));
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains(INSTRUCTIONS));
}

#[derive(Clone, Copy, Debug)]
enum SkillApprovalResumeProviderCase {
    Generic,
    DeepSeekExactGrouped,
}

impl SkillApprovalResumeProviderCase {
    fn label(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::DeepSeekExactGrouped => "deepseek",
        }
    }
}

async fn run_skill_activation_approval_resume_case(case: SkillApprovalResumeProviderCase) {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentApprovalDecision, AgentApprovalDecisionStatus, AgentCommandPermission,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentToolContinuation,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    const INSTRUCTIONS: &str =
        "APPROVAL_RESUME_SKILL_MARKER: Word tools are available on the next request only.";
    const PROVIDER_REASONING: &str =
        "Activate documents, request the existing write approval, then settle all siblings.";

    let label = case.label();
    let run_id = format!("run-skill-approval-{label}");
    let conversation_id = format!("conversation-skill-approval-{label}");
    let assistant_message_id = format!("assistant-skill-approval-{label}");
    let model_id = format!("model-skill-approval-{label}");
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.clone(),
            project_id: None,
            model_id: Some(model_id.clone()),
            title: format!("Skill Approval {label}"),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.clone(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );

    let mut provider_profile = match case {
        SkillApprovalResumeProviderCase::Generic => ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        SkillApprovalResumeProviderCase::DeepSeekExactGrouped => {
            ProviderProfileConfig::deepseek_v4_default()
        }
    };
    if matches!(case, SkillApprovalResumeProviderCase::DeepSeekExactGrouped) {
        provider_profile.reasoning = ReasoningPolicy {
            mode: ReasoningMode::Enabled,
            effort: ReasoningEffort::Max,
        };
    }
    let provider_configuration_revision = format!("provider-protocol-v1:skill-approval-{label}");
    let provider_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile,
        &model_id,
        Some(provider_configuration_revision.clone()),
    )
    .unwrap();

    let discovery = discoverable_skill("Create and verify Word documents after activation.");
    let activation_ref = discovery.skills[0].activation_ref.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": matches!(
                                case,
                                SkillApprovalResumeProviderCase::DeepSeekExactGrouped
                            ).then_some(PROVIDER_REASONING),
                            "tool_calls": [
                                {
                                    "id": format!("{label}-activate"),
                                    "type": "function",
                                    "function": {
                                        "name": "skills_activate",
                                        "arguments": serde_json::to_string(&json!({
                                            "skillRef": activation_ref,
                                            "reason": "Unlock documents for the next request"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": format!("{label}-approval"),
                                    "type": "function",
                                    "function": {
                                        "name": "apply_patch",
                                        "arguments": serde_json::to_string(&json!({
                                            "operation": "create",
                                            "filePath": "approved.txt",
                                            "content": "approved"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": format!("{label}-existing-sibling"),
                                    "type": "function",
                                    "function": {
                                        "name": "todo_update",
                                        "arguments": serde_json::to_string(&json!({
                                            "items": [],
                                            "explanation": "Execute exactly once after resume"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": format!("{label}-guessed-new-tool"),
                                    "type": "function",
                                    "function": {
                                        "name": "read_word",
                                        "arguments": serde_json::to_string(&json!({
                                            "path": "not-created.docx"
                                        })).unwrap()
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Approval resumed under the frozen batch contract.",
                            "reasoning_content": matches!(
                                case,
                                SkillApprovalResumeProviderCase::DeepSeekExactGrouped
                            ).then_some("The next request now includes the activated ToolSet.")
                        },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let entry = discovery.skills[0].clone();
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls_for_host = Arc::clone(&resolver_calls);
    let resolver: AgentSkillActivationResolver = Arc::new(move |selection| {
        resolver_calls_for_host.fetch_add(1, Ordering::SeqCst);
        assert_eq!(selection.skill_id().as_str(), entry.id);
        assert_eq!(selection.expected_revision().as_str(), entry.revision);
        let skill_id = crate::skills::SkillId::parse(entry.id.clone()).unwrap();
        let source_id = skill_id.source_id().clone();
        let resources = crate::skills::memory_resource_session_for_test(
            skill_id,
            crate::skills::SkillRevision::parse(entry.revision.clone()).unwrap(),
            source_id,
            Vec::new(),
        )
        .unwrap();
        Ok(AgentResolvedSkillActivation {
            skill: AgentActivatedSkill {
                id: entry.id.clone(),
                name: entry.name.clone(),
                revision: entry.revision.clone(),
                source: "bundled:application".to_string(),
                instructions: INSTRUCTIONS.to_string(),
                source_bytes: u64::try_from(INSTRUCTIONS.len()).unwrap(),
                resources: None,
            },
            resources: Arc::new(resources),
        })
    });
    let skill_resources = Arc::new(crate::skills::SkillResourceSession::empty());
    let host_services = AgentRuntimeHostServices::new()
        .with_storage(Arc::clone(&storage))
        .with_skill_activation_resolver(resolver)
        .with_skill_resources(Arc::clone(&skill_resources))
        .with_provider_continuation_vault(Arc::clone(&vault));

    let mut input = conversation_context_input(vec![message(
        "user",
        "Activate documents, request the write, and settle all siblings.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.provider_configuration_revision = Some(provider_configuration_revision);
    input.provider_profile_config = Some(provider_profile);
    input.provider_protocol_key = Some(provider_protocol.clone());
    input.model = model_id;
    input.max_tokens = Some(2_048);
    input.stream = Some(false);
    input.assistant_message_id = Some(assistant_message_id);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.clone()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: Default::default(),
            patch: AgentPatchPermission::RequireApproval,
        },
    });
    input.skill_discovery = Some(discovery);

    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input.clone(),
            Some(run_id.clone()),
            None,
            AgentCancellationToken::new(),
            Some(host_services.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        waiting.status,
        AgentRunStatus::WaitingForApproval,
        "{case:?}"
    );
    let checkpoint = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some((**checkpoint).clone()),
            _ => None,
        })
        .expect("approval checkpoint");
    assert!(checkpoint.tool_set.active_capability_ids.is_empty());
    assert!(!checkpoint
        .tool_set
        .exposed_tool_names
        .iter()
        .any(|name| name == "read_word"));
    assert_eq!(checkpoint.queued_tool_calls.len(), 2);
    assert!(serde_json::to_string(&checkpoint.extension_snapshots)
        .unwrap()
        .contains("bundled:application:documents"));
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let pending_call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == pending_call_id)
        .cloned()
        .expect("pending approval call is frozen");

    let mut resume_input = input;
    resume_input.messages.clear();
    resume_input.skill_discovery = None;
    resume_input.resume_checkpoint = Some(checkpoint);
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: pending_call_id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call_id.clone(),
            tool: pending_call.name.clone(),
            args: pending_call.args.clone(),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending_call_id,
            tool: pending_call.name,
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    });
    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some(run_id),
            None,
            AgentCancellationToken::new(),
            Some(host_services),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed, "{case:?}");
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    let existing_results = completed
        .events
        .iter()
        .filter(|event| {
            matches!(event, AgentEvent::ToolResult { result, .. } if result.tool == "todo_update" && result.ok)
        })
        .count();
    assert_eq!(existing_results, 1, "{case:?}");
    let guessed_result = completed
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "read_word" => Some(result),
            _ => None,
        })
        .expect("same-response guessed Tool must settle after resume");
    assert!(!guessed_result.ok);
    assert_eq!(
        guessed_result
            .result
            .as_ref()
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("agent.tool_requires_skill_activation")
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let request_tool_names = |request: &Value| {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert!(!request_tool_names(&requests[0]).contains(&"read_word".to_string()));
    assert!(request_tool_names(&requests[1]).contains(&"read_word".to_string()));
    let resumed_request = serde_json::to_string(&requests[1]).unwrap();
    assert!(resumed_request.contains("agent.tool_requires_skill_activation"));
    assert_eq!(resumed_request.matches(INSTRUCTIONS).count(), 1);
    assert!(!resumed_request.contains("skillActivationBoundary"));
    if matches!(case, SkillApprovalResumeProviderCase::DeepSeekExactGrouped) {
        let grouped_turn = requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["reasoning_content"].as_str() == Some(PROVIDER_REASONING))
            .expect("DeepSeek exact grouped turn must survive Approval resume");
        assert_eq!(grouped_turn["tool_calls"].as_array().unwrap().len(), 4);
        let provider_result_ids = requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .map(|message| message["tool_call_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            provider_result_ids,
            [
                "deepseek-activate",
                "deepseek-approval",
                "deepseek-existing-sibling",
                "deepseek-guessed-new-tool",
            ]
        );
        assert_eq!(
            vault
                .list_replayable_for_conversation(&conversation_id, &provider_protocol)
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn skill_activation_before_approval_restores_frozen_batch_for_generic_and_deepseek() {
    for case in [
        SkillApprovalResumeProviderCase::Generic,
        SkillApprovalResumeProviderCase::DeepSeekExactGrouped,
    ] {
        run_skill_activation_approval_resume_case(case).await;
    }
}

#[tokio::test]
async fn deepseek_grouped_activation_failure_settles_exposed_sibling_without_boundary() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    const CONVERSATION_ID: &str = "conversation-deepseek-skill-cocall";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-deepseek-skill-cocall";
    const RUN_ID: &str = "run-deepseek-skill-cocall";
    const MODEL_ID: &str = "deepseek-skill-cocall";
    const REASONING: &str = "Activate the selected Skill and update the existing todo contract.";

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek Skill co-call".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_MESSAGE_ID.to_string(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut provider_profile = ProviderProfileConfig::deepseek_v4_default();
    provider_profile.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::Max,
    };
    let provider_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let discovery = discoverable_skill("A fixture Skill whose activation fails deterministically.");
    let activation_ref = discovery.skills[0].activation_ref.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let second_request_for_server = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": REASONING,
                            "tool_calls": [
                                {
                                    "id": "deepseek-activate-fails",
                                    "type": "function",
                                    "function": {
                                        "name": "skills_activate",
                                        "arguments": serde_json::to_string(&json!({
                                            "skillRef": activation_ref,
                                            "reason": "Exercise independent same-response settlement"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": "deepseek-todo-succeeds",
                                    "type": "function",
                                    "function": {
                                        "name": "todo_update",
                                        "arguments": serde_json::to_string(&json!({
                                            "items": [],
                                            "explanation": "The sibling remains independent"
                                        })).unwrap()
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                *second_request_for_server.lock().unwrap() = Some(request);
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Grouped sibling settled after activation failure.",
                            "reasoning_content": "Both Tool results are authoritative."
                        },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls_for_host = Arc::clone(&resolver_calls);
    let failing_resolver: AgentSkillActivationResolver = Arc::new(move |_| {
        resolver_calls_for_host.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::new("fixture Skill activation failed"))
    });
    let mut input = conversation_context_input(vec![message(
        "user",
        "Activate the fixture Skill and update the todo in one response.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "deepseek-test-token".to_string();
    input.provider_profile_config = Some(provider_profile);
    input.provider_protocol_key = Some(provider_protocol.clone());
    input.model = MODEL_ID.to_string();
    input.max_tokens = Some(1_024);
    input.stream = Some(false);
    input.assistant_message_id = Some(ASSISTANT_MESSAGE_ID.to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    input.skill_discovery = Some(discovery);

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_skill_activation_resolver(failing_resolver)
                    .with_skill_resources(Arc::new(crate::skills::SkillResourceSession::empty()))
                    .with_provider_continuation_vault(Arc::clone(&vault)),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    let activation_result = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "skills_activate" => {
                Some(result)
            }
            _ => None,
        })
        .expect("failed activation result");
    assert!(!activation_result.ok);
    assert!(activation_result
        .error
        .as_deref()
        .is_some_and(|error| error.contains("fixture Skill activation failed")));
    let sibling_result = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "todo_update" => Some(result),
            _ => None,
        })
        .expect("independent sibling result");
    assert!(sibling_result.ok);
    assert!(!serde_json::to_string(&output.events)
        .unwrap()
        .contains("skillActivationBoundary"));

    let second_request = second_request.lock().unwrap().clone().unwrap();
    let messages = second_request["messages"].as_array().unwrap();
    let grouped_turn = messages
        .iter()
        .find(|message| message["reasoning_content"].as_str() == Some(REASONING))
        .expect("exact grouped Provider turn must replay");
    let provider_call_ids = grouped_turn["tool_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        provider_call_ids,
        ["deepseek-activate-fails", "deepseek-todo-succeeds"]
    );
    let result_ids = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        result_ids,
        ["deepseek-activate-fails", "deepseek-todo-succeeds"]
    );
    assert!(!serde_json::to_string(&second_request)
        .unwrap()
        .contains("skillActivationBoundary"));

    let persisted_turns = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &provider_protocol)
        .unwrap();
    assert_eq!(persisted_turns.len(), 1);
    assert_eq!(
        persisted_turns[0]
            .assistant_turn
            .provider_tool_calls()
            .len(),
        2
    );
}

#[tokio::test]
async fn anthropic_payload_keeps_current_user_skill_and_attachment_compatible() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut body_start = None;
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_len = Some(start + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(&request[body_start.unwrap()..expected_len.unwrap()]).unwrap()
    }

    async fn write_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(std::sync::Mutex::new(None));
    let captured_for_server = captured.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        *captured_for_server.lock().unwrap() = Some(read_json_request(&mut stream).await);
        write_response(
            &mut stream,
            json!({
                "content": [{ "type": "text", "text": "done" }],
                "stop_reason": "end_turn"
            }),
        )
        .await;
    });

    let mut input = conversation_context_input(vec![message("user", "CURRENT_USER_MARKER")]);
    input.api_url = format!("http://{address}/v1/messages");
    input.api_token = "test-token".to_string();
    input.api_style = Some(crate::protocol::AgentApiStyle::AnthropicCompatible);
    freeze_runtime_test_generic_provider(&mut input, "anthropic-user-skill-attachment");
    input.stream = Some(false);
    input.skill_activation = Some(activated_skill("ANTHROPIC_SKILL_MARKER"));
    input.attachments = vec![AgentInputAttachment {
        id: "attachment-anthropic".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "notes.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 19,
        encoding: AgentInputAttachmentEncoding::Utf8,
        data: "ATTACHMENT_MARKER".to_string(),
        truncated: None,
    }];

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            None,
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(activated_skill_authority())),
        )
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(output.content, "done");
    let payload = captured.lock().unwrap().take().unwrap();
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let serialized = serde_json::to_string(&messages[0]["content"]).unwrap();
    let current = serialized.find("CURRENT_USER_MARKER").unwrap();
    let skill = serialized.find("ANTHROPIC_SKILL_MARKER").unwrap();
    let attachment = serialized.find("ATTACHMENT_MARKER").unwrap();
    assert!(current < attachment && attachment < skill);
}

#[test]
fn conversation_history_tool_is_stable_even_without_a_persisted_conversation() {
    let mut input = conversation_context_input(vec![message("user", "Current question")]);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-1".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let capabilities =
        prepare_runtime_capabilities(&input, "history-capability", &[], true, None).unwrap();
    assert!(capabilities
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "conversation_history"));

    input.context = None;
    let capabilities =
        prepare_runtime_capabilities(&input, "no-history-capability", &[], true, None).unwrap();
    assert!(capabilities
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "conversation_history"));
}

#[tokio::test]
async fn durable_compaction_runs_before_capacity_gate_and_then_sends_rebuilt_context() {
    use crate::context::{ContextCompactionGeneration, ContextCompactionSummary};
    use crate::protocol::AgentApiStyle;
    use crate::{
        AgentUsage, ContextCompactionPrefix, ContextCompactionSourceItem,
        ContextCompactionSummaryDraft, ContextJournalCursor,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap();
        request[body_start..].to_vec()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let request_bodies_for_server = request_bodies.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let body = read_http_body(&mut stream).await;
            request_bodies_for_server.lock().unwrap().push(body);
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "todo-after-compaction",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": serde_json::to_string(&json!({
                                        "items": [{
                                            "title": "Verify compacted context",
                                            "status": "completed"
                                        }]
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "done" },
                        "finish_reason": "stop"
                    }]
                })
            };
            let response_body = serde_json::to_vec(&response).unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&response_body).await.unwrap();
        }
    });

    let old_user = AgentChatMessage {
        message_id: Some("user-old".to_string()),
        role: "user".to_string(),
        content: format!("OLD_USER_MARKER {}", "x".repeat(60_000)),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    };
    let old_assistant_content = format!("OLD_ASSISTANT_MARKER {}", "y".repeat(60_000));
    let mut old_assistant_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::Completed,
        vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            content: old_assistant_content.clone(),
            truncated: false,
        }],
    );
    old_assistant_trace.run_id = "run-old".to_string();
    old_assistant_trace.conversation_id = "conversation-1".to_string();
    old_assistant_trace.assistant_message_id = "assistant-old".to_string();
    let old_assistant = traced_assistant_message(&old_assistant_content, old_assistant_trace);
    let current_user = AgentChatMessage {
        message_id: Some("user-current".to_string()),
        role: "user".to_string(),
        content: "continue".to_string(),
        created_at: None,
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    };
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(50_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-current".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![old_user, old_assistant, current_user.clone()],
    };
    freeze_runtime_test_generic_provider(&mut input, "durable-compaction-capacity");
    let durable_prefix = Arc::new(ContextCompactionPrefix {
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-runtime".to_string(),
        covered_through: ContextJournalCursor::message("user-current"),
        previous_summary: None,
        source_items: vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-old"),
                role: "user".to_string(),
                content: "old request".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("assistant-old"),
                role: "assistant".to_string(),
                content: "old answer".to_string(),
                created_at: 2,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-current"),
                role: "user".to_string(),
                content: "continue".to_string(),
                created_at: 3,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
        ],
    });
    let compacted_continuity = crate::ContextContinuitySnapshot::from_prefix(&durable_prefix)
        .expect("test durable prefix should produce continuity records");
    let compacted_summary = ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-runtime".to_string(),
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-runtime".to_string(),
        previous_summary_id: None,
        covered_through: ContextJournalCursor::message("user-current"),
        content: "COMPACTED_HISTORY_MARKER: the old task was completed.".to_string(),
        continuity: compacted_continuity,
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 40_000,
        summary_input_tokens: 32,
        continuity_input_tokens: 64,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 96,
        created_at: 1,
    };
    let mut compacted_state = create_conversation_context_state(AgentChatInput {
        context_compaction_summary: Some(compacted_summary),
        messages: vec![current_user],
        ..input.clone()
    })
    .unwrap();
    let compacted_baseline = compacted_state.shared_baseline().unwrap();
    let mut uncompacted_state = create_conversation_context_state(input.clone()).unwrap();
    let uncompacted_baseline = uncompacted_state.shared_baseline().unwrap();
    let prepare_count = Arc::new(AtomicUsize::new(0));
    let generate_count = Arc::new(AtomicUsize::new(0));
    let commit_count = Arc::new(AtomicUsize::new(0));
    let trace_publish_count = Arc::new(AtomicUsize::new(0));
    let trace_snapshots = Arc::new(Mutex::new(Vec::new()));
    let prepare_counter = prepare_count.clone();
    let generate_counter = generate_count.clone();
    let commit_counter = commit_count.clone();
    let commit_count_for_trace = commit_count.clone();
    let trace_publish_counter = trace_publish_count.clone();
    let trace_snapshots_for_observer = trace_snapshots.clone();
    let compacted_baseline_for_commit = compacted_baseline.clone();
    let compacted_baseline_for_trace = compacted_baseline.clone();
    let durable_prefix_for_prepare = durable_prefix.clone();
    let steer_input = AgentSteerInputQueue::new();
    let steer_input_during_compaction = steer_input.clone();
    let services = AgentContextCompactionServices::new(
        move |request, _| {
            prepare_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                request.covered_through,
                ContextJournalCursor::message("user-current")
            );
            let durable_prefix = durable_prefix_for_prepare.clone();
            async move { Ok(AgentContextCompactionPrepareOutcome::Ready(durable_prefix)) }
        },
        move |request, _| {
            generate_counter.fetch_add(1, Ordering::SeqCst);
            let large_attachment_text =
                format!("LARGE_GUIDANCE_ATTACHMENT_MARKER {}", "z".repeat(20_000));
            assert_eq!(
                steer_input_during_compaction
                    .enqueue(crate::AgentSteerInput {
                        guidance_id: "guidance-during-compaction".to_string(),
                        client_message_id: "client-during-compaction".to_string(),
                        content: "Preserve this constraint across compaction.".to_string(),
                        attachments: vec![AgentInputAttachment {
                            id: "attachment-during-compaction".to_string(),
                            kind: AgentInputAttachmentKind::File,
                            name: "large-guidance.txt".to_string(),
                            mime_type: Some("text/plain".to_string()),
                            size_bytes: large_attachment_text.len() as u64,
                            encoding: AgentInputAttachmentEncoding::Utf8,
                            data: large_attachment_text,
                            truncated: None,
                        }],
                        attachment_library: Some(crate::AgentAttachmentLibraryContext {
                            root_path: None,
                            conversation_id: Some("conversation-1".to_string()),
                            project_id: None,
                            conversation_attachments: Vec::new(),
                            project_attachments: Vec::new(),
                        }),
                        created_at: 42,
                    })
                    .unwrap(),
                AgentSteerEnqueueOutcome::Queued
            );
            async move {
                let observation =
                    crate::model_request_observation::ModelRequestObservationBuilder::new(
                        format!("model-request-{}", request.operation_id),
                        request.run_id.clone(),
                        Some(request.conversation_id.clone()),
                        Some(request.assistant_message_id.clone()),
                        Some(request.operation_id.clone()),
                        request.request_index,
                        crate::ModelRequestPurpose::ContextCompaction,
                        "test-model",
                        AgentApiStyle::OpenAiCompatible,
                        None,
                        1,
                    )
                    .completed(
                        Some(AgentUsage {
                            input_tokens: Some(100),
                            output_tokens: Some(20),
                            output_thinking_tokens: None,
                            total_tokens: Some(120),
                            cached_input_tokens: None,
                            cache_creation_input_tokens: None,
                            billable_request_count: Some(1),
                        }),
                        Some("stop".to_string()),
                        2,
                    )
                    .unwrap();
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: "summary-runtime".to_string(),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "COMPACTED_HISTORY_MARKER: the old task was completed."
                            .to_string(),
                        continuity: request.continuity,
                        generation: ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: 32,
                        continuity_input_tokens: 64,
                        uncovered_tail_input_tokens: request.uncovered_tail_input_tokens,
                        replacement_input_tokens: 96,
                        created_at: 1,
                    },
                    observation,
                })
            }
        },
        move |request, _| {
            let baseline = compacted_baseline_for_commit.clone();
            commit_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.draft.id, "summary-runtime");
            async move {
                Ok(AgentContextCompactionCommitOutcome::Applied {
                    summary_id: "summary-runtime".to_string(),
                    baseline: Box::new(baseline),
                })
            }
        },
        |_, _| async { Ok(()) },
    );
    let emitted_events = Arc::new(Mutex::new(Vec::new()));
    let emitted_events_for_callback = emitted_events.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        emitted_events_for_callback.lock().unwrap().push(event);
    });
    let observations = Arc::new(Mutex::new(Vec::new()));
    let observations_for_callback = observations.clone();
    let model_request_observer: AgentModelRequestObserver = Arc::new(move |observation| {
        observations_for_callback.lock().unwrap().push(observation);
    });
    let trace_observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        trace_publish_counter.fetch_add(1, Ordering::SeqCst);
        trace_snapshots_for_observer.lock().unwrap().push(snapshot);
        let baseline = if commit_count_for_trace.load(Ordering::SeqCst) == 0 {
            uncompacted_baseline.clone()
        } else {
            compacted_baseline_for_trace.clone()
        };
        Ok(Some(baseline))
    });

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-compaction".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_context_compaction(services)
                    .with_trace_observer(trace_observer)
                    .with_model_request_observer(model_request_observer)
                    .with_steer_input(steer_input),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();
    let request_bodies = request_bodies
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .map(|body| String::from_utf8(body).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(output.content, "done");
    assert_eq!(request_bodies.len(), 2);
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);
    assert_eq!(generate_count.load(Ordering::SeqCst), 1);
    assert_eq!(commit_count.load(Ordering::SeqCst), 1);
    assert_eq!(trace_publish_count.load(Ordering::SeqCst), 8);
    let usage = output.usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.output_tokens, Some(20));
    assert_eq!(usage.billable_request_count, Some(3));
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().all(|observation| {
        observation.purpose == crate::ModelRequestPurpose::AgentLoop
            && observation.estimate.is_some()
    }));
    for request_body in &request_bodies {
        assert!(request_body.contains("COMPACTED_HISTORY_MARKER"));
        assert!(!request_body.contains("OLD_USER_MARKER"));
        assert!(!request_body.contains("OLD_ASSISTANT_MARKER"));
    }
    assert!(!request_bodies[0].contains("Preserve this constraint across compaction."));
    assert!(request_bodies[1].contains("Preserve this constraint across compaction."));
    assert!(request_bodies[1].contains("LARGE_GUIDANCE_ATTACHMENT_MARKER"));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied { guidance_id, .. }
            if guidance_id == "guidance-during-compaction"
    )));
    let events = emitted_events.lock().unwrap();
    let compaction_events = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ContextCompactionStarted { .. }
                    | AgentEvent::ContextCompactionFinished { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(compaction_events.len(), 2);
    let AgentEvent::ContextCompactionStarted {
        operation_id,
        trace_sequence,
        ..
    } = compaction_events[0]
    else {
        panic!("first compaction event should start the operation");
    };
    let AgentEvent::ContextCompactionFinished {
        operation_id: finished_operation_id,
        outcome,
        trace_sequence: finished_trace_sequence,
        ..
    } = compaction_events[1]
    else {
        panic!("second compaction event should finish the operation");
    };
    assert_eq!(finished_operation_id, operation_id);
    assert_eq!(*outcome, AgentContextCompactionEventOutcome::Applied);
    assert_eq!(finished_trace_sequence, trace_sequence);
    let snapshots = trace_snapshots.lock().unwrap();
    let settled_snapshot = snapshots
        .iter()
        .rev()
        .find(|snapshot| {
            snapshot.items.iter().any(|item| matches!(
                item,
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    phase: crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Finished,
                    ..
                }
            ))
        })
        .expect("compaction settlement must be published durably before its live event");
    let compaction_items = settled_snapshot
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(compaction_items.len(), 2);
    assert_eq!(compaction_items[0].sequence(), *trace_sequence);
    assert!(compaction_items[1].sequence() > compaction_items[0].sequence());
}

#[tokio::test]
async fn context_capacity_guard_rejects_the_initial_request_before_network_io() {
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(8_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![AgentChatMessage {
            message_id: None,
            role: "user".to_string(),
            content: "x".repeat(90_000),
            created_at: None,
            conversation_turn_trace: None,
            conversation_model_context_items: Vec::new(),
        }],
    };
    freeze_runtime_test_generic_provider(&mut input, "capacity-guard-initial");

    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();

    assert_eq!(error.code(), Some("context_capacity_exceeded"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["status"].as_str()),
        Some("over_budget")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details["usage"]["measurementMode"].as_str()),
        Some("incremental_cache")
    );
    assert!(timeout(Duration::from_millis(100), listener.accept())
        .await
        .is_err());
}

#[tokio::test]
async fn context_capacity_guard_accepts_budgeted_tool_results_for_the_next_request() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let large_file = (0..2_000)
        .map(|_| "x".repeat(120))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(workspace.join("large.txt"), large_file).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request_seen = Arc::new(AtomicBool::new(false));
    let second_request_seen_by_server = second_request_seen.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_http_request(&mut stream).await;
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "read-large-file",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"large.txt\",\"maxLines\":2000}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        )
        .await;

        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("budgeted read_file result should permit a second model request")
            .unwrap();
        second_request_seen_by_server.store(true, Ordering::SeqCst);
        read_http_request(&mut stream).await;
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "summarized" },
                    "finish_reason": "stop"
                }]
            }),
        )
        .await;
    });
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(80_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-capacity".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message("user", "Read large.txt and summarize it")],
    };
    freeze_runtime_test_generic_provider(&mut input, "capacity-guard-tool-results");

    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "summarized");
    assert!(second_request_seen.load(Ordering::SeqCst));
}

#[tokio::test]
async fn active_run_keeps_earlier_exact_tool_exchanges_across_later_model_samples() {
    use tokio::net::TcpListener;

    const FIRST_EXCHANGE_MARKER: &str = "FIRST_TODO_EXACT_MARKER";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "provider-first-todo",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": serde_json::to_string(&json!({
                                        "items": [{
                                            "title": FIRST_EXCHANGE_MARKER,
                                            "status": "completed"
                                        }]
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Checking one more source.",
                            "tool_calls": [{
                                "id": "provider-list-attachments",
                                "type": "function",
                                "function": {
                                    "name": "attachments_list",
                                    "arguments": "{}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                _ => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Both exchanges are still available."
                        },
                        "finish_reason": "stop"
                    }]
                }),
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let mut input =
        conversation_context_input(vec![message("user", "Exercise two tools, then answer.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-active-run-exact".to_string());

    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Both exchanges are still available.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let third_request = serde_json::to_string(&requests[2]).unwrap();
    assert!(
        third_request.contains(FIRST_EXCHANGE_MARKER),
        "a successful intermediate sample must not demote an earlier exact tool exchange"
    );
    assert!(third_request.contains("attachments_list"));
}

#[tokio::test]
async fn streams_write_file_previews_end_to_end_without_persisting_them() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::ChatConversationRecord;
    use crate::storage::service::StorageService;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_len.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected_len = Some(header_end + 4 + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn tool_completion(id: &str, arguments: Value) -> Value {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": "write_file",
                            "arguments": serde_json::to_string(&arguments).unwrap()
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
    }

    async fn write_append_stream(stream: &mut TcpStream, draft_id: &str) {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let fragments = [
            format!(
                "{{\"phase\":\"append\",\"draftId\":\"{draft_id}\",\"index\":0,\"content\":\"line 1\\n"
            ),
            "line 2\\n".to_string(),
            "line 3\\n".to_string(),
            "line 4\\n\"}".to_string(),
        ];
        let tool_name_fragments = ["write_", "file", "", ""];
        for (fragment, tool_name) in fragments.into_iter().zip(tool_name_fragments) {
            let frame = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-append",
                            "type": "function",
                            "function": {
                                "name": tool_name,
                                "arguments": fragment
                            }
                        }]
                    }
                }]
            });
            stream
                .write_all(format!("data: {frame}\n\n").as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        stream
            .write_all(
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("app.db")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-preview".to_string(),
            project_id: None,
            model_id: None,
            title: "Preview".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_storage = storage.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            match request_index {
                0 => {
                    write_json_response(
                        &mut stream,
                        tool_completion(
                            "call-begin",
                            json!({
                                "phase": "begin",
                                "filePath": "preview.md",
                                "mode": "create"
                            }),
                        ),
                    )
                    .await;
                }
                1 => {
                    let draft_id = server_storage
                        .list_agent_file_drafts_for_run("run-preview")
                        .unwrap()[0]
                        .id
                        .clone();
                    write_append_stream(&mut stream, &draft_id).await;
                }
                2 => {
                    let draft_id = server_storage
                        .list_agent_file_drafts_for_run("run-preview")
                        .unwrap()[0]
                        .id
                        .clone();
                    write_json_response(
                        &mut stream,
                        tool_completion(
                            "call-abort",
                            json!({ "phase": "abort", "draftId": draft_id }),
                        ),
                    )
                    .await;
                }
                _ => {
                    write_json_response(
                        &mut stream,
                        json!({
                            "choices": [{
                                "message": { "role": "assistant", "content": "done" },
                                "finish_reason": "stop"
                            }]
                        }),
                    )
                    .await;
                }
            }
        }
    });

    let captured = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = captured.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-preview".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-preview".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message("user", "create a preview")],
    };
    freeze_runtime_test_generic_provider(&mut input, "write-file-preview-stream");
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-preview".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_storage(storage.clone())),
        )
        .await
        .unwrap();
    server.await.unwrap();

    let previews = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::FileWritePreviewUpdated { preview, .. } => Some(preview.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let context_snapshots = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ContextWindowUpdated { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let preview_additions = previews
        .iter()
        .map(|preview| preview.additions)
        .collect::<Vec<_>>();
    let mut preview_content = String::new();
    for preview in &previews {
        assert_eq!(preview.content_offset_bytes, preview_content.len() as u64);
        preview_content.push_str(&preview.content_delta);
    }
    assert!(preview_additions.len() >= 3, "{preview_additions:?}");
    assert_eq!(preview_additions.last().copied(), Some(4));
    assert_eq!(preview_content, "line 1\nline 2\nline 3\nline 4\n");
    assert!(context_snapshots.is_empty());
    assert!(output.events.iter().all(|event| !matches!(
        event,
        AgentEvent::FileWritePreviewUpdated { .. } | AgentEvent::FileWritePreviewCleared { .. }
    )));
    assert_eq!(output.content, "done");
    let append_event = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. }
                if call.tool == "write_file" && call.args["phase"] == "append" =>
            {
                Some(call)
            }
            _ => None,
        })
        .expect("write_file append event");
    assert_runtime_owned_tool_call_id(&append_event.id);
    assert_eq!(append_event.args["content"], "[stored in private draft]");
    assert_eq!(append_event.args["contentBytes"], 28);
    let append_call_id = append_event.id.clone();

    let append_trace = output
        .conversation_turn_trace
        .as_ref()
        .expect("durable conversation trace")
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id, operation, ..
            } if call_id == &append_call_id => Some(operation),
            _ => None,
        })
        .expect("write_file append trace item");
    assert_eq!(
        append_trace["content"],
        "[write_file chunk omitted from conversation history]"
    );
    assert_eq!(append_trace["contentBytes"], 28);
    assert_eq!(
        storage
            .list_agent_file_drafts_for_run("run-preview")
            .unwrap()[0]
            .status,
        "aborted"
    );
}

#[tokio::test]
async fn approval_resume_restores_prior_context_and_continues_queued_tools() {
    use crate::protocol::{
        AgentApprovalDecision, AgentApprovalDecisionStatus, AgentCommandPermission,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentToolContinuation,
        AgentWritePermission,
    };
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut body_start = None;
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_len = Some(start + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(&request[body_start.unwrap()..expected_len.unwrap()]).unwrap()
    }

    async fn write_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn native_tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap()
            }
        })
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("source.txt"), "evidence-before-approval").unwrap();
    std::fs::write(workspace.join("queued.txt"), "evidence-from-queued-tool").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let final_request = Arc::new(std::sync::Mutex::new(None::<Value>));
    let server_final_request = final_request.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I will read the source first.",
                            "tool_calls": [
                                native_tool_call(
                                    "todo-before",
                                    "todo_update",
                                    json!({
                                        "items": [{
                                            "title": "Collect evidence and write report",
                                            "status": "in_progress"
                                        }]
                                    })
                                ),
                                native_tool_call(
                                    "read-before",
                                    "read_file",
                                    json!({ "path": "source.txt" })
                                )
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I have the evidence and will prepare the report.",
                            "tool_calls": [
                                native_tool_call(
                                    "patch-approval",
                                    "apply_patch",
                                    json!({
                                        "operation": "create",
                                        "filePath": "report.txt",
                                        "content": "draft report"
                                    })
                                ),
                                native_tool_call(
                                    "read-queued",
                                    "read_file",
                                    json!({ "path": "queued.txt" })
                                )
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                _ => {
                    *server_final_request.lock().unwrap() = Some(request);
                    json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "resumed with evidence" },
                            "finish_reason": "stop"
                        }]
                    })
                }
            };
            write_response(&mut stream, response).await;
        }
    });

    let mut base_input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-checkpoint".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-checkpoint".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: Some(activated_skill("SKILL_SNAPSHOT_BEFORE_APPROVAL")),
        skill_discovery: None,
        messages: vec![message("user", "collect evidence and write report.txt")],
    };
    freeze_runtime_test_generic_provider(&mut base_input, "approval-resume");
    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            base_input.clone(),
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(activated_skill_authority())),
        )
        .await
        .unwrap();
    assert_eq!(waiting.status, AgentRunStatus::WaitingForApproval);
    assert!(waiting.conversation_turn_trace.is_none());
    let checkpoint = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some((**checkpoint).clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(checkpoint.queued_tool_calls.len(), 1);
    let todo_call_id = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "todo_update" => {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("todo call event");
    let read_before_call_id = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. }
                if call.tool == "read_file" && call.args["path"] == "source.txt" =>
            {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("first read call event");
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let pending_checkpoint_call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == pending_call_id)
        .cloned()
        .expect("checkpoint must freeze the pending call");
    let queued_call_id = checkpoint.queued_tool_calls[0].call.id.clone();
    for call_id in [
        &todo_call_id,
        &read_before_call_id,
        &pending_call_id,
        &queued_call_id,
    ] {
        assert_runtime_owned_tool_call_id(call_id);
    }
    assert_eq!(
        [
            &todo_call_id,
            &read_before_call_id,
            &pending_call_id,
            &queued_call_id,
        ]
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .len(),
        4
    );
    for completed_call_id in [&todo_call_id, &read_before_call_id] {
        assert!(waiting.events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolResult { result, .. }
                if result.call_id == completed_call_id.as_str()
        )));
    }
    assert!(checkpoint
        .extension_snapshots
        .iter()
        .any(|snapshot| snapshot.extension_id == "todo"));
    assert!(checkpoint.context_items.iter().any(|item| {
        item.sources == vec!["skill_instructions"]
            && item.content.contains("SKILL_SNAPSHOT_BEFORE_APPROVAL")
            && item.origin.as_ref().is_some_and(|origin| {
                origin.kind == "skill" && origin.id == "workspace:workspace-1:review"
            })
    }));
    assert!(!format!("{checkpoint:?}").contains("SKILL_SNAPSHOT_BEFORE_APPROVAL"));
    assert!(!format!("{:?}", waiting.events).contains("SKILL_SNAPSHOT_BEFORE_APPROVAL"));
    assert!(checkpoint
        .conversation_trace_items
        .iter()
        .any(|item| matches!(
            item,
            ConversationTurnTraceItem::AssistantNarration { content, .. }
                if content == "I have the evidence and will prepare the report."
        )));
    assert!(
        !checkpoint.conversation_model_context_items.is_empty(),
        "the approval checkpoint must preserve its exact active-run model projection"
    );

    let mut resume_input = base_input;
    resume_input.messages.clear();
    // The checkpoint is authoritative for the logical run; a changed resume payload must not
    // replace the frozen Skill snapshot selected before approval.
    resume_input.skill_activation = Some(activated_skill("SKILL_CHANGED_DURING_RESUME"));
    resume_input.resume_checkpoint = Some(checkpoint);
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: pending_call_id.clone(),
        status: AgentApprovalDecisionStatus::Rejected,
        message: Some("Keep the evidence but revise the report first.".to_string()),
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call_id.clone(),
            tool: pending_checkpoint_call.name,
            args: pending_checkpoint_call.args,
            approval_status: AgentApprovalStatus::Rejected,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending_call_id.clone(),
            tool: "apply_patch".to_string(),
            ok: false,
            result: None,
            error: Some(
                "user rejected: Keep the evidence but revise the report first.".to_string(),
            ),
        },
    });
    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(activated_skill_authority())),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(completed.content, "resumed with evidence");
    assert_eq!(completed.todo.as_ref().unwrap().revision, 1);
    let request = final_request.lock().unwrap().take().unwrap();
    let messages = serde_json::to_string(&request["messages"]).unwrap();
    assert!(messages.contains("evidence-before-approval"));
    assert!(messages.contains("evidence-from-queued-tool"));
    assert!(messages.contains("user rejected"));
    for call_id in [
        &todo_call_id,
        &read_before_call_id,
        &pending_call_id,
        &queued_call_id,
    ] {
        assert!(messages.contains(call_id.as_str()));
    }
    for provider_call_id in [
        "todo-before",
        "read-before",
        "patch-approval",
        "read-queued",
    ] {
        assert!(!messages.contains(provider_call_id));
    }
    assert!(messages.contains("SKILL_SNAPSHOT_BEFORE_APPROVAL"));
    assert!(!messages.contains("SKILL_CHANGED_DURING_RESUME"));
    let trace = completed.conversation_turn_trace.as_ref().unwrap();
    trace.validate().unwrap();
    for call_id in [
        &todo_call_id,
        &read_before_call_id,
        &pending_call_id,
        &queued_call_id,
    ] {
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall { call_id: item_id, .. }
                        if item_id == call_id
                ))
                .count(),
            1,
            "tool call {call_id} should be retained exactly once"
        );
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolResult { call_id: item_id, .. }
                        if item_id == call_id
                ))
                .count(),
            1,
            "tool result {call_id} should be retained exactly once"
        );
    }
}

#[tokio::test]
async fn skill_resource_text_survives_approval_checkpoint_but_is_omitted_from_durable_history() {
    use crate::protocol::{
        AgentActivatedSkillResources, AgentApprovalDecision, AgentApprovalDecisionStatus,
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentToolContinuation, AgentWritePermission,
    };
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillResourcePath, SkillRevision, SkillSourceId,
    };
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const RESOURCE_MARKER: &str = "SKILL_RESOURCE_CHECKPOINT_SECRET_MARKER";

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut body_start = None;
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap();
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_len = Some(start + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(&request[body_start.unwrap()..expected_len.unwrap()]).unwrap()
    }

    async fn write_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn native_tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap()
            }
        })
    }

    let source_id = SkillSourceId::parse("workspace:workspace-1").unwrap();
    let skill_id = SkillId::parse("workspace:workspace-1:resource-checkpoint").unwrap();
    let revision =
        SkillRevision::parse(format!("skill-package-sha256-v3:{}", "d".repeat(64))).unwrap();
    let resource_path = SkillResourcePath::parse("references/guide.md").unwrap();
    let bytes = RESOURCE_MARKER.as_bytes().to_vec();
    let session = memory_resource_session_for_test(
        skill_id.clone(),
        revision.clone(),
        source_id,
        vec![(
            resource_path.as_str().to_string(),
            SkillResourceKind::Reference,
            bytes,
        )],
    )
    .unwrap();
    let resources = Arc::new(session);
    let package = SkillPackageUri::new(skill_id.clone(), revision.clone());
    let resource_uri = package.resource(resource_path);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_second_request = Arc::clone(&second_request);
    let resumed_request = Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_resumed_request = Arc::clone(&resumed_request);
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [native_tool_call(
                                "read-skill-resource",
                                "skills_read_resource",
                                json!({ "uri": resource_uri.as_str() })
                            )]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => {
                    *captured_second_request.lock().unwrap() = Some(request);
                    json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [native_tool_call(
                                "materialize-after-read",
                                "apply_patch",
                                json!({
                                    "operation": "create",
                                    "filePath": "report.txt",
                                    "content": "report"
                                })
                            )]
                        },
                        "finish_reason": "tool_calls"
                    }]
                    })
                }
                _ => {
                    *captured_resumed_request.lock().unwrap() = Some(request);
                    json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "continued after approval"
                            },
                            "finish_reason": "stop"
                        }]
                    })
                }
            };
            write_response(&mut stream, response).await;
        }
    });

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-skill-resource-checkpoint".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-skill-resource-checkpoint".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: Some(AgentSkillActivation {
            activation_revision: "activation-sha256-v1:resource-checkpoint".to_string(),
            skills: vec![AgentActivatedSkill {
                id: skill_id.as_str().to_string(),
                name: "resource-checkpoint".to_string(),
                revision: revision.as_str().to_string(),
                source: "workspace:workspace-1".to_string(),
                instructions: "Read references progressively.".to_string(),
                source_bytes: 30,
                resources: Some(AgentActivatedSkillResources {
                    root_uri: package.to_string(),
                    resource_count: 1,
                    kinds: vec!["reference".to_string()],
                }),
            }],
        }),
        skill_discovery: None,
        messages: vec![message(
            "user",
            "Read the Skill reference, then prepare a report.",
        )],
    };
    freeze_runtime_test_generic_provider(&mut input, "skill-resource-approval");

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input.clone(),
            Some("run-skill-resource-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(Arc::clone(&resources))),
        )
        .await
        .unwrap();

    assert_eq!(output.status, AgentRunStatus::WaitingForApproval);
    let model_request = second_request.lock().unwrap().clone().unwrap();
    assert!(model_request.to_string().contains(RESOURCE_MARKER));
    let checkpoint = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some(checkpoint.as_ref()),
            _ => None,
        })
        .unwrap();
    let resource_read_call_id = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "skills_read_resource" => {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("Skill resource read call");
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    assert_runtime_owned_tool_call_id(&resource_read_call_id);
    assert_runtime_owned_tool_call_id(&pending_call_id);
    assert_ne!(resource_read_call_id, pending_call_id);
    let model_request_messages = serde_json::to_string(&model_request["messages"]).unwrap();
    assert!(model_request_messages.contains(&resource_read_call_id));
    assert!(!model_request_messages.contains("read-skill-resource"));
    let checkpoint_json = serde_json::to_string(checkpoint).unwrap();
    assert!(checkpoint_json.contains(RESOURCE_MARKER));
    assert!(!serde_json::to_string(&output.events)
        .unwrap()
        .contains(RESOURCE_MARKER));
    assert!(
        !checkpoint.conversation_model_context_items.is_empty(),
        "the approval checkpoint must preserve its exact active-run model projection"
    );

    let mut resume_input = input;
    resume_input.messages.clear();
    resume_input.skill_activation = None;
    resume_input.resume_checkpoint = Some(checkpoint.clone());
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: pending_call_id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call_id.clone(),
            tool: "apply_patch".to_string(),
            args: json!({
                "operation": "create",
                "filePath": "report.txt",
                "content": "report"
            }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending_call_id.clone(),
            tool: "apply_patch".to_string(),
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    });
    let resumed_trace_snapshots = Arc::new(Mutex::new(Vec::new()));
    let resumed_trace_snapshots_for_observer = Arc::clone(&resumed_trace_snapshots);
    let resumed_trace_observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        resumed_trace_snapshots_for_observer
            .lock()
            .unwrap()
            .push(snapshot);
        Ok(None)
    });

    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some("run-skill-resource-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_skill_resources(resources)
                    .with_trace_observer(resumed_trace_observer),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(completed.content, "continued after approval");
    let resumed_trace_snapshots = resumed_trace_snapshots.lock().unwrap();
    let setup_snapshot = resumed_trace_snapshots
        .first()
        .expect("approval resume must publish a setup trace snapshot");
    assert!(
        !setup_snapshot.model_context_items.is_empty(),
        "the setup publication must not discard the checkpoint's exact model projection"
    );
    let resumed_request = resumed_request.lock().unwrap().clone().unwrap();
    let resumed_messages = serde_json::to_string(&resumed_request["messages"]).unwrap();
    assert!(resumed_messages.contains(&resource_read_call_id));
    assert!(resumed_messages.contains(&pending_call_id));
    assert!(!resumed_messages.contains("read-skill-resource"));
    assert!(!resumed_messages.contains("materialize-after-read"));
    assert!(resumed_messages.contains("applied"));
    assert!(resumed_messages.contains(RESOURCE_MARKER));
    let trace = completed
        .conversation_turn_trace
        .as_ref()
        .expect("terminal conversation trace");
    assert!(!serde_json::to_string(trace)
        .unwrap()
        .contains(RESOURCE_MARKER));
    for call_id in [&resource_read_call_id, &pending_call_id] {
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall {
                call_id: trace_call_id,
                ..
            } if trace_call_id == call_id
        )));
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                call_id: trace_call_id,
                ..
            } if trace_call_id == call_id
        )));
    }
}

#[tokio::test]
async fn deepseek_cancellation_during_result_publication_closes_grouped_suffix() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    const CONVERSATION_ID: &str = "conversation-deepseek-result-publish-cancel";
    const ASSISTANT_ID: &str = "assistant-deepseek-result-publish-cancel";
    const RUN_ID: &str = "run-deepseek-result-publish-cancel";
    const MODEL_ID: &str = "deepseek-result-publish-cancel-model";

    fn provider_read_call(id: &str, path: &str) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": "read_file",
                "arguments": serde_json::to_string(&json!({ "path": path })).unwrap(),
            }
        })
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("first.txt"), "first result").unwrap();
    std::fs::write(workspace.join("second.txt"), "second must not execute").unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek result publication cancel".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_ID.to_string(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    profile.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::High,
    };
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _request = read_runtime_test_json_request(&mut stream).await;
        write_runtime_test_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "Cancel only after the first result is recorded.",
                        "tool_calls": [
                            provider_read_call("provider-read-first", "first.txt"),
                            provider_read_call("provider-read-second", "second.txt"),
                        ],
                    },
                    "finish_reason": "tool_calls",
                }]
            }),
        )
        .await;
        timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err()
    });

    let cancellation = AgentCancellationToken::new();
    let cancellation_for_observer = cancellation.clone();
    let trace_snapshots = Arc::new(Mutex::new(Vec::new()));
    let trace_snapshots_for_observer = Arc::clone(&trace_snapshots);
    let observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        let result_count = snapshot
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
            .count();
        if result_count == 1 {
            cancellation_for_observer.cancel();
        }
        trace_snapshots_for_observer.lock().unwrap().push(snapshot);
        Ok(None)
    });
    let mut input = conversation_context_input(vec![message(
        "user",
        "Read two files but stop during first-result publication.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(protocol.clone());
    input.model = MODEL_ID.to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some(ASSISTANT_ID.to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek cancellation workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
        },
    });
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            cancellation,
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(storage)
                    .with_provider_continuation_vault(Arc::clone(&vault))
                    .with_trace_observer(observer),
            ),
        )
        .await
        .unwrap();
    let no_second_request = server.await.unwrap();
    assert!(no_second_request);
    assert_eq!(output.status, AgentRunStatus::Cancelled);
    let trace = output
        .conversation_turn_trace
        .as_ref()
        .expect("cancelled grouped turn must retain a complete trace");
    trace.validate().unwrap();
    let results = trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id,
                observation,
                ..
            } => Some((call_id, observation)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(results[0].1.to_string().contains("first result"));
    assert!(results[1].1.to_string().contains("runCancelled"));
    let terminal_snapshot = trace_snapshots
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|snapshot| {
            snapshot
                .items
                .iter()
                .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
                .count()
                == 2
        })
        .cloned()
        .expect("the grouped cancellation must publish one complete model-context snapshot");
    let result_context = terminal_snapshot
        .model_context_items
        .iter()
        .filter_map(|item| {
            item.tool_call_id
                .as_deref()
                .map(|call_id| (call_id, item.is_error))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        result_context,
        vec![(results[0].0.as_str(), false), (results[1].0.as_str(), true)],
        "the authoritative successful result must not be rewritten as an error while the synthetic cancelled suffix must remain an error"
    );
    let persisted = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &protocol)
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].assistant_turn.provider_tool_calls().len(), 2);
}

#[tokio::test]
async fn deepseek_commit_unknown_trace_publish_recovers_staged_turn_without_tool_dispatch() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    const CONVERSATION_ID: &str = "conversation-deepseek-commit-unknown";
    const ASSISTANT_ID: &str = "assistant-deepseek-commit-unknown";
    const RUN_ID: &str = "run-deepseek-commit-unknown";
    const MODEL_ID: &str = "deepseek-commit-unknown-model";
    const OBSERVER_ERROR: &str = "trace observer committed before returning an unknown outcome";

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("never-read.txt"), "must not be dispatched").unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek commit-unknown handoff".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_ID.to_string(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    profile.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::High,
    };
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _request = read_runtime_test_json_request(&mut stream).await;
        write_runtime_test_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "Seal before publishing the handoff.",
                        "tool_calls": [{
                            "id": "provider-commit-unknown-read",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": serde_json::to_string(
                                    &json!({ "path": "never-read.txt" })
                                )
                                .unwrap(),
                            }
                        }],
                    },
                    "finish_reason": "tool_calls",
                }]
            }),
        )
        .await;
        timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err()
    });

    let storage_for_observer = Arc::clone(&storage);
    let observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        if !snapshot
            .items
            .iter()
            .any(|item| matches!(item, ConversationTurnTraceItem::ToolCall { .. }))
        {
            return Ok(None);
        }
        let committed = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: RUN_ID.to_string(),
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: ASSISTANT_ID.to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: snapshot.truncated,
            items: snapshot.items,
        };
        storage_for_observer
            .append_in_progress_conversation_turn_trace(&committed, 1, 2)
            .map_err(AgentError::new)?;
        Err(AgentError::new(OBSERVER_ERROR))
    });

    let mut input = conversation_context_input(vec![message(
        "user",
        "Read one file after the durable provider handoff.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.provider_profile_config = Some(profile);
    input.provider_protocol_key = Some(protocol.clone());
    input.model = MODEL_ID.to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some(ASSISTANT_ID.to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek commit-unknown workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
        },
    });
    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault))
                    .with_trace_observer(observer),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), OBSERVER_ERROR);
    let terminal_trace = error
        .conversation_turn_trace()
        .expect("commit-unknown failure must still close the in-memory grouped turn");
    terminal_trace.validate().unwrap();
    assert_eq!(
        terminal_trace
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { .. }))
            .count(),
        1
    );
    assert!(
        server.await.unwrap(),
        "the failed handoff must not dispatch a Tool or send another Provider request"
    );

    let recovered = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &protocol)
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].assistant_turn.provider_tool_calls().len(), 1);
    assert_eq!(
        recovered[0].assistant_turn.provider_tool_calls()[0].id,
        "provider-commit-unknown-read"
    );
}

#[tokio::test]
async fn deepseek_checkpoint_abort_closes_unknown_suffix_and_replays_next_run() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    const CONVERSATION_ID: &str = "conversation-deepseek-checkpoint-abort";
    const FIRST_ASSISTANT_ID: &str = "assistant-deepseek-checkpoint-abort";
    const SECOND_ASSISTANT_ID: &str = "assistant-deepseek-checkpoint-recovery";
    const FIRST_RUN_ID: &str = "run-deepseek-checkpoint-abort";
    const SECOND_RUN_ID: &str = "run-deepseek-checkpoint-recovery";
    const MODEL_ID: &str = "deepseek-checkpoint-abort-model";
    const PROVIDER_REVISION: &str = "provider-protocol-v1:deepseek-checkpoint-abort";
    const REASONING: &str = "Preserve this reasoning across the aborted grouped turn.";

    fn provider_tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap(),
            }
        })
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek checkpoint abort".to_string(),
            messages: vec![
                ChatMessageRecord {
                    id: FIRST_ASSISTANT_ID.to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 1,
                    status: Some("failed".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
                ChatMessageRecord {
                    id: SECOND_ASSISTANT_ID.to_string(),
                    role: "assistant".to_string(),
                    content: String::new(),
                    created_at: 2,
                    status: Some("pending".to_string()),
                    attachments: Vec::new(),
                    agent_run_json: None,
                    ui_state_json: None,
                },
            ],
            created_at: 1,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut profile = ProviderProfileConfig::deepseek_v4_default();
    profile.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::High,
    };
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        MODEL_ID,
        Some(PROVIDER_REVISION.to_string()),
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let replayed_request = Arc::new(Mutex::new(None::<Value>));
    let replayed_request_for_server = Arc::clone(&replayed_request);
    let server = tokio::spawn(async move {
        let (mut first_stream, _) = listener.accept().await.unwrap();
        let first_request = read_runtime_test_json_request(&mut first_stream).await;
        assert!(first_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message.get("reasoning_content").is_none()));
        write_runtime_test_json_response(
            &mut first_stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": REASONING,
                        "tool_calls": [
                            provider_tool_call(
                                "provider-approval-call",
                                "run_command",
                                json!({ "command": "echo should-require-approval" }),
                            ),
                            provider_tool_call(
                                "provider-unknown-call",
                                "unknown_private_tool",
                                json!({ "secret": "must-stay-private" }),
                            ),
                        ],
                    },
                    "finish_reason": "tool_calls",
                }]
            }),
        )
        .await;

        let (mut second_stream, _) = listener.accept().await.unwrap();
        let second_request = read_runtime_test_json_request(&mut second_stream).await;
        *replayed_request_for_server.lock().unwrap() = Some(second_request);
        write_runtime_test_json_response(
            &mut second_stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "recovered from a protocol-complete aborted turn",
                        "reasoning_content": "The previous grouped turn is closed.",
                    },
                    "finish_reason": "stop",
                }]
            }),
        )
        .await;
    });

    let mut first_input = conversation_context_input(vec![message(
        "user",
        "Request an approval call followed by an unknown call.",
    )]);
    first_input.api_url = format!("http://{address}/v1/chat/completions");
    first_input.api_token = "test-token".to_string();
    first_input.provider_profile_config = Some(profile.clone());
    first_input.provider_protocol_key = Some(protocol.clone());
    first_input.provider_configuration_revision = Some(PROVIDER_REVISION.to_string());
    first_input.model = MODEL_ID.to_string();
    first_input.stream = Some(false);
    first_input.assistant_message_id = Some(FIRST_ASSISTANT_ID.to_string());
    first_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek abort workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
        },
    });
    let snapshots = Arc::new(Mutex::new(Vec::<ConversationTraceSnapshot>::new()));
    let snapshots_for_observer = Arc::clone(&snapshots);
    let observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        snapshots_for_observer.lock().unwrap().push(snapshot);
        Ok(None)
    });
    let first_error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            first_input,
            Some(FIRST_RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault))
                    .with_trace_observer(observer),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(
        first_error.code(),
        Some("agent.checkpoint_private_tool_arguments")
    );
    let failed_trace = first_error
        .conversation_turn_trace()
        .expect("terminal abort must retain a complete trace")
        .clone();
    failed_trace.validate().unwrap();
    let trace_pairs = failed_trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => Some(("call", call_id.clone())),
            ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                Some(("result", call_id.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(trace_pairs.len(), 4);
    assert_eq!(trace_pairs[0].0, "call");
    assert_eq!(trace_pairs[1], ("result", trace_pairs[0].1.clone()));
    assert_eq!(trace_pairs[2].0, "call");
    assert_eq!(trace_pairs[3], ("result", trace_pairs[2].1.clone()));
    let persisted = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &protocol)
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].assistant_turn.provider_tool_calls().len(), 2);

    let final_snapshot = snapshots
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("terminal abort must publish the protocol-complete model context");
    let aborted_history = AgentChatMessage {
        message_id: Some(FIRST_ASSISTANT_ID.to_string()),
        role: "assistant".to_string(),
        content: String::new(),
        created_at: Some(2),
        conversation_turn_trace: Some(failed_trace),
        conversation_model_context_items: final_snapshot.model_context_items,
    };
    let mut recovery_input = conversation_context_input(vec![
        message(
            "user",
            "Request an approval call followed by an unknown call.",
        ),
        aborted_history,
        message("user", "Continue without repeating the skipped tools."),
    ]);
    recovery_input.api_url = format!("http://{address}/v1/chat/completions");
    recovery_input.api_token = "test-token".to_string();
    recovery_input.provider_profile_config = Some(profile);
    recovery_input.provider_protocol_key = Some(protocol);
    recovery_input.provider_configuration_revision = Some(PROVIDER_REVISION.to_string());
    recovery_input.model = MODEL_ID.to_string();
    recovery_input.stream = Some(false);
    recovery_input.assistant_message_id = Some(SECOND_ASSISTANT_ID.to_string());
    recovery_input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("DeepSeek recovery workspace".to_string()),
            root_path: Some(workspace.to_string_lossy().into_owned()),
        }),
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: Default::default(),
        },
    });
    let recovered = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            recovery_input,
            Some(SECOND_RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_storage(storage)
                    .with_provider_continuation_vault(vault),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(recovered.status, AgentRunStatus::Completed);
    assert_eq!(
        recovered.content,
        "recovered from a protocol-complete aborted turn"
    );
    let replayed = replayed_request.lock().unwrap().clone().unwrap();
    let messages = replayed["messages"].as_array().unwrap();
    let replayed_turn = messages
        .iter()
        .find(|message| message["reasoning_content"].as_str() == Some(REASONING))
        .expect("the exact aborted Provider turn must replay");
    let provider_ids = replayed_turn["tool_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        provider_ids,
        ["provider-approval-call", "provider-unknown-call"]
    );
    let result_ids = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        result_ids,
        ["provider-approval-call", "provider-unknown-call"]
    );
    assert!(messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .all(|message| message["content"]
            .as_str()
            .is_some_and(|content| content.contains("groupedTurnAborted"))));
}

#[tokio::test]
async fn deepseek_runtime_persists_grouped_turns_before_tool_side_effects() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentCommandPermission, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::{timeout, Duration};

    const CONVERSATION_ID: &str = "conversation-deepseek-runtime-e2e";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-deepseek-runtime-e2e";
    const RUN_ID: &str = "run-deepseek-runtime-e2e";
    const MODEL_ID: &str = "deepseek-v4-runtime-e2e";
    const FIRST_REASONING: &str = "第一轮 reasoning：\n逐字保留  alpha  ";
    const SECOND_REASONING: &str = "第二轮 reasoning：工具 1/2 都已完成。\n";

    fn provider_tool_call(id: &str, command: &str) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": "run_command",
                "arguments": serde_json::to_string(&json!({ "command": command })).unwrap()
            }
        })
    }

    fn matching_reasoning_turns<'a>(request: &'a Value, reasoning: &str) -> Vec<&'a Value> {
        request["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|message| {
                message["role"] == "assistant"
                    && message["reasoning_content"].as_str() == Some(reasoning)
            })
            .collect()
    }

    fn validate_replayed_reasoning(request_index: usize, request: &Value) -> Result<(), String> {
        let messages = request["messages"]
            .as_array()
            .ok_or_else(|| "messages must be an array".to_string())?;
        let all_reasoning = messages
            .iter()
            .filter_map(|message| message.get("reasoning_content"))
            .collect::<Vec<_>>();
        let first_turns = matching_reasoning_turns(request, FIRST_REASONING);
        let second_turns = matching_reasoning_turns(request, SECOND_REASONING);
        let tool_result_ids = messages
            .iter()
            .filter(|message| message["role"] == "tool")
            .filter_map(|message| message["tool_call_id"].as_str())
            .collect::<Vec<_>>();

        match request_index {
            0 if all_reasoning.is_empty() => Ok(()),
            1 if first_turns.len() == 1 && second_turns.is_empty() => {
                let provider_call_ids = first_turns[0]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
                    .collect::<Vec<_>>();
                if all_reasoning.len() == 1
                    && provider_call_ids == ["deepseek-provider-1a", "deepseek-provider-1b"]
                    && tool_result_ids == ["deepseek-provider-1a", "deepseek-provider-1b"]
                {
                    Ok(())
                } else {
                    Err("request 2 did not replay one grouped first turn".to_string())
                }
            }
            2 if first_turns.len() == 1 && second_turns.len() == 1 => {
                let first_provider_call_ids = first_turns[0]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
                    .collect::<Vec<_>>();
                let second_provider_call_ids = second_turns[0]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
                    .collect::<Vec<_>>();
                if all_reasoning.len() == 2
                    && first_provider_call_ids == ["deepseek-provider-1a", "deepseek-provider-1b"]
                    && second_provider_call_ids == ["deepseek-provider-2"]
                    && tool_result_ids
                        == [
                            "deepseek-provider-1a",
                            "deepseek-provider-1b",
                            "deepseek-provider-2",
                        ]
                {
                    Ok(())
                } else {
                    Err("request 3 did not replay both exact grouped turns".to_string())
                }
            }
            _ => Err(format!(
                "request {} omitted or duplicated byte-exact reasoning_content",
                request_index + 1
            )),
        }
    }

    async fn write_status_response(stream: &mut TcpStream, status: &str, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek Runtime E2E".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_MESSAGE_ID.to_string(),
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

    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut provider_profile = ProviderProfileConfig::deepseek_v4_default();
    provider_profile.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::Max,
    };
    let provider_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests.push(request.clone());
            if let Err(reason) = validate_replayed_reasoning(request_index, &request) {
                write_status_response(
                    &mut stream,
                    "400 Bad Request",
                    json!({ "error": { "message": reason } }),
                )
                .await;
                return (requests, Some(reason), true);
            }

            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": FIRST_REASONING,
                            "tool_calls": [
                                provider_tool_call("deepseek-provider-1a", "touch first-effect"),
                                provider_tool_call("deepseek-provider-1b", "touch second-effect")
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": SECOND_REASONING,
                            "tool_calls": [provider_tool_call(
                                "deepseek-provider-2",
                                "touch third-effect"
                            )]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                _ => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "three persisted tool effects completed",
                            "reasoning_content": "最终 reasoning 不应触发第四次请求。"
                        },
                        "finish_reason": "stop"
                    }]
                }),
            };
            write_status_response(&mut stream, "200 OK", response).await;
        }

        let no_extra_request = match timeout(Duration::from_millis(300), listener.accept()).await {
            Ok(Ok((mut stream, _))) => {
                requests.push(read_runtime_test_json_request(&mut stream).await);
                write_status_response(
                    &mut stream,
                    "500 Internal Server Error",
                    json!({ "error": { "message": "unexpected fourth request" } }),
                )
                .await;
                false
            }
            Ok(Err(error)) => panic!("fake provider accept failed: {error}"),
            Err(_) => true,
        };
        (requests, None, no_extra_request)
    });

    let side_effects = Arc::new(AtomicUsize::new(0));
    let persisted_before_effect = Arc::new(Mutex::new(Vec::<(String, usize)>::new()));
    let executor_vault = Arc::clone(&vault);
    let executor_protocol = provider_protocol.clone();
    let executor_side_effects = Arc::clone(&side_effects);
    let executor_checks = Arc::clone(&persisted_before_effect);
    let host_executor: AgentHostActionExecutor = Arc::new(move |action, _, cancellation| {
        if cancellation.is_cancelled() {
            return Err(AgentError::new("unexpected cancellation"));
        }
        let AgentProposedAction::Command { command } = action else {
            return Err(AgentError::new("expected a command side effect"));
        };
        let expected_persisted_turns = if command.command == "touch third-effect" {
            2
        } else {
            1
        };
        let persisted_turns = executor_vault
            .list_replayable_for_conversation(CONVERSATION_ID, &executor_protocol)
            .map_err(|error| AgentError::new(error.to_string()))?;
        executor_checks
            .lock()
            .unwrap()
            .push((command.command.clone(), persisted_turns.len()));
        if persisted_turns.len() != expected_persisted_turns {
            return Err(AgentError::new(
                "tool side effect reached the Host before its provider turn was persisted",
            ));
        }
        executor_side_effects.fetch_add(1, Ordering::SeqCst);
        Ok(AgentToolResult {
            exact_archive_file: None,
            call_id: command.id,
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "exitCode": 0,
                "stdout": "fake side effect committed",
                "stderr": ""
            })),
            error: None,
        })
    });

    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "deepseek-runtime-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: Some(provider_profile),
        provider_protocol_key: Some(provider_protocol.clone()),
        model: MODEL_ID.to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_024),
        temperature: Some(0.3),
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some(CONVERSATION_ID.to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("DeepSeek E2E workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command: AgentCommandPermission::AutoApprove,
                command_safety: AgentCommandSafetyPolicy::FullAccess,
                patch: Default::default(),
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some(ASSISTANT_MESSAGE_ID.to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message(
            "user",
            "Run three fake side effects across two DeepSeek tool turns, then finish.",
        )],
    };
    let mut missing_reasoning_input = input.clone();
    let runtime_result = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_host_actions(host_executor, Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault)),
            ),
        )
        .await;
    let output = runtime_result.unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(output.content, "three persisted tool effects completed");
    let (requests, provider_rejection, no_extra_request) = server.await.unwrap();

    assert!(provider_rejection.is_none(), "{provider_rejection:?}");
    assert_eq!(side_effects.load(Ordering::SeqCst), 3);
    assert_eq!(
        persisted_before_effect.lock().unwrap().as_slice(),
        [
            ("touch first-effect".to_string(), 1),
            ("touch second-effect".to_string(), 1),
            ("touch third-effect".to_string(), 2),
        ]
    );
    assert_eq!(requests.len(), 3);
    assert!(
        no_extra_request,
        "runtime issued an unexpected fourth request"
    );

    let persisted_turns = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &provider_protocol)
        .unwrap();
    assert_eq!(persisted_turns.len(), 2);
    assert_eq!(
        persisted_turns[0]
            .assistant_turn
            .provider_tool_calls()
            .len(),
        2,
        "one multi-call provider turn must have one continuation envelope"
    );
    assert_eq!(
        persisted_turns[1]
            .assistant_turn
            .provider_tool_calls()
            .len(),
        1
    );

    const MISSING_CONVERSATION_ID: &str = "conversation-deepseek-missing-reasoning";
    const MISSING_ASSISTANT_MESSAGE_ID: &str = "assistant-deepseek-missing-reasoning";
    storage
        .save_conversation(ChatConversationRecord {
            id: MISSING_CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek missing reasoning".to_string(),
            messages: vec![ChatMessageRecord {
                id: MISSING_ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 2,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 2,
            updated_at: 2,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let missing_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let missing_address = missing_listener.local_addr().unwrap();
    let missing_server = tokio::spawn(async move {
        let (mut stream, _) = missing_listener.accept().await.unwrap();
        let request = read_runtime_test_json_request(&mut stream).await;
        write_status_response(
            &mut stream,
            "200 OK",
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [provider_tool_call(
                            "deepseek-provider-missing-reasoning",
                            "touch must-not-run"
                        )]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        )
        .await;
        let no_retry = timeout(Duration::from_millis(300), missing_listener.accept())
            .await
            .is_err();
        (request, no_retry)
    });
    missing_reasoning_input.api_url = format!("http://{missing_address}/v1/chat/completions");
    missing_reasoning_input
        .context
        .as_mut()
        .unwrap()
        .conversation_id = Some(MISSING_CONVERSATION_ID.to_string());
    missing_reasoning_input.assistant_message_id = Some(MISSING_ASSISTANT_MESSAGE_ID.to_string());
    missing_reasoning_input.messages = vec![message(
        "user",
        "Call one tool, but reject the response if its reasoning is missing.",
    )];
    let forbidden_side_effects = Arc::new(AtomicUsize::new(0));
    let executor_forbidden_side_effects = Arc::clone(&forbidden_side_effects);
    let forbidden_executor: AgentHostActionExecutor = Arc::new(move |action, _, _| {
        executor_forbidden_side_effects.fetch_add(1, Ordering::SeqCst);
        let AgentProposedAction::Command { command } = action else {
            return Err(AgentError::new("expected command"));
        };
        Ok(AgentToolResult {
            exact_archive_file: None,
            call_id: command.id,
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({ "exitCode": 0 })),
            error: None,
        })
    });
    let missing_error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            missing_reasoning_input,
            Some("run-deepseek-missing-reasoning".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_host_actions(forbidden_executor, Arc::clone(&storage))
                    .with_provider_continuation_vault(Arc::clone(&vault)),
            ),
        )
        .await
        .unwrap_err();
    let (_, missing_no_retry) = missing_server.await.unwrap();
    assert_eq!(missing_error.code(), Some("provider_reasoning_required"));
    assert_eq!(forbidden_side_effects.load(Ordering::SeqCst), 0);
    assert!(
        missing_no_retry,
        "missing reasoning must not trigger a retry"
    );
    assert!(vault
        .list_replayable_for_conversation(MISSING_CONVERSATION_ID, &provider_protocol)
        .unwrap()
        .is_empty());
}
