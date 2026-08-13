use super::*;
use crate::context::ContextOrigin;
use crate::world_state::{
    effective_permissions_section, interaction_profile_section, model_capabilities_section,
    model_selection_section, workspace_binding_section, WorldStateDiff, WorldStateLifetime,
    WorldStateRecord, WorldStateSectionEnvelope, WorldStateSectionId, WorldStateSnapshot,
};

/// Exact run-scoped World State ledger used by the active tool loop.
///
/// It is deliberately independent from the durable conversation ledger: the latter is persisted
/// by the host, while this tracker follows effective tools and other state that only exists for
/// one logical run. Approval resume starts a new exact run-state epoch after restoring the frozen
/// context, so no checkpoint needs to reconstruct authoritative state from rendered model text.
pub(super) struct RunWorldStateTracker {
    current: WorldStateSnapshot,
}

impl RunWorldStateTracker {
    #[cfg(test)]
    pub(super) fn new(
        epoch_id: impl Into<String>,
        input: &AgentChatInput,
        tool_set: &EffectiveToolSet,
    ) -> AgentResult<Self> {
        Self::new_with_extension_sections(epoch_id, input, tool_set, Vec::new())
    }

    pub(super) fn new_with_extension_sections(
        epoch_id: impl Into<String>,
        input: &AgentChatInput,
        tool_set: &EffectiveToolSet,
        extension_sections: Vec<WorldStateSectionEnvelope>,
    ) -> AgentResult<Self> {
        let current = WorldStateSnapshot::new(
            epoch_id,
            0,
            run_world_state_sections(input, tool_set, extension_sections)?,
        )
        .map_err(world_state_error)?;
        current
            .model_projection(WorldStateLifetime::Run)
            .map_err(world_state_error)?;
        Ok(Self { current })
    }

    /// Rebases the exact backend-authoritative snapshot frozen at an approval boundary.
    ///
    /// The new epoch is a chronological marker for the resumed request; section authority is
    /// copied exactly and is never reconstructed from the sanitized text stored in ContextFrame.
    pub(super) fn from_checkpoint(
        epoch_id: impl Into<String>,
        snapshot: &WorldStateSnapshot,
    ) -> AgentResult<Self> {
        snapshot
            .model_projection(WorldStateLifetime::Run)
            .map_err(world_state_error)?;
        let current = snapshot.rebase(epoch_id).map_err(world_state_error)?;
        Ok(Self { current })
    }

    pub(super) fn snapshot(&self) -> &WorldStateSnapshot {
        &self.current
    }

    pub(super) fn full_context_item(&self) -> AgentResult<ContextItem> {
        let projection = self
            .current
            .model_projection(WorldStateLifetime::Run)
            .map_err(world_state_error)?;
        Ok(world_state_context_item(
            WorldStateRecord::Full(self.current.clone()),
            projection,
        ))
    }

    /// Advances the authoritative ledger and returns a model-visible diff when the effective Tool
    /// projection changed. Host-only or revision-only changes still advance the exact state but
    /// intentionally produce no model message.
    #[cfg(test)]
    pub(super) fn update_effective_tools(
        &mut self,
        tool_set: &EffectiveToolSet,
    ) -> AgentResult<Option<ContextItem>> {
        let mut sections = self
            .current
            .sections
            .iter()
            .filter(|section| section.id != WorldStateSectionId::EffectiveTools)
            .cloned()
            .collect::<Vec<_>>();
        sections.push(effective_tools_section(tool_set)?);
        let sequence = self
            .current
            .sequence
            .checked_add(1)
            .ok_or_else(|| AgentError::new("Run World State sequence 已溢出。"))?;
        let target = WorldStateSnapshot::new(self.current.epoch_id.clone(), sequence, sections)
            .map_err(world_state_error)?;
        if target.revision == self.current.revision {
            return Ok(None);
        }
        let diff = WorldStateDiff::between(&self.current, &target).map_err(world_state_error)?;
        let visible = diff
            .model_projection_against(&self.current, WorldStateLifetime::Run)
            .map_err(world_state_error)?;
        self.current = target;
        Ok(visible.map(|projection| world_state_diff_context_item(diff, projection)))
    }

