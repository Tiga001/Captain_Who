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
    source_run_id: String,
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
struct ForkFileChange {
    target: AgentFileChangeRecord,
    history: file_change_repository::AgentFileChangeHistorySnapshot,
}

impl std::ops::Deref for ForkFileChange {
    type Target = AgentFileChangeRecord;

    fn deref(&self) -> &Self::Target {
        &self.target
    }
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
struct ForkContextCompactionReceipt {
    source_receipt: ContextCompactionReceipt,
    source_observation: Option<ModelRequestObservation>,
    target_operation_id: String,
    target_run_id: String,
    target_observation_id: Option<String>,
}

#[derive(Debug)]
struct CollaborationRootFork {
    source_agent_id: String,
    target_root: EnsureRootAgentInput,
}

#[derive(Debug)]
struct ForkSnapshotOrigin {
    target_message_id: String,
    source_message_id: String,
    original_kind: Option<&'static str>,
    original_agent_id: Option<String>,
    original_mailbox_message_id: Option<String>,
}

#[derive(Debug)]
struct ForkHumanInteractionRequest {
    source_request_id: String,
    target_request_id: String,
    target_agent_id: String,
    target_run_id: String,
    target_assistant_message_id: String,
    target_tool_call_id: String,
    policy_revision: u64,
    questions_json: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug)]
struct ConversationHistoryForkPlan {
    source_conversation_id: String,
    source_message_id: Option<String>,
    target: ChatConversationRecord,
    attachments: Vec<ForkAttachmentCopy>,
    archives: Vec<conversation_history_archive_repository::ConversationHistoryArchiveForkCopy>,
    traces: Vec<ForkTrace>,
    action_audits: Vec<AgentActionAuditRecord>,
    turn_diffs: Vec<turn_diff_repository::AgentTurnDiffForkCopy>,
    guidances: Vec<ForkGuidance>,
    file_changes: Vec<ForkFileChange>,
    summaries: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    compaction_receipts: Vec<ForkContextCompactionReceipt>,
    provider_transition_receipts: Vec<ForkProviderTransitionReceipt>,
    world_state_records: Vec<world_state_repository::ConversationWorldStateJournalEntry>,
    requires_context_adaptation: bool,
    adaptation_source_summary_id: Option<String>,
    message_id_map: HashMap<String, String>,
    snapshot_origins: Vec<ForkSnapshotOrigin>,
    human_interaction_requests: Vec<ForkHumanInteractionRequest>,
    id_replacements: HashMap<String, String>,
}

#[derive(Debug)]
struct MemberAgentForkPlan {
    source_agent: AgentNodeRecord,
    target_agent: AgentNodeRecord,
    history: ConversationHistoryForkPlan,
}

#[derive(Clone, Copy)]
struct ConversationHistoryForkPlanRef<'a> {
    source_conversation_id: &'a str,
    source_message_id: Option<&'a str>,
    target: &'a ChatConversationRecord,
    attachments: &'a [ForkAttachmentCopy],
    archives: &'a [conversation_history_archive_repository::ConversationHistoryArchiveForkCopy],
    traces: &'a [ForkTrace],
    action_audits: &'a [AgentActionAuditRecord],
    turn_diffs: &'a [turn_diff_repository::AgentTurnDiffForkCopy],
    guidances: &'a [ForkGuidance],
    file_changes: &'a [ForkFileChange],
    summaries: &'a [context_compaction_repository::ContextCompactionSummaryVersion],
    compaction_receipts: &'a [ForkContextCompactionReceipt],
    provider_transition_receipts: &'a [ForkProviderTransitionReceipt],
    world_state_records: &'a [world_state_repository::ConversationWorldStateJournalEntry],
    requires_context_adaptation: bool,
    adaptation_source_summary_id: Option<&'a str>,
    message_id_map: &'a HashMap<String, String>,
    snapshot_origins: &'a [ForkSnapshotOrigin],
    human_interaction_requests: &'a [ForkHumanInteractionRequest],
    id_replacements: &'a HashMap<String, String>,
}

