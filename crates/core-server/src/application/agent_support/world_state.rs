use super::*;
use mycopilot_core::{
    world_state::{
        effective_permissions_section, interaction_profile_section, model_capabilities_section,
        workspace_binding_section,
    },
    AnchoredWorldStateRecord, WorldStateDiff, WorldStateLifetime, WorldStateRecord,
    WorldStateReducer, WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
};

/// Establishes the durable, backend-owned World State that is effective before a new user
/// message, then returns the active epoch in context order.
///
/// The authoritative record may contain host-only values, but only each section's explicit
/// `model_projection` can cross the context-rendering boundary in `mycopilot-core`.
pub(crate) struct EnsureConversationWorldStateRequest<'a> {
    pub(crate) storage: &'a StorageService,
    pub(crate) conversation_id: &'a str,
    pub(crate) effective_before_message_id: &'a str,
    pub(crate) context: Option<&'a AgentRunContext>,
    pub(crate) prompt_preferences: Option<&'a AgentPromptPreferences>,
    pub(crate) model_capabilities: ModelCapabilities,
    pub(crate) active_summary: Option<&'a mycopilot_core::ContextCompactionSummary>,
    pub(crate) created_at: i64,
}

pub(crate) fn ensure_conversation_world_state(
    request: EnsureConversationWorldStateRequest<'_>,
) -> Result<Vec<AnchoredWorldStateRecord>, String> {
    let EnsureConversationWorldStateRequest {
        storage,
        conversation_id,
        effective_before_message_id,
        context,
        prompt_preferences,
        model_capabilities,
        active_summary,
        created_at,
    } = request;
    let active_summary_id = active_summary.map(|summary| summary.id.as_str());
    let desired_sections =
        conversation_world_state_sections(context, prompt_preferences, model_capabilities)?;
    let mut entries = storage.list_active_conversation_world_state_records(conversation_id)?;

    if entries.is_empty() {
        let snapshot = WorldStateSnapshot::new(create_id("world-state-epoch"), 0, desired_sections)
            .map_err(|error| format!("无法创建 Conversation World State：{error}"))?;
        storage.append_conversation_world_state_record(
            conversation_id,
            1,
            active_summary_id,
            None,
            &WorldStateRecord::Full(snapshot),
            created_at,
        )?;
        entries = storage.list_active_conversation_world_state_records(conversation_id)?;
    } else {
        let (mut current, mut epoch_generation, base_summary_id) = fold_active_entries(&entries)?;
        if base_summary_id.as_deref() != active_summary_id {
            let summary = active_summary
                .ok_or_else(|| "Conversation World State 指向已不存在的压缩摘要。".to_string())?;
            let source_epoch_id = current.epoch_id.clone();
            let source_revision = current.revision.clone();
            let new_epoch_id = create_id("world-state-epoch");
            let rebased = storage.rebase_active_conversation_world_state(
                &mycopilot_core::storage::world_state_repository::ConversationWorldStateRebaseRequest {
                    conversation_id,
                    expected_source_epoch_id: &source_epoch_id,
                    expected_source_revision: &source_revision,
                    covered_through_message_id: summary.covered_through.message_id(),
                    new_epoch_id: &new_epoch_id,
                    base_summary_id: &summary.id,
                    created_at,
                },
            )?;
            current = rebased.full_snapshot;
            epoch_generation = rebased.epoch_generation;
            entries = storage.list_active_conversation_world_state_records(conversation_id)?;
        }

        let target = WorldStateSnapshot::new(
            current.epoch_id.clone(),
            current.sequence.saturating_add(1),
            desired_sections,
        )
        .map_err(|error| format!("无法构造 Conversation World State 目标快照：{error}"))?;
        if current.revision != target.revision {
            let diff = WorldStateDiff::between(&current, &target)
                .map_err(|error| format!("无法构造 Conversation World State diff：{error}"))?;
            storage.append_conversation_world_state_record(
                conversation_id,
                epoch_generation,
                active_summary_id,
                Some(effective_before_message_id),
                &WorldStateRecord::Diff(diff),
                created_at,
            )?;
            entries = storage.list_active_conversation_world_state_records(conversation_id)?;
        }
    }

    entries
        .into_iter()
        .map(|entry| {
            AnchoredWorldStateRecord::new(entry.record, entry.effective_before_message_id)
                .map_err(|error| format!("Conversation World State anchor 无效：{error}"))
        })
        .collect()
}

