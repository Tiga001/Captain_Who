pub(super) fn resolve_pending_action_storage_id(
    pending_actions: &HashMap<String, PendingActionRecord>,
    run_id: &str,
    action_id: &str,
) -> Option<String> {
    let canonical = pending_action_storage_id(run_id, action_id);
    pending_actions
        .contains_key(&canonical)
        .then_some(canonical)
}

pub(super) fn pending_storage_record(
    record: &PendingActionRecord,
    updated_at: i64,
) -> Result<AgentPendingActionRecord, String> {
    Ok(AgentPendingActionRecord {
        action_id: record.storage_id.clone(),
        run_id: record.snapshot.run_id.clone(),
        conversation_id: record.snapshot.conversation_id.clone(),
        assistant_message_id: record.snapshot.assistant_message_id.clone(),
        action_type: record.snapshot.action_type.clone(),
        tool_name: record.snapshot.tool_name.clone(),
        tool_call_id: record.snapshot.tool_call_id.clone(),
        status: pending_status_label(record.snapshot.status).to_string(),
        target_status: None,
        action_json: serialize_json(&record.snapshot.action),
        agent_input_json: persisted_pending_agent_input_json(
            &record.agent_input,
            record.snapshot.status,
        )?,
        created_at: record.snapshot.created_at,
        updated_at,
    })
}

pub(super) fn same_pending_action_identity(
    existing: &PendingActionRecord,
    candidate: &PendingActionRecord,
) -> bool {
    existing.storage_id == candidate.storage_id
        && existing.snapshot.action_id == candidate.snapshot.action_id
        && existing.snapshot.run_id == candidate.snapshot.run_id
        && existing.snapshot.conversation_id == candidate.snapshot.conversation_id
        && existing.snapshot.assistant_message_id == candidate.snapshot.assistant_message_id
        && existing.snapshot.action_type == candidate.snapshot.action_type
        && existing.snapshot.tool_name == candidate.snapshot.tool_name
        && existing.snapshot.tool_call_id == candidate.snapshot.tool_call_id
        && existing.snapshot.status == candidate.snapshot.status
        && serialize_json(&existing.snapshot.action) == serialize_json(&candidate.snapshot.action)
        && same_persisted_pending_agent_input(existing, candidate)
}

fn same_persisted_pending_agent_input(
    existing: &PendingActionRecord,
    candidate: &PendingActionRecord,
) -> bool {
    match (
        persisted_pending_agent_input_json(&existing.agent_input, existing.snapshot.status),
        persisted_pending_agent_input_json(&candidate.agent_input, candidate.snapshot.status),
    ) {
        (Ok(existing), Ok(candidate)) => existing == candidate,
        _ => false,
    }
}

fn same_pending_action_identity_except_status(
    existing: &PendingActionRecord,
    candidate: &PendingActionRecord,
) -> bool {
    existing.storage_id == candidate.storage_id
        && existing.snapshot.action_id == candidate.snapshot.action_id
        && existing.snapshot.run_id == candidate.snapshot.run_id
        && existing.snapshot.conversation_id == candidate.snapshot.conversation_id
        && existing.snapshot.assistant_message_id == candidate.snapshot.assistant_message_id
        && existing.snapshot.action_type == candidate.snapshot.action_type
        && existing.snapshot.tool_name == candidate.snapshot.tool_name
        && existing.snapshot.tool_call_id == candidate.snapshot.tool_call_id
        && serialize_json(&existing.snapshot.action) == serialize_json(&candidate.snapshot.action)
}

pub(super) fn persisted_pending_agent_input_json(
    agent_input: &AgentChatInput,
    status: PendingActionStatus,
) -> Result<String, String> {
    let mut persisted_agent_input = agent_input.clone();
    // The explicit allowlist DTO below, rather than mutation of a full AgentChatInput
    // serialization, is the security boundary. Secret-bearing fields may remain in this
    // short-lived clone because `PersistedAgentResumeInput::from_agent_input` records only
    // non-secret identities and credential-required booleans.
    // Pending actions must survive restart, while an approved action may still be executing and
    // need its continuation input. Once the action is terminal, the live continuation owns any
    // remaining in-memory copy; the durable row only retains Skill identity and revision metadata.
    if pending_status_redacts_run_scoped_input(status) {
        persisted_agent_input.skill_discovery = None;
        if let Some(activation) = persisted_agent_input.skill_activation.as_mut() {
            for skill in &mut activation.skills {
                skill.instructions.clear();
            }
        }
        if let Some(checkpoint) = persisted_agent_input.resume_checkpoint.as_mut() {
            for item in &mut checkpoint.context_items {
                let is_skill_context = item.sources.iter().any(|source| {
                    matches!(source.as_str(), "skill_instructions" | "skill_catalog")
                }) || item
                    .origin
                    .as_ref()
                    .is_some_and(|origin| origin.kind == "skill");
                if is_skill_context {
                    item.content.clear();
                }
            }
            mycopilot_core::redact_terminal_skill_discovery(checkpoint);
        }
    }
    Ok(PersistedAgentResumeInput::from_agent_input(&persisted_agent_input)?.encode())
}