impl ConversationHistoryForkPlan {
    fn as_ref(&self) -> ConversationHistoryForkPlanRef<'_> {
        ConversationHistoryForkPlanRef {
            source_conversation_id: &self.source_conversation_id,
            source_message_id: self.source_message_id.as_deref(),
            target: &self.target,
            attachments: &self.attachments,
            archives: &self.archives,
            traces: &self.traces,
            action_audits: &self.action_audits,
            turn_diffs: &self.turn_diffs,
            guidances: &self.guidances,
            file_changes: &self.file_changes,
            summaries: &self.summaries,
            compaction_receipts: &self.compaction_receipts,
            provider_transition_receipts: &self.provider_transition_receipts,
            world_state_records: &self.world_state_records,
            requires_context_adaptation: self.requires_context_adaptation,
            adaptation_source_summary_id: self.adaptation_source_summary_id.as_deref(),
            message_id_map: &self.message_id_map,
            snapshot_origins: &self.snapshot_origins,
            human_interaction_requests: &self.human_interaction_requests,
            id_replacements: &self.id_replacements,
        }
    }
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
    action_audits: Vec<AgentActionAuditRecord>,
    turn_diffs: Vec<turn_diff_repository::AgentTurnDiffForkCopy>,
    guidances: Vec<ForkGuidance>,
    file_changes: Vec<ForkFileChange>,
    summaries: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    compaction_receipts: Vec<ForkContextCompactionReceipt>,
    provider_transition_receipts: Vec<ForkProviderTransitionReceipt>,
    world_state_records: Vec<world_state_repository::ConversationWorldStateJournalEntry>,
    pub(crate) requires_context_adaptation: bool,
    adaptation_source_summary_id: Option<String>,
    message_id_map: HashMap<String, String>,
    collaboration_root: Option<CollaborationRootFork>,
    snapshot_origins: Vec<ForkSnapshotOrigin>,
    human_interaction_requests: Vec<ForkHumanInteractionRequest>,
    #[cfg(test)]
    run_id_map: HashMap<String, String>,
    id_replacements: HashMap<String, String>,
    members: Vec<MemberAgentForkPlan>,
    pub(crate) provider_continuation_mappings: Vec<ProviderContinuationForkMapping>,
}

impl ConversationForkPlan {
    fn root_history_ref(&self) -> ConversationHistoryForkPlanRef<'_> {
        ConversationHistoryForkPlanRef {
            source_conversation_id: &self.source_conversation_id,
            source_message_id: Some(&self.source_message_id),
            target: &self.target,
            attachments: &self.attachments,
            archives: &self.archives,
            traces: &self.traces,
            action_audits: &self.action_audits,
            turn_diffs: &self.turn_diffs,
            guidances: &self.guidances,
            file_changes: &self.file_changes,
            summaries: &self.summaries,
            compaction_receipts: &self.compaction_receipts,
            provider_transition_receipts: &self.provider_transition_receipts,
            world_state_records: &self.world_state_records,
            requires_context_adaptation: self.requires_context_adaptation,
            adaptation_source_summary_id: self.adaptation_source_summary_id.as_deref(),
            message_id_map: &self.message_id_map,
            snapshot_origins: &self.snapshot_origins,
            human_interaction_requests: &self.human_interaction_requests,
            id_replacements: &self.id_replacements,
        }
    }

    pub(crate) fn attachments_mut(&mut self) -> impl Iterator<Item = &mut ForkAttachmentCopy> {
        self.attachments.iter_mut().chain(
            self.members
                .iter_mut()
                .flat_map(|member| member.history.attachments.iter_mut()),
        )
    }
}