pub(crate) fn load_conversation_world_state(
    storage: &StorageService,
    conversation_id: &str,
) -> Result<Vec<AnchoredWorldStateRecord>, String> {
    storage
        .list_active_conversation_world_state_records(conversation_id)?
        .into_iter()
        .map(|entry| {
            AnchoredWorldStateRecord::new(entry.record, entry.effective_before_message_id)
                .map_err(|error| format!("Conversation World State anchor 无效：{error}"))
        })
        .collect()
}

fn fold_active_entries(
    entries: &[mycopilot_core::storage::world_state_repository::ConversationWorldStateJournalEntry],
) -> Result<(WorldStateSnapshot, u64, Option<String>), String> {
    let Some(first) = entries.first() else {
        return Err("Conversation World State active epoch 为空。".to_string());
    };
    let WorldStateRecord::Full(initial) = &first.record else {
        return Err(
            "Conversation World State active epoch 没有 initial full snapshot。".to_string(),
        );
    };
    if first.effective_before_message_id.is_some() {
        return Err(
            "Conversation World State initial full snapshot 不能绑定消息 anchor。".to_string(),
        );
    }
    let mut diffs = Vec::with_capacity(entries.len().saturating_sub(1));
    for entry in &entries[1..] {
        let WorldStateRecord::Diff(diff) = &entry.record else {
            return Err(
                "Conversation World State active epoch 只能包含一个 initial full。".to_string(),
            );
        };
        if entry.effective_before_message_id.is_none() {
            return Err("Conversation World State diff 缺少消息 anchor。".to_string());
        }
        diffs.push(diff.clone());
    }
    let folded = WorldStateReducer::fold(initial.clone(), &diffs)
        .map_err(|error| format!("Conversation World State 无法精确折叠：{error}"))?;
    Ok((
        folded,
        first.epoch_generation,
        first.base_summary_id.clone(),
    ))
}

fn conversation_world_state_sections(
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    model_capabilities: ModelCapabilities,
) -> Result<Vec<WorldStateSectionEnvelope>, String> {
    let permissions = context
        .map(|context| context.permissions)
        .unwrap_or_default();
    let permission_section =
        effective_permissions_section(permissions, WorldStateLifetime::Conversation)
            .map_err(|error| format!("无法构造权限 World State：{error}"))?;

    let workspace = context.and_then(|context| context.workspace.as_ref());
    let workspace_section = workspace_binding_section(workspace, WorldStateLifetime::Conversation)
        .map_err(|error| format!("无法构造 workspace World State：{error}"))?;

    let interaction_section =
        interaction_profile_section(prompt_preferences, WorldStateLifetime::Conversation)
            .map_err(|error| format!("无法构造交互配置 World State：{error}"))?;

    // Conversation environment contains only facts observed independently of tool execution.
    // Operational availability (including Office) belongs to the run-scoped EffectiveTools
    // section, so this durable section must not guess or duplicate that authority.
    let environment_state = environment_projection();
    let environment_section = WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::Environment,
        WorldStateLifetime::Conversation,
        environment_state.clone(),
        environment_state,
    )
    .map_err(|error| format!("无法构造 environment World State：{error}"))?;

    // Model capabilities are execution authority, not prompting material. Keeping the section
    // HostOnly lets tools consult one coherent state model without teaching the model to assume a
    // capability that the runtime will still independently enforce.
    let capability_section =
        model_capabilities_section(model_capabilities, WorldStateLifetime::Conversation)
            .map_err(|error| format!("无法构造模型能力 World State：{error}"))?;

    Ok(vec![
        permission_section,
        workspace_section,
        interaction_section,
        environment_section,
        capability_section,
    ])
}

fn environment_projection() -> serde_json::Value {
    let shell_name = std::env::var("SHELL")
        .ok()
        .or_else(|| std::env::var("COMSPEC").ok())
        .and_then(|shell| {
            std::path::Path::new(&shell)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        });
    let timezone = std::env::var("TZ")
        .ok()
        .filter(|value| !value.trim().is_empty());
    json!({
        "os": {
            "family": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "shell": {
            "name": shell_name,
        },
        "timezone": {
            "name": timezone,
            "source": if timezone.is_some() { "TZ" } else { "system" },
        },
        "network": {
            "publicWeb": "tool_gated",
            "note": "Use registered web tools when available; do not infer arbitrary network access.",
        }
    })
}