    /// Reconciles every dynamic Run section from the same backend observations used by execution.
    ///
    /// Tool definitions, Skill activation and attachment authority are observed once by their
    /// owners, then projected into this durable-in-run ledger without re-running any tool.
    pub(super) fn reconcile(
        &mut self,
        tool_set: &EffectiveToolSet,
        extension_sections: Vec<WorldStateSectionEnvelope>,
        run_context: Option<&AgentRunContext>,
    ) -> AgentResult<Option<ContextItem>> {
        let dynamic_ids = [
            WorldStateSectionId::EffectiveTools,
            WorldStateSectionId::SkillActivation,
            WorldStateSectionId::AttachmentLibrarySummary,
        ];
        let mut sections = self
            .current
            .sections
            .iter()
            .filter(|section| !dynamic_ids.contains(&section.id))
            .cloned()
            .collect::<Vec<_>>();
        sections.push(effective_tools_section(tool_set)?);
        sections.extend(extension_sections);
        if let Some(section) = attachment_library_section(run_context)? {
            sections.push(section);
        }
        let sequence = self
            .current
            .sequence
            .checked_add(1)
            .ok_or_else(|| AgentError::new("Run World State sequence 已溢出。"))?;
        let target = WorldStateSnapshot::new(self.current.epoch_id.clone(), sequence, sections)
            .map_err(world_state_error)?;
        if target.revision == self.current.revision {
            return Ok(None);
        }
        let diff = WorldStateDiff::between(&self.current, &target).map_err(world_state_error)?;
        let visible = diff
            .model_projection_against(&self.current, WorldStateLifetime::Run)
            .map_err(world_state_error)?;
        self.current = target;
        Ok(visible.map(|projection| world_state_diff_context_item(diff, projection)))
    }
}

fn run_world_state_sections(
    input: &AgentChatInput,
    tool_set: &EffectiveToolSet,
    extension_sections: Vec<WorldStateSectionEnvelope>,
) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
    let mut sections = vec![
        effective_tools_section(tool_set)?,
        model_capabilities_section(input.model_capabilities, WorldStateLifetime::Run)
            .map_err(world_state_error)?,
    ];

    let extension_owns_skill_state = extension_sections
        .iter()
        .any(|section| section.id == WorldStateSectionId::SkillActivation);
    sections.extend(extension_sections);

    if !extension_owns_skill_state {
        if let Some(activation) = input
            .skill_activation
            .as_ref()
            .filter(|activation| !activation.skills.is_empty())
        {
            let skills = activation
                .skills
                .iter()
                .map(|skill| {
                    json!({
                        "id": skill.id,
                        "name": skill.name,
                        "revision": skill.revision,
                    })
                })
                .collect::<Vec<_>>();
            let state = json!({
                "activationRevision": activation.activation_revision,
                "skills": skills,
            });
            sections.push(
                WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::SkillActivation,
                    WorldStateLifetime::Run,
                    state.clone(),
                    state,
                )
                .map_err(world_state_error)?,
            );
        }
    }

    if let Some(section) = attachment_library_section(input.context.as_ref())? {
        sections.push(section);
    }

    // Direct library callers and a brand-new context preview may not have a backend conversation
    // ledger. Preserve a complete runtime contract in that compatibility path without duplicating
    // durable conversation sections for normal core-server runs.
    if input.world_state_records.is_empty() {
        sections.extend(fallback_conversation_sections(input)?);
    }

    Ok(sections)
}

fn attachment_library_section(
    context: Option<&AgentRunContext>,
) -> AgentResult<Option<WorldStateSectionEnvelope>> {
    let Some(library) = context.and_then(|context| context.attachment_library.as_ref()) else {
        return Ok(None);
    };
    let state = json!({
        "available": true,
        "conversationAttachmentCount": library.conversation_attachments.len(),
        "projectAttachmentCount": library.project_attachments.len(),
    });
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::AttachmentLibrarySummary,
        WorldStateLifetime::Run,
        state.clone(),
        state,
    )
    .map(Some)
    .map_err(world_state_error)
}

fn effective_tools_section(tool_set: &EffectiveToolSet) -> AgentResult<WorldStateSectionEnvelope> {
    let stable_tools = tool_set
        .stable_definitions()
        .iter()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    let dynamic_tools = tool_set
        .dynamic_definitions()
        .iter()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    let state = json!({
        "stableRevision": tool_set.stable_revision(),
        "dynamicRevision": tool_set.dynamic_revision(),
        "effectiveRevision": tool_set.revision(),
        "stableTools": stable_tools,
        "dynamicTools": dynamic_tools,
    });
    let projection = json!({
        "stableTools": stable_tools,
        "dynamicTools": dynamic_tools,
    });
    WorldStateSectionEnvelope::model_visible(
        WorldStateSectionId::EffectiveTools,
        WorldStateLifetime::Run,
        state,
        projection,
    )
    .map_err(world_state_error)
}

