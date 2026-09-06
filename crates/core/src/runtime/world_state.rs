use super::*;
use crate::context::ContextOrigin;
use crate::world_state::{
    effective_permissions_section, environment_section, interaction_profile_section,
    model_capabilities_section, model_selection_section, workspace_binding_section, WorldStateDiff,
    WorldStateLifetime, WorldStateRecord, WorldStateSectionEnvelope, WorldStateSectionId,
    WorldStateSnapshot,
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
        validate_run_sections(&extension_sections)?;
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
        validate_run_sections(&snapshot.sections)?;
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
        validate_run_sections(&extension_sections)?;
        let dynamic_ids = [
            WorldStateSectionId::EffectiveTools,
            WorldStateSectionId::SkillActivation,
            WorldStateSectionId::AttachmentLibrarySummary,
        ];
        let mut sections = self
            .current
            .sections
            .iter()
            .filter(|section| {
                !dynamic_ids.contains(&section.id)
                    && !matches!(section.id, WorldStateSectionId::Extension(_))
            })
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

fn validate_run_sections(sections: &[WorldStateSectionEnvelope]) -> AgentResult<()> {
    if sections
        .iter()
        .any(|section| section.lifetime != WorldStateLifetime::Run)
    {
        return Err(AgentError::new(
            "Run World State 不接受会话生命周期的状态。",
        ));
    }
    Ok(())
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
    WorldStateSectionEnvelope::host_only(
        WorldStateSectionId::EffectiveTools,
        WorldStateLifetime::Run,
        state,
    )
    .map_err(world_state_error)
}

pub(super) fn conversation_base_sections(
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
        environment_section(WorldStateLifetime::Conversation).map_err(world_state_error)?,
        model_capabilities_section(input.model_capabilities, WorldStateLifetime::Conversation)
            .map_err(world_state_error)?,
        effective_permissions_section(permissions, WorldStateLifetime::Conversation)
            .map_err(world_state_error)?,
        workspace_binding_section(workspace, WorldStateLifetime::Conversation)
            .map_err(world_state_error)?,
        interaction_profile_section(
            input.prompt_preferences.as_ref(),
            WorldStateLifetime::Conversation,
        )
        .map_err(world_state_error)?,
        model_selection_section(
            &input.model,
            input.model_capabilities,
            WorldStateLifetime::Conversation,
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
    fn run_snapshot_keeps_tool_inventory_host_only_and_conversation_facts_separate() {
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
                    "patch": "require_approval",
                    "builtinExecution": "require_approval"
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
        for name in [
            "tools.effective",
            "permissions.effective",
            "model.selection",
            "model.capabilities",
            "project-secret-id",
            "/private/workspace/root",
            "secret",
        ] {
            assert!(!rendered.contains(name));
        }
        let inventory = tracker
            .snapshot()
            .sections
            .iter()
            .find(|section| section.id == WorldStateSectionId::EffectiveTools)
            .unwrap();
        assert_eq!(
            inventory.visibility,
            crate::world_state::WorldStateVisibility::HostOnly
        );
        assert!(!inventory.state["stableTools"]
            .as_array()
            .unwrap()
            .is_empty());
        let conversation = WorldStateSnapshot::new(
            "conversation-world-state",
            0,
            conversation_base_sections(&input).unwrap(),
        )
        .unwrap();
        assert!(conversation
            .sections
            .iter()
            .all(|section| section.lifetime == WorldStateLifetime::Conversation));
        let rendered = conversation
            .model_projection(WorldStateLifetime::Conversation)
            .unwrap()
            .render_sanitized_text();
        for fact in [
            "permissions.effective",
            "model.selection",
            "test-model",
            "general",
            "Demo",
        ] {
            assert!(rendered.contains(fact));
        }
        assert!(!rendered.contains("/private/workspace/root"));
        assert!(!rendered.contains("project-secret-id"));
    }

    #[test]
    fn effective_tool_change_advances_host_state_without_a_model_message() {
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
        assert!(tracker
            .update_effective_tools(&projected)
            .unwrap()
            .is_none());
        assert_eq!(tracker.snapshot().sequence, 1);
        let inventory = tracker
            .snapshot()
            .sections
            .iter()
            .find(|section| section.id == WorldStateSectionId::EffectiveTools)
            .unwrap();
        assert!(inventory.state["dynamicTools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "skills_read_resource"));
        assert!(tracker
            .update_effective_tools(&projected)
            .unwrap()
            .is_none());
        assert_eq!(tracker.snapshot().sequence, 1);
    }

    #[test]
    fn run_world_state_replaces_run_extension_snapshots_without_duplicates() {
        let input: AgentChatInput = serde_json::from_value(json!({
            "apiUrl":"https://example.test/v1/chat/completions", "apiToken":"unused",
            "model":"test-model", "modelCapabilities":{"imageInput":false}, "messages":[],
        }))
        .unwrap();
        let capabilities =
            prepare_runtime_capabilities(&input, "extension-state", &[], false, None).unwrap();
        let section = |enabled| {
            WorldStateSectionEnvelope::model_visible(
                WorldStateSectionId::extension("runtime.fixture").unwrap(),
                WorldStateLifetime::Run,
                json!({"available":enabled}),
                json!({"available":enabled}),
            )
            .unwrap()
        };
        let mut tracker = RunWorldStateTracker::new_with_extension_sections(
            "extension-state",
            &input,
            &capabilities.initial_tool_set,
            vec![section(true)],
        )
        .unwrap();
        assert!(tracker
            .reconcile(&capabilities.initial_tool_set, vec![section(true)], None)
            .unwrap()
            .is_none());
        assert!(tracker
            .reconcile(&capabilities.initial_tool_set, vec![section(false)], None)
            .unwrap()
            .is_some());
        assert_eq!(
            tracker
                .snapshot()
                .sections
                .iter()
                .filter(|s| s.id.as_str() == "runtime.fixture")
                .count(),
            1
        );
        assert!(tracker
            .reconcile(&capabilities.initial_tool_set, Vec::new(), None)
            .unwrap()
            .is_some());
        assert!(!tracker
            .snapshot()
            .sections
            .iter()
            .any(|s| s.id.as_str() == "runtime.fixture"));
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

    #[test]
    fn run_tracker_rejects_conversation_sections_in_construction_reconcile_and_restore() {
        let input: AgentChatInput = serde_json::from_value(json!({
            "apiUrl":"https://example.test/v1/chat/completions", "apiToken":"unused",
            "model":"test-model", "modelCapabilities":{"imageInput":false}, "messages":[],
        }))
        .unwrap();
        let capabilities =
            prepare_runtime_capabilities(&input, "separate-lifetimes", &[], false, None).unwrap();
        let sections = conversation_base_sections(&input).unwrap();
        assert!(RunWorldStateTracker::new_with_extension_sections(
            "separate-lifetimes",
            &input,
            &capabilities.initial_tool_set,
            sections.clone()
        )
        .is_err());
        let mut tracker =
            RunWorldStateTracker::new("separate-lifetimes", &input, &capabilities.initial_tool_set)
                .unwrap();
        let previous = tracker.snapshot().clone();
        assert!(tracker
            .reconcile(&capabilities.initial_tool_set, sections.clone(), None)
            .is_err());
        assert_eq!(tracker.snapshot(), &previous);
        let conversation = WorldStateSnapshot::new("conversation", 0, sections).unwrap();
        assert!(RunWorldStateTracker::from_checkpoint("restored-run", &conversation).is_err());
    }
}
