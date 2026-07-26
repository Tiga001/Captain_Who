use crate::context::{
    ContextCompactionSummary, ContextContinuityEntry, ContextContinuitySnapshot, ContextHistoryRef,
    ContextJournalCursor,
};
use crate::storage::models::{
    AgentFileDraftRecord, AgentRunGuidanceRecord, AttachmentRecord, ChatConversationRecord,
    ChatMessageRecord, ConversationContinuationOriginRecord, ForkConversationInput,
};
use crate::storage::{
    attachment_repository, chat_repository, context_compaction_repository,
    conversation_goal_repository, conversation_history_archive_repository,
    conversation_trace_repository, file_draft_repository, guidance_repository,
    world_state_repository,
};
use crate::{AgentGuidanceStatus, ConversationTurnTrace, WorldStateRecord};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(crate) struct ForkAttachmentCopy {
    pub source: AttachmentRecord,
    pub target: AttachmentRecord,
}

#[derive(Debug)]
struct ForkTrace {
    trace: ConversationTurnTrace,
    created_at: i64,
    committed_at: i64,
}

#[derive(Debug)]
struct ForkGuidance {
    record: AgentRunGuidanceRecord,
    applied_trace_sequence: u64,
}

#[derive(Debug)]
pub(crate) struct ConversationForkPlan {
    pub request_id: String,
    pub source_conversation_id: String,
    pub source_message_id: String,
    pub target: ChatConversationRecord,
    pub attachments: Vec<ForkAttachmentCopy>,
    archives: Vec<conversation_history_archive_repository::ConversationHistoryArchiveForkCopy>,
    traces: Vec<ForkTrace>,
    guidances: Vec<ForkGuidance>,
    file_drafts: Vec<AgentFileDraftRecord>,
    summaries: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    world_state_records: Vec<world_state_repository::ConversationWorldStateJournalEntry>,
    message_id_map: HashMap<String, String>,
    run_id_map: HashMap<String, String>,
    id_replacements: HashMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct ExistingConversationFork {
    pub target_conversation_id: String,
    pub source_conversation_id: String,
    pub source_message_id: String,
}

pub(crate) fn find_existing_fork(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<ExistingConversationFork>> {
    connection
        .query_row(
            "SELECT target_conversation_id, source_conversation_id, source_message_id
             FROM conversation_forks
             WHERE request_id = ?1",
            [request_id],
            |row| {
                Ok(ExistingConversationFork {
                    target_conversation_id: row.get(0)?,
                    source_conversation_id: row.get(1)?,
                    source_message_id: row.get(2)?,
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

pub(crate) fn build_fork_plan(
    connection: &Connection,
    input: &ForkConversationInput,
    created_at: i64,
) -> Result<ConversationForkPlan, String> {
    validate_input(input)?;
    let source = chat_repository::get_conversation(connection, &input.source_conversation_id)
        .map_err(database_error)?
        .ok_or_else(|| "原任务不存在。".to_string())?;
    let cutoff = source
        .messages
        .iter()
        .position(|message| message.id == input.through_assistant_message_id)
        .ok_or_else(|| "所选回复不属于原任务。".to_string())?;
    let cutoff_message = &source.messages[cutoff];
    if cutoff_message.role != "assistant" {
        return Err("只能从 assistant 回复继续新任务。".to_string());
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
                return Err("历史回复的运行标识与后端工具轨迹不一致。".to_string());
            }
            let new_run_id = new_id("run");
            run_id_map.insert(trace.run_id.clone(), new_run_id.clone());
            let (trace_created_at, committed_at) = trace_times(connection, &message.id)?;
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
                created_at: trace_created_at,
                committed_at,
            });
        } else if let Some(run_id) = agent_run_id(message.agent_run_json.as_deref()) {
            run_id_map.entry(run_id).or_insert_with(|| new_id("run"));
        }
    }

    let mut file_drafts = Vec::new();
    let mut draft_id_map = HashMap::new();
    for (source_run_id, target_run_id) in &run_id_map {
        for source_draft in file_draft_repository::list_drafts_for_run(connection, source_run_id)
            .map_err(database_error)?
        {
            if source_draft.conversation_id != source.id {
                return Err("文件草稿的任务归属与历史回复不一致。".to_string());
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
            let crate::ConversationTurnTraceItem::ToolResult {
                call_id, archive, ..
            } = item
            else {
                continue;
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
                call_id,
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

    let active_chain =
        context_compaction_repository::list_active_summary_chain(connection, &source.id)
            .map_err(|error| error.to_string())?;
    let summaries = summaries_visible_at_cutoff(active_chain, &source_positions, cutoff)?;
    let world_state_records = world_state_records_visible_at_cutoff(
        connection,
        &source.id,
        &source_positions,
        cutoff,
        &summaries,
    )?;

    Ok(ConversationForkPlan {
        request_id: input.request_id.trim().to_string(),
        source_conversation_id: source.id,
        source_message_id: input.through_assistant_message_id.clone(),
        target: ChatConversationRecord {
            id: target_conversation_id,
            project_id: source.project_id,
            model_id: source.model_id,
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
        guidances,
        file_drafts,
        summaries,
        world_state_records,
        message_id_map,
        run_id_map,
        id_replacements: replacements,
    })
}

pub(crate) fn commit_fork_plan(
    connection: &mut Connection,
    plan: &ConversationForkPlan,
) -> Result<(), String> {
    let target_message_id = mapped_id(
        &plan.message_id_map,
        &plan.source_message_id,
        "新任务接续边界消息",
    )?;
    let transaction = connection.transaction().map_err(database_error)?;
    insert_conversation(&transaction, &plan.target)?;
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
    }
    conversation_goal_repository::clone_visible_goal_for_fork(
        &transaction,
        &plan.source_conversation_id,
        &plan.target.id,
        &plan.message_id_map,
        plan.target.created_at,
    )?;
    for draft in &plan.file_drafts {
        file_draft_repository::insert_draft(&transaction, draft).map_err(database_error)?;
    }
    let summary_id_map = clone_summary_chain(&transaction, plan)?;
    clone_world_state_records(&transaction, plan, &summary_id_map)?;
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
                return Err(format!("克隆用户引导 journal 时发生意外冲突：{outcome:?}"));
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
                return Err(format!(
                    "克隆用户引导 trace 状态时发生意外冲突：{outcome:?}"
                ));
            }
        }
    }
    transaction
        .execute(
            "INSERT INTO conversation_forks (
                request_id, target_conversation_id, source_conversation_id,
                source_message_id, target_message_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &plan.request_id,
                &plan.target.id,
                &plan.source_conversation_id,
                &plan.source_message_id,
                &target_message_id,
                plan.target.created_at,
            ],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)
}

fn validate_input(input: &ForkConversationInput) -> Result<(), String> {
    for (label, value) in [
        ("请求 ID", input.request_id.as_str()),
        ("原任务 ID", input.source_conversation_id.as_str()),
        ("回复 ID", input.through_assistant_message_id.as_str()),
    ] {
        if value.trim().is_empty() || value.chars().count() > 512 {
            return Err(format!("{label}无效。"));
        }
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

fn summaries_visible_at_cutoff(
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
        if owner_position <= cutoff {
            if crossed_cutoff {
                return Err("摘要链跨越分叉边界后又回到了更早轮次。".to_string());
            }
            visible.push(version);
        } else {
            crossed_cutoff = true;
        }
    }
    Ok(visible)
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
            &plan.run_id_map,
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
    run_id_map: &HashMap<String, String>,
    replacements: &HashMap<String, String>,
) -> Result<ContextContinuitySnapshot, String> {
    let remap_refs = |refs: &[ContextHistoryRef]| {
        refs.iter()
            .map(|reference| remap_history_ref(reference, message_id_map, replacements))
            .collect::<Result<Vec<_>, String>>()
    };
    let entries = source
        .entries
        .iter()
        .map(|entry| match entry {
            ContextContinuityEntry::UserMessage {
                cursor,
                created_at,
                text,
                status,
            } => Ok(ContextContinuityEntry::UserMessage {
                cursor: remap_cursor(cursor, message_id_map)?,
                created_at: created_at.clone(),
                text: text.clone(),
                status: status.clone(),
            }),
            ContextContinuityEntry::AssistantMessage {
                cursor,
                created_at,
                text,
                status,
                terminal_status,
                terminal_error,
            } => Ok(ContextContinuityEntry::AssistantMessage {
                cursor: remap_cursor(cursor, message_id_map)?,
                created_at: created_at.clone(),
                text: text.clone(),
                status: status.clone(),
                terminal_status: *terminal_status,
                terminal_error: terminal_error.clone(),
            }),
            ContextContinuityEntry::AssistantNarration {
                cursor,
                run_id,
                created_at,
                preview,
                total_chars,
                truncated,
            } => Ok(ContextContinuityEntry::AssistantNarration {
                cursor: remap_cursor(cursor, message_id_map)?,
                run_id: mapped_id(run_id_map, run_id, "摘要 trace run")?,
                created_at: created_at.clone(),
                preview: preview.clone(),
                total_chars: *total_chars,
                truncated: *truncated,
            }),
            ContextContinuityEntry::ToolExchange {
                call_cursor,
                result_cursor,
                run_id,
                created_at,
                call_id,
                tool,
                operation,
                status,
                success,
                outcome,
                approval_status,
                error,
                truncated,
            } => Ok(ContextContinuityEntry::ToolExchange {
                call_cursor: remap_cursor(call_cursor, message_id_map)?,
                result_cursor: remap_cursor(result_cursor, message_id_map)?,
                run_id: mapped_id(run_id_map, run_id, "摘要 trace run")?,
                created_at: created_at.clone(),
                call_id: call_id.clone(),
                tool: tool.clone(),
                operation: rewritten_value(operation, replacements),
                status: *status,
                success: *success,
                outcome: rewritten_value(outcome, replacements),
                approval_status: *approval_status,
                error: error.clone(),
                truncated: *truncated,
            }),
        })
        .collect::<Result<Vec<_>, String>>()?;
    let snapshot = ContextContinuitySnapshot {
        schema_version: source.schema_version,
        covered_through: remap_cursor(&source.covered_through, message_id_map)?,
        task_evidence_refs: remap_refs(&source.task_evidence_refs)?,
        unresolved_failure_refs: remap_refs(&source.unresolved_failure_refs)?,
        approval_refs: remap_refs(&source.approval_refs)?,
        important_decision_refs: remap_refs(&source.important_decision_refs)?,
        recent_refs: remap_refs(&source.recent_refs)?,
        archived_counts: source.archived_counts.clone(),
        entries,
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
    trace.items = serde_json::from_value(value)
        .map_err(|error| format!("无法重建复制后的历史工具轨迹：{error}"))?;
    trace.validate().map_err(|error| error.to_string())
}

fn rewritten_value(value: &Value, replacements: &HashMap<String, String>) -> Value {
    let mut value = value.clone();
    rewrite_exact_ids(&mut value, replacements);
    value
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
        ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus, WorldStateDiff,
        WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };

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

        let plan = build_fork_plan(
            &connection,
            &ForkConversationInput {
                request_id: "fork-at-c".to_string(),
                source_conversation_id: source.id.clone(),
                through_assistant_message_id: "assistant-c".to_string(),
            },
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

        let before_summary = build_fork_plan(
            &connection,
            &ForkConversationInput {
                request_id: "recursive-before-summary".to_string(),
                source_conversation_id: plan.target.id.clone(),
                through_assistant_message_id: plan.message_id_map["assistant-a"].clone(),
            },
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

        let after_summary = build_fork_plan(
            &connection,
            &ForkConversationInput {
                request_id: "recursive-after-summary".to_string(),
                source_conversation_id: plan.target.id.clone(),
                through_assistant_message_id: plan.message_id_map["assistant-b"].clone(),
            },
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

        let plan = build_fork_plan(
            &connection,
            &ForkConversationInput {
                request_id: "fork-world-state-at-c".to_string(),
                source_conversation_id: source.id.clone(),
                through_assistant_message_id: "assistant-c".to_string(),
            },
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

        let early = build_fork_plan(
            &connection,
            &ForkConversationInput {
                request_id: "fork-before-all-summaries".to_string(),
                source_conversation_id: source.id.clone(),
                through_assistant_message_id: "assistant-a".to_string(),
            },
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

        let middle = build_fork_plan(
            &connection,
            &ForkConversationInput {
                request_id: "fork-between-world-state-summaries".to_string(),
                source_conversation_id: source.id.clone(),
                through_assistant_message_id: "assistant-c".to_string(),
            },
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
}