/// Renders the same provider-neutral World State diff that the runtime will append after a dynamic
/// Tool capability change. Capacity reservation must price this message rather than a parallel
/// availability notice with independent wording.
pub(super) fn effective_tools_transition_message(
    current_tool_set: &EffectiveToolSet,
    projected_tool_set: &EffectiveToolSet,
) -> AgentResult<Option<LlmMessage>> {
    let current = WorldStateSnapshot::new(
        "tool-capacity-projection",
        0,
        vec![effective_tools_section(current_tool_set)?],
    )
    .map_err(world_state_error)?;
    let target = WorldStateSnapshot::new(
        current.epoch_id.clone(),
        1,
        vec![effective_tools_section(projected_tool_set)?],
    )
    .map_err(world_state_error)?;
    if current.revision == target.revision {
        return Ok(None);
    }
    let diff = WorldStateDiff::between(&current, &target).map_err(world_state_error)?;
    diff.model_projection_against(&current, WorldStateLifetime::Run)
        .map_err(world_state_error)
        .map(|projection| {
            projection
                .map(|projection| LlmMessage::backend_state(projection.render_sanitized_text()))
        })
}

fn fallback_conversation_sections(
    input: &AgentChatInput,
) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
    let permissions = input
        .context
        .as_ref()
        .map(|context| context.permissions)
        .unwrap_or_default();
    let workspace = input
        .context
        .as_ref()
        .and_then(|context| context.workspace.as_ref());

    Ok(vec![
        effective_permissions_section(permissions, WorldStateLifetime::Run)
            .map_err(world_state_error)?,
        workspace_binding_section(workspace, WorldStateLifetime::Run).map_err(world_state_error)?,
        interaction_profile_section(input.prompt_preferences.as_ref(), WorldStateLifetime::Run)
            .map_err(world_state_error)?,
        model_selection_section(
            &input.model,
            input.model_capabilities,
            WorldStateLifetime::Run,
        )
        .map_err(world_state_error)?,
    ])
}

fn world_state_context_item(
    record: WorldStateRecord,
    projection: crate::world_state::WorldStateModelRecord,
) -> ContextItem {
    let WorldStateRecord::Full(_) = &record else {
        unreachable!("full World State context helper requires a full snapshot");
    };
    ContextItem::new(
        LlmMessage::backend_state(projection.render_sanitized_text()),
        ContextMetadata::new(
            ContextSource::WorldStateSnapshot,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::world_state_record(format!(
            "{}:{}",
            record.epoch_id(),
            record.sequence()
        ))),
    )
}

fn world_state_diff_context_item(
    diff: WorldStateDiff,
    projection: crate::world_state::WorldStateModelRecord,
) -> ContextItem {
    ContextItem::new(
        LlmMessage::backend_state(projection.render_sanitized_text()),
        ContextMetadata::new(
            ContextSource::WorldStateDiff,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::world_state_record(format!(
            "{}:{}",
            diff.epoch_id, diff.sequence
        ))),
    )
}

