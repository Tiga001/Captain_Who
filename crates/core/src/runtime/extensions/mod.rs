//! Typed runtime extensions for agent-loop capabilities.
//!
//! Extensions may contribute request-only context, register normal agent tools, react to
//! completed runtime events, and persist versioned state across an approval pause. They do not
//! mutate the loop or emit frontend events directly; the runtime remains the single owner of
//! control flow and applies explicit effects returned by extensions.
//!
//! Per model request, the runtime first clones retained context, then atomically appends extension
//! contributions and runtime-owned guards before sending the provider request. Blocking operations
//! such as context compaction remain runtime-owned: after replacing retained context, the runtime
//! restarts request preparation so transient Todo state and guards are injected exactly once into
//! the rebuilt agent-work request. Extensions may later contribute purpose-specific context to a
//! compaction-generation request, but they never control that restart.

mod builtin_capability;
mod human_interaction;
mod skills;
mod todo;

use crate::context::{ContextFrame, ContextItem, ContextTextBudget};
use crate::protocol::{
    AgentError, AgentEvent, AgentExtensionSnapshot, AgentResult, AgentSkillActivation,
    AgentTodoState, AgentToolResult,
};
use crate::runtime::AgentSkillActivationResolver;
use crate::skills::{AgentSkillDiscoverySnapshot, SkillResourceSession};
use crate::tools::{AgentTool, EffectiveToolSet, ToolCapabilityId, ToolRegistry};
use crate::world_state::WorldStateSectionEnvelope;
use builtin_capability::BuiltinCapabilityExtension;
use serde_json::Value;
pub(in crate::runtime) use skills::checkpoint_authority_from_snapshots;
pub(in crate::runtime) use skills::redact_discovery_from_snapshots;
use skills::{SkillActivationExtension, SKILL_EXTENSION_ID};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use todo::{TodoExtension, TodoStateHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExtensionDescriptor {
    pub(super) id: &'static str,
    pub(super) version: u32,
    pub(super) order: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModelRequestPurpose {
    AgentWork,
    ContextCompaction,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ModelRequestContext {
    pub(super) purpose: ModelRequestPurpose,
}

/// Capacity left after the exact request frame has passed the runtime's sendability gate.
///
/// The allowance carries the same estimator used for that request. Extensions may reserve part
/// of it for tool protocol messages before accepting context that will be retained on the next
/// request.
#[derive(Debug, Clone)]
pub(super) struct ModelInputCapacity {
    pub(super) remaining_tokens: u64,
    pub(super) text_budget: ContextTextBudget,
    /// Exact effective Tool contract already charged to the current request.
    ///
    /// Skill activation projects this immutable baseline forward before committing activation
    /// state, ensuring newly exposed schemas and their retained World State diff fit too.
    pub(super) effective_tool_set: EffectiveToolSet,
}

#[derive(Debug)]
pub(super) struct DynamicToolCapacityProjection {
    pub(super) additional_tokens: u64,
    pub(super) effective_tool_set: EffectiveToolSet,
}

impl ModelInputCapacity {
    /// Projects and prices the next request's Skill-gated Tool contract without mutating runtime
    /// state.
    ///
    /// Existing dynamic schemas and the current Run World State were already charged by the capacity
    /// detector for the current request. Only positive per-category deltas are reserved here.
    /// Avoiding cross-category offsets is intentionally conservative and keeps activation
    /// fail-closed if a future prompt wording change happens to shrink one category.
    fn project_additional_tool_capabilities(
        &self,
        additional_capabilities: &BTreeSet<ToolCapabilityId>,
    ) -> AgentResult<DynamicToolCapacityProjection> {
        let projected = self
            .effective_tool_set
            .with_additional_capabilities(additional_capabilities)?;

        let current_schema_tokens = self
            .text_budget
            .estimate_tool_definitions(self.effective_tool_set.dynamic_definitions());
        let projected_schema_tokens = self
            .text_budget
            .estimate_tool_definitions(projected.dynamic_definitions());
        let schema_delta = projected_schema_tokens.saturating_sub(current_schema_tokens);

        let state_transition_tokens =
            crate::runtime::world_state::effective_tools_transition_message(
                &self.effective_tool_set,
                &projected,
            )?
            .map(|message| self.text_budget.estimate_message(&message))
            .unwrap_or(0);

        Ok(DynamicToolCapacityProjection {
            additional_tokens: schema_delta.saturating_add(state_transition_tokens),
            effective_tool_set: projected,
        })
    }
}

impl ModelRequestContext {
    pub(super) fn agent_work() -> Self {
        Self {
            purpose: ModelRequestPurpose::AgentWork,
        }
    }
}

pub(super) enum RuntimeExtensionEvent<'a> {
    /// The tool-owned durable projection, never the raw runtime result. Extensions must not gain
    /// access to transient binary model-delivery payloads.
    ToolCompleted { result: &'a AgentToolResult },
}

pub(super) enum RuntimeEffect {
    EmitEvent(Box<AgentEvent>),
    AppendRetainedContext(Box<ContextItem>),
}

trait RuntimeExtension: Send {
    fn descriptor(&self) -> ExtensionDescriptor;

    /// Freezes any live policy once, before schema/capabilities, World State and request context
    /// are projected. Those projections must not independently re-read mutable Host state.
    fn prepare_model_request(&mut self) -> AgentResult<()> {
        Ok(())
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        Vec::new()
    }

    fn register_additional_tools(&self, _registry: &mut ToolRegistry) -> AgentResult<()> {
        Ok(())
    }

    fn active_tool_capabilities(&self) -> AgentResult<BTreeSet<ToolCapabilityId>> {
        Ok(BTreeSet::new())
    }

    fn request_context(&self, _request: &ModelRequestContext) -> AgentResult<Vec<ContextItem>> {
        Ok(Vec::new())
    }

    /// Exact provider-neutral Run World State contributed by this extension.
    ///
    /// This is distinct from request context and frontend events. The runtime folds all extension
    /// sections through the same Run World State reducer before the next model request.
    fn world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        Ok(Vec::new())
    }

    fn update_model_input_capacity(&mut self, _capacity: Option<ModelInputCapacity>) {}

    fn consume_model_input_capacity(&mut self, _tokens: u64) {}

    fn on_event(&mut self, _event: &RuntimeExtensionEvent<'_>) -> AgentResult<Vec<RuntimeEffect>> {
        Ok(Vec::new())
    }

    fn snapshot_state(&self) -> AgentResult<Value>;

    fn restore_state(&mut self, version: u32, state: Value) -> AgentResult<()>;
}

pub(super) struct RuntimeExtensions {
    extensions: Vec<Box<dyn RuntimeExtension>>,
    todo: Option<TodoStateHandle>,
}

#[derive(Default)]
pub(super) struct RuntimeExtensionHostServices {
    pub(super) builtin_capabilities: Option<crate::BuiltinCapabilityRuntime>,
    pub(super) human_interaction_policy: Option<Arc<dyn crate::HumanInteractionPolicySource>>,
    pub(super) human_interaction_execution_ready: bool,
    pub(super) human_interaction_async_execution_ready: bool,
    pub(super) human_root: bool,
}

impl RuntimeExtensions {
    #[cfg(test)]
    pub(super) fn for_run(run_id: &str, snapshots: &[AgentExtensionSnapshot]) -> AgentResult<Self> {
        Self::for_run_with_skills(run_id, None, None, None, None, snapshots)
    }

    #[cfg(test)]
    pub(super) fn for_run_with_skills(
        run_id: &str,
        discovery: Option<AgentSkillDiscoverySnapshot>,
        initial_activation: Option<&AgentSkillActivation>,
        activation_resolver: Option<AgentSkillActivationResolver>,
        skill_resources: Option<Arc<SkillResourceSession>>,
        snapshots: &[AgentExtensionSnapshot],
    ) -> AgentResult<Self> {
        Self::for_run_with_capabilities(
            run_id,
            discovery,
            initial_activation,
            activation_resolver,
            skill_resources,
            RuntimeExtensionHostServices::default(),
            snapshots,
        )
    }

    pub(super) fn for_run_with_capabilities(
        run_id: &str,
        discovery: Option<AgentSkillDiscoverySnapshot>,
        initial_activation: Option<&AgentSkillActivation>,
        activation_resolver: Option<AgentSkillActivationResolver>,
        skill_resources: Option<Arc<SkillResourceSession>>,
        host_services: RuntimeExtensionHostServices,
        snapshots: &[AgentExtensionSnapshot],
    ) -> AgentResult<Self> {
        // A checkpoint is the authority for the logical run. Resume payloads may be rebuilt from
        // newer UI or settings state, so they must not replace the catalog or activations frozen
        // before an approval pause. The Skill snapshot restores both below.
        let restores_skill_state = snapshots
            .iter()
            .any(|snapshot| snapshot.extension_id == SKILL_EXTENSION_ID);
        let skills = if restores_skill_state {
            // The checkpoint snapshot is the logical authority, while `skill_resources` is the
            // matching Host-only byte authority restored from its immutable selections. Keep the
            // shell private until `from_extensions` has restored and jointly validated both.
            SkillActivationExtension::new_for_checkpoint_restore(
                run_id.to_string(),
                activation_resolver,
                skill_resources,
            )
        } else {
            SkillActivationExtension::new(
                run_id.to_string(),
                discovery,
                initial_activation,
                activation_resolver,
                skill_resources,
            )?
        };
        let (todo, todo_handle) = TodoExtension::new(run_id.to_string());
        let mut extensions: Vec<Box<dyn RuntimeExtension>> = vec![Box::new(skills)];
        if let Some(runtime) = host_services.builtin_capabilities {
            extensions.push(Box::new(BuiltinCapabilityExtension::new(
                run_id.to_string(),
                runtime,
            )));
        }
        if host_services.human_root
            && (host_services.human_interaction_policy.is_some()
                || snapshots.iter().any(|snapshot| {
                    snapshot.extension_id == human_interaction::HUMAN_INTERACTION_EXTENSION_ID
                }))
        {
            extensions.push(Box::new(
                human_interaction::HumanInteractionExtension::new(
                    host_services.human_interaction_policy,
                )
                .with_execution_ready(host_services.human_interaction_execution_ready)
                .with_async_execution_ready(host_services.human_interaction_async_execution_ready),
            ));
        }
        extensions.push(Box::new(todo));
        Self::from_extensions(extensions, Some(todo_handle), snapshots)
    }

    fn from_extensions(
        mut extensions: Vec<Box<dyn RuntimeExtension>>,
        todo: Option<TodoStateHandle>,
        snapshots: &[AgentExtensionSnapshot],
    ) -> AgentResult<Self> {
        validate_extensions(&extensions)?;
        extensions.sort_by_key(|extension| {
            let descriptor = extension.descriptor();
            (descriptor.order, descriptor.id)
        });

        let mut snapshots_by_id = BTreeMap::new();
        for snapshot in snapshots {
            if snapshots_by_id
                .insert(snapshot.extension_id.as_str(), snapshot)
                .is_some()
            {
                return Err(AgentError::new(format!(
                    "扩展状态无效：`{}` 出现了重复快照。",
                    snapshot.extension_id
                )));
            }
        }

        for extension in &mut extensions {
            let descriptor = extension.descriptor();
            if let Some(snapshot) = snapshots_by_id.remove(descriptor.id) {
                extension.restore_state(snapshot.version, snapshot.state.clone())?;
            }
        }
        if let Some(unknown_id) = snapshots_by_id.keys().next() {
            return Err(AgentError::new(format!(
                "无法恢复扩展状态：当前运行未注册扩展 `{unknown_id}`。"
            )));
        }

        Ok(Self { extensions, todo })
    }

    pub(super) fn register_tools(&self, registry: &mut ToolRegistry) -> AgentResult<()> {
        for extension in &self.extensions {
            let descriptor = extension.descriptor();
            extension.register_additional_tools(registry)?;
            for tool in extension.tools() {
                registry.register_extension_tool(descriptor.id, tool)?;
            }
        }
        Ok(())
    }

    pub(super) fn prepare_model_request(&mut self) -> AgentResult<()> {
        for extension in &mut self.extensions {
            extension.prepare_model_request()?;
        }
        Ok(())
    }

    pub(super) fn active_tool_capabilities(&self) -> AgentResult<BTreeSet<ToolCapabilityId>> {
        let mut capabilities = BTreeSet::new();
        for extension in &self.extensions {
            capabilities.extend(extension.active_tool_capabilities()?);
        }
        Ok(capabilities)
    }

    pub(super) fn contribute_request_context(
        &self,
        request: &ModelRequestContext,
        context: &mut ContextFrame,
    ) -> AgentResult<()> {
        let mut contributions = Vec::new();
        for extension in &self.extensions {
            contributions.extend(extension.request_context(request)?);
        }
        for item in contributions {
            context.push(item);
        }
        Ok(())
    }

    pub(super) fn world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let mut sections = Vec::new();
        for extension in &self.extensions {
            sections.extend(extension.world_state_sections()?);
        }
        Ok(sections)
    }

    pub(super) fn update_model_input_capacity(&mut self, capacity: Option<ModelInputCapacity>) {
        for extension in &mut self.extensions {
            extension.update_model_input_capacity(capacity.clone());
        }
    }

    pub(super) fn consume_model_input_capacity(&mut self, tokens: u64) {
        for extension in &mut self.extensions {
            extension.consume_model_input_capacity(tokens);
        }
    }

    pub(super) fn on_event(
        &mut self,
        event: RuntimeExtensionEvent<'_>,
    ) -> AgentResult<Vec<RuntimeEffect>> {
        let mut effects = Vec::new();
        for extension in &mut self.extensions {
            effects.extend(extension.on_event(&event)?);
        }
        Ok(effects)
    }

    pub(super) fn snapshots(&self) -> AgentResult<Vec<AgentExtensionSnapshot>> {
        self.extensions
            .iter()
            .map(|extension| {
                let descriptor = extension.descriptor();
                Ok(AgentExtensionSnapshot {
                    extension_id: descriptor.id.to_string(),
                    version: descriptor.version,
                    state: extension.snapshot_state()?,
                })
            })
            .collect()
    }

    pub(super) fn todo_state(&self) -> Option<AgentTodoState> {
        self.todo.as_ref().map(TodoStateHandle::state)
    }
}

fn validate_extensions(extensions: &[Box<dyn RuntimeExtension>]) -> AgentResult<()> {
    let mut ids = BTreeSet::new();
    for extension in extensions {
        let descriptor = extension.descriptor();
        if descriptor.id.trim().is_empty() {
            return Err(AgentError::new("扩展注册失败：扩展 id 不能为空。"));
        }
        if !descriptor.id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }) {
            return Err(AgentError::new(format!(
                "扩展注册失败：`{}` 只能包含小写 ASCII 字母、数字、点、下划线或连字符。",
                descriptor.id
            )));
        }
        if descriptor.version == 0 {
            return Err(AgentError::new(format!(
                "扩展注册失败：`{}` 的版本必须大于 0。",
                descriptor.id
            )));
        }
        if !ids.insert(descriptor.id) {
            return Err(AgentError::new(format!(
                "扩展注册冲突：`{}` 被重复注册。",
                descriptor.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentActivatedSkill, AgentActivatedSkillResources, AgentApprovalStatus,
        AgentProposedAction, AgentToolCall, AgentToolDefinition, AgentToolSafety,
    };
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillResourceKind, SkillRevision, SkillSourceId,
    };
    use crate::tools::ToolExecutionContext;
    use serde_json::json;

    struct TestExtension {
        descriptor: ExtensionDescriptor,
        tool_name: Option<&'static str>,
    }

    impl RuntimeExtension for TestExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            self.descriptor
        }

        fn tools(&self) -> Vec<Box<dyn AgentTool>> {
            self.tool_name
                .map(|name| vec![Box::new(TestTool(name)) as Box<dyn AgentTool>])
                .unwrap_or_default()
        }

        fn snapshot_state(&self) -> AgentResult<Value> {
            Ok(json!({}))
        }

        fn restore_state(&mut self, version: u32, _state: Value) -> AgentResult<()> {
            if version != self.descriptor.version {
                return Err(AgentError::new("unsupported test snapshot"));
            }
            Ok(())
        }
    }

    struct TestTool(&'static str);

    impl AgentTool for TestTool {
        fn exposure(&self) -> crate::tools::AgentToolExposure {
            crate::tools::AgentToolExposure::Stable
        }

        fn permission_policy(&self) -> crate::tools::AgentToolPermissionPolicy {
            crate::tools::AgentToolPermissionPolicy::Default
        }

        fn definition(&self) -> AgentToolDefinition {
            AgentToolDefinition {
                name: self.0.to_string(),
                description: "test".to_string(),
                input_schema: json!({ "type": "object" }),
                safety: AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: false,
                approval_mode: crate::protocol::AgentToolApprovalMode::Never,
            }
        }

        fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
            Ok(json!({}))
        }
    }

    fn descriptor(id: &'static str, order: i32) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id,
            version: 1,
            order,
        }
    }

    fn resource_bound_skill_fixture(
        revision_fill: char,
    ) -> (AgentSkillActivation, Arc<SkillResourceSession>) {
        let skill_id = SkillId::parse("workspace:workspace-1:spreadsheets").unwrap();
        let revision = SkillRevision::parse(format!(
            "skill-package-sha256-v3:{}",
            revision_fill.to_string().repeat(64)
        ))
        .unwrap();
        let session = Arc::new(
            memory_resource_session_for_test(
                skill_id.clone(),
                revision.clone(),
                SkillSourceId::parse("workspace:workspace-1").unwrap(),
                vec![(
                    "references/workflows.md".to_string(),
                    SkillResourceKind::Reference,
                    b"trusted spreadsheet workflow".to_vec(),
                )],
            )
            .unwrap(),
        );
        let package = session.package_uris().into_iter().next().unwrap();
        let activation = AgentSkillActivation {
            activation_revision: "activation-sha256-v1:checkpoint-fixture".to_string(),
            skills: vec![AgentActivatedSkill {
                id: skill_id.to_string(),
                name: "spreadsheets".to_string(),
                revision: revision.to_string(),
                source: "workspace:workspace-1".to_string(),
                instructions: "Use the trusted spreadsheet workflow.".to_string(),
                source_bytes: 37,
                resources: Some(AgentActivatedSkillResources {
                    root_uri: package.to_string(),
                    resource_count: 1,
                    kinds: vec!["reference".to_string()],
                }),
            }],
        };
        (activation, session)
    }

    #[test]
    fn rejects_duplicate_extension_ids() {
        let result = RuntimeExtensions::from_extensions(
            vec![
                Box::new(TestExtension {
                    descriptor: descriptor("duplicate", 0),
                    tool_name: None,
                }),
                Box::new(TestExtension {
                    descriptor: descriptor("duplicate", 1),
                    tool_name: None,
                }),
            ],
            None,
            &[],
        );

        let error = match result {
            Ok(_) => panic!("duplicate extension ids must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("重复注册"));
    }

    #[test]
    fn extension_tools_cannot_shadow_core_tools() {
        let extensions = RuntimeExtensions::from_extensions(
            vec![Box::new(TestExtension {
                descriptor: descriptor("shadow", 0),
                tool_name: Some("read_file"),
            })],
            None,
            &[],
        )
        .unwrap();
        let mut registry = ToolRegistry::defaults_with_search(None);

        let error = extensions.register_tools(&mut registry).unwrap_err();

        assert!(error.to_string().contains("read_file"));
        assert!(error.to_string().contains("core"));
    }

    #[test]
    fn todo_uses_registry_execution_and_restores_through_manager_snapshot() {
        let mut extensions = RuntimeExtensions::for_run("run-1", &[]).unwrap();
        let mut registry = ToolRegistry::defaults_with_search(None);
        extensions.register_tools(&mut registry).unwrap();
        assert!(registry.definition_for("todo_update").is_some());
        let call = AgentToolCall {
            id: "todo-call".to_string(),
            tool: "todo_update".to_string(),
            args: json!({
                "items": [{ "title": "Persist plan", "status": "in_progress" }]
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let result = registry.execute(&ToolExecutionContext::from_run_context(None), &call);
        assert!(result.ok, "{:?}", result.error);
        let effects = extensions
            .on_event(RuntimeExtensionEvent::ToolCompleted { result: &result })
            .unwrap();
        assert_eq!(effects.len(), 1);

        let snapshots = extensions.snapshots().unwrap();
        let restored = RuntimeExtensions::for_run("run-2", &snapshots).unwrap();
        let state = restored.todo_state().unwrap();
        assert_eq!(state.revision, 1);
        assert_eq!(state.items[0].title, "Persist plan");
    }

    #[test]
    fn resource_bound_skill_restores_from_checkpoint_before_authority_validation() {
        let (activation, resources) = resource_bound_skill_fixture('d');
        let original = RuntimeExtensions::for_run_with_skills(
            "run-original",
            None,
            Some(&activation),
            None,
            Some(Arc::clone(&resources)),
            &[],
        )
        .unwrap();
        let snapshots = original.snapshots().unwrap();

        let restored = RuntimeExtensions::for_run_with_skills(
            "run-restored",
            None,
            None,
            None,
            Some(resources),
            &snapshots,
        )
        .unwrap();

        let restored_snapshots = restored.snapshots().unwrap();
        let skill_snapshot = restored_snapshots
            .iter()
            .find(|snapshot| snapshot.extension_id == SKILL_EXTENSION_ID)
            .unwrap();
        assert_eq!(
            skill_snapshot.state["skills"][0]["id"],
            "workspace:workspace-1:spreadsheets"
        );
        assert_eq!(skill_snapshot.state["skills"][0]["hasResources"], true);
    }

    #[test]
    fn fresh_run_still_rejects_resource_authority_without_activation() {
        let (_, resources) = resource_bound_skill_fixture('d');

        let error = RuntimeExtensions::for_run_with_skills(
            "run-fresh",
            None,
            None,
            None,
            Some(resources),
            &[],
        )
        .err()
        .expect("unactivated resource authority must remain fail-closed");

        assert!(error.to_string().contains("unactivated Skill"));
    }

    #[test]
    fn checkpoint_restore_rejects_mismatched_resource_revision() {
        let (activation, checkpoint_resources) = resource_bound_skill_fixture('d');
        let original = RuntimeExtensions::for_run_with_skills(
            "run-original",
            None,
            Some(&activation),
            None,
            Some(checkpoint_resources),
            &[],
        )
        .unwrap();
        let snapshots = original.snapshots().unwrap();
        let (_, mismatched_resources) = resource_bound_skill_fixture('e');

        let error = RuntimeExtensions::for_run_with_skills(
            "run-restored",
            None,
            None,
            None,
            Some(mismatched_resources),
            &snapshots,
        )
        .err()
        .expect("checkpoint restore must reject a different resource revision");

        assert!(error.to_string().contains("has revision"));
        assert!(error.to_string().contains("instead of"));
    }

    #[test]
    fn checkpoint_restore_rejects_missing_resource_authority() {
        let (activation, checkpoint_resources) = resource_bound_skill_fixture('d');
        let original = RuntimeExtensions::for_run_with_skills(
            "run-original",
            None,
            Some(&activation),
            None,
            Some(checkpoint_resources),
            &[],
        )
        .unwrap();
        let snapshots = original.snapshots().unwrap();

        let error = RuntimeExtensions::for_run_with_skills(
            "run-restored",
            None,
            None,
            None,
            None,
            &snapshots,
        )
        .err()
        .expect("resource-bearing checkpoint must require its exact Host authority");

        assert!(error
            .to_string()
            .contains("has no exact Host package authority"));
    }

    #[test]
    fn checkpoint_restore_rejects_resource_authority_for_another_skill() {
        let (activation, checkpoint_resources) = resource_bound_skill_fixture('d');
        let original = RuntimeExtensions::for_run_with_skills(
            "run-original",
            None,
            Some(&activation),
            None,
            Some(checkpoint_resources),
            &[],
        )
        .unwrap();
        let snapshots = original.snapshots().unwrap();
        let foreign_resources = Arc::new(
            memory_resource_session_for_test(
                SkillId::parse("workspace:workspace-1:documents").unwrap(),
                SkillRevision::parse(format!("skill-package-sha256-v3:{}", "d".repeat(64)))
                    .unwrap(),
                SkillSourceId::parse("workspace:workspace-1").unwrap(),
                vec![(
                    "references/workflows.md".to_string(),
                    SkillResourceKind::Reference,
                    b"foreign document workflow".to_vec(),
                )],
            )
            .unwrap(),
        );

        let error = RuntimeExtensions::for_run_with_skills(
            "run-restored",
            None,
            None,
            None,
            Some(foreign_resources),
            &snapshots,
        )
        .err()
        .expect("checkpoint restore must reject authority for another Skill");

        assert!(error.to_string().contains("unactivated Skill"));
        assert!(error
            .to_string()
            .contains("workspace:workspace-1:documents"));
    }

    #[test]
    fn approval_event_keeps_run_checkpoint_internal() {
        let event = AgentEvent::ApprovalRequired {
            run_id: "run-1".to_string(),
            action: Box::new(AgentProposedAction::ToolCall {
                call: AgentToolCall {
                    id: "call-1".to_string(),
                    tool: "test".to_string(),
                    args: json!({}),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            }),
            checkpoint: Box::new(crate::protocol::AgentRunCheckpoint {
                pause_reason: crate::AgentRunCheckpointPauseReason::Approval,
                version: crate::protocol::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
                run_id: "run-1".to_string(),
                context_items: Vec::new(),
                next_model_request_index: 1,
                queued_tool_calls: Vec::new(),
                deferred_external_tool_call_count: 0,
                suppressed_narration: false,
                extension_snapshots: vec![AgentExtensionSnapshot {
                    extension_id: "private".to_string(),
                    version: 1,
                    state: json!({ "secret": "internal state" }),
                }],
                tool_set: crate::protocol::AgentRunToolSetCheckpoint {
                    stable_revision: "stable-tool-set-v1:test".to_string(),
                    dynamic_revision: "dynamic-tool-set-v1:test".to_string(),
                    effective_revision: "effective-tool-set-v1:test".to_string(),
                    active_capability_ids: Vec::new(),
                    exposed_tool_names: Vec::new(),
                },
                run_context: None,
                collaboration_run_snapshot: None,
                model_capabilities: crate::protocol::ModelCapabilities::default(),
                provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
                    crate::ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                provider_protocol_key: crate::ProviderProtocolKey::new(
                    crate::ProviderProtocolDialect::OpenAiChatCompletions,
                    &crate::ProviderProfileConfig::generic_for_dialect(
                        crate::ProviderProtocolDialect::OpenAiChatCompletions,
                    ),
                    "test-model",
                    None,
                )
                .unwrap(),
                assistant_turn_identity: crate::AgentAssistantTurnCheckpointIdentity {
                    assistant_turn_id: "assistant-turn-1".to_string(),
                    assistant_turn_digest: "digest-assistant-turn-1".to_string(),
                    tool_call_identities: vec![crate::AgentProviderToolCallIdentity {
                        provider_tool_index: 0,
                        provider_call_id: "call-1".to_string(),
                        runtime_call_id: "call-1".to_string(),
                    }],
                },
                provider_continuation_refs: Vec::new(),
                run_world_state: crate::world_state::WorldStateSnapshot::new(
                    "approval-event-test",
                    0,
                    vec![crate::world_state::WorldStateSectionEnvelope::host_only(
                        crate::world_state::WorldStateSectionId::ModelCapabilities,
                        crate::world_state::WorldStateLifetime::Run,
                        json!({ "imageInput": false }),
                    )
                    .unwrap()],
                )
                .unwrap(),
                pending_action_id: None,
                file_change_run_grant_ref: None,
                pending_file_observation: None,
                pending_tool_call_id: "call-1".to_string(),
                conversation_trace_items: Vec::new(),
                conversation_model_context_items: Vec::new(),
                next_conversation_trace_sequence: 0,
                conversation_trace_truncated: false,
            }),
            segment_usage: None,
        };

        let serialized = serde_json::to_string(&event).unwrap();

        assert!(!serialized.contains("checkpoint"));
        assert!(!serialized.contains("internal state"));
    }
}
