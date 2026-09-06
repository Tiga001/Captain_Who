use super::*;
use mycopilot_core::{
    world_state::{
        effective_permissions_section, environment_section, interaction_profile_section,
        model_capabilities_section, model_selection_section, workspace_binding_section,
    },
    AnchoredWorldStateRecord, WorldStateLifetime, WorldStateSectionEnvelope,
};

pub(crate) fn load_conversation_world_state(
    storage: &StorageService,
    conversation_id: &str,
) -> Result<Vec<AnchoredWorldStateRecord>, String> {
    storage
        .list_active_conversation_world_state_records(conversation_id)?
        .into_iter()
        .map(|entry| {
            {
                let anchored = entry.anchored_record();
                anchored.validate().map(|()| anchored)
            }
            .map_err(|error| format!("Conversation World State anchor 无效：{error}"))
        })
        .collect()
}

pub(crate) fn conversation_world_state_sections(
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    model_id: &str,
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

    let environment_section = environment_section(WorldStateLifetime::Conversation)
        .map_err(|error| format!("无法构造 environment World State：{error}"))?;

    // The complete capability record remains Host-only execution authority. `model.selection`
    // below is the deliberately narrow model-visible projection; tools still enforce the frozen
    // Host capability independently.
    let capability_section =
        model_capabilities_section(model_capabilities, WorldStateLifetime::Conversation)
            .map_err(|error| format!("无法构造模型能力 World State：{error}"))?;
    let selection_section = model_selection_section(
        model_id,
        model_capabilities,
        WorldStateLifetime::Conversation,
    )
    .map_err(|error| format!("无法构造模型选择 World State：{error}"))?;

    Ok(vec![
        permission_section,
        workspace_section,
        interaction_section,
        environment_section,
        selection_section,
        capability_section,
    ])
}
