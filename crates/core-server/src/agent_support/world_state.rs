use super::*;
use mycopilot_core::{
    AnchoredWorldStateRecord, WorldStateDiff, WorldStateLifetime, WorldStateRecord,
    WorldStateReducer, WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
};

/// Establishes the durable, backend-owned World State that is effective before a new user
/// message, then returns the active epoch in context order.
///
/// The authoritative record may contain host-only values, but only each section's explicit
/// `model_projection` can cross the context-rendering boundary in `mycopilot-core`.
pub(crate) fn ensure_conversation_world_state(
    storage: &StorageService,
    conversation_id: &str,
    effective_before_message_id: &str,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    model_capabilities: ModelCapabilities,
    active_summary: Option<&mycopilot_core::ContextCompactionSummary>,
    created_at: i64,
) -> Result<Vec<AnchoredWorldStateRecord>, String> {
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
    let permission_state = json!({
        "read": read_permission_label(permissions.read),
        "write": write_permission_label(permissions.write),
        "command": command_permission_label(permissions.command),
        "commandSafety": command_safety_label(permissions.command_safety),
        "patch": patch_permission_label(permissions.patch),
    });
    let permission_section = WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::EffectivePermissions,
        WorldStateLifetime::Conversation,
        permission_state.clone(),
        permission_state,
    )
    .map_err(|error| format!("无法构造权限 World State：{error}"))?;

    let workspace = context.and_then(|context| context.workspace.as_ref());
    let workspace_available = workspace
        .and_then(|workspace| workspace.root_path.as_deref())
        .map(str::trim)
        .is_some_and(|root| !root.is_empty());
    let workspace_state = json!({
        "available": workspace_available,
        "projectId": workspace.and_then(|workspace| workspace.project_id.as_deref()),
        "displayName": workspace.and_then(|workspace| workspace.display_name.as_deref()),
        "rootPath": workspace.and_then(|workspace| workspace.root_path.as_deref()),
    });
    let workspace_projection = json!({
        "available": workspace_available,
        "displayName": workspace.and_then(|workspace| workspace.display_name.as_deref()),
        "pathConvention": if workspace_available { "workspace_relative" } else { "no_workspace" },
    });
    let workspace_section = WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::WorkspaceBinding,
        WorldStateLifetime::Conversation,
        workspace_state,
        workspace_projection,
    )
    .map_err(|error| format!("无法构造 workspace World State：{error}"))?;

    let work_mode = match prompt_preferences.and_then(|preferences| preferences.work_mode) {
        Some(AgentPromptWorkMode::General) => "general",
        Some(AgentPromptWorkMode::Coding) | None => "coding",
    };
    let tone = match prompt_preferences.and_then(|preferences| preferences.tone) {
        Some(AgentPromptTone::Friendly) => "friendly",
        Some(AgentPromptTone::Pragmatic) | None => "pragmatic",
    };
    let detail_level = match prompt_preferences.and_then(|preferences| preferences.detail_level) {
        Some(AgentPromptDetailLevel::Low) => "low",
        Some(AgentPromptDetailLevel::High) => "high",
        Some(AgentPromptDetailLevel::Medium) | None => "medium",
    };
    let interaction_state = json!({
        "workMode": work_mode,
        "tone": tone,
        "detailLevel": detail_level,
    });
    let interaction_section = WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::InteractionProfile,
        WorldStateLifetime::Conversation,
        interaction_state.clone(),
        interaction_state,
    )
    .map_err(|error| format!("无法构造交互配置 World State：{error}"))?;

    // Model capabilities are execution authority, not prompting material. Keeping the section
    // HostOnly lets tools consult one coherent state model without teaching the model to assume a
    // capability that the runtime will still independently enforce.
    let capability_section = WorldStateSectionEnvelope::host_only(
        WorldStateSectionId::ModelCapabilities,
        WorldStateLifetime::Conversation,
        json!({
            "imageInput": model_capabilities.image_input,
        }),
    )
    .map_err(|error| format!("无法构造模型能力 World State：{error}"))?;

    Ok(vec![
        permission_section,
        workspace_section,
        interaction_section,
        capability_section,
    ])
}

fn read_permission_label(permission: mycopilot_core::AgentReadPermission) -> &'static str {
    match permission {
        mycopilot_core::AgentReadPermission::WorkspaceOnly => "workspace_only",
        mycopilot_core::AgentReadPermission::All => "all",
    }
}

fn write_permission_label(permission: mycopilot_core::AgentWritePermission) -> &'static str {
    match permission {
        mycopilot_core::AgentWritePermission::Denied => "denied",
        mycopilot_core::AgentWritePermission::WorkspaceOnly => "workspace_only",
        mycopilot_core::AgentWritePermission::All => "all",
    }
}

fn command_permission_label(permission: mycopilot_core::AgentCommandPermission) -> &'static str {
    match permission {
        mycopilot_core::AgentCommandPermission::RequireApproval => "require_approval",
        mycopilot_core::AgentCommandPermission::AutoApprove => "auto_approve",
    }
}

fn command_safety_label(permission: mycopilot_core::AgentCommandSafetyPolicy) -> &'static str {
    match permission {
        mycopilot_core::AgentCommandSafetyPolicy::Guarded => "guarded",
        mycopilot_core::AgentCommandSafetyPolicy::FullAccess => "full_access",
    }
}

fn patch_permission_label(permission: mycopilot_core::AgentPatchPermission) -> &'static str {
    match permission {
        mycopilot_core::AgentPatchPermission::RequireApproval => "require_approval",
        mycopilot_core::AgentPatchPermission::AutoApprove => "auto_approve",
    }
}
