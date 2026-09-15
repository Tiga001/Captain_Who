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

fn apply_patch_args(request: Value) -> Value {
    json!({ "request": request })
}

fn staged_mutation_receipt(transaction_id: &str, index: u64) -> String {
    serde_json::to_string(&crate::file_change::FileChangeMutationReceipt {
        schema_version: 1,
        transaction_id: transaction_id.to_string(),
        index,
        draft_revision: index + 1,
        next_index: index + 1,
        byte_count: 6,
        line_count: 1,
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
include!("tests/continuity.rs");
include!("tests/manual_boundary.rs");
include!("tests/recovery.rs");
include!("tests/human_interaction.rs");
include!("tests/async_question_inheritance.rs");
include!("tests/workspace_binding.rs");
