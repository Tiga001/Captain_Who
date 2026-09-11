use super::*;

use crate::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AgentCommandSessionCreateOutcome, AgentCommandSessionModelRead,
    AgentCommandSessionModelReadRequest, AgentCommandSessionOutputAppend,
    AgentCommandSessionRecord, AgentCommandSessionTerminalUpdate,
    AgentCommandSessionTransitionOutcome,
};
use crate::storage::conversation_trace_repository::CommandSessionLifecycleAppendOutcome;
use crate::{
    AgentCommandSessionStatus, AgentCommandSessionTranscript, ConversationCommandSessionLifecycle,
    ConversationCommandSessionLifecyclePhase, ConversationHistoryArchiveTraceMetadata,
    ConversationTurnTrace,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommandSessionLifecycleAppendOutcome {
    Appended { sequence: u64 },
    Idempotent { sequence: u64 },
    SessionNotFound,
}

impl StorageService {
    /// Resolves the complete process archive advertised by a durable `run_command` result.
    ///
    /// Command Sessions archive the authoritative stdout/stderr spool before a ToolResult is
    /// exposed. Approval settlement and Runtime resume must reuse that same immutable archive;
    /// creating a second trace-item archive would give the same ToolResult two different durable
    /// identities and break the append-only Trace prefix contract.
    ///
    /// A `run_command` result without `historyOpen` is not Session-backed and returns `None`.
    /// Once a result claims such a route, every ownership check is fail-closed.
    pub fn resolve_authoritative_command_archive_metadata(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        result: &AgentToolResult,
    ) -> Result<Option<ConversationHistoryArchiveTraceMetadata>, String> {
        if result.tool != "run_command" {
            return Ok(None);
        }
        let Some(result_object) = result
            .result
            .as_ref()
            .and_then(serde_json::Value::as_object)
        else {
            return Ok(None);
        };
        if result_object.contains_key("historyOpenInvalid") {
            return Err(
                "run_command durable historyOpen claim failed validation; refusing fallback archival"
                    .to_string(),
            );
        }
        let Some(history_open) = result_object.get("historyOpen") else {
            return Ok(None);
        };
        let open = history_open.as_str().ok_or_else(|| {
            "run_command historyOpen must be an opaque string capability".to_string()
        })?;
        let crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            archive_ref,
            start_char: 0,
        } = crate::storage::conversation_history_open::decode_history_open(open)
            .map_err(|error| format!("run_command historyOpen is invalid: {error}"))?
        else {
            return Err(
                "run_command historyOpen must reference the beginning of an Exact Archive"
                    .to_string(),
            );
        };

        let connection = self.state.connection()?;
        let archive = conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            &archive_ref,
        )
        .map_err(storage_error)?
        .ok_or_else(|| "run_command historyOpen archive does not exist".to_string())?;
        let session_id = archive
            .call_id
            .strip_prefix("command-session:")
            .ok_or_else(|| {
                "run_command historyOpen does not reference a Command Session archive".to_string()
            })?;
        let session =
            agent_command_session_repository::get_session(&connection, conversation_id, session_id)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    "run_command historyOpen Command Session does not exist".to_string()
                })?;
        if archive.assistant_message_id != assistant_message_id
            || archive.tool != "run_command"
            || session.snapshot.assistant_message_id != assistant_message_id
            || session.snapshot.call_id != result.call_id
            || session.snapshot.archive_ref.as_deref() != Some(archive.archive_ref.as_str())
            || !session.snapshot.status.is_terminal()
        {
            return Err(
                "run_command historyOpen does not belong to the settled ToolCall".to_string(),
            );
        }

        Ok(Some(ConversationHistoryArchiveTraceMetadata {
            archive_ref: Some(archive.archive_ref),
            content_hash: Some(archive.content_hash),
            archived_bytes: Some(archive.total_bytes),
            archived_completely: Some(archive.archived_completely),
            truncated_at_source: archive.truncated_at_source,
            model_projection_truncated: archive.model_projection_truncated,
            history_projection_truncated: false,
            archive_projection_truncated: archive.archive_projection_truncated,
        }))
    }

    pub fn create_agent_command_session(
        &self,
        input: &AgentCommandSessionCreate,
    ) -> Result<AgentCommandSessionCreateOutcome, String> {
        let mut connection = self.state.connection()?;
        agent_command_session_repository::create_session(&mut connection, input)
            .map_err(storage_error)
    }

    pub fn mark_agent_command_session_running(
        &self,
        conversation_id: &str,
        session_id: &str,
        updated_at: i64,
    ) -> Result<AgentCommandSessionTransitionOutcome, String> {
        let mut connection = self.state.connection()?;
        agent_command_session_repository::mark_running(
            &mut connection,
            conversation_id,
            session_id,
            updated_at,
        )
        .map_err(storage_error)
    }

    /// Atomically crosses the durable running boundary and records its sidecar audit event.
    pub fn mark_agent_command_session_running_with_lifecycle(
        &self,
        conversation_id: &str,
        session_id: &str,
        updated_at: i64,
    ) -> Result<AgentCommandSessionTransitionOutcome, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let outcome = agent_command_session_repository::mark_running_in_connection(
            &transaction,
            conversation_id,
            session_id,
            updated_at,
        )
        .map_err(storage_error)?;
        if matches!(
            outcome,
            AgentCommandSessionTransitionOutcome::Updated
                | AgentCommandSessionTransitionOutcome::Idempotent
        ) {
            let record = agent_command_session_repository::get_session(
                &transaction,
                conversation_id,
                session_id,
            )
            .map_err(storage_error)?
            .ok_or_else(|| "命令 Session 在 running 转换期间消失。".to_string())?;
            conversation_trace_repository::append_command_session_lifecycle_in_connection(
                &transaction,
                &record.snapshot.assistant_message_id,
                conversation_id,
                started_lifecycle(&record)?,
                false,
                updated_at,
            )
            .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Appends the deterministic `started` event to the lifecycle sidecar journal.
    ///
    /// The method never accepts a caller-owned Trace snapshot, so multiple Session workers for one
    /// assistant message cannot overwrite each other's lifecycle items. A terminal source Trace is
    /// materialized immediately; an active or not-yet-created Trace consumes the sidecar later.
    pub fn append_agent_command_session_started_lifecycle(
        &self,
        conversation_id: &str,
        session_id: &str,
        committed_at: i64,
    ) -> Result<AgentCommandSessionLifecycleAppendOutcome, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let Some(record) = agent_command_session_repository::get_session(
            &transaction,
            conversation_id,
            session_id,
        )
        .map_err(storage_error)?
        else {
            transaction.commit().map_err(storage_error)?;
            return Ok(AgentCommandSessionLifecycleAppendOutcome::SessionNotFound);
        };
        let lifecycle = started_lifecycle(&record)?;
        let outcome =
            conversation_trace_repository::append_command_session_lifecycle_in_connection(
                &transaction,
                &record.snapshot.assistant_message_id,
                conversation_id,
                lifecycle,
                true,
                committed_at,
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(map_lifecycle_outcome(outcome))
    }

    pub fn append_agent_command_session_output(
        &self,
        input: &AgentCommandSessionOutputAppend<'_>,
    ) -> Result<AgentCommandSessionTransitionOutcome, String> {
        let mut connection = self.state.connection()?;
        agent_command_session_repository::append_output(&mut connection, input)
            .map_err(storage_error)
    }

    /// Commits only the operational Session state.
    ///
    /// Runtime settlement should normally use
    /// [`Self::settle_agent_command_session_with_trace`] so the terminal audit event and Session
    /// CAS become visible atomically. This narrower method exists for startup repair and tests.
    pub fn settle_agent_command_session(
        &self,
        input: &AgentCommandSessionTerminalUpdate<'_>,
    ) -> Result<AgentCommandSessionTransitionOutcome, String> {
        let mut connection = self.state.connection()?;
        agent_command_session_repository::commit_terminal(&mut connection, input)
            .map_err(storage_error)
    }

    /// Atomically appends an audit-only terminal item to the existing turn Trace and settles the
    /// operational Session row.
    ///
    /// Exact History must be archived before this boundary. Publishing terminal events and
    /// releasing the File Effect lease happen only after this method succeeds.
    pub fn settle_agent_command_session_with_trace(
        &self,
        input: &AgentCommandSessionTerminalUpdate<'_>,
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
    ) -> Result<AgentCommandSessionTransitionOutcome, String> {
        validate_terminal_trace_argument(input, trace)?;
        let _ = trace_created_at;
        self.settle_agent_command_session_with_lifecycle(input)
    }

    /// Settles a handed-off Session and appends its started/terminal audit events atomically.
    ///
    /// Exact History must already exist when `archive_ref` is present. Archive metadata is loaded
    /// from the authoritative archive row instead of trusting a caller-built Trace projection.
    pub fn settle_agent_command_session_with_lifecycle(
        &self,
        input: &AgentCommandSessionTerminalUpdate<'_>,
    ) -> Result<AgentCommandSessionTransitionOutcome, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let Some(record) = agent_command_session_repository::get_session(
            &transaction,
            input.conversation_id,
            input.session_id,
        )
        .map_err(storage_error)?
        else {
            transaction.commit().map_err(storage_error)?;
            return Ok(AgentCommandSessionTransitionOutcome::NotFound);
        };
        let outcome =
            agent_command_session_repository::commit_terminal_in_connection(&transaction, input)
                .map_err(storage_error)?;
        if matches!(
            outcome,
            AgentCommandSessionTransitionOutcome::Updated
                | AgentCommandSessionTransitionOutcome::Idempotent
        ) {
            append_required_lifecycle(
                &transaction,
                &record,
                started_lifecycle(&record)?,
                true,
                input.committed_at,
            )?;
            append_required_lifecycle(
                &transaction,
                &record,
                terminal_lifecycle(&transaction, &record, input)?,
                true,
                input.committed_at,
            )?;
            agent_command_session_repository::prune_terminal_sessions_in_connection(
                &transaction,
                input.conversation_id,
            )
            .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    pub fn load_agent_command_session(
        &self,
        conversation_id: &str,
        session_id: &str,
    ) -> Result<Option<AgentCommandSessionRecord>, String> {
        let connection = self.state.connection()?;
        agent_command_session_repository::get_session(&connection, conversation_id, session_id)
            .map_err(storage_error)
    }

    pub fn list_agent_command_sessions(
        &self,
        conversation_id: &str,
        terminal_limit: usize,
    ) -> Result<Vec<AgentCommandSessionRecord>, String> {
        let connection = self.state.connection()?;
        agent_command_session_repository::list_sessions_for_conversation(
            &connection,
            conversation_id,
            terminal_limit,
        )
        .map_err(storage_error)
    }

    pub fn load_agent_command_session_transcript(
        &self,
        conversation_id: &str,
        session_id: &str,
        after_sequence: u64,
        max_bytes: usize,
    ) -> Result<Option<AgentCommandSessionTranscript>, String> {
        let connection = self.state.connection()?;
        agent_command_session_repository::read_transcript(
            &connection,
            conversation_id,
            session_id,
            after_sequence,
            max_bytes,
        )
        .map_err(storage_error)
    }

    /// Reads the durable Session projection and transcript from one SQLite snapshot. This avoids
    /// exposing a Session sequence from one commit together with transcript chunks from another
    /// while the asynchronous output writer is active.
    pub fn load_agent_command_session_with_transcript(
        &self,
        conversation_id: &str,
        session_id: &str,
        after_sequence: u64,
        max_bytes: usize,
    ) -> Result<Option<(AgentCommandSessionRecord, AgentCommandSessionTranscript)>, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let Some(record) = agent_command_session_repository::get_session(
            &transaction,
            conversation_id,
            session_id,
        )
        .map_err(storage_error)?
        else {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        };
        let transcript = agent_command_session_repository::read_transcript(
            &transaction,
            conversation_id,
            session_id,
            after_sequence,
            max_bytes,
        )
        .map_err(storage_error)?
        .ok_or_else(|| "命令 Session 在读取 transcript 期间消失。".to_string())?;
        transaction.commit().map_err(storage_error)?;
        Ok(Some((record, transcript)))
    }

    /// Allocates or replays an idempotent model transcript cut for one runtime ToolCall.
    ///
    /// The repository commits the read receipt and cursor movement atomically. A crash after this
    /// method returns can therefore replay the same sequence range by run/call identity instead of
    /// silently consuming output before the ToolResult reaches durable model context.
    pub fn read_or_create_agent_command_session_model_read(
        &self,
        input: &AgentCommandSessionModelReadRequest<'_>,
    ) -> Result<Option<AgentCommandSessionModelRead>, String> {
        let mut connection = self.state.connection()?;
        agent_command_session_repository::read_or_create_model_read(&mut connection, input)
            .map_err(storage_error)
    }

    /// Replays an existing ToolCall receipt without waiting, interrupting, or advancing output.
    pub fn load_agent_command_session_model_read(
        &self,
        input: &AgentCommandSessionModelReadRequest<'_>,
    ) -> Result<Option<AgentCommandSessionModelRead>, String> {
        let connection = self.state.connection()?;
        agent_command_session_repository::load_model_read(&connection, input).map_err(storage_error)
    }

    /// Must run during Host startup before orphaned Agent turn traces are retired.
    ///
    /// The returned rows are every still-unresolved `outcome_unknown` Session, not only rows
    /// converted by this invocation. Callers must rebuild their in-memory file-effect deletion
    /// fences from this complete authoritative projection on every process start.
    pub fn reconcile_agent_command_sessions_on_startup(
        &self,
        reconciled_at: i64,
    ) -> Result<Vec<AgentCommandSessionRecord>, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let active =
            agent_command_session_repository::list_active_sessions_in_connection(&transaction)
                .map_err(storage_error)?;
        let mut reconciled = Vec::with_capacity(active.len());
        for record in active {
            let Some(updated) =
                agent_command_session_repository::reconcile_active_session_in_connection(
                    &transaction,
                    &record.snapshot.conversation_id,
                    &record.snapshot.session_id,
                    reconciled_at,
                )
                .map_err(storage_error)?
            else {
                continue;
            };
            let lifecycle = ConversationCommandSessionLifecycle {
                phase: ConversationCommandSessionLifecyclePhase::Terminal,
                session_id: updated.snapshot.session_id.clone(),
                call_id: updated.snapshot.call_id.clone(),
                status: AgentCommandSessionStatus::OutcomeUnknown,
                exit_code: None,
                latest_sequence: updated.snapshot.latest_sequence,
                output_truncated: updated.snapshot.output_truncated,
                archive: ConversationHistoryArchiveTraceMetadata::default(),
                created_at: updated
                    .snapshot
                    .ended_at
                    .and_then(|value| i64::try_from(value).ok())
                    .unwrap_or(updated.updated_at),
            };
            match conversation_trace_repository::append_command_session_lifecycle_in_connection(
                &transaction,
                &updated.snapshot.assistant_message_id,
                &updated.snapshot.conversation_id,
                lifecycle,
                false,
                reconciled_at,
            )
            .map_err(storage_error)?
            {
                CommandSessionLifecycleAppendOutcome::Appended { .. }
                | CommandSessionLifecycleAppendOutcome::Idempotent { .. } => {}
            }
            reconciled.push(updated);
        }
        let conversations = reconciled
            .iter()
            .map(|record| record.snapshot.conversation_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for conversation_id in conversations {
            agent_command_session_repository::prune_terminal_sessions_in_connection(
                &transaction,
                conversation_id,
            )
            .map_err(storage_error)?;
        }
        let unresolved =
            agent_command_session_repository::list_outcome_unknown_sessions_in_connection(
                &transaction,
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(unresolved)
    }
}

fn started_lifecycle(
    record: &AgentCommandSessionRecord,
) -> Result<ConversationCommandSessionLifecycle, String> {
    Ok(ConversationCommandSessionLifecycle {
        phase: ConversationCommandSessionLifecyclePhase::Started,
        session_id: record.snapshot.session_id.clone(),
        call_id: record.snapshot.call_id.clone(),
        status: AgentCommandSessionStatus::Running,
        exit_code: None,
        latest_sequence: 0,
        output_truncated: false,
        archive: ConversationHistoryArchiveTraceMetadata::default(),
        created_at: i64::try_from(record.snapshot.started_at)
            .map_err(|_| "命令 Session 启动时间超出 Trace 范围。".to_string())?,
    })
}

fn terminal_lifecycle(
    connection: &rusqlite::Connection,
    record: &AgentCommandSessionRecord,
    input: &AgentCommandSessionTerminalUpdate<'_>,
) -> Result<ConversationCommandSessionLifecycle, String> {
    let archive = match input.archive_ref {
        Some(archive_ref) => {
            let descriptor = conversation_history_archive_repository::find_archive_by_ref(
                connection,
                input.conversation_id,
                archive_ref,
            )
            .map_err(storage_error)?
            .ok_or_else(|| "命令 Session 的 Exact History Archive 不存在。".to_string())?;
            if descriptor.assistant_message_id != record.snapshot.assistant_message_id {
                return Err(
                    "命令 Session 的 Exact History Archive 不属于来源 assistant message。"
                        .to_string(),
                );
            }
            ConversationHistoryArchiveTraceMetadata {
                archive_ref: Some(descriptor.archive_ref),
                content_hash: Some(descriptor.content_hash),
                archived_bytes: Some(descriptor.total_bytes),
                archived_completely: Some(descriptor.archived_completely),
                truncated_at_source: descriptor.truncated_at_source,
                model_projection_truncated: descriptor.model_projection_truncated,
                history_projection_truncated: false,
                archive_projection_truncated: descriptor.archive_projection_truncated,
            }
        }
        None => ConversationHistoryArchiveTraceMetadata::default(),
    };
    Ok(ConversationCommandSessionLifecycle {
        phase: ConversationCommandSessionLifecyclePhase::Terminal,
        session_id: record.snapshot.session_id.clone(),
        call_id: record.snapshot.call_id.clone(),
        status: input.status,
        exit_code: input.exit_code,
        latest_sequence: input.latest_sequence,
        output_truncated: input.transcript_truncated || input.output_capture_truncated,
        archive,
        created_at: i64::try_from(input.ended_at)
            .map_err(|_| "命令 Session 结束时间超出 Trace 范围。".to_string())?,
    })
}

fn append_required_lifecycle(
    connection: &rusqlite::Connection,
    record: &AgentCommandSessionRecord,
    lifecycle: ConversationCommandSessionLifecycle,
    require_terminal_trace: bool,
    committed_at: i64,
) -> Result<(), String> {
    match conversation_trace_repository::append_command_session_lifecycle_in_connection(
        connection,
        &record.snapshot.assistant_message_id,
        &record.snapshot.conversation_id,
        lifecycle,
        require_terminal_trace,
        committed_at,
    )
    .map_err(storage_error)?
    {
        CommandSessionLifecycleAppendOutcome::Appended { .. }
        | CommandSessionLifecycleAppendOutcome::Idempotent { .. } => Ok(()),
    }
}

fn map_lifecycle_outcome(
    outcome: CommandSessionLifecycleAppendOutcome,
) -> AgentCommandSessionLifecycleAppendOutcome {
    match outcome {
        CommandSessionLifecycleAppendOutcome::Appended { sequence } => {
            AgentCommandSessionLifecycleAppendOutcome::Appended { sequence }
        }
        CommandSessionLifecycleAppendOutcome::Idempotent { sequence } => {
            AgentCommandSessionLifecycleAppendOutcome::Idempotent { sequence }
        }
    }
}

fn validate_terminal_trace_argument(
    input: &AgentCommandSessionTerminalUpdate<'_>,
    trace: &ConversationTurnTrace,
) -> Result<(), String> {
    if trace.conversation_id != input.conversation_id {
        return Err("命令 Session 终态 Trace 属于其他对话。".to_string());
    }
    let matching = trace.items.iter().filter(|item| {
        matches!(
            item,
            crate::ConversationTurnTraceItem::CommandSessionLifecycle {
                phase: ConversationCommandSessionLifecyclePhase::Terminal,
                session_id,
                status,
                exit_code,
                latest_sequence,
                archive,
                ..
            } if session_id == input.session_id
                && *status == input.status
                && *exit_code == input.exit_code
                && *latest_sequence == input.latest_sequence
                && archive.archive_ref.as_deref() == input.archive_ref
        )
    });
    if matching.count() != 1 {
        return Err("命令 Session 终态 Trace 参数与终态更新不一致。".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandAuthorizationSource;
    use crate::storage::agent_command_session_repository::{
        AgentCommandSessionCreate, AGENT_COMMAND_SESSION_SCHEMA_VERSION,
    };
    use crate::{
        AgentApprovalStatus, AgentCommandSessionSnapshot, ConversationHistoryArchiveTraceMetadata,
        ConversationTraceToolResultStatus, ConversationTurnTraceItem,
        ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use tempfile::tempdir;

    const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    fn open_pair() -> (tempfile::TempDir, Arc<StorageService>, Arc<StorageService>) {
        let directory = tempdir().unwrap();
        let database = directory.path().join("sessions.sqlite");
        let first = Arc::new(StorageService::open(&database).unwrap());
        let second = Arc::new(StorageService::open(&database).unwrap());
        (directory, first, second)
    }

    fn seed_conversation_and_trace(service: &StorageService, terminal: bool) {
        {
            let connection = service.state.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO projects (
                         id, name, created_at, pinned_at, updated_at
                     ) VALUES ('project-1', 'project-1', 1, NULL, 1)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO conversations (
                         id, project_id, title, created_at, updated_at
                     ) VALUES ('conversation-1', 'project-1', 'session test', 1, 1)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO messages (
                         id, conversation_id, role, content, status, created_at, position
                     ) VALUES (
                         'assistant-1', 'conversation-1', 'assistant', '', 'sent', 2, 0
                     )",
                    [],
                )
                .unwrap();
        }
        service
            .replace_conversation_turn_trace(
                &command_trace(terminal),
                2,
                if terminal { 20 } else { 10 },
            )
            .unwrap();
    }

    fn command_trace(terminal: bool) -> ConversationTurnTrace {
        let mut items = Vec::new();
        for (index, call_id) in ["call-1", "call-2"].into_iter().enumerate() {
            let sequence = u64::try_from(index * 2).unwrap();
            items.push(ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                provenance: crate::AgentToolIdentity::Builtin {
                    tool_name: "run_command".to_string(),
                },
                operation: json!({"command": format!("long-command-{index}")}),
                approval_status: AgentApprovalStatus::Approved,
                truncated: false,
            });
            items.push(ConversationTurnTraceItem::ToolResult {
                sequence: sequence + 1,
                call_id: call_id.to_string(),
                tool: "run_command".to_string(),
                status: ConversationTraceToolResultStatus::Succeeded,
                success: true,
                observation: json!({"status": "running"}),
                approval_status: AgentApprovalStatus::Approved,
                error: None,
                truncated: false,
                archive: ConversationHistoryArchiveTraceMetadata::default(),
            });
        }
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            terminal_status: if terminal {
                ConversationTurnTraceTerminalStatus::Completed
            } else {
                ConversationTurnTraceTerminalStatus::InProgress
            },
            terminal_error: None,
            truncated: false,
            items,
        }
    }

    fn create_starting_session(
        service: &StorageService,
        session_id: &str,
        call_id: &str,
        started_at: u64,
    ) {
        service
            .create_agent_command_session(&AgentCommandSessionCreate {
                snapshot: AgentCommandSessionSnapshot {
                    schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                    session_id: session_id.to_string(),
                    conversation_id: "conversation-1".to_string(),
                    assistant_message_id: "assistant-1".to_string(),
                    origin_run_id: "run-1".to_string(),
                    call_id: call_id.to_string(),
                    project_id: Some("project-1".to_string()),
                    command: "long-command".to_string(),
                    cwd: "/tmp/project".to_string(),
                    command_digest: DIGEST.to_string(),
                    status: AgentCommandSessionStatus::Starting,
                    started_at,
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
                created_at: i64::try_from(started_at).unwrap(),
            })
            .unwrap();
    }

    fn create_session(service: &StorageService, session_id: &str, call_id: &str, started_at: u64) {
        create_starting_session(service, session_id, call_id, started_at);
        service
            .mark_agent_command_session_running(
                "conversation-1",
                session_id,
                i64::try_from(started_at + 1).unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn running_handoff_and_started_lifecycle_are_one_atomic_boundary() {
        let (_directory, service, _second) = open_pair();
        seed_conversation_and_trace(&service, false);
        let session_id = "cmd_00000000000000000000000000000000";
        create_starting_session(&service, session_id, "call-1", 30);

        assert_eq!(
            service
                .mark_agent_command_session_running_with_lifecycle(
                    "conversation-1",
                    session_id,
                    31,
                )
                .unwrap(),
            AgentCommandSessionTransitionOutcome::Updated
        );
        assert_eq!(
            service
                .mark_agent_command_session_running_with_lifecycle(
                    "conversation-1",
                    session_id,
                    32,
                )
                .unwrap(),
            AgentCommandSessionTransitionOutcome::Idempotent
        );
        assert_eq!(
            service
                .load_agent_command_session("conversation-1", session_id)
                .unwrap()
                .unwrap()
                .snapshot
                .status,
            AgentCommandSessionStatus::Running
        );
        let connection = service.state.connection().unwrap();
        let (count, materialized): (u64, u64) = connection
            .query_row(
                "SELECT COUNT(*), COUNT(trace_sequence)
                 FROM agent_command_session_lifecycle_events
                 WHERE session_id = ?1 AND phase = 'started'",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(materialized, 0);
        drop(connection);

        let active_trace = service
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        assert!(!active_trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::CommandSessionLifecycle { .. }
        )));
    }

    #[test]
    fn concurrent_sessions_append_to_one_trace_without_replacing_each_other() {
        let (_directory, first, second) = open_pair();
        seed_conversation_and_trace(&first, true);
        let first_id = "cmd_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let second_id = "cmd_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        create_session(&first, first_id, "call-1", 30);
        create_session(&first, second_id, "call-2", 31);
        let barrier = Arc::new(Barrier::new(2));
        let threads = [
            (Arc::clone(&first), first_id),
            (Arc::clone(&second), second_id),
        ]
        .into_iter()
        .map(|(service, session_id)| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                service
                    .append_agent_command_session_started_lifecycle(
                        "conversation-1",
                        session_id,
                        40,
                    )
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
        for thread in threads {
            assert!(matches!(
                thread.join().unwrap(),
                AgentCommandSessionLifecycleAppendOutcome::Appended { .. }
            ));
        }

        let trace = first
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        let started = trace
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ConversationTurnTraceItem::CommandSessionLifecycle {
                        phase: ConversationCommandSessionLifecyclePhase::Started,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(started, 2);
        assert!(matches!(
            first
                .append_agent_command_session_started_lifecycle("conversation-1", first_id, 41,)
                .unwrap(),
            AgentCommandSessionLifecycleAppendOutcome::Idempotent { .. }
        ));
    }

    #[test]
    fn concurrent_terminal_settlement_commits_exactly_one_lifecycle_pair() {
        let (_directory, first, second) = open_pair();
        seed_conversation_and_trace(&first, true);
        let session_id = "cmd_cccccccccccccccccccccccccccccccc";
        create_session(&first, session_id, "call-1", 30);
        let barrier = Arc::new(Barrier::new(2));
        let threads = [Arc::clone(&first), Arc::clone(&second)]
            .into_iter()
            .map(|service| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    service
                        .settle_agent_command_session_with_lifecycle(
                            &AgentCommandSessionTerminalUpdate {
                                conversation_id: "conversation-1",
                                session_id,
                                status: AgentCommandSessionStatus::Exited,
                                ended_at: 50,
                                exit_code: Some(0),
                                latest_sequence: 0,
                                transcript_truncated: false,
                                output_capture_truncated: false,
                                archive_ref: None,
                                terminal_reason: None,
                                published_outputs: &[],
                                artifact_observation: None,
                                committed_at: 50,
                            },
                        )
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let outcomes = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert!(outcomes.contains(&AgentCommandSessionTransitionOutcome::Updated));
        assert!(outcomes.contains(&AgentCommandSessionTransitionOutcome::Idempotent));

        let trace = first
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        let lifecycle = trace
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ConversationTurnTraceItem::CommandSessionLifecycle {
                        session_id: candidate,
                        ..
                    } if candidate == session_id
                )
            })
            .count();
        assert_eq!(lifecycle, 2);
        assert!(matches!(
            first
                .settle_agent_command_session_with_lifecycle(&AgentCommandSessionTerminalUpdate {
                    conversation_id: "conversation-1",
                    session_id,
                    status: AgentCommandSessionStatus::Interrupted,
                    ended_at: 51,
                    exit_code: None,
                    latest_sequence: 0,
                    transcript_truncated: false,
                    output_capture_truncated: false,
                    archive_ref: None,
                    terminal_reason: Some("conflicting retry"),
                    published_outputs: &[],
                    artifact_observation: None,
                    committed_at: 51,
                },)
                .unwrap(),
            AgentCommandSessionTransitionOutcome::Conflict {
                current_status: AgentCommandSessionStatus::Exited
            }
        ));
    }

    #[test]
    fn startup_reconciliation_atomically_marks_state_and_appends_audit() {
        let (_directory, service, _second) = open_pair();
        seed_conversation_and_trace(&service, false);
        let session_id = "cmd_dddddddddddddddddddddddddddddddd";
        create_session(&service, session_id, "call-1", 30);

        let reconciled = service
            .reconcile_agent_command_sessions_on_startup(100)
            .unwrap();
        assert_eq!(reconciled.len(), 1);
        assert_eq!(
            reconciled[0].snapshot.status,
            AgentCommandSessionStatus::OutcomeUnknown
        );
        let still_unresolved = service
            .reconcile_agent_command_sessions_on_startup(101)
            .unwrap();
        assert_eq!(still_unresolved.len(), 1);
        assert_eq!(still_unresolved[0].snapshot.session_id, session_id);
        assert_eq!(
            still_unresolved[0].snapshot.status,
            AgentCommandSessionStatus::OutcomeUnknown
        );
        let active_trace = service
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            active_trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::CommandSessionLifecycle { .. }
                ))
                .count(),
            0,
            "active runtime items must not be changed by startup sidecar audit"
        );
        let sidecar_count: u64 = service
            .state
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM agent_command_session_lifecycle_events",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sidecar_count, 1);
        let mut terminal_trace = active_trace;
        terminal_trace.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
        terminal_trace.terminal_error = Some("Host restarted".to_string());
        service
            .replace_conversation_turn_trace(&terminal_trace, 2, 110)
            .unwrap();
        let trace = service
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| {
                    matches!(
                        item,
                        ConversationTurnTraceItem::CommandSessionLifecycle {
                            phase: ConversationCommandSessionLifecyclePhase::Terminal,
                            session_id: candidate,
                            status: AgentCommandSessionStatus::OutcomeUnknown,
                            ..
                        } if candidate == session_id
                    )
                })
                .count(),
            1
        );
    }

    #[test]
    fn terminal_settlement_does_not_wait_for_the_active_agent_trace() {
        let (_directory, service, _second) = open_pair();
        seed_conversation_and_trace(&service, false);
        let session_id = "cmd_ffffffffffffffffffffffffffffffff";
        create_session(&service, session_id, "call-1", 30);

        assert_eq!(
            service
                .settle_agent_command_session_with_lifecycle(&AgentCommandSessionTerminalUpdate {
                    conversation_id: "conversation-1",
                    session_id,
                    status: AgentCommandSessionStatus::Exited,
                    ended_at: 50,
                    exit_code: Some(0),
                    latest_sequence: 0,
                    transcript_truncated: false,
                    output_capture_truncated: false,
                    archive_ref: None,
                    terminal_reason: None,
                    published_outputs: &[],
                    artifact_observation: None,
                    committed_at: 50,
                },)
                .unwrap(),
            AgentCommandSessionTransitionOutcome::Updated
        );
        assert_eq!(
            service
                .load_agent_command_session("conversation-1", session_id)
                .unwrap()
                .unwrap()
                .snapshot
                .status,
            AgentCommandSessionStatus::Exited
        );
        let active_trace = service
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        assert!(!active_trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::CommandSessionLifecycle { .. }
        )));
        let pending_sidecars: u64 = service
            .state
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*)
                 FROM agent_command_session_lifecycle_events
                 WHERE trace_sequence IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_sidecars, 2);

        let mut terminal_trace = active_trace;
        terminal_trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
        service
            .replace_conversation_turn_trace(&terminal_trace, 2, 60)
            .unwrap();
        let settled_trace = service
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        assert_eq!(
            settled_trace
                .items
                .iter()
                .filter(|item| {
                    matches!(
                        item,
                        ConversationTurnTraceItem::CommandSessionLifecycle {
                            session_id: candidate,
                            ..
                        } if candidate == session_id
                    )
                })
                .count(),
            2
        );
        let pending_sidecars: u64 = service
            .state
            .connection()
            .unwrap()
            .query_row(
                "SELECT COUNT(*)
                 FROM agent_command_session_lifecycle_events
                 WHERE trace_sequence IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_sidecars, 0);
    }

    #[test]
    fn terminal_archive_is_required_first_and_trace_metadata_comes_from_storage() {
        let (_directory, service, _second) = open_pair();
        seed_conversation_and_trace(&service, true);
        let session_id = "cmd_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        create_session(&service, session_id, "call-1", 30);
        let missing_archive = AgentCommandSessionTerminalUpdate {
            conversation_id: "conversation-1",
            session_id,
            status: AgentCommandSessionStatus::Exited,
            ended_at: 50,
            exit_code: Some(0),
            latest_sequence: 0,
            transcript_truncated: false,
            output_capture_truncated: false,
            archive_ref: Some("history-archive-missing"),
            terminal_reason: None,
            published_outputs: &[],
            artifact_observation: None,
            committed_at: 50,
        };
        assert!(service
            .settle_agent_command_session_with_lifecycle(&missing_archive)
            .is_err());
        assert_eq!(
            service
                .load_agent_command_session("conversation-1", session_id)
                .unwrap()
                .unwrap()
                .snapshot
                .status,
            AgentCommandSessionStatus::Running
        );

        let descriptor = service
            .archive_conversation_tool_result(
                crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput {
                    conversation_id: "conversation-1".to_string(),
                    assistant_message_id: "assistant-1".to_string(),
                    sequence: 99,
                    call_id: format!("command-session:{session_id}"),
                    tool: "run_command".to_string(),
                    content_type: "application/json".to_string(),
                    content: "{\"status\":\"exited\",\"stdout\":\"exact output\"}".to_string(),
                    truncated_at_source: false,
                    model_projection_truncated: true,
                    archive_projection_truncated: false,
                    created_at: 50,
                },
            )
            .unwrap();
        let terminal = AgentCommandSessionTerminalUpdate {
            archive_ref: Some(&descriptor.archive_ref),
            ..missing_archive
        };
        assert_eq!(
            service
                .settle_agent_command_session_with_lifecycle(&terminal)
                .unwrap(),
            AgentCommandSessionTransitionOutcome::Updated
        );
        let trace = service
            .get_conversation_turn_trace("assistant-1")
            .unwrap()
            .unwrap();
        let archive = trace
            .items
            .iter()
            .find_map(|item| match item {
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id: candidate,
                    archive,
                    ..
                } if candidate == session_id => Some(archive),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            archive.archive_ref.as_deref(),
            Some(descriptor.archive_ref.as_str())
        );
        assert_eq!(
            archive.content_hash.as_deref(),
            Some(descriptor.content_hash.as_str())
        );
        assert_eq!(archive.archived_bytes, Some(descriptor.total_bytes));
        assert_eq!(archive.archived_completely, Some(true));
    }
}