#[derive(Debug)]
pub(crate) struct ExistingConversationFork {
    pub target_conversation_id: String,
    pub source_conversation_id: String,
    pub source_fork_point: ConversationForkPoint,
    fork_authority: String,
    source_root_agent_id: Option<String>,
    target_root_agent_id: Option<String>,
}

struct ResolvedConversationForkPoint {
    world_state_cutoff: ContextJournalCursor,
    assistant_message_id: String,
    summary_id: Option<String>,
    model_id: Option<String>,
}

pub(crate) fn build_fork_plan_at_point(
    connection: &Connection,
    request_id: &str,
    source_conversation_id: &str,
    fork_point: &ConversationForkPoint,
    created_at: i64,
) -> Result<ConversationForkPlan, ConversationForkError> {
    validate_fork_point_input(request_id, source_conversation_id, fork_point)?;
    let source_root =
        agent_graph_repository::get_agent_node_by_conversation(connection, source_conversation_id)
            .map_err(|error| ConversationForkError::Other(error.to_string()))?;
    if source_root
        .as_ref()
        .is_some_and(|node| node.parent_agent_id.is_some())
    {
        return Err(ConversationForkError::Other(
            "用户只能从根 Agent Conversation 继续新任务；子 Agent 保持只读。".to_string(),
        ));
    }
    if chat_repository::get_active_conversation(connection, source_conversation_id)
        .map_err(database_error)?
        .is_none()
    {
        return Err(ConversationForkError::Other("原任务不存在。".to_string()));
    }
    let cutoff_at = authoritative_fork_cutoff_at(connection, source_conversation_id, fork_point)?;

    let target_root_conversation_id = new_id("conversation");
    let mut global_id_replacements = HashMap::new();
    insert_global_replacement(
        &mut global_id_replacements,
        source_conversation_id,
        &target_root_conversation_id,
    )?;

    let mut visible_members = Vec::new();
    let mut target_member_identities = HashMap::new();
    if let Some(root) = &source_root {
        let target_root_agent_id = root_agent_id_for_conversation(&target_root_conversation_id);
        insert_global_replacement(
            &mut global_id_replacements,
            &root.agent_id,
            &target_root_agent_id,
        )?;
        let tree = agent_graph_repository::list_agent_tree(connection, &root.root_agent_id)
            .map_err(|error| ConversationForkError::Other(error.to_string()))?;
        visible_members = visible_member_agents(&tree, root, cutoff_at)?;
        for source_member in &visible_members {
            let target_agent_id = new_id("agent");
            let target_conversation_id = new_id("conversation");
            insert_global_replacement(
                &mut global_id_replacements,
                &source_member.agent_id,
                &target_agent_id,
            )?;
            insert_global_replacement(
                &mut global_id_replacements,
                &source_member.conversation_id,
                &target_conversation_id,
            )?;
            target_member_identities.insert(
                source_member.agent_id.clone(),
                (target_agent_id, target_conversation_id),
            );
        }
        let source = chat_repository::get_active_conversation(connection, source_conversation_id)
            .map_err(database_error)?
            .ok_or_else(|| ConversationForkError::Other("原任务不存在。".to_string()))?;
        let active_chain =
            context_compaction_repository::list_active_summary_chain(connection, &source.id)
                .map_err(|error| ConversationForkError::Other(error.to_string()))?;
        let resolved = resolve_fork_point(connection, &source, fork_point, &active_chain)?;
        let root_message_limit = source
            .messages
            .iter()
            .position(|message| message.id == resolved.assistant_message_id)
            .ok_or_else(|| ConversationForkError::Other("所选回复不属于原任务。".to_string()))?;
        preallocate_conversation_prefix_identities(
            connection,
            &source,
            Some(root_message_limit),
            &mut global_id_replacements,
        )?;
        for source_member in &visible_members {
            let member_conversation = chat_repository::get_active_conversation(
                connection,
                &source_member.conversation_id,
            )
            .map_err(database_error)?
            .ok_or_else(|| {
                ConversationForkError::Other("成员 Agent Conversation 不存在。".to_string())
            })?;
            let boundary =
                visible_member_message_boundary(connection, &member_conversation, cutoff_at)?;
            preallocate_conversation_prefix_identities(
                connection,
                &member_conversation,
                boundary.message_limit,
                &mut global_id_replacements,
            )?;
        }
    }

    let mut plan = build_single_conversation_fork_plan_at_point(
        connection,
        request_id,
        source_conversation_id,
        fork_point,
        created_at,
        false,
        Some(&target_root_conversation_id),
        None,
        Some(cutoff_at),
        Some(cutoff_at),
        &global_id_replacements,
    )?;

    let Some(source_root) = source_root else {
        return Ok(plan);
    };
    let target_root_agent_id = root_agent_id_for_conversation(&target_root_conversation_id);
    let copied_conversation_ids = std::iter::once(source_root.conversation_id.clone())
        .chain(
            visible_members
                .iter()
                .map(|member| member.conversation_id.clone()),
        )
        .collect::<Vec<_>>();
    let copied_agent_ids = std::iter::once(source_root.agent_id.clone())
        .chain(visible_members.iter().map(|member| member.agent_id.clone()))
        .collect::<Vec<_>>();
    ensure_agent_tree_stable(connection, &copied_conversation_ids, &copied_agent_ids)?;

    for source_member in visible_members {
        let (target_agent_id, target_conversation_id) = target_member_identities
            .get(&source_member.agent_id)
            .cloned()
            .ok_or_else(|| {
                ConversationForkError::Other(
                    "成员 Agent 的目标身份映射不完整，已安全取消分叉。".to_string(),
                )
            })?;
        let source_parent_agent_id = source_member
            .parent_agent_id
            .as_deref()
            .ok_or_else(|| ConversationForkError::Other("成员 Agent 缺少父节点。".to_string()))?;
        let target_parent_agent_id = if source_parent_agent_id == source_root.agent_id {
            target_root_agent_id.clone()
        } else {
            target_member_identities
                .get(source_parent_agent_id)
                .map(|(agent_id, _)| agent_id.clone())
                .ok_or_else(|| {
                    ConversationForkError::Other(
                        "成员 Agent 的目标父节点映射不完整，已安全取消分叉。".to_string(),
                    )
                })?
        };
        let model_id = source_member
            .model_snapshot
            .as_ref()
            .map(|snapshot| snapshot.model_config_id.clone())
            .ok_or_else(|| {
                ConversationForkError::Other("成员 Agent 缺少冻结模型快照。".to_string())
            })?;
        let source_conversation =
            chat_repository::get_active_conversation(connection, &source_member.conversation_id)
                .map_err(database_error)?
                .ok_or_else(|| {
                    ConversationForkError::Other("成员 Agent Conversation 不存在。".to_string())
                })?;
        if source_conversation.project_id != source_member.project_id
            || source_conversation.model_id.as_deref() != Some(model_id.as_str())
        {
            return Err(ConversationForkError::Other(
                "成员 Agent Conversation 的项目或模型身份无效。".to_string(),
            ));
        }
        let boundary =
            visible_member_message_boundary(connection, &source_conversation, cutoff_at)?;
        let (history, member_mappings) = if let Some((assistant_message_id, message_limit)) =
            boundary.last_assistant.as_ref().map(|message_id| {
                (
                    message_id.clone(),
                    boundary
                        .message_limit
                        .expect("a visible assistant always establishes a message boundary"),
                )
            }) {
            let member_plan = build_single_conversation_fork_plan_at_point(
                connection,
                &new_id("member-fork-plan"),
                &source_member.conversation_id,
                &ConversationForkPoint::AssistantReply {
                    assistant_message_id,
                },
                created_at,
                true,
                Some(&target_conversation_id),
                Some(message_limit),
                Some(cutoff_at),
                Some(cutoff_at),
                &global_id_replacements,
            )?;
            history_from_single_plan(member_plan)?
        } else {
            (
                build_message_only_history_plan(
                    connection,
                    &source_conversation,
                    boundary.message_limit,
                    &target_conversation_id,
                    created_at,
                    &global_id_replacements,
                )?,
                Vec::new(),
            )
        };
        if history.target.model_id.as_deref() != Some(model_id.as_str()) {
            return Err(ConversationForkError::Other(
                "成员历史边界模型与冻结 Agent 模型不一致。".to_string(),
            ));
        }
        plan.provider_continuation_mappings.extend(member_mappings);
        plan.members.push(MemberAgentForkPlan {
            source_agent: source_member.clone(),
            target_agent: AgentNodeRecord {
                agent_id: target_agent_id,
                root_agent_id: target_root_agent_id.clone(),
                root_conversation_id: target_root_conversation_id.clone(),
                parent_agent_id: Some(target_parent_agent_id),
                conversation_id: target_conversation_id,
                project_id: source_member.project_id.clone(),
                creation_request_id: new_id("agent-fork-request"),
                task_name: source_member.task_name.clone(),
                task_path: source_member.task_path.clone(),
                template_snapshot: source_member.template_snapshot.clone(),
                model_snapshot: source_member.model_snapshot.clone(),
                model_selection_source: source_member.model_selection_source,
                reasoning_effort_snapshot: source_member.reasoning_effort_snapshot,
                lifecycle: AgentLifecycle::Active,
                revision: source_member.revision,
                created_at: source_member.created_at,
                updated_at: source_member.updated_at,
            },
            history,
        });
    }
    Ok(plan)
}

