use crate::context::{
    ContextCompactionSummary, ContextContinuitySnapshot, ContextHistoryRef, ContextJournalCursor,
};
use crate::storage::models::{
    AgentFileDraftRecord, AgentRunGuidanceRecord, AttachmentRecord, ChatConversationRecord,
    ChatMessageRecord, ConversationContinuationOriginRecord, ConversationForkPoint,
};
use crate::storage::{
    attachment_repository, chat_repository, context_compaction_receipt_repository,
    context_compaction_repository, conversation_context_adaptation_repository,
    conversation_history_archive_repository, conversation_history_open,
    conversation_model_context_repository, conversation_trace_repository, file_draft_repository,
    guidance_repository, model_request_observation_repository, provider_continuation_repository,
    turn_diff_repository, world_state_repository,
};
use crate::{
    provider_continuation_store::{
        PreparedProviderContinuationClone, ProviderContinuationForkMapping,
    },
    AgentGuidanceStatus, ContextCompactionReceipt, ContextCompactionReceiptStage,
    ContextCompactionReceiptStatus, ConversationModelContextItem, ConversationTurnTrace,
    ModelRequestObservation, ProviderContinuationRef, WorldStateRecord,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const CONVERSATION_FORK_ERROR_TYPE: &str = "conversation_fork";
pub const CONVERSATION_FORK_ACTIVE_COMMAND_ERROR_CODE: &str = "active_command_session";
pub const CONVERSATION_FORK_ACTIVE_COMMAND_MESSAGE: &str =
    "当前任务仍有命令正在运行，请先关闭程序或等待命令结束后再继续新任务。";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationForkError {
    ActiveCommandSession {
        conversation_id: String,
        active_session_count: u64,
    },
    Other(String),
}

impl ConversationForkError {
    pub fn message(&self) -> &str {
        match self {
            Self::ActiveCommandSession { .. } => CONVERSATION_FORK_ACTIVE_COMMAND_MESSAGE,
            Self::Other(message) => message,
        }
    }
}

impl std::fmt::Display for ConversationForkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ConversationForkError {}

impl From<String> for ConversationForkError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ForkAttachmentCopy {
    pub source: AttachmentRecord,
    pub target: AttachmentRecord,
}

#[derive(Debug)]
struct ForkTrace {
    trace: ConversationTurnTrace,
    model_context_items: Vec<ConversationModelContextItem>,
    created_at: i64,
    committed_at: i64,
}

#[derive(Debug)]
struct ForkGuidance {
    record: AgentRunGuidanceRecord,
    applied_trace_sequence: u64,
}

#[derive(Debug)]
struct ForkProviderTransitionReceipt {
    source_receipt: ContextCompactionReceipt,
    source_observation: ModelRequestObservation,
    target_operation_id: String,
    target_run_id: String,
    target_observation_id: String,
}

#[derive(Debug)]
pub(crate) struct ConversationForkPlan {
    pub request_id: String,
    pub source_conversation_id: String,
    pub source_message_id: String,
    pub source_fork_point: ConversationForkPoint,
    pub target: ChatConversationRecord,
    pub attachments: Vec<ForkAttachmentCopy>,
    archives: Vec<conversation_history_archive_repository::ConversationHistoryArchiveForkCopy>,
    traces: Vec<ForkTrace>,
    turn_diffs: Vec<turn_diff_repository::AgentTurnDiffForkCopy>,
    guidances: Vec<ForkGuidance>,
    file_drafts: Vec<AgentFileDraftRecord>,
    summaries: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    provider_transition_receipts: Vec<ForkProviderTransitionReceipt>,
    world_state_records: Vec<world_state_repository::ConversationWorldStateJournalEntry>,
    pub(crate) provider_continuation_mappings: Vec<ProviderContinuationForkMapping>,
    pub(crate) requires_context_adaptation: bool,
    adaptation_source_summary_id: Option<String>,
    message_id_map: HashMap<String, String>,
    #[cfg(test)]
    run_id_map: HashMap<String, String>,
    id_replacements: HashMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct ExistingConversationFork {
    pub target_conversation_id: String,
    pub source_conversation_id: String,
    pub source_fork_point: ConversationForkPoint,
}

struct ResolvedConversationForkPoint {
    assistant_message_id: String,
    summary_id: Option<String>,
    model_id: Option<String>,
}

pub(crate) fn find_existing_fork(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<ExistingConversationFork>> {
    connection
        .query_row(
            "SELECT target_conversation_id, source_conversation_id, source_fork_point_json
             FROM conversation_forks
             WHERE request_id = ?1",
            [request_id],
            |row| {
                let fork_point_json = row.get::<_, String>(2)?;
                let source_fork_point =
                    serde_json::from_str::<ConversationForkPoint>(&fork_point_json)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok(ExistingConversationFork {
                    target_conversation_id: row.get(0)?,
                    source_conversation_id: row.get(1)?,
                    source_fork_point,
                })
            },
        )
        .optional()
}

pub(crate) fn get_continuation_origin(
    connection: &Connection,
    target_conversation_id: &str,
) -> rusqlite::Result<Option<ConversationContinuationOriginRecord>> {
    connection
        .query_row(
            "SELECT source_conversation_id, source_message_id, target_message_id
             FROM conversation_forks
             WHERE target_conversation_id = ?1
               AND target_message_id IS NOT NULL",
            [target_conversation_id],
            |row| {
                Ok(ConversationContinuationOriginRecord {
                    source_conversation_id: row.get(0)?,
                    source_message_id: row.get(1)?,
                    boundary_message_id: row.get(2)?,
                })
            },
        )
        .optional()
}

pub(crate) fn build_fork_plan_at_point(
    connection: &Connection,
    request_id: &str,
    source_conversation_id: &str,
    fork_point: &ConversationForkPoint,
    created_at: i64,
) -> Result<ConversationForkPlan, ConversationForkError> {
    validate_fork_point_input(request_id, source_conversation_id, fork_point)?;
    let source_is_agent_bound = connection
        .query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM agent_nodes WHERE conversation_id = ?1
             )",
            [source_conversation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if source_is_agent_bound {
        return Err(ConversationForkError::Other(
            "Persistent Agent conversations must be forked through the collaboration service."
                .to_string(),
        ));
    }
    ensure_no_active_command_sessions(connection, source_conversation_id)?;
    let source = chat_repository::get_conversation(connection, source_conversation_id)
        .map_err(database_error)?
        .ok_or_else(|| "原任务不存在。".to_string())?;
    let active_chain =
        context_compaction_repository::list_active_summary_chain(connection, &source.id)
            .map_err(|error| error.to_string())?;
    let resolved = resolve_fork_point(connection, &source, fork_point, &active_chain)?;
    let cutoff = source
        .messages
        .iter()
        .position(|message| message.id == resolved.assistant_message_id)
        .ok_or_else(|| "所选回复不属于原任务。".to_string())?;
    let cutoff_message = &source.messages[cutoff];
    if cutoff_message.role != "assistant" {
        return Err("只能从 assistant 回复继续新任务。".to_string().into());
    }
    let source_positions = source
        .messages
        .iter()
        .enumerate()
        .map(|(position, message)| (message.id.clone(), position))
        .collect::<HashMap<_, _>>();
    let source_messages = source.messages[..=cutoff].to_vec();
    let target_conversation_id = new_id("conversation");
    let message_id_map = source_messages
        .iter()
        .map(|message| (message.id.clone(), new_id("message")))
        .collect::<HashMap<_, _>>();

    let source_message_ids = source_messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let summaries = match resolved.summary_id.as_deref() {
        Some(summary_id) => summaries_visible_through_transition_boundary(
            active_chain,
            &source_positions,
            cutoff,
            summary_id,
        )?,
        None => summaries_visible_at_assistant_reply(
            connection,
            &source.id,
            active_chain,
            &source_positions,
            cutoff,
        )?,
    };
    // Provider-transition receipts are part of the visible timeline, not disposable audit noise.
    // Copy every transition whose summary is visible at the selected fork point so a later fork
    // of the child conversation retains the same semantic history and UI boundary.
    let provider_transition_receipts =
        collect_visible_provider_transition_receipts(connection, &source.id, &summaries)?;
    let summary_covered_runtime_tool_calls = summaries
        .last()
        .map(|version| {
            context_compaction_repository::covered_runtime_tool_calls_through_cursor(
                connection,
                &source.id,
                &version.summary.covered_through,
            )
        })
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let source_adaptation = conversation_context_adaptation_repository::get(connection, &source.id)
        .map_err(database_error)?;
    let selected_summary_ids = summaries
        .iter()
        .map(|version| version.summary.id.as_str())
        .collect::<HashSet<_>>();
    let released_state_outside_summary =
        provider_continuation_repository::has_released_for_messages_outside_summary_coverage(
            connection,
            &source.id,
            &source_message_ids,
            &summary_covered_runtime_tool_calls,
        )
        .map_err(database_error)?;
    let source_adaptation_boundary_selected = source_adaptation
        .as_ref()
        .and_then(|requirement| requirement.resolved_summary_id.as_deref())
        .is_some_and(|summary_id| selected_summary_ids.contains(summary_id));
    let requires_context_adaptation = released_state_outside_summary
        || source_adaptation.as_ref().is_some_and(|requirement| {
            requirement.is_required() || !source_adaptation_boundary_selected
        });
    let contains_released_provider_history =
        provider_continuation_repository::has_released_for_messages(
            connection,
            &source.id,
            &source_message_ids,
        )
        .map_err(database_error)?;
    let adaptation_source_summary_id = (!requires_context_adaptation)
        .then(|| {
            source_adaptation
                .as_ref()
                .and_then(|requirement| requirement.resolved_summary_id.clone())
                .filter(|summary_id| selected_summary_ids.contains(summary_id.as_str()))
                .or_else(|| {
                    contains_released_provider_history
                        .then(|| summaries.last().map(|version| version.summary.id.clone()))
                        .flatten()
                })
        })
        .flatten();
    let source_attachments = attachment_repository::list_message_attachments_for_fork(
        connection,
        &source.id,
        &source_message_ids,
    )
    .map_err(database_error)?;
    let attachment_id_map = source_attachments
        .iter()
        .map(|attachment| (attachment.id.clone(), new_id("attachment")))
        .collect::<HashMap<_, _>>();

    let mut traces = Vec::new();
    let mut run_id_map = HashMap::new();
    for message in &source_messages {
        let trace = conversation_trace_repository::get_trace_for_message(connection, &message.id)
            .map_err(database_error)?;
        ensure_settled_assistant(message, trace.as_ref())?;
        if let Some(trace) = trace {
            if agent_run_id(message.agent_run_json.as_deref())
                .as_deref()
                .is_some_and(|run_id| run_id != trace.run_id)
            {
                return Err("历史回复的运行标识与后端工具轨迹不一致。"
                    .to_string()
                    .into());
            }
            let new_run_id = new_id("run");
            run_id_map.insert(trace.run_id.clone(), new_run_id.clone());
            let (trace_created_at, committed_at) = trace_times(connection, &message.id)?;
            let model_context_items =
                conversation_model_context_repository::get_log_for_message(connection, &message.id)
                    .map_err(database_error)?
                    .map(|log| log.items)
                    .unwrap_or_default();
            traces.push(ForkTrace {
                trace: ConversationTurnTrace {
                    schema_version: trace.schema_version,
                    run_id: new_run_id,
                    conversation_id: target_conversation_id.clone(),
                    assistant_message_id: mapped_id(&message_id_map, &message.id, "消息")?,
                    terminal_status: trace.terminal_status,
                    terminal_error: trace.terminal_error,
                    truncated: trace.truncated,
                    items: trace.items,
                },
                model_context_items,
                created_at: trace_created_at,
                committed_at,
            });
        } else if let Some(run_id) = agent_run_id(message.agent_run_json.as_deref()) {
            run_id_map.entry(run_id).or_insert_with(|| new_id("run"));
        }
    }

    // A recursive fork needs every visible turn diff, not only the boundary turn.
    let mut turn_diffs = turn_diff_repository::list_fork_copies_through_message(
        connection,
        &source.id,
        &resolved.assistant_message_id,
    )
    .map_err(database_error)?;
    for turn_diff in &mut turn_diffs {
        if turn_diff.record.identity.conversation_id != source.id
            || source.project_id.as_deref() != Some(turn_diff.record.identity.project_id.as_str())
        {
            return Err("历史文件变更证据的任务或项目归属不一致。"
                .to_string()
                .into());
        }
        turn_diff.record.identity.conversation_id = target_conversation_id.clone();
        turn_diff.record.identity.assistant_message_id = mapped_id(
            &message_id_map,
            &turn_diff.record.identity.assistant_message_id,
            "文件变更证据所属消息",
        )?;
        turn_diff.record.identity.run_id = mapped_id(
            &run_id_map,
            &turn_diff.record.identity.run_id,
            "文件变更证据所属运行",
        )?;
    }

    let mut file_drafts = Vec::new();
    let mut draft_id_map = HashMap::new();
    for (source_run_id, target_run_id) in &run_id_map {
        for source_draft in file_draft_repository::list_drafts_for_run(connection, source_run_id)
            .map_err(database_error)?
        {
            if source_draft.conversation_id != source.id {
                return Err("文件草稿的任务归属与历史回复不一致。".to_string().into());
            }
            let target_draft_id = new_id("file-draft");
            draft_id_map.insert(source_draft.id.clone(), target_draft_id.clone());
            file_drafts.push(AgentFileDraftRecord {
                id: target_draft_id,
                conversation_id: target_conversation_id.clone(),
                project_id: source.project_id.clone(),
                run_id: target_run_id.clone(),
                file_path: source_draft.file_path,
                mode: source_draft.mode,
                status: source_draft.status,
                base_revision: source_draft.base_revision,
                base_content: source_draft.base_content,
                content: source_draft.content,
                additions: source_draft.additions,
                deletions: source_draft.deletions,
                line_count: source_draft.line_count,
                byte_count: source_draft.byte_count,
                chunk_count: source_draft.chunk_count,
                next_chunk_index: source_draft.next_chunk_index,
                stats_final: source_draft.stats_final,
                summary: source_draft.summary,
                final_action_id: source_draft.final_action_id,
                created_at: source_draft.created_at,
                updated_at: source_draft.updated_at,
                expires_at: source_draft.expires_at,
            });
        }
    }

    let mut guidance_id_map = HashMap::new();
    let mut guidances = Vec::new();
    for message in &source_messages {
        for guidance in
            guidance_repository::list_guidances_for_assistant_message(connection, &message.id)
                .map_err(database_error)?
        {
            if guidance.status != AgentGuidanceStatus::Applied {
                continue;
            }
            let target_guidance_id = new_id("guidance");
            let target_run_id = mapped_id(&run_id_map, &guidance.run_id, "引导所属运行")?;
            let target_assistant_message_id = mapped_id(
                &message_id_map,
                &guidance.assistant_message_id,
                "引导所属消息",
            )?;
            let target_attachment_ids = guidance
                .attachment_ids
                .iter()
                .map(|attachment_id| mapped_id(&attachment_id_map, attachment_id, "引导附件"))
                .collect::<Result<Vec<_>, _>>()?;
            let applied_trace_sequence = guidance
                .applied_trace_sequence
                .ok_or_else(|| "已应用引导缺少 trace 顺序。".to_string())?;
            guidance_id_map.insert(guidance.guidance_id.clone(), target_guidance_id.clone());
            guidances.push(ForkGuidance {
                record: AgentRunGuidanceRecord {
                    guidance_id: target_guidance_id,
                    client_message_id: guidance.client_message_id,
                    run_id: target_run_id,
                    conversation_id: target_conversation_id.clone(),
                    assistant_message_id: target_assistant_message_id,
                    content: guidance.content,
                    status: AgentGuidanceStatus::Queued,
                    attachment_ids: target_attachment_ids,
                    applied_trace_sequence: None,
                    terminal_reason: None,
                    created_at: guidance.created_at,
                    updated_at: guidance.updated_at,
                },
                applied_trace_sequence,
            });
        }
    }

    let mut archive_id_map = HashMap::new();
    let mut archives = Vec::new();
    for trace in &traces {
        for item in &trace.trace.items {
            let archive = match item {
                crate::ConversationTurnTraceItem::ToolResult { archive, .. }
                | crate::ConversationTurnTraceItem::CommandSessionLifecycle { archive, .. } => {
                    archive
                }
                _ => continue,
            };
            let Some(source_archive_ref) = archive.archive_ref.as_deref() else {
                continue;
            };
            if archive_id_map.contains_key(source_archive_ref) {
                continue;
            }
            let copy = conversation_history_archive_repository::load_fork_copy(
                connection,
                &source.id,
                source_archive_ref,
                &target_conversation_id,
                &trace.trace.assistant_message_id,
            )
            .map_err(database_error)?;
            archive_id_map.insert(
                source_archive_ref.to_string(),
                copy.target_archive_ref.clone(),
            );
            archives.push(copy);
        }
    }

    let mut replacements = message_id_map.clone();
    replacements.extend(run_id_map.clone());
    replacements.extend(attachment_id_map.clone());
    replacements.extend(guidance_id_map);
    replacements.extend(draft_id_map);
    replacements.extend(archive_id_map);
    replacements.insert(source.id.clone(), target_conversation_id.clone());
    for fork_trace in &mut traces {
        rewrite_trace_items(&mut fork_trace.trace, &replacements)?;
        rewrite_model_context_items(&mut fork_trace.model_context_items, &replacements)?;
    }
    let target_messages = source_messages
        .iter()
        .map(|message| clone_message(message, &message_id_map, &replacements))
        .collect::<Result<Vec<_>, _>>()?;

    let attachments = source_attachments
        .into_iter()
        .map(|source_attachment| {
            let target_attachment_id =
                mapped_id(&attachment_id_map, &source_attachment.id, "附件")?;
            let target_message_id = mapped_id(
                &message_id_map,
                &source_attachment.message_id,
                "附件所属消息",
            )?;
            Ok(ForkAttachmentCopy {
                target: AttachmentRecord {
                    id: target_attachment_id,
                    conversation_id: target_conversation_id.clone(),
                    message_id: target_message_id,
                    project_id: source.project_id.clone(),
                    kind: source_attachment.kind.clone(),
                    original_name: source_attachment.original_name.clone(),
                    mime_type: source_attachment.mime_type.clone(),
                    size_bytes: source_attachment.size_bytes,
                    storage_rel_path: String::new(),
                    created_at: source_attachment.created_at,
                },
                source: source_attachment,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let world_state_records = world_state_records_visible_at_cutoff(
        connection,
        &source.id,
        &source_positions,
        cutoff,
        &summaries,
    )?;
    let provider_continuation_mappings = if requires_context_adaptation {
        // The exact old payload was already released, so a partial clone of whatever happens to
        // remain replayable would create a misleading mixed snapshot and unnecessarily depend on
        // Vault availability. The backend-only marker below forces a tool-free full-prefix
        // compaction before this fork can send anything.
        Vec::new()
    } else {
        provider_continuation_repository::list_replayable_for_conversation(connection, &source.id)
            .map_err(database_error)?
            .into_iter()
            .filter_map(|record| {
                let target_assistant_message_id =
                    message_id_map.get(&record.assistant_message_id)?.clone();
                Some((record, target_assistant_message_id))
            })
            .map(|(record, target_assistant_message_id)| {
                let source_ref = ProviderContinuationRef::parse(
                    crate::protocol::PROVIDER_CONTINUATION_REF_VERSION,
                    record.continuation_id.clone(),
                )?;
                let target_run_id = mapped_id(
                    &run_id_map,
                    &record.run_id,
                    "Provider continuation 所属运行",
                )?;
                Ok(ProviderContinuationForkMapping {
                    source_ref,
                    source_record: record.clone(),
                    source_conversation_id: source.id.clone(),
                    source_assistant_message_id: record.assistant_message_id.clone(),
                    source_run_id: record.run_id.clone(),
                    request_index: record.request_index,
                    target_conversation_id: target_conversation_id.clone(),
                    target_assistant_message_id,
                    target_run_id,
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    };

    Ok(ConversationForkPlan {
        request_id: request_id.to_string(),
        source_conversation_id: source.id,
        source_message_id: resolved.assistant_message_id,
        source_fork_point: fork_point.clone(),
        target: ChatConversationRecord {
            id: target_conversation_id,
            project_id: source.project_id,
            model_id: resolved.model_id.or(source.model_id),
            title: source.title,
            messages: target_messages,
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        },
        attachments,
        archives,
        traces,
        turn_diffs,
        guidances,
        file_drafts,
        summaries,
        provider_transition_receipts,
        world_state_records,
        provider_continuation_mappings,
        requires_context_adaptation,
        adaptation_source_summary_id,
        message_id_map,
        #[cfg(test)]
        run_id_map,
        id_replacements: replacements,
    })
}

#[cfg(test)]
pub(crate) fn commit_fork_plan(
    connection: &mut Connection,
    plan: &ConversationForkPlan,
) -> Result<(), ConversationForkError> {
    commit_fork_plan_with_provider_continuations(connection, plan, &[])
}

pub(crate) fn commit_fork_plan_with_provider_continuations(
    connection: &mut Connection,
    plan: &ConversationForkPlan,
    provider_continuations: &[PreparedProviderContinuationClone],
) -> Result<(), ConversationForkError> {
    if provider_continuations.len() != plan.provider_continuation_mappings.len() {
        return Err("Provider continuation 克隆未完整准备，已安全取消整个分叉。"
            .to_string()
            .into());
    }
    for (mapping, prepared) in plan
        .provider_continuation_mappings
        .iter()
        .zip(provider_continuations)
    {
        if prepared.source_ref != mapping.source_ref
            || prepared.target_ref.id != prepared.record.continuation_id
            || prepared.record.conversation_id != mapping.target_conversation_id
            || prepared.record.assistant_message_id != mapping.target_assistant_message_id
            || prepared.record.run_id != mapping.target_run_id
            || prepared.record.request_index != mapping.request_index
        {
            return Err("Provider continuation 克隆身份不匹配，已安全取消整个分叉。"
                .to_string()
                .into());
        }
    }
    let target_message_id = mapped_id(
        &plan.message_id_map,
        &plan.source_message_id,
        "新任务接续边界消息",
    )?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database_error)?;
    ensure_no_active_command_sessions(&transaction, &plan.source_conversation_id)?;
    insert_conversation(&transaction, &plan.target)?;
    for prepared in provider_continuations {
        match provider_continuation_repository::store_active_in_connection(
            &transaction,
            &prepared.record,
        )
        .map_err(database_error)?
        {
            provider_continuation_repository::ProviderContinuationStoreOutcome::Inserted {
                ..
            } => {}
            provider_continuation_repository::ProviderContinuationStoreOutcome::Idempotent {
                ..
            }
            | provider_continuation_repository::ProviderContinuationStoreOutcome::Conflict => {
                return Err("Provider continuation 克隆写入冲突，已安全取消整个分叉。"
                    .to_string()
                    .into());
            }
        }
    }
    for archive in &plan.archives {
        conversation_history_archive_repository::clone_archive_in_connection(&transaction, archive)
            .map_err(database_error)?;
    }
    for trace in &plan.traces {
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            &trace.trace,
            trace.created_at,
            trace.committed_at,
        )
        .map_err(database_error)?;
        conversation_model_context_repository::commit_items_in_connection(
            &transaction,
            &trace.trace.conversation_id,
            &trace.trace.assistant_message_id,
            &trace.model_context_items,
        )
        .map_err(database_error)?;
    }
    for turn_diff in &plan.turn_diffs {
        turn_diff_repository::insert_fork_copy(&transaction, turn_diff).map_err(database_error)?;
    }
    for draft in &plan.file_drafts {
        file_draft_repository::insert_draft(&transaction, draft).map_err(database_error)?;
    }
    let summary_id_map = clone_summary_chain(&transaction, plan)?;
    clone_provider_transition_receipts(&transaction, plan, &summary_id_map)?;
    clone_world_state_records(&transaction, plan, &summary_id_map)?;
    if plan.requires_context_adaptation {
        conversation_context_adaptation_repository::insert_in_connection(
            &transaction,
            &conversation_context_adaptation_repository::ConversationContextAdaptationRequirement {
                conversation_id: plan.target.id.clone(),
                reason:
                    conversation_context_adaptation_repository::FORK_RELEASED_PROVIDER_STATE_REASON
                        .to_string(),
                source_conversation_id: plan.source_conversation_id.clone(),
                source_message_id: plan.source_message_id.clone(),
                created_at: plan.target.created_at,
                resolved_summary_id: None,
                resolved_at: None,
            },
        )
        .map_err(database_error)?;
    } else if let Some(source_summary_id) = plan.adaptation_source_summary_id.as_deref() {
        let target_summary_id = mapped_id(
            &summary_id_map,
            source_summary_id,
            "Provider-neutral 适配摘要",
        )?;
        conversation_context_adaptation_repository::insert_in_connection(
            &transaction,
            &conversation_context_adaptation_repository::ConversationContextAdaptationRequirement {
                conversation_id: plan.target.id.clone(),
                reason:
                    conversation_context_adaptation_repository::FORK_RELEASED_PROVIDER_STATE_REASON
                        .to_string(),
                source_conversation_id: plan.source_conversation_id.clone(),
                source_message_id: plan.source_message_id.clone(),
                created_at: plan.target.created_at,
                resolved_summary_id: Some(target_summary_id),
                resolved_at: Some(plan.target.created_at),
            },
        )
        .map_err(database_error)?;
    }
    for attachment in &plan.attachments {
        attachment_repository::save_attachment(&transaction, &attachment.target)
            .map_err(database_error)?;
    }
    for guidance in &plan.guidances {
        match guidance_repository::store_guidance_in_connection(&transaction, &guidance.record)
            .map_err(database_error)?
        {
            guidance_repository::AgentRunGuidanceStoreOutcome::Inserted => {}
            outcome => {
                return Err(format!("克隆用户引导 journal 时发生意外冲突：{outcome:?}").into());
            }
        }
        match guidance_repository::mark_guidance_applied(
            &transaction,
            &guidance.record.guidance_id,
            guidance.applied_trace_sequence,
            guidance.record.updated_at,
        )
        .map_err(database_error)?
        {
            guidance_repository::AgentRunGuidanceTransitionOutcome::Updated => {}
            outcome => {
                return Err(format!("克隆用户引导 trace 状态时发生意外冲突：{outcome:?}").into());
            }
        }
    }
    transaction
        .execute(
            "INSERT INTO conversation_forks (
                request_id, target_conversation_id, source_conversation_id,
                source_message_id, target_message_id, created_at, source_fork_point_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &plan.request_id,
                &plan.target.id,
                &plan.source_conversation_id,
                &plan.source_message_id,
                &target_message_id,
                plan.target.created_at,
                serde_json::to_string(&plan.source_fork_point)
                    .map_err(|error| format!("无法序列化分叉时间线边界：{error}"))?,
            ],
        )
        .map_err(database_error)?;
    transaction
        .commit()
        .map_err(database_error)
        .map_err(Into::into)
}

fn resolve_fork_point(
    connection: &Connection,
    source: &ChatConversationRecord,
    fork_point: &ConversationForkPoint,
    active_chain: &[context_compaction_repository::ContextCompactionSummaryVersion],
) -> Result<ResolvedConversationForkPoint, ConversationForkError> {
    match fork_point {
        ConversationForkPoint::AssistantReply {
            assistant_message_id,
        } => Ok(ResolvedConversationForkPoint {
            assistant_message_id: assistant_message_id.clone(),
            summary_id: None,
            model_id: assistant_message_model_id(connection, &source.id, assistant_message_id)?,
        }),
        ConversationForkPoint::ProviderTransitionBoundary { operation_id } => {
            if !operation_id.starts_with("provider-transition-") {
                return Err("Provider transition 分叉边界无效。".to_string().into());
            }
            let receipt =
                context_compaction_receipt_repository::get_receipt(connection, operation_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "找不到指定的 Provider transition 分叉边界。".to_string())?;
            let summary_id = receipt
                .summary_id
                .as_deref()
                .ok_or_else(|| "指定的 Provider transition 没有已提交摘要。".to_string())?;
            if receipt.operation_id != *operation_id
                || receipt.conversation_id != source.id
                || receipt.status != ContextCompactionReceiptStatus::Applied
                || receipt.stage != ContextCompactionReceiptStage::Completed
                || receipt
                    .result
                    .as_ref()
                    .map(|result| result.summary_id.as_str())
                    != Some(summary_id)
            {
                return Err("Provider transition 分叉边界与已提交记录不一致。"
                    .to_string()
                    .into());
            }
            let version = active_chain
                .iter()
                .find(|version| version.summary.id == summary_id)
                .ok_or_else(|| {
                    "Provider transition 摘要已不在当前受支持的 active 历史链中。".to_string()
                })?;
            if version.summary.conversation_id != source.id
                || version.lineage.introduced_by_assistant_message_id
                    != receipt.assistant_message_id
                || version.summary.covered_through != receipt.plan.covered_through
                || receipt.source_revision.as_deref()
                    != Some(version.summary.source_revision.as_str())
            {
                return Err("Provider transition 摘要身份或历史覆盖范围不匹配。"
                    .to_string()
                    .into());
            }
            Ok(ResolvedConversationForkPoint {
                assistant_message_id: receipt.assistant_message_id,
                summary_id: Some(summary_id.to_string()),
                model_id: Some(receipt.model),
            })
        }
    }
}

fn assistant_message_model_id(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
) -> Result<Option<String>, ConversationForkError> {
    let usage_model = connection
        .query_row(
            "SELECT model_id
             FROM agent_usage_records
             WHERE conversation_id = ?1 AND message_id = ?2",
            params![conversation_id, assistant_message_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?;
    if let Some(model_id) = usage_model {
        if model_id.trim().is_empty() {
            return Err("回复绑定的模型身份无效。".to_string().into());
        }
        return Ok(Some(model_id));
    }
    let observed_model = connection
        .query_row(
            "SELECT model
             FROM model_request_observations
             WHERE conversation_id = ?1
               AND assistant_message_id = ?2
               AND purpose = 'agent_loop'
             ORDER BY request_index DESC, completed_at DESC, id DESC
             LIMIT 1",
            params![conversation_id, assistant_message_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?;
    if observed_model
        .as_deref()
        .is_some_and(|model_id| model_id.trim().is_empty())
    {
        return Err("回复绑定的模型请求身份无效。".to_string().into());
    }
    Ok(observed_model)
}

fn ensure_no_active_command_sessions(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), ConversationForkError> {
    let active_session_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM agent_command_sessions
             WHERE conversation_id = ?1
               AND status IN ('starting', 'running')",
            [conversation_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    if active_session_count == 0 {
        return Ok(());
    }
    Err(ConversationForkError::ActiveCommandSession {
        conversation_id: conversation_id.to_string(),
        active_session_count,
    })
}

pub(crate) fn validate_fork_point_input(
    request_id: &str,
    source_conversation_id: &str,
    fork_point: &ConversationForkPoint,
) -> Result<(), String> {
    for (label, value) in [
        ("请求 ID", request_id),
        ("原任务 ID", source_conversation_id),
    ] {
        if value.trim().is_empty() || value.trim() != value || value.chars().count() > 512 {
            return Err(format!("{label}无效。"));
        }
    }
    let (label, value, maximum) = match fork_point {
        ConversationForkPoint::AssistantReply {
            assistant_message_id,
        } => ("回复 ID", assistant_message_id.as_str(), 512),
        ConversationForkPoint::ProviderTransitionBoundary { operation_id } => (
            "Provider transition operation ID",
            operation_id.as_str(),
            1024,
        ),
    };
    if value.trim().is_empty() || value.trim() != value || value.chars().count() > maximum {
        return Err(format!("{label}无效。"));
    }
    Ok(())
}

fn ensure_settled_assistant(
    message: &ChatMessageRecord,
    trace: Option<&ConversationTurnTrace>,
) -> Result<(), String> {
    if message.role != "assistant" {
        return Ok(());
    }
    let agent_status = agent_run_status(message.agent_run_json.as_deref())?;
    let agent_is_terminal = agent_status
        .as_deref()
        .is_none_or(|status| matches!(status, "idle" | "completed" | "failed" | "cancelled"));
    if trace.is_some_and(|trace| !trace.terminal_status.is_terminal())
        || message.status.as_deref() == Some("pending")
        || !agent_is_terminal
    {
        return Err("这条回复仍在生成，结束后才能在新任务中继续。".to_string());
    }
    Ok(())
}

fn trace_times(connection: &Connection, message_id: &str) -> Result<(i64, i64), String> {
    connection
        .query_row(
            "SELECT created_at, COALESCE(completed_at, updated_at)
             FROM conversation_turn_traces
             WHERE assistant_message_id = ?1",
            [message_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(database_error)
}

fn summaries_visible_at_assistant_reply(
    connection: &Connection,
    conversation_id: &str,
    chain: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    source_positions: &HashMap<String, usize>,
    cutoff: usize,
) -> Result<Vec<context_compaction_repository::ContextCompactionSummaryVersion>, String> {
    let mut visible = Vec::new();
    let mut previous_owner_position = None;
    let mut crossed_cutoff = false;
    for version in chain {
        let owner_position = source_positions
            .get(&version.lineage.introduced_by_assistant_message_id)
            .copied()
            .ok_or_else(|| "摘要链引用了不属于原任务的生成回复。".to_string())?;
        if previous_owner_position.is_some_and(|previous| owner_position < previous) {
            return Err("摘要链的因果顺序无效。".to_string());
        }
        previous_owner_position = Some(owner_position);
        if owner_position > cutoff {
            crossed_cutoff = true;
            continue;
        }
        if owner_position == cutoff {
            let transition = context_compaction_receipt_repository::get_applied_provider_transition_receipt_for_summary(
                connection,
                conversation_id,
                &version.summary.id,
            )
            .map_err(|error| error.to_string())?;
            // A Provider-transition summary owned by this reply is displayed after the reply's
            // Fork action. Once crossed, every later summary belongs after that timeline point,
            // even if it happens to reuse the same owner message.
            if transition.is_some() || crossed_cutoff {
                crossed_cutoff = true;
                continue;
            }
        } else if crossed_cutoff {
            return Err("摘要链跨越分叉边界后又回到了更早轮次。".to_string());
        }
        visible.push(version);
    }
    Ok(visible)
}

fn summaries_visible_through_transition_boundary(
    chain: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    source_positions: &HashMap<String, usize>,
    cutoff: usize,
    boundary_summary_id: &str,
) -> Result<Vec<context_compaction_repository::ContextCompactionSummaryVersion>, String> {
    let boundary_index = chain
        .iter()
        .position(|version| version.summary.id == boundary_summary_id)
        .ok_or_else(|| "Provider transition 摘要不在 active 摘要链中。".to_string())?;
    let visible = chain
        .into_iter()
        .take(boundary_index + 1)
        .collect::<Vec<_>>();
    let mut previous_owner_position = None;
    for version in &visible {
        let owner_position = source_positions
            .get(&version.lineage.introduced_by_assistant_message_id)
            .copied()
            .ok_or_else(|| "摘要链引用了不属于原任务的生成回复。".to_string())?;
        if owner_position > cutoff
            || previous_owner_position.is_some_and(|previous| owner_position < previous)
        {
            return Err("Provider transition 摘要链的因果顺序无效。".to_string());
        }
        previous_owner_position = Some(owner_position);
    }
    Ok(visible)
}

fn collect_visible_provider_transition_receipts(
    connection: &Connection,
    source_conversation_id: &str,
    visible_summaries: &[context_compaction_repository::ContextCompactionSummaryVersion],
) -> Result<Vec<ForkProviderTransitionReceipt>, String> {
    let mut receipts = Vec::new();
    for version in visible_summaries {
        let Some(receipt) = context_compaction_receipt_repository::get_applied_provider_transition_receipt_for_summary(
            connection,
            source_conversation_id,
            &version.summary.id,
        )
        .map_err(|error| error.to_string())?
        else {
            continue;
        };
        if receipt.conversation_id != source_conversation_id
            || receipt.assistant_message_id != version.lineage.introduced_by_assistant_message_id
            || receipt.summary_id.as_deref() != Some(version.summary.id.as_str())
            || receipt.plan.previous_summary_id != version.summary.previous_summary_id
            || receipt.plan.covered_through != version.summary.covered_through
            || receipt.source_revision.as_deref() != Some(version.summary.source_revision.as_str())
        {
            return Err(
                "Provider transition receipt 与可见摘要的身份或覆盖范围不一致。".to_string(),
            );
        }
        let observation_id = receipt
            .generation_observation_id
            .as_deref()
            .ok_or_else(|| "Provider transition receipt 缺少模型请求观测。".to_string())?;
        let observation =
            model_request_observation_repository::get_observation(connection, observation_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    "Provider transition receipt 引用的模型请求观测不存在。".to_string()
                })?;
        receipt
            .validate_generation_observation(&observation)
            .map_err(|error| error.to_string())?;
        receipts.push(ForkProviderTransitionReceipt {
            source_receipt: receipt,
            source_observation: observation,
            target_operation_id: new_id("provider-transition"),
            target_run_id: new_id("context-compaction-run"),
            target_observation_id: new_id("model-request-observation"),
        });
    }
    Ok(receipts)
}

fn world_state_records_visible_at_cutoff(
    connection: &Connection,
    conversation_id: &str,
    source_positions: &HashMap<String, usize>,
    cutoff: usize,
    visible_summaries: &[context_compaction_repository::ContextCompactionSummaryVersion],
) -> Result<Vec<world_state_repository::ConversationWorldStateJournalEntry>, String> {
    let epochs = {
        let mut statement = connection
            .prepare(
                "SELECT epoch_id, base_summary_id
                 FROM conversation_world_state_epochs
                 WHERE conversation_id = ?1
                 ORDER BY generation DESC",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([conversation_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(database_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_error)?;
        rows
    };
    if epochs.is_empty() {
        return Ok(Vec::new());
    }
    let visible_summary_ids = visible_summaries
        .iter()
        .map(|version| version.summary.id.as_str())
        .collect::<HashSet<_>>();
    let (epoch_id, selected_base_summary_id) = epochs
        .into_iter()
        .find(|(_, base_summary_id)| {
            base_summary_id
                .as_deref()
                .is_none_or(|summary_id| visible_summary_ids.contains(summary_id))
        })
        .ok_or_else(|| "没有任何 World State epoch 的压缩边界在分叉 cutoff 内可见。".to_string())?;
    let entries =
        world_state_repository::list_records_for_epoch(connection, conversation_id, &epoch_id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(world_state_repository::ConversationWorldStateJournalEntry::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
    let Some(first) = entries.first() else {
        return Err("选中的 World State epoch 没有 initial full snapshot。".to_string());
    };
    let WorldStateRecord::Full(_) = &first.record else {
        return Err("选中的 World State epoch 没有 initial full snapshot。".to_string());
    };
    if first.effective_before_message_id.is_some() {
        return Err("选中的 World State full snapshot 不能绑定消息 anchor。".to_string());
    }
    let base_summary_id = first.base_summary_id.as_deref();
    if base_summary_id != selected_base_summary_id.as_deref() {
        return Err("World State epoch 索引与记录的 summary 边界不一致。".to_string());
    }
    if entries
        .iter()
        .any(|entry| entry.base_summary_id.as_deref() != base_summary_id)
    {
        return Err("选中的 World State epoch 的 summary 边界不一致。".to_string());
    }

    let mut visible = Vec::new();
    let mut crossed_cutoff = false;
    for entry in entries {
        let is_visible =
            match entry.effective_before_message_id.as_deref() {
                None => true,
                Some(message_id) => {
                    source_positions.get(message_id).copied().ok_or_else(|| {
                        format!("World State anchor 不属于原任务消息：{message_id}")
                    })? <= cutoff
                }
            };
        if is_visible {
            if crossed_cutoff {
                return Err("World State diff 顺序跨越分叉边界后又回到可见历史。".to_string());
            }
            visible.push(entry);
        } else {
            crossed_cutoff = true;
        }
    }
    Ok(visible)
}

fn clone_message(
    source: &ChatMessageRecord,
    message_id_map: &HashMap<String, String>,
    replacements: &HashMap<String, String>,
) -> Result<ChatMessageRecord, String> {
    let agent_run_json = source
        .agent_run_json
        .as_deref()
        .map(|raw| clone_agent_run_json(raw, replacements))
        .transpose()?;
    Ok(ChatMessageRecord {
        id: mapped_id(message_id_map, &source.id, "消息")?,
        role: source.role.clone(),
        content: source.content.clone(),
        created_at: source.created_at,
        status: source.status.clone(),
        attachments: Vec::new(),
        agent_run_json,
        ui_state_json: source.ui_state_json.clone(),
    })
}

fn clone_agent_run_json(
    raw: &str,
    replacements: &HashMap<String, String>,
) -> Result<String, String> {
    let mut value = serde_json::from_str::<Value>(raw)
        .map_err(|error| format!("历史 agent 状态不是有效 JSON：{error}"))?;
    rewrite_exact_ids(&mut value, replacements);
    let object = value
        .as_object_mut()
        .ok_or_else(|| "历史 agent 状态必须是 JSON 对象。".to_string())?;
    object.insert(
        "usage".to_string(),
        json!({
            "inputTokens": 0,
            "outputTokens": 0,
            "outputThinkingTokens": 0,
            "totalTokens": 0,
            "cachedInputTokens": 0,
            "cacheCreationInputTokens": 0,
            "billableRequestCount": 0
        }),
    );
    object.insert("messageStreamCheckpoints".to_string(), json!({}));
    if let Some(state) = object.get_mut("state").and_then(Value::as_object_mut) {
        state.insert("activeRunId".to_string(), Value::Null);
    }
    serde_json::to_string(&value).map_err(|error| format!("无法序列化复制后的 agent 状态：{error}"))
}

fn rewrite_exact_ids(value: &mut Value, replacements: &HashMap<String, String>) {
    match value {
        Value::String(current) => {
            if let Some(replacement) = replacements.get(current) {
                *current = replacement.clone();
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_exact_ids(value, replacements);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_exact_ids(value, replacements);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn agent_run_id(raw: Option<&str>) -> Option<String> {
    raw.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value
                .get("runId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn agent_run_status(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = serde_json::from_str::<Value>(raw)
        .map_err(|error| format!("历史 agent 状态不是有效 JSON：{error}"))?;
    match value.get("status") {
        Some(Value::String(status)) => Ok(Some(status.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err("历史 agent 状态的 status 字段无效。".to_string()),
    }
}

fn insert_conversation(
    connection: &Connection,
    target: &ChatConversationRecord,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL)",
            params![
                &target.id,
                &target.project_id,
                &target.model_id,
                &target.title,
                target.created_at,
                target.updated_at,
            ],
        )
        .map_err(database_error)?;
    for (position, message) in target.messages.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    ui_state_json, created_at, position
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &message.id,
                    &target.id,
                    &message.role,
                    &message.content,
                    &message.status,
                    &message.agent_run_json,
                    &message.ui_state_json,
                    message.created_at,
                    position as i64,
                ],
            )
            .map_err(database_error)?;
    }
    Ok(())
}

fn clone_summary_chain(
    connection: &Connection,
    plan: &ConversationForkPlan,
) -> Result<HashMap<String, String>, String> {
    if plan.summaries.is_empty() {
        return Ok(HashMap::new());
    }
    let mut summary_id_map = HashMap::new();
    let mut latest_summary_id = None;
    for version in &plan.summaries {
        let source = &version.summary;
        let summary_id = new_id("context-summary");
        let covered_through = remap_cursor(&source.covered_through, &plan.message_id_map)?;
        let continuity = remap_continuity(
            &source.continuity,
            &plan.message_id_map,
            &plan.id_replacements,
        )?;
        let previous_summary_id = source
            .previous_summary_id
            .as_ref()
            .map(|previous| mapped_id(&summary_id_map, previous, "上一版摘要"))
            .transpose()?;
        let source_revision = context_compaction_repository::source_revision_for_cursor(
            connection,
            &plan.target.id,
            &covered_through,
        )
        .map_err(|error| error.to_string())?;
        let summary = ContextCompactionSummary {
            schema_version: source.schema_version,
            id: summary_id.clone(),
            conversation_id: plan.target.id.clone(),
            source_revision,
            previous_summary_id,
            covered_through,
            content: source.content.clone(),
            continuity,
            generation: source.generation.clone(),
            source_input_tokens: source.source_input_tokens,
            summary_input_tokens: source.summary_input_tokens,
            continuity_input_tokens: source.continuity_input_tokens,
            uncovered_tail_input_tokens: source.uncovered_tail_input_tokens,
            replacement_input_tokens: source.replacement_input_tokens,
            created_at: source.created_at,
        };
        context_compaction_repository::insert_summary(connection, &summary)
            .map_err(|error| error.to_string())?;
        context_compaction_repository::insert_summary_lineage(
            connection,
            &summary,
            &context_compaction_repository::ContextCompactionSummaryLineage {
                introduced_by_assistant_message_id: mapped_id(
                    &plan.message_id_map,
                    &version.lineage.introduced_by_assistant_message_id,
                    "摘要生成回复",
                )?,
                source_conversation_id: Some(plan.source_conversation_id.clone()),
                source_summary_id: Some(source.id.clone()),
            },
        )
        .map_err(|error| error.to_string())?;
        summary_id_map.insert(source.id.clone(), summary_id.clone());
        latest_summary_id = Some(summary_id);
    }
    context_compaction_repository::set_active_summary_head(
        connection,
        &plan.target.id,
        latest_summary_id
            .as_deref()
            .expect("non-empty summary chain has a head"),
        plan.summaries.len() as u64,
        plan.target.created_at,
    )
    .map_err(|error| error.to_string())?;
    Ok(summary_id_map)
}

fn clone_provider_transition_receipts(
    connection: &Connection,
    plan: &ConversationForkPlan,
    summary_id_map: &HashMap<String, String>,
) -> Result<(), String> {
    for copy in &plan.provider_transition_receipts {
        let source_receipt = &copy.source_receipt;
        let source_summary_id = source_receipt
            .summary_id
            .as_deref()
            .ok_or_else(|| "Provider transition receipt 缺少摘要。".to_string())?;
        let target_summary_id = mapped_id(
            summary_id_map,
            source_summary_id,
            "Provider transition 摘要",
        )?;
        let target_assistant_message_id = mapped_id(
            &plan.message_id_map,
            &source_receipt.assistant_message_id,
            "Provider transition 所属消息",
        )?;
        let target_covered_through =
            remap_cursor(&source_receipt.plan.covered_through, &plan.message_id_map)?;
        let target_previous_summary_id = source_receipt
            .plan
            .previous_summary_id
            .as_ref()
            .map(|summary_id| {
                mapped_id(
                    summary_id_map,
                    summary_id,
                    "上一版 Provider transition 摘要",
                )
            })
            .transpose()?;
        let target_source_revision = context_compaction_repository::source_revision_for_cursor(
            connection,
            &plan.target.id,
            &target_covered_through,
        )
        .map_err(|error| error.to_string())?;

        let mut receipt = source_receipt.clone();
        receipt.operation_id = copy.target_operation_id.clone();
        receipt.run_id = copy.target_run_id.clone();
        receipt.conversation_id = plan.target.id.clone();
        receipt.assistant_message_id = target_assistant_message_id.clone();
        receipt.plan.context_revision = target_source_revision.clone();
        receipt.plan.persistent_revision = target_source_revision.clone();
        receipt.plan.previous_summary_id = target_previous_summary_id;
        receipt.plan.covered_through = target_covered_through;
        receipt.source_revision = Some(target_source_revision.clone());
        receipt.generation_observation_id = Some(copy.target_observation_id.clone());
        receipt.summary_id = Some(target_summary_id.clone());
        receipt
            .result
            .as_mut()
            .ok_or_else(|| "Provider transition receipt 缺少应用结果。".to_string())?
            .summary_id = target_summary_id;

        let mut observation = copy.source_observation.clone();
        observation.id = copy.target_observation_id.clone();
        observation.run_id = copy.target_run_id.clone();
        observation.conversation_id = Some(plan.target.id.clone());
        observation.assistant_message_id = Some(target_assistant_message_id);
        observation.operation_id = Some(copy.target_operation_id.clone());
        if let Some(estimate) = observation.estimate.as_mut() {
            estimate.context_revision = target_source_revision.clone();
            estimate.persistent_revision = target_source_revision;
        }
        receipt
            .validate_generation_observation(&observation)
            .map_err(|error| error.to_string())?;
        receipt.validate().map_err(|error| error.to_string())?;

        // Use the same receipt state machine as a live transition. The planned row and terminal
        // update remain inside the fork transaction, so readers can never observe a half-cloned
        // transition while recursive forks still receive a fully valid operation boundary.
        let mut planned = receipt.clone();
        planned.status = ContextCompactionReceiptStatus::InProgress;
        planned.stage = ContextCompactionReceiptStage::Planned;
        planned.source_revision = None;
        planned.generation_observation_id = None;
        planned.summary_id = None;
        planned.result = None;
        planned.error = None;
        planned.updated_at = planned.started_at;
        planned.completed_at = None;
        planned.validate().map_err(|error| error.to_string())?;
        context_compaction_receipt_repository::record_receipt_in_connection(
            connection, &planned, None,
        )
        .map_err(|error| error.to_string())?;
        context_compaction_receipt_repository::record_receipt_in_connection(
            connection,
            &receipt,
            Some(&observation),
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn clone_world_state_records(
    connection: &Connection,
    plan: &ConversationForkPlan,
    summary_id_map: &HashMap<String, String>,
) -> Result<(), String> {
    let Some(first) = plan.world_state_records.first() else {
        return Ok(());
    };
    let source_base_summary_id = first.base_summary_id.as_deref();
    let target_base_summary_id = source_base_summary_id
        .map(|summary_id| mapped_id(summary_id_map, summary_id, "World State 基础摘要"))
        .transpose()?;

    for (index, entry) in plan.world_state_records.iter().enumerate() {
        if entry.conversation_id != plan.source_conversation_id
            || entry.epoch_generation != first.epoch_generation
            || entry.base_summary_id.as_deref() != source_base_summary_id
            || entry.record.epoch_id() != first.record.epoch_id()
            || entry.record.sequence() != index as u64
        {
            return Err("待克隆的 active World State epoch 不是连续且一致的前缀。".to_string());
        }
        let target_anchor = entry
            .effective_before_message_id
            .as_ref()
            .map(|message_id| mapped_id(&plan.message_id_map, message_id, "World State anchor"))
            .transpose()?;
        let outcome = world_state_repository::append_record_in_connection(
            connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &plan.target.id,
                epoch_generation: 1,
                base_summary_id: target_base_summary_id.as_deref(),
                effective_before_message_id: target_anchor.as_deref(),
                record: &entry.record,
                created_at: entry.created_at,
            },
        )
        .map_err(|error| error.to_string())?;
        if outcome != world_state_repository::ConversationWorldStateAppendOutcome::Inserted {
            return Err("新任务 World State 出现意外的幂等写入。".to_string());
        }
    }
    Ok(())
}

fn remap_continuity(
    source: &ContextContinuitySnapshot,
    message_id_map: &HashMap<String, String>,
    replacements: &HashMap<String, String>,
) -> Result<ContextContinuitySnapshot, String> {
    let remap_refs = |refs: &[ContextHistoryRef]| {
        refs.iter()
            .map(|reference| remap_history_ref(reference, message_id_map, replacements))
            .collect::<Result<Vec<_>, String>>()
    };
    let snapshot = ContextContinuitySnapshot {
        schema_version: source.schema_version,
        covered_through: remap_cursor(&source.covered_through, message_id_map)?,
        task_evidence_refs: remap_refs(&source.task_evidence_refs)?,
        unresolved_failure_refs: remap_refs(&source.unresolved_failure_refs)?,
        approval_refs: remap_refs(&source.approval_refs)?,
        important_decision_refs: remap_refs(&source.important_decision_refs)?,
        recent_refs: remap_refs(&source.recent_refs)?,
        archived_counts: source.archived_counts.clone(),
    };
    snapshot.validate().map_err(|error| error.to_string())?;
    Ok(snapshot)
}

fn remap_history_ref(
    reference: &ContextHistoryRef,
    message_id_map: &HashMap<String, String>,
    replacements: &HashMap<String, String>,
) -> Result<ContextHistoryRef, String> {
    match reference {
        ContextHistoryRef::Message { message_id } => Ok(ContextHistoryRef::message(mapped_id(
            message_id_map,
            message_id,
            "Continuity 消息引用",
        )?)),
        ContextHistoryRef::TraceItem {
            assistant_message_id,
            sequence,
        } => Ok(ContextHistoryRef::trace_item(
            mapped_id(
                message_id_map,
                assistant_message_id,
                "Continuity trace 引用",
            )?,
            *sequence,
        )),
        ContextHistoryRef::Archive { archive_ref } => Ok(ContextHistoryRef::archive(mapped_id(
            replacements,
            archive_ref,
            "Continuity Archive 引用",
        )?)),
    }
}

fn rewrite_trace_items(
    trace: &mut ConversationTurnTrace,
    replacements: &HashMap<String, String>,
) -> Result<(), String> {
    let mut value = serde_json::to_value(&trace.items)
        .map_err(|error| format!("无法序列化历史工具轨迹：{error}"))?;
    rewrite_exact_ids(&mut value, replacements);
    rewrite_history_open_tokens(&mut value, replacements)?;
    trace.items = serde_json::from_value(value)
        .map_err(|error| format!("无法重建复制后的历史工具轨迹：{error}"))?;
    trace.validate().map_err(|error| error.to_string())
}

fn rewrite_model_context_items(
    items: &mut Vec<ConversationModelContextItem>,
    replacements: &HashMap<String, String>,
) -> Result<(), String> {
    let mut value = serde_json::to_value(&*items)
        .map_err(|error| format!("无法序列化历史模型上下文：{error}"))?;
    rewrite_exact_ids(&mut value, replacements);
    rewrite_history_open_tokens(&mut value, replacements)?;
    *items = serde_json::from_value(value)
        .map_err(|error| format!("无法重建复制后的历史模型上下文：{error}"))?;
    for item in items {
        item.validate()
            .map_err(|error| format!("复制后的历史模型上下文无效：{error}"))?;
    }
    Ok(())
}

fn rewrite_history_open_tokens(
    value: &mut Value,
    replacements: &HashMap<String, String>,
) -> Result<(), String> {
    match value {
        Value::String(current) => {
            *current = rewritten_history_open_tokens(current, replacements)?;
        }
        Value::Array(values) => {
            for value in values {
                rewrite_history_open_tokens(value, replacements)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_history_open_tokens(value, replacements)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn rewritten_history_open_tokens(
    value: &str,
    replacements: &HashMap<String, String>,
) -> Result<String, String> {
    let prefix = conversation_history_open::HISTORY_OPEN_PREFIX;
    let Some(first_match) = value.find(prefix) else {
        return Ok(value.to_string());
    };
    let mut rewritten = String::with_capacity(value.len());
    rewritten.push_str(&value[..first_match]);
    let mut cursor = first_match;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find(prefix) else {
            rewritten.push_str(&value[cursor..]);
            break;
        };
        let start = cursor.saturating_add(relative_start);
        rewritten.push_str(&value[cursor..start]);
        let encoded_start = start.saturating_add(prefix.len());
        let encoded_len = value[encoded_start..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            .count();
        if encoded_len == 0 {
            rewritten.push_str(prefix);
            cursor = encoded_start;
            continue;
        }
        let end = encoded_start.saturating_add(encoded_len);
        let token = &value[start..end];
        if let Some(remapped) = conversation_history_open::remap_history_open(token, replacements)?
        {
            rewritten.push_str(&remapped);
        } else {
            rewritten.push_str(token);
        }
        cursor = end;
    }
    Ok(rewritten)
}

fn remap_cursor(
    cursor: &ContextJournalCursor,
    message_id_map: &HashMap<String, String>,
) -> Result<ContextJournalCursor, String> {
    match cursor {
        ContextJournalCursor::Message { message_id } => Ok(ContextJournalCursor::message(
            mapped_id(message_id_map, message_id, "摘要消息游标")?,
        )),
        ContextJournalCursor::TraceItem {
            assistant_message_id,
            sequence,
        } => Ok(ContextJournalCursor::trace_item(
            mapped_id(message_id_map, assistant_message_id, "摘要 trace 游标")?,
            *sequence,
        )),
    }
}

fn mapped_id(
    mapping: &HashMap<String, String>,
    source: &str,
    label: &str,
) -> Result<String, String> {
    mapping
        .get(source)
        .cloned()
        .ok_or_else(|| format!("{label}未包含在分叉快照中：{source}"))
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

fn database_error(error: rusqlite::Error) -> String {
    format!("本地数据库操作失败：{error}")
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn cloned_agent_usage_is_zero_and_ids_are_rewritten() {
        let replacements = HashMap::from([
            ("run-old".to_string(), "run-new".to_string()),
            ("message-old".to_string(), "message-new".to_string()),
        ]);
        let cloned = clone_agent_run_json(
            &json!({
                "runId": "run-old",
                "assistantMessageId": "message-old",
                "usage": { "inputTokens": 10, "totalTokens": 12 },
                "messageStreamCheckpoints": { "1": "partial" },
                "state": { "activeRunId": "run-old", "status": "completed" }
            })
            .to_string(),
            &replacements,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&cloned).unwrap();
        assert_eq!(value["runId"], "run-new");
        assert_eq!(value["assistantMessageId"], "message-new");
        assert_eq!(value["usage"]["inputTokens"], 0);
        assert_eq!(value["usage"]["billableRequestCount"], 0);
        assert_eq!(value["state"]["activeRunId"], Value::Null);
        assert_eq!(value["messageStreamCheckpoints"], json!({}));
    }

    #[test]
    fn unsettled_assistant_state_cannot_enter_a_fork_snapshot() {
        let mut message = ChatMessageRecord {
            id: "assistant-running".to_string(),
            role: "assistant".to_string(),
            content: "partial".to_string(),
            created_at: 1,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: Some(json!({ "status": "waiting_for_approval" }).to_string()),
            ui_state_json: None,
        };
        assert!(ensure_settled_assistant(&message, None).is_err());

        message.agent_run_json = Some(json!({ "status": "completed" }).to_string());
        assert!(ensure_settled_assistant(&message, None).is_ok());

        let stale_trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-stale".to_string(),
            conversation_id: "conversation-stale".to_string(),
            assistant_message_id: message.id.clone(),
            terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
            terminal_error: None,
            truncated: false,
            items: Vec::new(),
        };
        assert!(
            ensure_settled_assistant(&message, Some(&stale_trace)).is_err(),
            "a renderer-owned completed status must not bypass an in-progress backend trace"
        );
    }

    #[test]
    fn fork_with_released_provider_history_is_marked_for_safe_adaptation() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let source = source_conversation();
        chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
        let record = provider_continuation_repository::ProviderContinuationEnvelopeRecord {
            continuation_id: format!(
                "{}{}",
                provider_continuation_repository::PROVIDER_CONTINUATION_REF_PREFIX,
                uuid::Uuid::new_v4().hyphenated()
            ),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-a".to_string(),
            run_id: "run-source-0".to_string(),
            request_index: 0,
            assistant_turn_id: format!("at1_{}", "a".repeat(64)),
            assistant_turn_digest: format!("sha256:{}", "b".repeat(64)),
            provider_protocol_digest: format!("sha256:{}", "c".repeat(64)),
            payload_digest: format!("sha256:{}", "d".repeat(64)),
            nonce: vec![1; 12],
            ciphertext: vec![2; 17],
            decoded_bytes: 1,
            compressed_bytes: 1,
            created_at: 2,
            runtime_tool_calls: vec![
                provider_continuation_repository::ProviderContinuationRuntimeToolIdentity {
                    provider_tool_index: 0,
                    runtime_call_id: "call-source-a".to_string(),
                },
            ],
        };
        provider_continuation_repository::store_active_in_connection(&connection, &record).unwrap();
        assert_eq!(
            provider_continuation_repository::release(&connection, &record.continuation_id, 10)
                .unwrap(),
            provider_continuation_repository::ProviderContinuationReleaseOutcome::Released
        );

        let plan = build_assistant_reply_fork_plan(
            &connection,
            "fork-released-provider-history",
            &source.id,
            "assistant-a",
            20,
        )
        .unwrap();
        assert!(plan.requires_context_adaptation);
        assert!(plan.provider_continuation_mappings.is_empty());
        commit_fork_plan(&mut connection, &plan).unwrap();
        assert!(
            conversation_context_adaptation_repository::get(&connection, &plan.target.id,)
                .unwrap()
                .is_some()
        );

        let recursive = build_assistant_reply_fork_plan(
            &connection,
            "fork-released-provider-history-recursive",
            &plan.target.id,
            &plan.message_id_map["assistant-a"],
            21,
        )
        .unwrap();
        assert!(recursive.requires_context_adaptation);
        assert!(recursive.provider_continuation_mappings.is_empty());
    }

    #[test]
    fn fork_omits_released_turn_only_when_the_selected_summary_covers_its_complete_exchange() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let source = source_conversation();
        chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-0".to_string(),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-a".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-source-a".to_string(),
                    tool: "web_fetch".to_string(),
                    provenance: crate::AgentToolIdentity::Builtin {
                        tool_name: "web_fetch".to_string(),
                    },
                    operation: json!({ "url": "https://example.com" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-source-a".to_string(),
                    tool: "web_fetch".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "content": "complete" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                    archive: Default::default(),
                },
            ],
        };
        conversation_trace_repository::replace_trace(&mut connection, &trace, 2, 3).unwrap();
        let record = provider_continuation_repository::ProviderContinuationEnvelopeRecord {
            continuation_id: format!(
                "{}{}",
                provider_continuation_repository::PROVIDER_CONTINUATION_REF_PREFIX,
                uuid::Uuid::new_v4().hyphenated()
            ),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-a".to_string(),
            run_id: "run-source-0".to_string(),
            request_index: 0,
            assistant_turn_id: format!("at1_{}", "a".repeat(64)),
            assistant_turn_digest: format!("sha256:{}", "b".repeat(64)),
            provider_protocol_digest: format!("sha256:{}", "c".repeat(64)),
            payload_digest: format!("sha256:{}", "d".repeat(64)),
            nonce: vec![1; 12],
            ciphertext: vec![2; 17],
            decoded_bytes: 1,
            compressed_bytes: 1,
            created_at: 2,
            runtime_tool_calls: vec![
                provider_continuation_repository::ProviderContinuationRuntimeToolIdentity {
                    provider_tool_index: 0,
                    runtime_call_id: "call-source-a".to_string(),
                },
            ],
        };
        provider_continuation_repository::store_active_in_connection(&connection, &record).unwrap();
        let prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::trace_item("assistant-a", 1),
        )
        .unwrap();
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            &prefix,
            summary_draft(&prefix, "summary-covers-provider-a", 20),
            "assistant-b",
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM provider_continuations WHERE continuation_id = ?1",
                    [&record.continuation_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "released"
        );

        let before_summary = build_assistant_reply_fork_plan(
            &connection,
            "fork-before-provider-summary",
            &source.id,
            "assistant-a",
            30,
        )
        .unwrap();
        assert!(before_summary.requires_context_adaptation);
        assert!(before_summary.summaries.is_empty());
        assert!(before_summary.provider_continuation_mappings.is_empty());

        let after_summary = build_assistant_reply_fork_plan(
            &connection,
            "fork-after-provider-summary",
            &source.id,
            "assistant-b",
            40,
        )
        .unwrap();
        assert_eq!(after_summary.summaries.len(), 1);
        assert!(!after_summary.requires_context_adaptation);
        assert!(after_summary.provider_continuation_mappings.is_empty());
        commit_fork_plan(&mut connection, &after_summary).unwrap();
        assert_eq!(
            context_compaction_repository::list_active_summary_chain(
                &connection,
                &after_summary.target.id,
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn explicit_timeline_points_separate_reply_from_transition_and_pin_boundary_model() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let mut source = source_conversation();
        source.model_id = Some("model-c".to_string());
        chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
        record_agent_loop_model(
            &connection,
            &source.id,
            "assistant-a",
            "run-source-0",
            "model-a",
            2,
        );

        let first_prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::message("assistant-a"),
        )
        .unwrap();
        record_provider_transition_summary(
            &mut connection,
            &first_prefix,
            "summary-transition-b",
            "provider-transition-a-to-b",
            "assistant-a",
            "model-b",
            20,
        );
        let second_prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::message("assistant-b"),
        )
        .unwrap();
        record_provider_transition_summary(
            &mut connection,
            &second_prefix,
            "summary-transition-c",
            "provider-transition-b-to-c",
            "assistant-b",
            "model-c",
            30,
        );

        let before_divider = build_fork_plan_at_point(
            &connection,
            "fork-before-divider",
            &source.id,
            &ConversationForkPoint::AssistantReply {
                assistant_message_id: "assistant-a".to_string(),
            },
            40,
        )
        .unwrap();
        assert!(before_divider.summaries.is_empty());
        assert!(before_divider.provider_transition_receipts.is_empty());
        assert_eq!(before_divider.target.model_id.as_deref(), Some("model-a"));

        let after_first_divider = build_fork_plan_at_point(
            &connection,
            "fork-after-first-divider",
            &source.id,
            &ConversationForkPoint::AssistantReply {
                assistant_message_id: "assistant-b".to_string(),
            },
            40,
        )
        .unwrap();
        assert_eq!(after_first_divider.summaries.len(), 1);
        assert_eq!(after_first_divider.provider_transition_receipts.len(), 1);

        let at_second_divider = build_fork_plan_at_point(
            &connection,
            "fork-at-second-divider",
            &source.id,
            &ConversationForkPoint::ProviderTransitionBoundary {
                operation_id: "provider-transition-b-to-c".to_string(),
            },
            40,
        )
        .unwrap();
        assert_eq!(at_second_divider.summaries.len(), 2);
        assert_eq!(at_second_divider.provider_transition_receipts.len(), 2);

        let at_first_divider = build_fork_plan_at_point(
            &connection,
            "fork-at-first-divider",
            &source.id,
            &ConversationForkPoint::ProviderTransitionBoundary {
                operation_id: "provider-transition-a-to-b".to_string(),
            },
            41,
        )
        .unwrap();
        assert_eq!(at_first_divider.summaries.len(), 1);
        assert_eq!(
            at_first_divider.summaries[0].summary.id,
            "summary-transition-b"
        );
        assert_eq!(at_first_divider.target.model_id.as_deref(), Some("model-b"));
        assert_eq!(at_first_divider.target.messages.len(), 2);
        assert_eq!(at_first_divider.provider_transition_receipts.len(), 1);

        commit_fork_plan(&mut connection, &at_first_divider).unwrap();
        let target_chain = context_compaction_repository::list_active_summary_chain(
            &connection,
            &at_first_divider.target.id,
        )
        .unwrap();
        assert_eq!(target_chain.len(), 1);
        assert_eq!(
            target_chain[0].lineage.source_summary_id.as_deref(),
            Some("summary-transition-b")
        );
        let cloned_receipts =
            context_compaction_receipt_repository::list_provider_transition_receipts(
                &connection,
                &at_first_divider.target.id,
                None,
                100,
            )
            .unwrap();
        assert_eq!(cloned_receipts.len(), 1);
        let cloned_receipt = &cloned_receipts[0];
        assert_ne!(cloned_receipt.operation_id, "provider-transition-a-to-b");
        assert_eq!(cloned_receipt.conversation_id, at_first_divider.target.id);
        assert_eq!(
            cloned_receipt.assistant_message_id,
            at_first_divider.message_id_map["assistant-a"]
        );
        assert_eq!(
            cloned_receipt.summary_id.as_deref(),
            Some(target_chain[0].summary.id.as_str())
        );
        assert_eq!(
            cloned_receipt.plan.covered_through,
            ContextJournalCursor::message(&at_first_divider.message_id_map["assistant-a"])
        );
        let cloned_observation = model_request_observation_repository::get_observation(
            &connection,
            cloned_receipt
                .generation_observation_id
                .as_deref()
                .expect("cloned receipt observation"),
        )
        .unwrap()
        .expect("cloned provider transition observation");
        cloned_receipt
            .validate_generation_observation(&cloned_observation)
            .unwrap();
        let continuation_origin = get_continuation_origin(&connection, &at_first_divider.target.id)
            .unwrap()
            .expect("fork continuation origin");
        assert_eq!(
            continuation_origin.boundary_message_id,
            at_first_divider.message_id_map["assistant-a"]
        );
        assert!(conversation_context_adaptation_repository::get(
            &connection,
            &at_first_divider.target.id,
        )
        .unwrap()
        .is_none());

        let recursive = build_fork_plan_at_point(
            &connection,
            "fork-recursive-provider-divider",
            &at_first_divider.target.id,
            &ConversationForkPoint::ProviderTransitionBoundary {
                operation_id: cloned_receipt.operation_id.clone(),
            },
            50,
        )
        .unwrap();
        assert_eq!(recursive.summaries.len(), 1);
        assert_eq!(recursive.provider_transition_receipts.len(), 1);
        assert_eq!(recursive.target.model_id.as_deref(), Some("model-b"));
        commit_fork_plan(&mut connection, &recursive).unwrap();
        assert_eq!(
            context_compaction_receipt_repository::list_provider_transition_receipts(
                &connection,
                &recursive.target.id,
                None,
                100,
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn fork_clones_only_causally_visible_summary_history_and_remains_recursive() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let source = source_conversation();
        chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
        for (index, message) in source
            .messages
            .iter()
            .filter(|message| message.role == "assistant")
            .enumerate()
        {
            let trace = ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: format!("run-source-{index}"),
                conversation_id: source.id.clone(),
                assistant_message_id: message.id.clone(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![ConversationTurnTraceItem::AssistantNarration {
                    sequence: 0,
                    content: format!("narration {index}"),
                    truncated: false,
                }],
            };
            conversation_trace_repository::replace_trace(
                &mut connection,
                &trace,
                message.created_at,
                message.created_at + 1,
            )
            .unwrap();
        }
        file_draft_repository::insert_draft(
            &connection,
            &AgentFileDraftRecord {
                id: "draft-source".to_string(),
                conversation_id: source.id.clone(),
                project_id: None,
                run_id: "run-source-1".to_string(),
                file_path: "notes.md".to_string(),
                mode: "update".to_string(),
                status: "applied".to_string(),
                base_revision: Some("revision-old".to_string()),
                base_content: "before\n".to_string(),
                content: "after\n".to_string(),
                additions: 1,
                deletions: 1,
                line_count: 1,
                byte_count: 6,
                chunk_count: 1,
                next_chunk_index: 1,
                stats_final: true,
                summary: Some("updated notes".to_string()),
                final_action_id: None,
                created_at: 4,
                updated_at: 4,
                expires_at: i64::MAX,
            },
        )
        .unwrap();

        let first_prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::message("user-b"),
        )
        .unwrap();
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            &first_prefix,
            summary_draft(&first_prefix, "summary-1", 20),
            "assistant-b",
        )
        .unwrap();
        let second_prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::message("user-d"),
        )
        .unwrap();
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            &second_prefix,
            summary_draft(&second_prefix, "summary-2", 40),
            "assistant-d",
        )
        .unwrap();

        let plan = build_assistant_reply_fork_plan(
            &connection,
            "fork-at-c",
            &source.id,
            "assistant-c",
            100,
        )
        .unwrap();
        assert_eq!(plan.target.messages.len(), 6);
        assert_eq!(plan.summaries.len(), 1);
        assert_eq!(plan.file_drafts.len(), 1);
        assert_ne!(plan.file_drafts[0].id, "draft-source");
        commit_fork_plan(&mut connection, &plan).unwrap();

        let target_chain =
            context_compaction_repository::list_active_summary_chain(&connection, &plan.target.id)
                .unwrap();
        assert_eq!(target_chain.len(), 1);
        assert_eq!(
            target_chain[0].lineage.source_summary_id.as_deref(),
            Some("summary-1")
        );
        assert_eq!(
            target_chain[0].lineage.introduced_by_assistant_message_id,
            plan.message_id_map["assistant-b"]
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM context_compaction_receipts WHERE conversation_id = ?1",
                    [&plan.target.id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM model_request_observations WHERE conversation_id = ?1",
                    [&plan.target.id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        for message in &plan.target.messages {
            if message.role != "assistant" {
                continue;
            }
            let run: Value =
                serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
            for field in [
                "inputTokens",
                "outputTokens",
                "outputThinkingTokens",
                "totalTokens",
                "cachedInputTokens",
                "cacheCreationInputTokens",
                "billableRequestCount",
            ] {
                assert_eq!(run["usage"][field], 0, "usage field {field}");
            }
        }

        chat_repository::delete_conversation(&connection, &source.id).unwrap();
        assert!(
            chat_repository::get_conversation(&connection, &plan.target.id)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            context_compaction_repository::list_active_summary_chain(&connection, &plan.target.id,)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            file_draft_repository::list_drafts_for_run(
                &connection,
                &plan.run_id_map["run-source-1"],
            )
            .unwrap()
            .len(),
            1
        );

        let before_summary = build_assistant_reply_fork_plan(
            &connection,
            "recursive-before-summary",
            &plan.target.id,
            &plan.message_id_map["assistant-a"],
            110,
        )
        .unwrap();
        assert!(before_summary.summaries.is_empty());
        commit_fork_plan(&mut connection, &before_summary).unwrap();
        assert!(context_compaction_repository::get_active_summary(
            &connection,
            &before_summary.target.id,
        )
        .unwrap()
        .is_none());

        let after_summary = build_assistant_reply_fork_plan(
            &connection,
            "recursive-after-summary",
            &plan.target.id,
            &plan.message_id_map["assistant-b"],
            120,
        )
        .unwrap();
        assert_eq!(after_summary.summaries.len(), 1);
        commit_fork_plan(&mut connection, &after_summary).unwrap();
        assert_eq!(
            context_compaction_repository::list_active_summary_chain(
                &connection,
                &after_summary.target.id,
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn fork_clones_only_world_state_visible_at_cutoff_and_remaps_anchors_and_summary() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let source = source_conversation();
        chat_repository::save_conversation(&mut connection, source.clone()).unwrap();

        let initial = fork_world_state_snapshot("source-world-state", 0, "initial");
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 1,
                base_summary_id: None,
                effective_before_message_id: None,
                record: &WorldStateRecord::Full(initial),
                created_at: 9,
            },
        )
        .unwrap();
        let prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::message("user-b"),
        )
        .unwrap();
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            &prefix,
            summary_draft(&prefix, "summary-world-state", 20),
            "assistant-b",
        )
        .unwrap();

        let rebased = world_state_repository::fold_active_snapshot(&connection, &source.id)
            .unwrap()
            .unwrap();
        let at_c =
            fork_world_state_snapshot(&rebased.epoch_id, rebased.sequence + 1, "visible-at-c");
        let diff_at_c = WorldStateDiff::between(&rebased, &at_c).unwrap();
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 2,
                base_summary_id: Some("summary-world-state"),
                effective_before_message_id: Some("user-c"),
                record: &WorldStateRecord::Diff(diff_at_c),
                created_at: 21,
            },
        )
        .unwrap();
        let after_cutoff =
            fork_world_state_snapshot(&at_c.epoch_id, at_c.sequence + 1, "after-cutoff");
        let diff_after_cutoff = WorldStateDiff::between(&at_c, &after_cutoff).unwrap();
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 2,
                base_summary_id: Some("summary-world-state"),
                effective_before_message_id: Some("user-d"),
                record: &WorldStateRecord::Diff(diff_after_cutoff),
                created_at: 22,
            },
        )
        .unwrap();

        let plan = build_assistant_reply_fork_plan(
            &connection,
            "fork-world-state-at-c",
            &source.id,
            "assistant-c",
            100,
        )
        .unwrap();
        assert_eq!(plan.summaries.len(), 1);
        assert_eq!(plan.world_state_records.len(), 2);
        assert_eq!(
            plan.world_state_records[1]
                .effective_before_message_id
                .as_deref(),
            Some("user-c")
        );
        commit_fork_plan(&mut connection, &plan).unwrap();

        let target_chain =
            context_compaction_repository::list_active_summary_chain(&connection, &plan.target.id)
                .unwrap();
        assert_eq!(target_chain.len(), 1);
        assert_eq!(
            target_chain[0].lineage.source_summary_id.as_deref(),
            Some("summary-world-state")
        );
        let target_records =
            world_state_repository::list_active_journal_entries(&connection, &plan.target.id)
                .unwrap();
        assert_eq!(target_records.len(), 2);
        assert_eq!(target_records[0].epoch_generation, 1);
        assert_eq!(
            target_records[0].base_summary_id.as_deref(),
            Some(target_chain[0].summary.id.as_str())
        );
        assert_eq!(target_records[0].effective_before_message_id, None);
        assert_eq!(
            target_records[1].effective_before_message_id.as_deref(),
            Some(plan.message_id_map["user-c"].as_str())
        );
        assert!(target_records
            .iter()
            .all(|entry| entry.effective_before_message_id.as_deref() != Some("user-d")));
        assert_eq!(
            world_state_repository::fold_active_snapshot(&connection, &plan.target.id)
                .unwrap()
                .unwrap()
                .revision,
            at_c.revision
        );
    }

    #[test]
    fn fork_selects_the_latest_world_state_epoch_whose_summary_is_visible_at_cutoff() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let source = source_conversation();
        chat_repository::save_conversation(&mut connection, source.clone()).unwrap();

        let initial = fork_world_state_snapshot("world-state-epoch-1", 0, "initial");
        let at_b = fork_world_state_snapshot("world-state-epoch-1", 1, "at-b");
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 1,
                base_summary_id: None,
                effective_before_message_id: None,
                record: &WorldStateRecord::Full(initial.clone()),
                created_at: 9,
            },
        )
        .unwrap();
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 1,
                base_summary_id: None,
                effective_before_message_id: Some("user-b"),
                record: &WorldStateRecord::Diff(WorldStateDiff::between(&initial, &at_b).unwrap()),
                created_at: 10,
            },
        )
        .unwrap();

        let first_prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::message("user-b"),
        )
        .unwrap();
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            &first_prefix,
            summary_draft(&first_prefix, "summary-world-state-1", 20),
            "assistant-b",
        )
        .unwrap();
        let epoch_two = world_state_repository::fold_active_snapshot(&connection, &source.id)
            .unwrap()
            .unwrap();
        assert_eq!(epoch_two.revision, at_b.revision);
        let at_c = fork_world_state_snapshot(&epoch_two.epoch_id, epoch_two.sequence + 1, "at-c");
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &source.id,
                epoch_generation: 2,
                base_summary_id: Some("summary-world-state-1"),
                effective_before_message_id: Some("user-c"),
                record: &WorldStateRecord::Diff(
                    WorldStateDiff::between(&epoch_two, &at_c).unwrap(),
                ),
                created_at: 21,
            },
        )
        .unwrap();

        let second_prefix = context_compaction_repository::prepare_prefix(
            &connection,
            &source.id,
            &ContextJournalCursor::message("user-d"),
        )
        .unwrap();
        context_compaction_repository::commit_prefix_replacement(
            &mut connection,
            &second_prefix,
            summary_draft(&second_prefix, "summary-world-state-2", 40),
            "assistant-d",
        )
        .unwrap();
        let active =
            world_state_repository::list_active_journal_entries(&connection, &source.id).unwrap();
        assert_eq!(active[0].epoch_generation, 3);
        assert_eq!(
            active[0].base_summary_id.as_deref(),
            Some("summary-world-state-2")
        );

        let early = build_assistant_reply_fork_plan(
            &connection,
            "fork-before-all-summaries",
            &source.id,
            "assistant-a",
            100,
        )
        .unwrap();
        assert!(early.summaries.is_empty());
        assert_eq!(early.world_state_records.len(), 1);
        assert_eq!(early.world_state_records[0].epoch_generation, 1);
        assert_eq!(early.world_state_records[0].base_summary_id, None);
        assert_eq!(
            early.world_state_records[0].record.revision(),
            initial.revision
        );
        commit_fork_plan(&mut connection, &early).unwrap();
        assert_eq!(
            world_state_repository::fold_active_snapshot(&connection, &early.target.id)
                .unwrap()
                .unwrap()
                .revision,
            initial.revision
        );

        let middle = build_assistant_reply_fork_plan(
            &connection,
            "fork-between-world-state-summaries",
            &source.id,
            "assistant-c",
            110,
        )
        .unwrap();
        assert_eq!(middle.summaries.len(), 1);
        assert_eq!(middle.world_state_records.len(), 2);
        assert!(middle
            .world_state_records
            .iter()
            .all(|entry| entry.epoch_generation == 2));
        assert_eq!(
            middle.world_state_records[0].base_summary_id.as_deref(),
            Some("summary-world-state-1")
        );
        assert_eq!(
            middle.world_state_records[1]
                .effective_before_message_id
                .as_deref(),
            Some("user-c")
        );
        commit_fork_plan(&mut connection, &middle).unwrap();
        let middle_target =
            world_state_repository::list_active_journal_entries(&connection, &middle.target.id)
                .unwrap();
        assert_eq!(middle_target.len(), 2);
        assert_eq!(
            middle_target[0].base_summary_id.as_deref(),
            context_compaction_repository::get_active_summary(&connection, &middle.target.id)
                .unwrap()
                .as_ref()
                .map(|summary| summary.id.as_str())
        );
        assert_eq!(
            middle_target[1].effective_before_message_id.as_deref(),
            Some(middle.message_id_map["user-c"].as_str())
        );
        assert_eq!(
            world_state_repository::fold_active_snapshot(&connection, &middle.target.id)
                .unwrap()
                .unwrap()
                .revision,
            at_c.revision
        );
    }

    fn fork_world_state_snapshot(epoch_id: &str, sequence: u64, value: &str) -> WorldStateSnapshot {
        WorldStateSnapshot::new(
            epoch_id,
            sequence,
            vec![WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::EffectivePermissions,
                WorldStateLifetime::Conversation,
                json!({ "value": value }),
                json!({ "value": value }),
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn source_conversation() -> ChatConversationRecord {
        let messages = [
            ("user-a", "user", 1),
            ("assistant-a", "assistant", 2),
            ("user-b", "user", 3),
            ("assistant-b", "assistant", 4),
            ("user-c", "user", 5),
            ("assistant-c", "assistant", 6),
            ("user-d", "user", 7),
            ("assistant-d", "assistant", 8),
        ]
        .into_iter()
        .map(|(id, role, created_at)| ChatMessageRecord {
            id: id.to_string(),
            role: role.to_string(),
            content: format!("content {id}"),
            created_at,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: (role == "assistant").then(|| {
                json!({
                    "runId": format!("run-source-{}", (created_at / 2) - 1),
                    "status": "completed",
                    "toolDefinitions": [],
                    "toolCalls": [],
                    "toolResults": [],
                    "approvals": [],
                    "diffs": [],
                    "timeline": [],
                    "usage": {
                        "inputTokens": 100,
                        "outputTokens": 20,
                        "outputThinkingTokens": 5,
                        "totalTokens": 125,
                        "cachedInputTokens": 10,
                        "cacheCreationInputTokens": 2,
                        "billableRequestCount": 1
                    }
                })
                .to_string()
            }),
            ui_state_json: None,
        })
        .collect();
        ChatConversationRecord {
            id: "conversation-source".to_string(),
            project_id: None,
            model_id: Some("model-1".to_string()),
            title: "Source task".to_string(),
            messages,
            created_at: 1,
            updated_at: 8,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        }
    }

    fn summary_draft(
        prefix: &crate::ContextCompactionPrefix,
        id: &str,
        created_at: i64,
    ) -> ContextCompactionSummaryDraft {
        ContextCompactionSummaryDraft {
            id: id.to_string(),
            source_revision: prefix.source_revision.clone(),
            content: format!("summary {id}"),
            continuity: ContextContinuitySnapshot::from_prefix(prefix).unwrap(),
            generation: ContextCompactionGeneration::test(),
            source_input_tokens: 1_000,
            summary_input_tokens: 100,
            continuity_input_tokens: 100,
            uncovered_tail_input_tokens: 0,
            replacement_input_tokens: 200,
            created_at,
        }
    }

    fn record_agent_loop_model(
        connection: &Connection,
        conversation_id: &str,
        assistant_message_id: &str,
        run_id: &str,
        model_id: &str,
        completed_at: i64,
    ) {
        let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
            format!("observation-{assistant_message_id}"),
            run_id,
            Some(conversation_id.to_string()),
            Some(assistant_message_id.to_string()),
            None,
            1,
            crate::ModelRequestPurpose::AgentLoop,
            model_id,
            crate::AgentApiStyle::OpenAiCompatible,
            None,
            completed_at.saturating_sub(1),
        )
        .completed(None, Some("stop".to_string()), completed_at)
        .unwrap();
        crate::storage::model_request_observation_repository::insert_observation(
            connection,
            &observation,
        )
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn record_provider_transition_summary(
        connection: &mut Connection,
        prefix: &crate::ContextCompactionPrefix,
        summary_id: &str,
        operation_id: &str,
        assistant_message_id: &str,
        target_model_id: &str,
        created_at: i64,
    ) {
        let mut receipt = crate::ContextCompactionReceipt::begin_provider_transition(
            operation_id,
            format!("run-{operation_id}"),
            &prefix.conversation_id,
            assistant_message_id,
            target_model_id,
            Some("Source".to_string()),
            Some("Target".to_string()),
            crate::AgentApiStyle::OpenAiCompatible,
            prefix,
            1_000,
            200,
            created_at.saturating_sub(4),
        )
        .unwrap();
        crate::storage::context_compaction_receipt_repository::record_receipt(
            connection, &receipt, None,
        )
        .unwrap();
        receipt
            .advance_stage(
                crate::ContextCompactionReceiptStage::Preparing,
                created_at.saturating_sub(3),
            )
            .unwrap();
        crate::storage::context_compaction_receipt_repository::record_receipt(
            connection, &receipt, None,
        )
        .unwrap();
        receipt
            .attach_prepared_prefix(prefix, created_at.saturating_sub(2))
            .unwrap();
        crate::storage::context_compaction_receipt_repository::record_receipt(
            connection, &receipt, None,
        )
        .unwrap();
        receipt
            .advance_stage(
                crate::ContextCompactionReceiptStage::Committing,
                created_at.saturating_sub(1),
            )
            .unwrap();
        crate::storage::context_compaction_receipt_repository::record_receipt(
            connection, &receipt, None,
        )
        .unwrap();
        let draft = summary_draft(prefix, summary_id, created_at);
        context_compaction_repository::commit_prefix_replacement(
            connection,
            prefix,
            draft.clone(),
            assistant_message_id,
        )
        .unwrap();
        let observation = crate::model_request_observation::ModelRequestObservationBuilder::new(
            format!("observation-{operation_id}"),
            format!("run-{operation_id}"),
            Some(prefix.conversation_id.clone()),
            Some(assistant_message_id.to_string()),
            Some(operation_id.to_string()),
            1,
            crate::ModelRequestPurpose::ContextCompaction,
            target_model_id,
            crate::AgentApiStyle::OpenAiCompatible,
            None,
            created_at.saturating_sub(2),
        )
        .completed(None, Some("stop".to_string()), created_at)
        .unwrap();
        receipt
            .complete_applied(&draft, &observation, created_at)
            .unwrap();
        crate::storage::context_compaction_receipt_repository::record_receipt(
            connection,
            &receipt,
            Some(&observation),
        )
        .unwrap();
    }
}
