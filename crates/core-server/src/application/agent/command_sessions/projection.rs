//! Pure projection and validation helpers for Host-owned command Sessions.

use super::CommandSessionOwner;
use mycopilot_core::command::{CommandSessionSnapshot as CoreSessionSnapshot, CommandSessionState};
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionModelRead, AGENT_COMMAND_SESSION_SCHEMA_VERSION,
};
use mycopilot_core::{
    AgentCommandExitStatus, AgentCommandSessionExecutionOutput, AgentCommandSessionSnapshot,
    AgentCommandSessionStatus,
};
use sha2::{Digest, Sha256};

pub(super) fn host_snapshot(
    owner: &CommandSessionOwner,
    snapshot: &CoreSessionSnapshot,
) -> AgentCommandSessionSnapshot {
    AgentCommandSessionSnapshot {
        schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
        session_id: snapshot.session_id.to_string(),
        conversation_id: owner.conversation_id.clone(),
        assistant_message_id: owner.assistant_message_id.clone(),
        origin_run_id: owner.origin_run_id.clone(),
        call_id: owner.call_id.clone(),
        project_id: owner.project_id.clone(),
        command: snapshot.projection.command.clone(),
        cwd: snapshot.projection.cwd.clone(),
        command_digest: command_digest(&snapshot.projection.command),
        status: protocol_status(&snapshot.state),
        started_at: snapshot.started_at,
        ended_at: snapshot.ended_at,
        exit_code: protocol_exit_code(&snapshot.state, snapshot.exit_code),
        latest_sequence: snapshot.latest_output_sequence,
        output_truncated: snapshot.output_truncated,
        outputs: Vec::new(),
        archive_ref: None,
    }
}

pub(super) fn protocol_status(state: &CommandSessionState) -> AgentCommandSessionStatus {
    match state {
        CommandSessionState::Starting => AgentCommandSessionStatus::Starting,
        CommandSessionState::Running => AgentCommandSessionStatus::Running,
        CommandSessionState::Exited { .. } => AgentCommandSessionStatus::Exited,
        CommandSessionState::Interrupted => AgentCommandSessionStatus::Interrupted,
        CommandSessionState::TimedOut => AgentCommandSessionStatus::TimedOut,
        CommandSessionState::Failed => AgentCommandSessionStatus::Failed,
    }
}

pub(super) fn protocol_exit_code(
    state: &CommandSessionState,
    exit_code: Option<i32>,
) -> Option<i32> {
    matches!(state, CommandSessionState::Exited { .. })
        .then_some(exit_code)
        .flatten()
}

pub(super) fn exit_event_status(state: &CommandSessionState) -> AgentCommandExitStatus {
    match state {
        CommandSessionState::Exited { .. } => AgentCommandExitStatus::Exited,
        CommandSessionState::TimedOut => AgentCommandExitStatus::TimedOut,
        CommandSessionState::Failed
        | CommandSessionState::Starting
        | CommandSessionState::Running => AgentCommandExitStatus::Failed,
        CommandSessionState::Interrupted => {
            unreachable!("interrupted Sessions use command_interrupted")
        }
    }
}

pub(super) fn model_execution_output_from_read(
    read: &AgentCommandSessionModelRead,
    history_open: Option<String>,
) -> AgentCommandSessionExecutionOutput {
    AgentCommandSessionExecutionOutput {
        session_id: read.receipt.session_id.clone(),
        status: read.receipt.status,
        output: read
            .chunks
            .iter()
            .map(|chunk| chunk.output.as_str())
            .collect(),
        exit_code: read.receipt.exit_code,
        requested_after_sequence: read.receipt.requested_after_sequence,
        first_output_sequence: read.receipt.first_output_sequence,
        last_output_sequence: read.receipt.last_output_sequence,
        latest_sequence: read.receipt.latest_sequence,
        truncated_before: read.receipt.truncated_before,
        output_truncated: read.receipt.output_truncated,
        outputs: read.outputs.clone(),
        history_open,
    }
}

pub(super) fn command_digest(command: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(command.as_bytes()))
}

pub(super) fn archive_sequence(session_id: &str) -> u64 {
    let digest = Sha256::digest(session_id.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    (u64::from_be_bytes(bytes) & ((1_u64 << 61) - 1)) | (1_u64 << 61)
}

pub(super) fn validate_owner(owner: &CommandSessionOwner) -> Result<(), String> {
    for (label, value) in [
        ("conversationId", owner.conversation_id.as_str()),
        ("assistantMessageId", owner.assistant_message_id.as_str()),
        ("originRunId", owner.origin_run_id.as_str()),
        ("callId", owner.call_id.as_str()),
    ] {
        if value.trim().is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
            return Err(format!("命令 Session 的 {label} 无效。"));
        }
    }
    Ok(())
}

pub(super) fn validate_conversation_id(conversation_id: &str) -> Result<(), String> {
    if conversation_id.trim().is_empty()
        || conversation_id.len() > 1_024
        || conversation_id.chars().any(char::is_control)
    {
        return Err("conversationId 无效。".to_string());
    }
    Ok(())
}
