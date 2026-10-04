use super::*;
use crate::context::{
    ContextCompactionGeneration, ContextCompactionSummaryDraft, ContextContinuitySnapshot,
};
use crate::storage::migrations;
use crate::{
    AgentApprovalStatus, ConversationTraceToolResultStatus, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, WorldStateDiff, WorldStateLifetime,
    WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};

fn build_assistant_reply_fork_plan(
    connection: &Connection,
    request_id: &str,
    source_conversation_id: &str,
    assistant_message_id: &str,
    created_at: i64,
) -> Result<ConversationForkPlan, ConversationForkError> {
    build_fork_plan_at_point(
        connection,
        request_id,
        source_conversation_id,
        &ConversationForkPoint::AssistantReply {
            assistant_message_id: assistant_message_id.to_string(),
        },
        created_at,
    )
}

fn staged_tool_exchange(
    sequence: u64,
    call_id: &str,
    tool: &str,
    args: Value,
) -> Vec<ConversationTurnTraceItem> {
    vec![
        ConversationTurnTraceItem::ToolCall {
            sequence,
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: tool.to_string(),
            },
            operation: args,
            approval_status: AgentApprovalStatus::NotRequired,
            truncated: false,
        },
        ConversationTurnTraceItem::ToolResult {
            sequence: sequence + 1,
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            status: ConversationTraceToolResultStatus::Succeeded,
            success: true,
            observation: json!({"accepted": true}),
            approval_status: AgentApprovalStatus::NotRequired,
            error: None,
            truncated: false,
            archive: Default::default(),
        },
    ]
}

// Build both durable projections through the runtime recorder. In particular, apply_patch
// arguments retain their bodies in model context but never in the audit/search trace.
fn commit_current_fork_fixture_trace(
    connection: &mut Connection,
    raw_trace: &ConversationTurnTrace,
    started_at: i64,
    completed_at: i64,
) {
    use crate::conversation_trace::ConversationTraceRecorder;
    use crate::llm::{LlmMessage, LlmToolCall};

    let mut recorder = ConversationTraceRecorder::default();
    let mut calls = HashMap::new();
    for item in &raw_trace.items {
        let sequence = match item {
            ConversationTurnTraceItem::AssistantNarration { content, .. } => {
                recorder.record_narration(content).unwrap().unwrap()
            }
            ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                approval_status,
                ..
            } => {
                let call = crate::AgentToolCall {
                    id: call_id.clone(),
                    tool: tool.clone(),
                    args: operation.clone(),
                    approval_status: *approval_status,
                    reason: None,
                };
                let sequence = recorder.record_tool_call(&call).unwrap();
                recorder
                    .record_model_message(
                        sequence,
                        0,
                        &LlmMessage::assistant(
                            "",
                            vec![LlmToolCall {
                                id: call.id.clone(),
                                name: call.tool.clone(),
                                args: call.args.clone(),
                            }],
                        ),
                    )
                    .unwrap();
                calls.insert(call_id, call);
                sequence
            }
            ConversationTurnTraceItem::ToolResult {
                call_id,
                tool,
                success,
                observation,
                approval_status,
                error,
                ..
            } => {
                let call = calls.get_mut(call_id).unwrap();
                call.approval_status = *approval_status;
                let result = AgentToolResult {
                    exact_archive_file: None,
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    ok: *success,
                    result: Some(observation.clone()),
                    error: error.clone(),
                };
                let sequence = recorder.record_tool_result(call, &result).unwrap();
                recorder
                    .record_model_message(
                        sequence,
                        0,
                        &LlmMessage::tool_result(call_id, observation.to_string(), !success),
                    )
                    .unwrap();
                sequence
            }
            _ => panic!("fixture expects narration and tool exchanges only"),
        };
        assert_eq!(sequence, item.sequence());
    }
    let snapshot = recorder.snapshot();
    let trace = recorder.finish(
        &raw_trace.run_id,
        &raw_trace.conversation_id,
        &raw_trace.assistant_message_id,
        raw_trace.terminal_status,
        raw_trace.terminal_error.as_deref(),
    );
    trace
        .validate_complete_model_context(&snapshot.model_context_items)
        .unwrap();
    conversation_trace_repository::replace_trace(connection, &trace, started_at, completed_at)
        .unwrap();
    conversation_model_context_repository::commit_items_in_connection(
        connection,
        &trace.conversation_id,
        &trace.assistant_message_id,
        &snapshot.model_context_items,
    )
    .unwrap();
}