#[derive(Debug)]
struct VisibleMemberMessageBoundary {
    message_limit: Option<usize>,
    last_assistant: Option<String>,
}

fn visible_member_message_boundary(
    connection: &Connection,
    source: &ChatConversationRecord,
    cutoff_at: i64,
) -> Result<VisibleMemberMessageBoundary, ConversationForkError> {
    let mut message_limit = None;
    let mut last_assistant = None;
    for (position, message) in source.messages.iter().enumerate() {
        if message.created_at > cutoff_at {
            break;
        }
        if message.role == "assistant" {
            let trace =
                conversation_trace_repository::get_trace_for_message(connection, &message.id)
                    .map_err(database_error)?;
            ensure_settled_assistant(message, trace.as_ref())?;
            if trace.is_some() && trace_times(connection, &message.id)?.1 > cutoff_at {
                break;
            }
            last_assistant = Some(message.id.clone());
        }
        message_limit = Some(position);
    }
    Ok(VisibleMemberMessageBoundary {
        message_limit,
        last_assistant,
    })
}

fn visible_member_agents(
    tree: &[AgentNodeRecord],
    root: &AgentNodeRecord,
    cutoff_at: i64,
) -> Result<Vec<AgentNodeRecord>, ConversationForkError> {
    let mut candidates = tree
        .iter()
        .filter(|node| node.parent_agent_id.is_some() && node.created_at <= cutoff_at)
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.task_path
            .matches('/')
            .count()
            .cmp(&right.task_path.matches('/').count())
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    let mut visible_ids = HashSet::from([root.agent_id.clone()]);
    let mut visible = Vec::new();
    for candidate in candidates {
        let parent = candidate
            .parent_agent_id
            .as_deref()
            .ok_or_else(|| ConversationForkError::Other("成员 Agent 缺少父节点。".to_string()))?;
        if visible_ids.contains(parent) {
            visible_ids.insert(candidate.agent_id.clone());
            visible.push(candidate);
        }
    }
    Ok(visible)
}