pub(super) fn pending_status_redacts_run_scoped_input(status: PendingActionStatus) -> bool {
    match status {
        PendingActionStatus::Pending
        | PendingActionStatus::Approved
        | PendingActionStatus::Executing => false,
        PendingActionStatus::Rejected
        | PendingActionStatus::Cancelled
        | PendingActionStatus::Completed
        | PendingActionStatus::Failed => true,
    }
}

fn validate_current_effective_provider_protocol(
    frozen_revision: &str,
    current_protocol_revision: &str,
    model: &mycopilot_core::storage::models::ModelConfigRecord,
    connection: &mycopilot_core::storage::models::ModelConnectionConfig,
    frozen_profile: &mycopilot_core::ProviderProfileConfig,
    frozen_key: &ProviderProtocolKey,
) -> Result<(), String> {
    if !mycopilot_core::storage::config_repository::is_provider_protocol_revision(frozen_revision) {
        return Err("frozen pending-action Provider Protocol revision is invalid".to_string());
    }
    if frozen_revision != current_protocol_revision {
        return Err("frozen pending-action Provider Protocol no longer matches".to_string());
    }

    let current_dialect = ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
    let current_profile = model
        .resolved_provider_profile_config(current_dialect)
        .map_err(|_| "frozen pending-action Provider Profile is unavailable".to_string())?;
    if &current_profile != frozen_profile
        || frozen_key.dialect != current_dialect
        || frozen_key.model_id != model.provider_model_id
    {
        return Err("frozen pending-action Provider Protocol no longer matches".to_string());
    }
    Ok(())
}

pub(super) fn restore_agent_input_secrets(
    storage: &Arc<StorageService>,
    persisted: DecodedPersistedAgentResumeInput,
) -> Result<AgentChatInput, String> {
    let mut agent_input = persisted.agent_input;
    let model_config_id = agent_input
        .model_config_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "frozen pending-action local model configuration is unavailable".to_string()
        })?;
    let settings_snapshot = storage
        .load_model_settings_snapshot_for_model(model_config_id, false)
        .map_err(|_| "failed to resolve frozen pending-action provider settings".to_string())?
        .ok_or_else(|| "frozen pending-action provider settings are unavailable".to_string())?;
    let current_provider_connection_revision = settings_snapshot
        .provider_connection_revisions
        .get(model_config_id)
        .ok_or_else(|| "frozen pending-action provider connection is unavailable".to_string())?;
    if current_provider_connection_revision != &persisted.provider_connection_revision {
        return Err("frozen pending-action provider connection no longer matches".to_string());
    }
    let settings = settings_snapshot.settings;

    let conversation_model_id = match agent_input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
    {
        Some(conversation_id) => storage
            .load_conversation(conversation_id)
            .map_err(|_| "failed to verify frozen pending-action conversation".to_string())?
            .and_then(|conversation| conversation.model_id),
        None => None,
    };
    if conversation_model_id
        .as_deref()
        .is_some_and(|model_id| model_id != model_config_id)
    {
        return Err(
            "frozen pending-action model identity no longer matches its conversation".to_string(),
        );
    }
    let model = settings
        .models
        .iter()
        .find(|model| model.id == model_config_id)
        .ok_or_else(|| "frozen pending-action model configuration is unavailable".to_string())?;

    // Pending actions deliberately persist without API tokens. On restoration, resolve the
    // exact model-specific-or-global pair used by the frozen run. Endpoint changes fail closed;
    // credentials are rehydrated only after the endpoint digest has matched.
    let connection = settings.effective_connection_for(model).map_err(|_| {
        let credential_missing = match (
            model.api_url_override.as_deref(),
            model.api_token_override.as_deref(),
        ) {
            (Some(url), Some(token)) => !url.trim().is_empty() && token.trim().is_empty(),
            (None, None) => {
                !settings.api_url.trim().is_empty() && settings.api_token.trim().is_empty()
            }
            _ => false,
        };
        if persisted.provider_credential_required && credential_missing {
            "frozen pending-action provider credential is unavailable".to_string()
        } else {
            "frozen pending-action provider connection is unavailable".to_string()
        }
    })?;
    if persisted_endpoint_digest(&connection.api_url) != persisted.provider_endpoint_digest {
        return Err("frozen pending-action provider endpoint no longer matches".to_string());
    }
    let provider_profile_config = agent_input
        .provider_profile_config
        .as_ref()
        .ok_or_else(|| "frozen pending-action Provider Profile is unavailable".to_string())?;
    let provider_protocol_key = agent_input
        .provider_protocol_key
        .as_ref()
        .ok_or_else(|| "frozen pending-action Provider Protocol is unavailable".to_string())?;
    provider_protocol_key
        .validate_against_config(provider_profile_config)
        .map_err(|_| "frozen pending-action Provider Protocol is invalid".to_string())?;
    if provider_protocol_key.model_id != agent_input.model
        || provider_protocol_key
            .provider_configuration_revision
            .as_deref()
            != Some(persisted.provider_configuration_revision.as_str())
    {
        return Err("frozen pending-action Provider Protocol provenance diverged".to_string());
    }
    // The profile/key/capabilities belong to the already-running approval checkpoint. A settings
    // edit may rotate the current model's Provider Protocol while this action is pending; that new
    // identity is authoritative only for the next Run. The connection revision and endpoint digest
    // above still fail closed on endpoint/token drift before any credential is rehydrated.
    if let Some(checkpoint) = agent_input.resume_checkpoint.as_ref() {
        if &checkpoint.provider_profile_config != provider_profile_config
            || &checkpoint.provider_protocol_key != provider_protocol_key
        {
            return Err("frozen pending-action checkpoint Provider Protocol diverged".to_string());
        }
    }
    let provider_credential_present = !connection.api_token.trim().is_empty();
    if provider_credential_present != persisted.provider_credential_required {
        return Err(
            "frozen pending-action provider credential presence no longer matches".to_string(),
        );
    }
    agent_input.api_url = connection.api_url;
    agent_input.api_token = connection.api_token;

    // Search is live Host policy, independent of the frozen Provider connection. A toggle or
    // credential edit during a pause must neither invalidate this Run nor rehydrate search
    // credentials into its continuation. Runtime admission reads authoritative settings again.
    if let Some(search) = agent_input.search_config.as_mut() {
        search.tavily_api_key = None;
    }
    crate::application::agent_support::hydrate_context_image_attachments(storage, &mut agent_input)?;
    Ok(agent_input)
}