fn apply_patch_args(request: Value) -> Value {
    json!({ "request": request })
}

fn staged_mutation_receipt(transaction_id: &str, index: u64) -> String {
    staged_mutation_receipt_for_content(transaction_id, index, "after\n")
}

fn staged_mutation_receipt_for_content(transaction_id: &str, index: u64, content: &str) -> String {
    serde_json::to_string(&crate::file_change::FileChangeMutationReceipt {
        schema_version: 1,
        transaction_id: transaction_id.to_string(),
        index,
        draft_revision: index + 1,
        next_index: index + 1,
        byte_count: content.len() as u64,
        line_count: content.lines().count() as u64,
        allowed_next_actions: vec![
            crate::file_change::FileChangeStagedAction::Append,
            crate::file_change::FileChangeStagedAction::Edit,
            crate::file_change::FileChangeStagedAction::Commit,
            crate::file_change::FileChangeStagedAction::Status,
            crate::file_change::FileChangeStagedAction::Abort,
        ],
        requires_commit_before_response: true,
    })
    .unwrap()
}

fn file_change_observation(
    conversation_id: &str,
    run_id: &str,
    begin_call_id: &str,
    canonical_target: &str,
    revision: &str,
) -> (String, String) {
    use crate::file_change::{
        FileObservationCheckpoint, FileObservationDirectoryIdentity, FileObservationIdentity,
        FileObservationState, FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
    };
    let observation_id = format!("fobs_{}", "1".repeat(32));
    let metadata = std::fs::metadata(std::env::temp_dir()).unwrap();
    let checkpoint = FileObservationCheckpoint {
        schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
        observation_id: observation_id.clone(),
        source_tool_call_id: begin_call_id.to_string(),
        conversation_id: conversation_id.to_string(),
        run_id: run_id.to_string(),
        canonical_target: canonical_target.to_string(),
        state: FileObservationState::Existing {
            revision: revision.to_string(),
            identity: FileObservationIdentity::from_metadata(&metadata),
        },
        parent_directory_identity: FileObservationDirectoryIdentity::read(
            &std::fs::canonicalize(std::env::temp_dir()).unwrap(),
        )
        .unwrap(),
        created_at_ms: 1,
        expires_at_ms: 1 + FILE_OBSERVATION_TTL_MS,
    };
    (observation_id, serde_json::to_string(&checkpoint).unwrap())
}

fn apply_patch_observation(
    conversation_id: &str,
    run_id: &str,
    read_call_id: &str,
    canonical_target: &std::path::Path,
    revision: &str,
) -> (String, String) {
    use crate::file_change::{
        FileObservationCheckpoint, FileObservationDirectoryIdentity, FileObservationIdentity,
        FileObservationState, FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
    };
    let observation_id = format!("fobs_{}", "2".repeat(32));
    let target_metadata = std::fs::metadata(canonical_target).unwrap();
    let checkpoint = FileObservationCheckpoint {
        schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
        observation_id: observation_id.clone(),
        source_tool_call_id: read_call_id.to_string(),
        conversation_id: conversation_id.to_string(),
        run_id: run_id.to_string(),
        canonical_target: canonical_target.to_string_lossy().into_owned(),
        state: FileObservationState::Existing {
            revision: revision.to_string(),
            identity: FileObservationIdentity::from_metadata(&target_metadata),
        },
        parent_directory_identity: FileObservationDirectoryIdentity::read(
            canonical_target.parent().unwrap(),
        )
        .unwrap(),
        created_at_ms: 1,
        expires_at_ms: 1 + FILE_OBSERVATION_TTL_MS,
    };
    (observation_id, serde_json::to_string(&checkpoint).unwrap())
}

include!("tests/planning.rs");
include!("tests/commit.rs");
include!("tests/staged_observation.rs");
include!("tests/continuity.rs");
include!("tests/manual_boundary.rs");
include!("tests/recovery.rs");
include!("tests/human_interaction.rs");
include!("tests/async_question_inheritance.rs");
include!("tests/workspace_binding.rs");

include!("tests/planning_reuse.rs");
include!("tests/trace_journal.rs");
include!("tests/file_change_binding.rs");