fn insert_global_replacement(
    replacements: &mut HashMap<String, String>,
    source: &str,
    target: &str,
) -> Result<(), ConversationForkError> {
    if let Some(existing) = replacements.insert(source.to_string(), target.to_string()) {
        if existing != target {
            return Err(ConversationForkError::Other(
                "分叉树存在冲突的跨实体身份，已安全取消。".to_string(),
            ));
        }
    }
    Ok(())
}

fn preallocate_conversation_prefix_identities(
    connection: &Connection,
    source: &ChatConversationRecord,
    message_limit: Option<usize>,
    replacements: &mut HashMap<String, String>,
) -> Result<(), ConversationForkError> {
    let messages = match message_limit {
        Some(limit) => source.messages.get(..=limit).ok_or_else(|| {
            ConversationForkError::Other("预分配的 Conversation 历史边界无效。".to_string())
        })?,
        None => &[],
    };
    let mut run_ids = HashSet::new();
    let mut archive_refs = HashSet::new();
    for message in messages {
        insert_global_replacement(replacements, &message.id, &new_id("message"))?;
        if let Some(trace) =
            conversation_trace_repository::get_trace_for_message(connection, &message.id)
                .map_err(database_error)?
        {
            let target_run_id = replacements
                .get(&trace.run_id)
                .cloned()
                .unwrap_or_else(|| new_id("run"));
            insert_global_replacement(replacements, &trace.run_id, &target_run_id)?;
            run_ids.insert(trace.run_id.clone());
            for (item_index, item) in trace.items.iter().enumerate() {
                let call_id = match item {
                    crate::ConversationTurnTraceItem::ToolCall { call_id, .. }
                    | crate::ConversationTurnTraceItem::ToolResult { call_id, .. }
                    | crate::ConversationTurnTraceItem::CommandSessionLifecycle {
                        call_id, ..
                    } => Some(call_id),
                    _ => None,
                };
                if let Some(call_id) = call_id {
                    let target_call_id = replacements.get(call_id).cloned().unwrap_or_else(|| {
                        crate::llm::model_response_tool_call_id(
                            &target_run_id,
                            item_index,
                            0,
                            call_id,
                        )
                    });
                    insert_global_replacement(replacements, call_id, &target_call_id)?;
                }
                let archive = match item {
                    crate::ConversationTurnTraceItem::ToolResult { archive, .. }
                    | crate::ConversationTurnTraceItem::CommandSessionLifecycle {
                        archive, ..
                    } => Some(archive),
                    _ => None,
                };
                if let Some(archive_ref) = archive.and_then(|value| value.archive_ref.as_deref()) {
                    archive_refs.insert(archive_ref.to_string());
                }
            }
        }
        if let Some(run_id) = agent_run_id(message.agent_run_json.as_deref()) {
            let target_run_id = replacements
                .get(&run_id)
                .cloned()
                .unwrap_or_else(|| new_id("run"));
            insert_global_replacement(replacements, &run_id, &target_run_id)?;
            run_ids.insert(run_id);
        }
        for guidance in
            guidance_repository::list_guidances_for_assistant_message(connection, &message.id)
                .map_err(database_error)?
        {
            if guidance.status == AgentGuidanceStatus::Applied {
                insert_global_replacement(
                    replacements,
                    &guidance.guidance_id,
                    &new_id("guidance"),
                )?;
            }
        }
    }
    for run_id in run_ids {
        for change in file_change_repository::list_file_changes_for_run(connection, &run_id)
            .map_err(database_error)?
        {
            insert_global_replacement(replacements, &change.id, &new_id("file-change"))?;
            let target_observation_id = format!("fobs_{}", Uuid::new_v4().simple());
            insert_global_replacement(
                replacements,
                &change.observation_id,
                &target_observation_id,
            )?;
        }
    }
    for archive_ref in archive_refs {
        insert_global_replacement(replacements, &archive_ref, &new_id("history-archive"))?;
    }
    let message_ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    for attachment in attachment_repository::list_message_attachments_for_fork(
        connection,
        &source.id,
        &message_ids,
    )
    .map_err(database_error)?
    {
        insert_global_replacement(replacements, &attachment.id, &new_id("attachment"))?;
    }
    Ok(())
}