fn bind_pending_provider_configuration(
    storage: &Arc<StorageService>,
    agent_input: AgentChatInput,
) -> Result<AgentChatInput, String> {
    let provider_configuration_revision = agent_input
        .provider_configuration_revision
        .as_deref()
        .filter(|revision| {
            mycopilot_core::storage::config_repository::is_provider_protocol_revision(revision)
        })
        .ok_or_else(|| "pending-action Provider Protocol revision is unavailable".to_string())?;
    let model_config_id = agent_input
        .model_config_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "pending-action local model configuration is unavailable".to_string())?;
    let snapshot = storage
        .load_model_settings_snapshot_for_model(model_config_id, false)
        .map_err(|_| "failed to freeze pending-action provider configuration".to_string())?
        .ok_or_else(|| "pending-action provider configuration is unavailable".to_string())?;
    let model = snapshot
        .settings
        .models
        .iter()
        .find(|model| model.id == model_config_id)
        .ok_or_else(|| "pending-action model configuration is unavailable".to_string())?;
    let connection = snapshot
        .settings
        .effective_connection_for(model)
        .map_err(|_| "pending-action provider connection is unavailable".to_string())?;
    let current_provider_connection_revision = snapshot
        .provider_connection_revisions
        .get(&model.id)
        .ok_or_else(|| "pending-action provider connection identity is unavailable".to_string())?;
    let current_provider_protocol_revision = snapshot
        .provider_protocol_revisions
        .get(&model.id)
        .ok_or_else(|| "pending-action Provider Protocol identity is unavailable".to_string())?;
    if agent_input.provider_connection_revision.as_ref()
        != Some(current_provider_connection_revision)
    {
        return Err("pending-action provider connection changed before persistence".to_string());
    }
    let provider_profile_config = agent_input
        .provider_profile_config
        .as_ref()
        .ok_or_else(|| "pending-action Provider Profile is unavailable".to_string())?;
    provider_profile_config
        .validate()
        .map_err(|_| "pending-action Provider Profile is invalid".to_string())?;
    let provider_protocol_key = agent_input
        .provider_protocol_key
        .as_ref()
        .ok_or_else(|| "pending-action Provider Protocol is unavailable".to_string())?;
    provider_protocol_key
        .validate_against_config(provider_profile_config)
        .map_err(|_| "pending-action Provider Protocol is invalid".to_string())?;
    if provider_protocol_key.model_id != agent_input.model
        || provider_protocol_key
            .provider_configuration_revision
            .as_deref()
            != Some(provider_configuration_revision)
    {
        return Err("pending-action Provider Protocol provenance is inconsistent".to_string());
    }
    validate_current_effective_provider_protocol(
        provider_configuration_revision,
        current_provider_protocol_revision,
        model,
        &connection,
        provider_profile_config,
        provider_protocol_key,
    )?;
    if connection.api_url != agent_input.api_url
        || connection.api_token != agent_input.api_token
        || model.supports_image != agent_input.model_capabilities.image_input
        || agent_input
            .context_window_tokens
            .is_some_and(|tokens| tokens != model.effective_context_window_tokens())
    {
        return Err(
            "pending-action provider configuration does not match the active run".to_string(),
        );
    }
    if let Some(checkpoint) = agent_input.resume_checkpoint.as_ref() {
        if &checkpoint.provider_profile_config != provider_profile_config
            || &checkpoint.provider_protocol_key != provider_protocol_key
        {
            return Err("pending-action checkpoint Provider Protocol is inconsistent".to_string());
        }
    }
    Ok(agent_input)
}