fn world_state_error(error: crate::world_state::WorldStateError) -> AgentError {
    AgentError::new(format!("Run World State 无效：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentAttachmentLibraryContext;
    use crate::tools::{ToolCapabilityId, SKILL_RESOURCES_READ_CAPABILITY};
    use std::collections::BTreeSet;

    #[test]
    fn run_snapshot_exposes_model_selection_but_keeps_execution_capabilities_host_only() {
        let input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "modelCapabilities": { "imageInput": true },
            "context": {
                "conversationId": null,
                "projectId": "project-secret-id",
                "workspace": {
                    "projectId": "project-secret-id",
                    "displayName": "Demo",
                    "rootPath": "/private/workspace/root"
                },
                "permissions": {
                    "read": "workspace_only",
                    "write": "workspace_only",
                    "command": "require_approval",
                    "commandSafety": "guarded",
                    "patch": "require_approval"
                }
            },
            "promptPreferences": {
                "workMode": "general",
                "tone": "friendly",
                "detailLevel": "high"
            },
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .unwrap();
        let capabilities =
            prepare_runtime_capabilities(&input, "world-state-test", &[], true, None).unwrap();
        let tracker =
            RunWorldStateTracker::new("run-world-state", &input, &capabilities.initial_tool_set)
                .unwrap();
        assert!(tracker
            .snapshot()
            .sections
            .iter()
            .all(|section| section.lifetime == WorldStateLifetime::Run));
        let frame = ContextFrame::new(vec![tracker.full_context_item().unwrap()]);
        frame.validate_cache_layout().unwrap();
        let rendered = frame.to_messages()[0].content().to_string();

        assert!(rendered.contains("\"lifetime\":\"run\""));
        assert!(rendered.contains("tools.effective"));
        assert!(rendered.contains("permissions.effective"));
        assert!(rendered.contains("model.selection"));
        assert!(rendered.contains("\"configuredModelId\":\"test-model\""));
        assert!(rendered.contains("\"imageInput\":true"));
        assert!(rendered.contains("\"workMode\":\"general\""));
        assert!(rendered.contains("\"displayName\":\"Demo\""));
        assert!(!rendered.contains("model.capabilities"));
        assert!(!rendered.contains("project-secret-id"));
        assert!(!rendered.contains("/private/workspace/root"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn effective_tool_change_appends_one_real_model_diff_and_no_noop() {
        let input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .unwrap();
        let capabilities =
            prepare_runtime_capabilities(&input, "world-state-tools", &[], true, None).unwrap();
        let mut tracker =
            RunWorldStateTracker::new("run-world-state", &input, &capabilities.initial_tool_set)
                .unwrap();

        assert!(tracker
            .update_effective_tools(&capabilities.initial_tool_set)
            .unwrap()
            .is_none());
        assert_eq!(tracker.snapshot().sequence, 0);

        let projected = capabilities
            .initial_tool_set
            .with_additional_capabilities(&BTreeSet::from([ToolCapabilityId::application_owned(
                SKILL_RESOURCES_READ_CAPABILITY,
            )]))
            .unwrap();
        let item = tracker
            .update_effective_tools(&projected)
            .unwrap()
            .expect("new dynamic tools must produce a model-visible diff");
        let frame = ContextFrame::new(vec![item]);
        frame.validate_cache_layout().unwrap();
        let messages = frame.to_messages();
        let rendered = messages[0].content();

        assert_eq!(tracker.snapshot().sequence, 1);
        assert!(rendered.contains("\"recordType\":\"diff\""));
        assert!(rendered.contains("tools.effective"));
        assert!(rendered.contains("skills_read_resource"));
        assert!(!rendered.contains("effective-tool-set-v"));
    }

    #[test]
    fn reconcile_tracks_skill_and_attachment_changes_without_duplicate_diffs() {
        let input = serde_json::from_value::<AgentChatInput>(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "test-model",
            "modelCapabilities": { "imageInput": false },
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .unwrap();
        let capabilities =
            prepare_runtime_capabilities(&input, "world-state-reconcile", &[], true, None).unwrap();
        let mut tracker =
            RunWorldStateTracker::new("run-world-state", &input, &capabilities.initial_tool_set)
                .unwrap();
        let skill_value = json!({
            "activationRevision": "skill-activation-test",
            "skills": [{ "id": "documents", "name": "Documents", "revision": "r1" }]
        });
        let skill_section = WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::SkillActivation,
            WorldStateLifetime::Run,
            skill_value.clone(),
            skill_value,
        )
        .unwrap();
        let run_context = AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: Some(AgentAttachmentLibraryContext {
                root_path: None,
                conversation_id: Some("conversation-1".to_string()),
                project_id: None,
                conversation_attachments: Vec::new(),
                project_attachments: Vec::new(),
            }),
            permissions: AgentPermissions::default(),
        };

        let first = tracker
            .reconcile(
                &capabilities.initial_tool_set,
                vec![skill_section.clone()],
                Some(&run_context),
            )
            .unwrap()
            .expect("new Skill and attachment summary must emit one diff");
        let rendered = ContextFrame::new(vec![first]).to_messages()[0]
            .content()
            .to_string();
        assert!(rendered.contains("skills.activation"));
        assert!(rendered.contains("attachments.library_summary"));
        assert_eq!(tracker.snapshot().sequence, 1);

        assert!(tracker
            .reconcile(
                &capabilities.initial_tool_set,
                vec![skill_section],
                Some(&run_context),
            )
            .unwrap()
            .is_none());
        assert_eq!(tracker.snapshot().sequence, 1);

        let removed = tracker
            .reconcile(&capabilities.initial_tool_set, Vec::new(), None)
            .unwrap()
            .expect("removed dynamic state must emit tombstones");
        assert!(ContextFrame::new(vec![removed]).to_messages()[0]
            .content()
            .contains("\"op\":\"remove\""));
        assert_eq!(tracker.snapshot().sequence, 2);
    }
}