fn authoritative_fork_cutoff_at(
    connection: &Connection,
    source_conversation_id: &str,
    fork_point: &ConversationForkPoint,
) -> Result<i64, ConversationForkError> {
    match fork_point {
        ConversationForkPoint::Latest {} => Ok(i64::MAX),
        ConversationForkPoint::ManualCompactionBoundary { operation_id } => {
            let (operation, _) =
                load_manual_compaction_boundary(connection, source_conversation_id, operation_id)?;
            operation
                .completed_at
                .ok_or_else(|| "手动压缩分支边界缺少完成时间。".to_string().into())
        }
        ConversationForkPoint::AssistantReply {
            assistant_message_id,
        } => connection
            .query_row(
                "SELECT MAX(message.created_at,
                            COALESCE(trace.completed_at, trace.updated_at, message.created_at))
                 FROM messages AS message
                 LEFT JOIN conversation_turn_traces AS trace
                   ON trace.assistant_message_id = message.id
                 WHERE message.conversation_id = ?1 AND message.id = ?2",
                params![source_conversation_id, assistant_message_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(database_error)
            .map_err(Into::into),
        ConversationForkPoint::ProviderTransitionBoundary { operation_id } => {
            let receipt =
                context_compaction_receipt_repository::get_receipt(connection, operation_id)
                    .map_err(|error| ConversationForkError::Other(error.to_string()))?
                    .ok_or_else(|| {
                        ConversationForkError::Other(
                            "找不到指定的 Provider transition 分叉边界。".to_string(),
                        )
                    })?;
            Ok(receipt.completed_at.unwrap_or(receipt.updated_at))
        }
    }
}

pub(crate) fn find_existing_fork(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<ExistingConversationFork>> {
    connection
        .query_row(
            "SELECT target_conversation_id, source_conversation_id, source_fork_point_json,
                    fork_authority, source_root_agent_id, target_root_agent_id
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
                    fork_authority: row.get(3)?,
                    source_root_agent_id: row.get(4)?,
                    target_root_agent_id: row.get(5)?,
                })
            },
        )
        .optional()
}

pub(crate) fn validate_existing_fork_authority(
    connection: &Connection,
    existing: &ExistingConversationFork,
) -> Result<(), ConversationForkError> {
    let valid = match existing.fork_authority.as_str() {
        "legacy" => {
            existing.source_root_agent_id.is_none()
                && existing.target_root_agent_id.is_none()
                && !connection
                    .query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM agent_nodes
                             WHERE conversation_id IN (?1, ?2)
                         )",
                        params![
                            &existing.source_conversation_id,
                            &existing.target_conversation_id
                        ],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(database_error)?
        }
        "collaboration_root" => match (
            existing.source_root_agent_id.as_deref(),
            existing.target_root_agent_id.as_deref(),
        ) {
            (Some(source_agent_id), Some(target_agent_id)) => connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM agent_nodes AS source
                         INNER JOIN agent_nodes AS target ON target.agent_id = ?2
                         WHERE source.agent_id = ?1
                           AND source.parent_agent_id IS NULL
                           AND source.conversation_id = ?3
                           AND source.root_conversation_id = ?3
                           AND target.parent_agent_id IS NULL
                           AND target.conversation_id = ?4
                           AND target.root_conversation_id = ?4
                           AND target.root_agent_id != source.root_agent_id
                           AND target.project_id IS source.project_id
                     )",
                    params![
                        source_agent_id,
                        target_agent_id,
                        &existing.source_conversation_id,
                        &existing.target_conversation_id,
                    ],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?,
            _ => false,
        },
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ConversationForkError::Other(
            "已存在的分叉记录身份无效，已拒绝重试。".to_string(),
        ))
    }
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
