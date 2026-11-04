use super::{
    ExtensionDescriptor, ModelInputCapacity, ModelRequestContext, RuntimeEffect, RuntimeExtension,
    RuntimeExtensionEvent,
};
use crate::context::{activated_skill_context_item, ContextFrame};
use crate::conversation_trace::{canonical_tool_result_for_context, render_tool_observation};
use crate::llm::LlmMessage;
use crate::protocol::{
    AgentActivatedSkill, AgentError, AgentEvent, AgentExtensionSnapshot, AgentResult,
    AgentSkillActivatedEvent, AgentSkillActivation, AgentSkillActivationActor,
    AgentSkillSourceSummary, AgentToolApprovalMode, AgentToolDefinition, AgentToolResult,
    AgentToolSafety,
};
use crate::runtime::{AgentResolvedSkillActivation, AgentSkillActivationResolver};
use crate::skills::{
    activation_revision_for_identities, AgentDiscoverableSkill, AgentSkillDiscoverySnapshot,
    SkillId, SkillPackageUri, SkillResourceSession, SkillRevision, SkillSelection,
    APPLICATION_BUNDLED_SKILL_SOURCE_ID, DOCUMENTS_LOCAL_ID, IMAGE_GENERATION_LOCAL_ID,
    PRESENTATIONS_LOCAL_ID, SKILL_INSTALLER_LOCAL_ID, SPREADSHEETS_LOCAL_ID,
};
use crate::tools::{
    AgentTool, AgentToolPermissionPolicy, EffectiveToolSet, ToolCapabilityId, ToolExecutionContext,
    IMAGE_GENERATION_CAPABILITY, OFFICE_DOCUMENTS_CAPABILITY, OFFICE_PRESENTATIONS_CAPABILITY,
    OFFICE_SPREADSHEETS_CAPABILITY, SKILL_INSTALLATION_CAPABILITY,
    SKILL_RESOURCES_MATERIALIZE_CAPABILITY, SKILL_RESOURCES_READ_CAPABILITY,
    SKILL_SCRIPTS_CAPABILITY,
};
use crate::world_state::{WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

#[cfg(test)]
use crate::skills::SKILL_CREATOR_LOCAL_ID;

pub(in crate::runtime) const SKILL_EXTENSION_ID: &str = "skills";
const SKILL_EXTENSION_VERSION: u32 = 3;
const SKILL_ACTIVATE_TOOL_NAME: &str = "skills_activate";
const SKILL_ACTIVATION_RESULT_SCHEMA_VERSION: u32 = 1;
const MAX_ACTIVATION_REASON_CHARS: usize = 240;
const SKILL_ACTIVATION_REF_CHARS: usize = 26;

pub(super) struct SkillActivationExtension {
    run_id: String,
    state: SkillActivationStateHandle,
}

#[derive(Clone)]
struct SkillActivationStateHandle {
    inner: Arc<Mutex<SkillActivationState>>,
}

struct SkillActivationState {
    discovery: Option<AgentSkillDiscoverySnapshot>,
    active: BTreeMap<String, ActivatedSkillRecord>,
    order: Vec<String>,
    pending_context: BTreeMap<String, crate::context::ContextItem>,
    resolver: Option<AgentSkillActivationResolver>,
    resources: Option<Arc<SkillResourceSession>>,
    model_input_capacity: Option<ModelInputCapacity>,
    model_input_capacity_observed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivatedSkillRecord {
    id: String,
    name: String,
    revision: String,
    source: String,
    source_bytes: u64,
    has_resources: bool,
    resource_kinds: Vec<String>,
    activated_by: AgentSkillActivationActor,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillExtensionSnapshot {
    discovery: Option<AgentSkillDiscoverySnapshot>,
    skills: Vec<ActivatedSkillRecord>,
}

impl std::fmt::Debug for SkillExtensionSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SkillExtensionSnapshot")
            .field("discovery", &self.discovery)
            .field("skill_count", &self.skills.len())
            .finish()
    }
}

impl SkillActivationExtension {
    pub(super) fn new(
        run_id: String,
        discovery: Option<AgentSkillDiscoverySnapshot>,
        initial_activation: Option<&AgentSkillActivation>,
        resolver: Option<AgentSkillActivationResolver>,
        resources: Option<Arc<SkillResourceSession>>,
    ) -> AgentResult<Self> {
        if let Some(discovery) = &discovery {
            discovery
                .validate()
                .map_err(|error| AgentError::new(format!("Skill discovery is invalid: {error}")))?;
        }
        let mut active = BTreeMap::new();
        let mut order = Vec::new();
        if let Some(activation) = initial_activation {
            for skill in &activation.skills {
                validate_activated_skill(skill)?;
                let record = record_from_skill(skill, AgentSkillActivationActor::User);
                if active.insert(skill.id.clone(), record).is_some() {
                    return Err(AgentError::new(format!(
                        "Skill activation contains duplicate id `{}`.",
                        skill.id
                    )));
                }
                order.push(skill.id.clone());
            }
        }
        validate_restored_state(&active, discovery.as_ref())?;
        validate_initial_activation_authority(&active, resources.as_deref())?;
        Ok(Self {
            run_id,
            state: SkillActivationStateHandle {
                inner: Arc::new(Mutex::new(SkillActivationState {
                    discovery,
                    active,
                    order,
                    pending_context: BTreeMap::new(),
                    resolver,
                    resources,
                    model_input_capacity: None,
                    model_input_capacity_observed: false,
                })),
            },
        })
    }

    /// Creates the private shell used only while [`RuntimeExtensions`](super::RuntimeExtensions)
    /// is restoring a checkpoint that contains this extension's snapshot.
    ///
    /// The Host resource session is already reconstructed from the checkpoint's immutable Skill
    /// identities, while the matching logical activation records still live in the extension
    /// snapshot. Validating those resources in this temporary empty state would compare them with
    /// an empty activation map. The extension manager must therefore call `restore_state`
    /// immediately; that method validates the complete restored activation and resource authority
    /// before publishing either into this state handle.
    pub(super) fn new_for_checkpoint_restore(
        run_id: String,
        resolver: Option<AgentSkillActivationResolver>,
        resources: Option<Arc<SkillResourceSession>>,
    ) -> Self {
        Self {
            run_id,
            state: SkillActivationStateHandle {
                inner: Arc::new(Mutex::new(SkillActivationState {
                    discovery: None,
                    active: BTreeMap::new(),
                    order: Vec::new(),
                    pending_context: BTreeMap::new(),
                    resolver,
                    resources,
                    model_input_capacity: None,
                    model_input_capacity_observed: false,
                })),
            },
        }
    }
}

impl RuntimeExtension for SkillActivationExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id: SKILL_EXTENSION_ID,
            version: SKILL_EXTENSION_VERSION,
            order: 50,
        }
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(SkillsActivateTool {
            state: self.state.clone(),
        })]
    }

    fn active_tool_capabilities(&self) -> AgentResult<BTreeSet<ToolCapabilityId>> {
        let state = self.state.lock();
        let mut capabilities = BTreeSet::new();
        for id in &state.order {
            if let Some(record) = state.active.get(id) {
                capabilities.extend(tool_capabilities_for_skill(record));
            }
        }
        Ok(capabilities)
    }

    fn request_context(
        &self,
        _request: &ModelRequestContext,
    ) -> AgentResult<Vec<crate::context::ContextItem>> {
        Ok(Vec::new())
    }

    fn update_model_input_capacity(&mut self, capacity: Option<ModelInputCapacity>) {
        let mut state = self.state.lock();
        state.model_input_capacity = capacity;
        state.model_input_capacity_observed = true;
    }

    fn consume_model_input_capacity(&mut self, tokens: u64) {
        let mut state = self.state.lock();
        if let Some(capacity) = state.model_input_capacity.as_mut() {
            capacity.remaining_tokens = capacity.remaining_tokens.saturating_sub(tokens);
        }
    }

    fn on_event(&mut self, event: &RuntimeExtensionEvent<'_>) -> AgentResult<Vec<RuntimeEffect>> {
        let RuntimeExtensionEvent::ToolCompleted { result } = event;
        if result.tool != SKILL_ACTIVATE_TOOL_NAME {
            return Ok(Vec::new());
        }
        if !result.ok {
            let mut state = self.state.lock();
            consume_failed_activation_result_capacity(&mut state, result)?;
            return Ok(Vec::new());
        }
        let value = result
            .result
            .as_ref()
            .ok_or_else(|| AgentError::new("skills_activate succeeded without a result."))?;
        if value.get("status").and_then(Value::as_str) != Some("activated") {
            return Ok(Vec::new());
        }
        let skill_id = value
            .get("skill")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::new("skills_activate result is missing skill.id."))?;
        let mut state = self.state.lock();
        let context = state.pending_context.remove(skill_id).ok_or_else(|| {
            AgentError::new(format!(
                "skills_activate result for `{skill_id}` has no pending instruction context."
            ))
        })?;
        let record = state.active.get(skill_id).ok_or_else(|| {
            AgentError::new(format!(
                "activated Skill `{skill_id}` is missing from runtime state."
            ))
        })?;
        let source_kind = state
            .discovery
            .as_ref()
            .and_then(|discovery| {
                discovery
                    .skills
                    .iter()
                    .find(|entry| entry.id == record.id && entry.revision == record.revision)
            })
            .map(|entry| entry.source_kind.clone())
            .ok_or_else(|| {
                AgentError::new(format!(
                    "activated Skill `{skill_id}` is absent from the frozen discovery catalog."
                ))
            })?;
        let activation_revision = state.activation_revision()?;
        let event_skill = AgentSkillActivatedEvent {
            id: record.id.clone(),
            name: record.name.clone(),
            revision: record.revision.clone(),
            source: AgentSkillSourceSummary {
                kind: source_kind,
                id: record.source.clone(),
            },
        };
        Ok(vec![
            RuntimeEffect::EmitEvent(Box::new(AgentEvent::SkillActivated {
                run_id: self.run_id.clone(),
                activation_revision,
                activated_by: record.activated_by,
                skill: event_skill,
            })),
            RuntimeEffect::AppendRetainedContext(Box::new(context)),
        ])
    }

    fn world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let state = self.state.lock();
        if state.order.is_empty() {
            return Ok(Vec::new());
        }
        let skills = state
            .order
            .iter()
            .filter_map(|id| state.active.get(id))
            .map(|skill| {
                json!({
                    "id": skill.id,
                    "name": skill.name,
                    "revision": skill.revision,
                })
            })
            .collect::<Vec<_>>();
        let value = json!({
            "activationRevision": state.activation_revision()?,
            "skills": skills,
        });
        Ok(vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::SkillActivation,
            WorldStateLifetime::Run,
            value.clone(),
            value,
        )
        .map_err(|error| {
            AgentError::new(format!(
                "cannot build Skill activation World State: {error}"
            ))
        })?])
    }

    fn snapshot_state(&self) -> AgentResult<Value> {
        let state = self.state.lock();
        let snapshot = SkillExtensionSnapshot {
            discovery: state.discovery.clone(),
            skills: state
                .order
                .iter()
                .filter_map(|id| state.active.get(id).cloned())
                .collect(),
        };
        serde_json::to_value(snapshot).map_err(|error| {
            AgentError::new(format!("cannot save `{SKILL_EXTENSION_ID}` state: {error}"))
        })
    }

    fn restore_state(&mut self, version: u32, value: Value) -> AgentResult<()> {
        if version != SKILL_EXTENSION_VERSION {
            return Err(AgentError::new(format!(
                "cannot restore `{SKILL_EXTENSION_ID}` state version {version}; expected {SKILL_EXTENSION_VERSION}."
            )));
        }
        let snapshot: SkillExtensionSnapshot = serde_json::from_value(value).map_err(|error| {
            AgentError::new(format!("invalid `{SKILL_EXTENSION_ID}` state: {error}"))
        })?;
        let SkillExtensionSnapshot { discovery, skills } = snapshot;
        if let Some(discovery) = &discovery {
            discovery.validate().map_err(|error| {
                AgentError::new(format!(
                    "invalid `{SKILL_EXTENSION_ID}` discovery state: {error}"
                ))
            })?;
        }
        let mut active = BTreeMap::new();
        let mut order = Vec::with_capacity(skills.len());
        for record in skills {
            validate_record(&record)?;
            if active.insert(record.id.clone(), record.clone()).is_some() {
                return Err(AgentError::new(format!(
                    "invalid `{SKILL_EXTENSION_ID}` state: duplicate Skill `{}`.",
                    record.id
                )));
            }
            order.push(record.id);
        }
        let mut state = self.state.lock();
        validate_restored_state(&active, discovery.as_ref())?;
        validate_resource_authority(
            &active,
            state.resources.as_deref(),
            ResourceAuthorityRequirement::Restored,
        )?;
        state.discovery = discovery;
        state.active = active;
        state.order = order;
        state.pending_context.clear();
        state.model_input_capacity = None;
        state.model_input_capacity_observed = false;
        Ok(())
    }
}

struct SkillsActivateTool {
    state: SkillActivationStateHandle,
}

impl AgentTool for SkillsActivateTool {
    fn exposure(&self) -> crate::tools::AgentToolExposure {
        crate::tools::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: SKILL_ACTIVATE_TOOL_NAME.to_string(),
            description: "Load the complete instructions and revision-bound resources for one Skill from the backend-provided available-Skills catalog. Call this when the user's task clearly matches a Skill description or the user explicitly names that Skill. Activation does not grant file, command, network, or approval permissions.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "skillRef": {
                        "type": "string",
                        "description": "Exact opaque ref from backend_available_skills, such as s_4f7a…. Never invent or reuse a ref from another run.",
                        "minLength": SKILL_ACTIVATION_REF_CHARS,
                        "maxLength": SKILL_ACTIVATION_REF_CHARS,
                        "pattern": "^s_[0-9a-f]{24}$"
                    },
                    "reason": {
                        "type": "string",
                        "description": "Brief user-understandable reason this Skill is needed now.",
                        "minLength": 1,
                        "maxLength": MAX_ACTIVATION_REASON_CHARS
                    }
                },
                "required": ["skillRef", "reason"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        let args: SkillsActivateArgs = serde_json::from_value(args).map_err(|error| {
            structured_activation_error(
                "skill.invalidActivationRequest",
                format!("skills_activate arguments are invalid: {error}"),
                "correctArguments",
                None,
            )
        })?;
        let skill_ref = normalize_required(&args.skill_ref, "skillRef")?;
        if !valid_activation_ref(&skill_ref) {
            return Err(structured_activation_error(
                "skill.invalidActivationRef",
                "skills_activate.skillRef must be an exact opaque ref from this run's catalog.",
                "useAvailableRef",
                None,
            ));
        }
        let reason = normalize_required(&args.reason, "reason")?;
        if reason.chars().count() > MAX_ACTIVATION_REASON_CHARS {
            return Err(structured_activation_error(
                "skill.activationReasonTooLong",
                format!("skills_activate.reason exceeds {MAX_ACTIVATION_REASON_CHARS} characters."),
                "shortenReason",
                None,
            ));
        }

        let (entry, resolver) = {
            let mut state = self.state.lock();
            let discovery = state.discovery.as_ref().ok_or_else(|| {
                structured_activation_error(
                    "skill.catalogUnavailable",
                    "No backend Skill discovery catalog is available for this run.",
                    "doNotRetry",
                    None,
                )
            })?;
            let entry = discovery.find_by_ref(&skill_ref).cloned().ok_or_else(|| {
                structured_activation_error(
                    "skill.unknownActivationRef",
                    format!(
                        "Skill activation ref `{skill_ref}` is not in this run's frozen catalog."
                    ),
                    "useAvailableRef",
                    None,
                )
            })?;
            ensure_model_input_capacity_available(&state, &entry)?;
            if let Some(existing) = state.active.get(&entry.id) {
                if existing.revision != entry.revision {
                    return Err(structured_activation_error(
                        "skill.activationRevisionConflict",
                        format!(
                            "Skill `{}` is already active at another revision.",
                            entry.name
                        ),
                        "restartRun",
                        Some(&entry),
                    ));
                }
                let result = activation_result(
                    "alreadyActivated",
                    existing,
                    &state.activation_revision()?,
                    &reason,
                )?;
                let capacity =
                    activation_capacity_reservation(&state, context, &result, None, None, &entry)?;
                consume_model_input_capacity(&mut state, capacity);
                return Ok(result);
            }
            let resolver = state.resolver.clone().ok_or_else(|| {
                structured_activation_error(
                    "skill.activationBackendUnavailable",
                    "The host did not provide a Skill activation resolver for this run.",
                    "retryRun",
                    Some(&entry),
                )
            })?;
            (entry, resolver)
        };

        let selection =
            SkillSelection::parse(entry.id.clone(), entry.revision.clone()).map_err(|error| {
                structured_activation_error(
                    "skill.invalidFrozenSelection",
                    format!("The frozen Skill selection is invalid: {error}"),
                    "restartRun",
                    Some(&entry),
                )
            })?;
        let resolved = resolver(&selection)?;
        let resolved_resources = validate_resolved_candidate(&entry, &resolved)?;

        let mut state = self.state.lock();
        if let Some(existing) = state.active.get(&entry.id) {
            if existing.revision == entry.revision {
                let result = activation_result(
                    "alreadyActivated",
                    existing,
                    &state.activation_revision()?,
                    &reason,
                )?;
                let capacity =
                    activation_capacity_reservation(&state, context, &result, None, None, &entry)?;
                consume_model_input_capacity(&mut state, capacity);
                return Ok(result);
            }
            return Err(structured_activation_error(
                "skill.activationRevisionConflict",
                format!("Skill `{}` became active at another revision.", entry.name),
                "restartRun",
                Some(&entry),
            ));
        }
        let discovery = state.discovery.as_ref().ok_or_else(|| {
            structured_activation_error(
                "skill.catalogUnavailable",
                "The frozen Skill catalog is no longer available.",
                "restartRun",
                Some(&entry),
            )
        })?;
        let max_skills = discovery.max_activated_skills;
        if state.active.len().saturating_add(1) > max_skills {
            return Err(structured_activation_error(
                "skill.tooManySkills",
                format!("This run can activate at most {max_skills} Skills."),
                "reduceSelection",
                Some(&entry),
            ));
        }
        let record = record_from_skill(&resolved.skill, AgentSkillActivationActor::Model);
        let current_bytes = state
            .active
            .values()
            .try_fold(0_u64, |total, skill| total.checked_add(skill.source_bytes))
            .ok_or_else(|| AgentError::new("activated Skill source byte total overflowed."))?;
        let next_bytes = current_bytes
            .checked_add(record.source_bytes)
            .ok_or_else(|| AgentError::new("activated Skill source byte total overflowed."))?;
        let max_bytes = u64::try_from(discovery.max_total_source_bytes).unwrap_or(u64::MAX);
        if next_bytes > max_bytes {
            return Err(structured_activation_error(
                "skill.sourceBudgetExceeded",
                format!(
                    "Activated Skill sources would require {next_bytes} bytes; the limit is {max_bytes}."
                ),
                "reduceSelection",
                Some(&entry),
            ));
        }
        let activation_revision = state.activation_revision_with(&record)?;
        let pending_context = activated_skill_context_item(&activation_revision, &resolved.skill)?;
        let result = activation_result("activated", &record, &activation_revision, &reason)?;
        let additional_capabilities = tool_capabilities_for_skill(&record);
        let capacity = activation_capacity_reservation(
            &state,
            context,
            &result,
            Some(&pending_context),
            Some(&additional_capabilities),
            &entry,
        )?;
        let resources = state.resources.as_ref().ok_or_else(|| {
            structured_activation_error(
                "skill.resourceSessionUnavailable",
                "The run has no mutable Skill resource session.",
                "retryRun",
                Some(&entry),
            )
        })?;
        if let Some(resolved_resources) = &resolved_resources {
            resources.extend_from(resolved_resources).map_err(|error| {
                structured_activation_error(
                    "skill.resourceActivationFailed",
                    format!("Cannot extend the run's Skill resources: {error}"),
                    "retryRun",
                    Some(&entry),
                )
            })?;
        }
        consume_model_input_capacity(&mut state, capacity);
        state.order.push(record.id.clone());
        state.active.insert(record.id.clone(), record.clone());
        state
            .pending_context
            .insert(record.id.clone(), pending_context);
        Ok(result)
    }

    fn model_projection(&self, result: &AgentToolResult) -> AgentToolResult {
        skill_activation_model_projection(result)
    }
}

fn skill_activation_model_projection(result: &AgentToolResult) -> AgentToolResult {
    let projected = result.result.as_ref().and_then(|value| {
        let mut output = serde_json::Map::new();
        crate::tools::model_projection::insert_field(&mut output, value, "status");
        if let Some(skill) = value.get("skill").and_then(|skill| {
            crate::tools::model_projection::retain_object_fields(skill, &["name", "hasResources"])
        }) {
            output.insert("skill".to_string(), skill);
        }
        (!output.is_empty()).then_some(Value::Object(output))
    });
    crate::tools::model_projection::compact_model_result(result, projected)
}

impl SkillActivationStateHandle {
    fn lock(&self) -> MutexGuard<'_, SkillActivationState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn ensure_model_input_capacity_available(
    state: &SkillActivationState,
    entry: &AgentDiscoverableSkill,
) -> AgentResult<()> {
    if state.model_input_capacity_observed && state.model_input_capacity.is_none() {
        return Err(structured_activation_error(
            "skill.contextCapacityUnavailable",
            "The backend cannot verify enough model context capacity for dynamic Skill activation.",
            "configureModelContextWindow",
            Some(entry),
        ));
    }
    Ok(())
}

struct ActivationCapacityReservation {
    required_tokens: u64,
    projected_tool_set: Option<EffectiveToolSet>,
}

/// Measures the exact successful ToolResult, newly disclosed Skill context, and incremental
/// dynamic Tool contract that the next model request will contain.
///
/// The check happens before resources or activation state are mutated, so a capacity failure
/// cannot leave a half-activated Skill. The projected Tool set joins the in-run reservation only
/// after resource publication succeeds.
fn activation_capacity_reservation(
    state: &SkillActivationState,
    context: &ToolExecutionContext,
    result: &Value,
    pending_context: Option<&crate::context::ContextItem>,
    additional_capabilities: Option<&BTreeSet<ToolCapabilityId>>,
    entry: &AgentDiscoverableSkill,
) -> AgentResult<ActivationCapacityReservation> {
    ensure_model_input_capacity_available(state, entry)?;
    let Some(capacity) = state.model_input_capacity.as_ref() else {
        // Direct unit-level tool use does not cross a model request and therefore has no capacity
        // observation. Production runtime dispatch always records Some/None before execution.
        return Ok(ActivationCapacityReservation {
            required_tokens: 0,
            projected_tool_set: None,
        });
    };
    let tool_call_id = context.tool_call_id()?;
    let tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: tool_call_id.to_string(),
        tool: SKILL_ACTIVATE_TOOL_NAME.to_string(),
        ok: true,
        result: Some(result.clone()),
        error: None,
    };
    // Measure the same canonical projection the runtime will actually retain. The current
    // activation result is already bounded, but keeping this projection boundary shared prevents
    // future result-schema additions from silently drifting away from capacity accounting.
    let canonical = skill_activation_model_projection(&tool_result);
    let result_message = LlmMessage::tool_result(
        &canonical.call_id,
        render_tool_observation(&canonical),
        false,
    );
    let mut required_tokens = capacity.text_budget.estimate_message(&result_message);
    if let Some(pending_context) = pending_context {
        for message in ContextFrame::new(vec![pending_context.clone()]).into_messages() {
            required_tokens = required_tokens
                .checked_add(capacity.text_budget.estimate_message(&message))
                .ok_or_else(|| {
                    AgentError::new("activated Skill context token total overflowed.")
                })?;
        }
    }
    let projected_tool_set = additional_capabilities
        .map(|capabilities| {
            capacity
                .project_additional_tool_capabilities(capabilities)
                .map_err(|error| {
                    structured_activation_error(
                        "skill.contextCapacityProjectionFailed",
                        format!(
                            "Cannot project the post-activation Tool contract for Skill `{}`: {error}",
                            entry.name
                        ),
                        "restartRun",
                        Some(entry),
                    )
                })
        })
        .transpose()?;
    if let Some(projection) = &projected_tool_set {
        required_tokens = required_tokens
            .checked_add(projection.additional_tokens)
            .ok_or_else(|| {
                AgentError::new("activated Skill model-input token total overflowed.")
            })?;
    }
    if required_tokens > capacity.remaining_tokens {
        return Err(structured_activation_error(
            "skill.contextCapacityExceeded",
            format!(
                "Skill `{}` needs about {required_tokens} additional input tokens for its paired result, instructions, and dynamic Tool contract, but this run has only {} available.",
                entry.name, capacity.remaining_tokens,
            ),
            "startNewRunOrReduceContext",
            Some(entry),
        ));
    }
    Ok(ActivationCapacityReservation {
        required_tokens,
        projected_tool_set: projected_tool_set.map(|projection| projection.effective_tool_set),
    })
}

fn consume_model_input_capacity(
    state: &mut SkillActivationState,
    reservation: ActivationCapacityReservation,
) {
    if let Some(capacity) = state.model_input_capacity.as_mut() {
        debug_assert!(reservation.required_tokens <= capacity.remaining_tokens);
        capacity.remaining_tokens = capacity
            .remaining_tokens
            .saturating_sub(reservation.required_tokens);
        if let Some(projected_tool_set) = reservation.projected_tool_set {
            capacity.effective_tool_set = projected_tool_set;
        }
    }
}

fn consume_failed_activation_result_capacity(
    state: &mut SkillActivationState,
    result: &AgentToolResult,
) -> AgentResult<()> {
    if !state.model_input_capacity_observed {
        return Ok(());
    }
    let capacity = state.model_input_capacity.as_mut().ok_or_else(|| {
        structured_activation_error(
            "skill.contextCapacityUnavailable",
            "The backend cannot verify enough model context capacity for the Skill activation result.",
            "configureModelContextWindow",
            None,
        )
    })?;
    let canonical = canonical_tool_result_for_context(result);
    let message = LlmMessage::tool_result(
        &canonical.call_id,
        render_tool_observation(&canonical),
        true,
    );
    let required_tokens = capacity.text_budget.estimate_message(&message);
    if required_tokens > capacity.remaining_tokens {
        return Err(structured_activation_error(
            "skill.contextCapacityExceeded",
            format!(
                "The failed Skill activation result needs about {required_tokens} additional input tokens, but this run has only {} available.",
                capacity.remaining_tokens,
            ),
            "startNewRunOrReduceContext",
            None,
        ));
    }
    capacity.remaining_tokens -= required_tokens;
    Ok(())
}

impl SkillActivationState {
    fn activation_revision(&self) -> AgentResult<String> {
        Ok(activation_revision_for_identities(
            self.order
                .iter()
                .filter_map(|id| self.active.get(id))
                .map(|skill| (skill.id.as_str(), skill.revision.as_str())),
        )
        .as_str()
        .to_string())
    }

    fn activation_revision_with(&self, additional: &ActivatedSkillRecord) -> AgentResult<String> {
        let mut identities = self
            .order
            .iter()
            .filter_map(|id| self.active.get(id))
            .map(|skill| (skill.id.as_str(), skill.revision.as_str()))
            .collect::<Vec<_>>();
        identities.push((additional.id.as_str(), additional.revision.as_str()));
        Ok(activation_revision_for_identities(identities)
            .as_str()
            .to_string())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillsActivateArgs {
    skill_ref: String,
    reason: String,
}

fn record_from_skill(
    skill: &AgentActivatedSkill,
    activated_by: AgentSkillActivationActor,
) -> ActivatedSkillRecord {
    let mut resource_kinds = skill
        .resources
        .as_ref()
        .map(|resources| resources.kinds.clone())
        .unwrap_or_default();
    resource_kinds.sort();
    resource_kinds.dedup();
    ActivatedSkillRecord {
        id: skill.id.clone(),
        name: skill.name.clone(),
        revision: skill.revision.clone(),
        source: skill.source.clone(),
        source_bytes: skill
            .source_bytes
            .max(u64::try_from(skill.instructions.len()).unwrap_or(u64::MAX)),
        has_resources: skill.resources.is_some(),
        resource_kinds,
        activated_by,
    }
}

fn validate_record(record: &ActivatedSkillRecord) -> AgentResult<()> {
    if record.id.trim().is_empty()
        || record.name.trim().is_empty()
        || record.revision.trim().is_empty()
        || record.source.trim().is_empty()
        || record.source_bytes == 0
    {
        return Err(AgentError::new(
            "Skill extension records require id, name, revision, source, and sourceBytes.",
        ));
    }
    let skill_id = SkillId::parse(record.id.clone()).map_err(|error| {
        AgentError::new(format!(
            "Skill extension record `{}` has an invalid id: {error}",
            record.id
        ))
    })?;
    if record.source != skill_id.source_id().as_str() {
        return Err(AgentError::new(format!(
            "Skill extension record `{}` declares source `{}` instead of its exact source id `{}`.",
            record.id,
            record.source,
            skill_id.source_id()
        )));
    }
    if record.has_resources == record.resource_kinds.is_empty() {
        return Err(AgentError::new(
            "Skill extension resource metadata must bind hasResources to non-empty resourceKinds.",
        ));
    }
    if !record
        .resource_kinds
        .windows(2)
        .all(|pair| pair[0] < pair[1])
        || record
            .resource_kinds
            .iter()
            .any(|kind| !matches!(kind.as_str(), "reference" | "asset" | "script" | "other"))
    {
        return Err(AgentError::new(
            "Skill extension resourceKinds must be sorted, unique, and use known resource kinds.",
        ));
    }
    Ok(())
}

fn source_kind(source_id: &str) -> Option<&str> {
    source_id.split_once(':').map(|(kind, _)| kind)
}

fn is_discovery_managed(record: &ActivatedSkillRecord) -> bool {
    matches!(
        source_kind(&record.source),
        Some("bundled" | "installed" | "workspace")
    )
}

fn validate_record_against_discovery(
    record: &ActivatedSkillRecord,
    discovery: Option<&AgentSkillDiscoverySnapshot>,
) -> AgentResult<()> {
    if !is_discovery_managed(record) {
        return Ok(());
    }
    if source_kind(&record.source) == Some("workspace") && discovery.is_none() {
        // Explicit user selections from pre-discovery or project-preview paths retain their
        // request-scoped Host authority without pretending to be globally enabled.
        return Ok(());
    }
    let entry = discovery
        .and_then(|snapshot| snapshot.skills.iter().find(|entry| entry.id == record.id))
        .ok_or_else(|| {
            AgentError::new(format!(
                "activated managed Skill `{}` is absent from the frozen discovery catalog.",
                record.id
            ))
        })?;
    if entry.revision != record.revision
        || entry.name != record.name
        || Some(entry.source_kind.as_str()) != source_kind(&record.source)
    {
        return Err(AgentError::new(format!(
            "activated managed Skill `{}` does not match the frozen discovery catalog.",
            record.id
        )));
    }
    Ok(())
}

fn tool_capabilities_for_skill(record: &ActivatedSkillRecord) -> BTreeSet<ToolCapabilityId> {
    let mut capabilities = BTreeSet::new();
    let bundled_id = |local_id: &str| format!("{APPLICATION_BUNDLED_SKILL_SOURCE_ID}:{local_id}");
    if record.source == APPLICATION_BUNDLED_SKILL_SOURCE_ID {
        if record.id == bundled_id(DOCUMENTS_LOCAL_ID) {
            capabilities.insert(ToolCapabilityId::application_owned(
                OFFICE_DOCUMENTS_CAPABILITY,
            ));
        } else if record.id == bundled_id(SPREADSHEETS_LOCAL_ID) {
            capabilities.insert(ToolCapabilityId::application_owned(
                OFFICE_SPREADSHEETS_CAPABILITY,
            ));
        } else if record.id == bundled_id(PRESENTATIONS_LOCAL_ID) {
            capabilities.insert(ToolCapabilityId::application_owned(
                OFFICE_PRESENTATIONS_CAPABILITY,
            ));
        } else if record.id == bundled_id(IMAGE_GENERATION_LOCAL_ID) {
            capabilities.insert(ToolCapabilityId::application_owned(
                IMAGE_GENERATION_CAPABILITY,
            ));
        } else if record.id == bundled_id(SKILL_INSTALLER_LOCAL_ID) {
            capabilities.insert(ToolCapabilityId::application_owned(
                SKILL_INSTALLATION_CAPABILITY,
            ));
        }
    }

    if record.has_resources {
        capabilities.insert(ToolCapabilityId::application_owned(
            SKILL_RESOURCES_READ_CAPABILITY,
        ));
    }
    if record
        .resource_kinds
        .iter()
        .any(|kind| matches!(kind.as_str(), "asset" | "other"))
    {
        capabilities.insert(ToolCapabilityId::application_owned(
            SKILL_RESOURCES_MATERIALIZE_CAPABILITY,
        ));
    }
    if record.resource_kinds.iter().any(|kind| kind == "script") {
        capabilities.insert(ToolCapabilityId::application_owned(
            SKILL_SCRIPTS_CAPABILITY,
        ));
    }
    capabilities
}

fn validate_restored_state(
    active: &BTreeMap<String, ActivatedSkillRecord>,
    discovery: Option<&AgentSkillDiscoverySnapshot>,
) -> AgentResult<()> {
    let mut total_source_bytes = 0_u64;
    for record in active.values() {
        total_source_bytes = total_source_bytes
            .checked_add(record.source_bytes)
            .ok_or_else(|| AgentError::new("restored Skill source byte total overflowed."))?;
        validate_record_against_discovery(record, discovery)?;
    }
    if let Some(discovery) = discovery {
        if active.len() > discovery.max_activated_skills {
            return Err(AgentError::new(format!(
                "restored Skill state contains {} activations; the run limit is {}.",
                active.len(),
                discovery.max_activated_skills
            )));
        }
        let max_bytes = u64::try_from(discovery.max_total_source_bytes).unwrap_or(u64::MAX);
        if total_source_bytes > max_bytes {
            return Err(AgentError::new(format!(
                "restored Skill state contains {total_source_bytes} source bytes; the run limit is {max_bytes}."
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ResourceAuthorityRequirement {
    Initial,
    Restored,
}

fn validate_initial_activation_authority(
    active: &BTreeMap<String, ActivatedSkillRecord>,
    resources: Option<&SkillResourceSession>,
) -> AgentResult<()> {
    validate_resource_authority(active, resources, ResourceAuthorityRequirement::Initial)
}

fn validate_resource_authority(
    active: &BTreeMap<String, ActivatedSkillRecord>,
    resources: Option<&SkillResourceSession>,
    requirement: ResourceAuthorityRequirement,
) -> AgentResult<()> {
    let resource_packages = resources
        .map(SkillResourceSession::package_uris)
        .unwrap_or_default();
    for package in &resource_packages {
        let Some(record) = active.get(package.skill_id().as_str()) else {
            return Err(AgentError::new(format!(
                "run resource authority contains unactivated Skill `{}`.",
                package.skill_id()
            )));
        };
        if record.revision != package.revision().as_str() {
            return Err(AgentError::new(format!(
                "run resource authority for Skill `{}` has revision `{}` instead of `{}`.",
                record.id,
                package.revision(),
                record.revision
            )));
        }
        let (resource_count, kinds) = resources
            .expect("resource_packages can be non-empty only when a resource session exists")
            .binding_manifest(package)
            .map_err(|error| {
                AgentError::new(format!(
                    "cannot inspect run resource authority for Skill `{}`: {error}",
                    record.id
                ))
            })?;
        let actual_kinds = kinds
            .iter()
            .map(|kind| kind.stable_name().to_string())
            .collect::<Vec<_>>();
        if record.has_resources != (resource_count > 0) || record.resource_kinds != actual_kinds {
            return Err(AgentError::new(format!(
                "activated Skill `{}` resource capability metadata does not match exact run resource authority.",
                record.id
            )));
        }
    }
    for record in active.values().filter(|record| match requirement {
        ResourceAuthorityRequirement::Initial => true,
        ResourceAuthorityRequirement::Restored => {
            record.has_resources || is_discovery_managed(record)
        }
    }) {
        if !resource_packages.iter().any(|package| {
            package.skill_id().as_str() == record.id
                && package.revision().as_str() == record.revision
        }) {
            return Err(AgentError::new(format!(
                "activated Skill `{}` has no exact Host package authority for revision `{}`.",
                record.id, record.revision
            )));
        }
    }
    Ok(())
}

fn validate_activated_skill(skill: &AgentActivatedSkill) -> AgentResult<()> {
    if skill.instructions.trim().is_empty() {
        return Err(AgentError::new(format!(
            "Activated Skill `{}` has empty instructions.",
            skill.id
        )));
    }
    validate_record(&record_from_skill(skill, AgentSkillActivationActor::User))
}

fn validate_resolved_candidate(
    entry: &AgentDiscoverableSkill,
    candidate: &AgentResolvedSkillActivation,
) -> AgentResult<Option<SkillResourceSession>> {
    validate_activated_skill(&candidate.skill)?;
    if candidate.skill.id != entry.id
        || candidate.skill.revision != entry.revision
        || candidate.skill.name != entry.name
        || !candidate
            .skill
            .source
            .starts_with(&format!("{}:", entry.source_kind))
    {
        return Err(activation_contract_error(
            entry,
            "The host resolved a Skill different from the frozen catalog entry.",
        ));
    }

    let skill_id = SkillId::parse(entry.id.clone()).map_err(|error| {
        activation_contract_error(
            entry,
            format!("The frozen Skill id cannot identify resource authority: {error}"),
        )
    })?;
    let revision = SkillRevision::parse(entry.revision.clone()).map_err(|error| {
        activation_contract_error(
            entry,
            format!("The frozen Skill revision cannot identify resource authority: {error}"),
        )
    })?;
    let expected_package = SkillPackageUri::new(skill_id, revision);
    let binding = candidate
        .resources
        .freeze_exact_activation_binding(&expected_package)
        .map_err(|error| {
            activation_contract_error(
                entry,
                format!("The host returned invalid Skill resource authority: {error}"),
            )
        })?;

    match (candidate.skill.resources.as_ref(), binding) {
        (Some(_), None) => Err(activation_contract_error(
            entry,
            "The activated Skill declares resources but the host returned no package binding.",
        )),
        (Some(metadata), Some(binding)) => {
            let (actual_package, actual_count, actual_kinds) = binding
                .sole_activation_binding_manifest()
                .map_err(|error| {
                    activation_contract_error(
                        entry,
                        format!("The frozen Skill resource binding is invalid: {error}"),
                    )
                })?;
            let declared_package = SkillPackageUri::parse(&metadata.root_uri).map_err(|error| {
                activation_contract_error(
                    entry,
                    format!("The activated Skill resource rootUri is invalid: {error}"),
                )
            })?;
            let actual_kinds = actual_kinds
                .iter()
                .map(|kind| kind.stable_name().to_string())
                .collect::<Vec<_>>();
            if declared_package != actual_package
                || metadata.root_uri != actual_package.as_str()
                || metadata.resource_count != actual_count
                || metadata.kinds != actual_kinds
                || actual_count == 0
            {
                return Err(activation_contract_error(
                    entry,
                    "The activated Skill resource metadata does not exactly match its frozen package binding.",
                ));
            }
            Ok(Some(binding))
        }
        (None, Some(binding)) => {
            let (_, resource_count, _) =
                binding
                    .sole_activation_binding_manifest()
                    .map_err(|error| {
                        activation_contract_error(
                            entry,
                            format!("The frozen Skill resource binding is invalid: {error}"),
                        )
                    })?;
            if resource_count != 0 {
                return Err(activation_contract_error(
                    entry,
                    "The host returned undeclared resources for the activated Skill.",
                ));
            }
            // Keep the zero-resource package binding in the run authority. It grants no Resource
            // Runtime capability, but proves that the trusted Host resolved this exact immutable
            // package before native capabilities are unlocked.
            Ok(Some(binding))
        }
        (None, None) => Err(activation_contract_error(
            entry,
            "The host returned no exact package authority for the activated Skill.",
        )),
    }
}

fn activation_contract_error(
    entry: &AgentDiscoverableSkill,
    message: impl Into<String>,
) -> AgentError {
    structured_activation_error(
        "skill.activationContractViolation",
        message,
        "restartRun",
        Some(entry),
    )
}

fn activation_result(
    status: &str,
    skill: &ActivatedSkillRecord,
    activation_revision: &str,
    reason: &str,
) -> AgentResult<Value> {
    Ok(json!({
        "schemaVersion": SKILL_ACTIVATION_RESULT_SCHEMA_VERSION,
        "status": status,
        "activatedBy": skill.activated_by,
        "activationRevision": activation_revision,
        "reason": reason,
        "skill": {
            "id": skill.id,
            "name": skill.name,
            "revision": skill.revision,
            "source": skill.source,
            "hasResources": skill.has_resources,
        }
    }))
}

fn normalize_required(value: &str, field: &str) -> AgentResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(structured_activation_error(
            "skill.invalidActivationRequest",
            format!("skills_activate.{field} is required."),
            "correctArguments",
            None,
        ));
    }
    Ok(value.to_string())
}

fn valid_activation_ref(value: &str) -> bool {
    value.len() == SKILL_ACTIVATION_REF_CHARS
        && value.strip_prefix("s_").is_some_and(|digest| {
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn structured_activation_error(
    code: &str,
    message: impl Into<String>,
    recovery: &str,
    skill: Option<&AgentDiscoverableSkill>,
) -> AgentError {
    let message = message.into();
    AgentError::structured(
        code,
        message.clone(),
        json!({
            "type": "skillActivation",
            "code": code,
            "recovery": recovery,
            "message": message,
            "skillRef": skill.map(|skill| skill.activation_ref.as_str()),
            "skillId": skill.map(|skill| skill.id.as_str()),
        }),
    )
}

pub(in crate::runtime) fn checkpoint_authority_from_snapshots(
    snapshots: &[AgentExtensionSnapshot],
) -> AgentResult<Option<(Option<AgentSkillDiscoverySnapshot>, Vec<SkillSelection>)>> {
    let mut matching = snapshots
        .iter()
        .filter(|snapshot| snapshot.extension_id == SKILL_EXTENSION_ID);
    let Some(snapshot) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Err(AgentError::new(
            "Skill extension appears more than once in the checkpoint.",
        ));
    }
    if snapshot.version != SKILL_EXTENSION_VERSION {
        return Err(AgentError::new(format!(
            "Cannot restore Skill extension version {}; expected {}.",
            snapshot.version, SKILL_EXTENSION_VERSION
        )));
    }
    let state: SkillExtensionSnapshot = serde_json::from_value(snapshot.state.clone())
        .map_err(|error| AgentError::new(format!("Invalid Skill extension checkpoint: {error}")))?;
    if let Some(discovery) = &state.discovery {
        discovery.validate().map_err(|error| {
            AgentError::new(format!(
                "Invalid Skill extension discovery checkpoint: {error}"
            ))
        })?;
    }
    let mut ids = BTreeSet::new();
    let mut selections = Vec::new();
    let mut active = BTreeMap::new();
    for skill in state.skills {
        validate_record(&skill)?;
        if !ids.insert(skill.id.clone()) {
            return Err(AgentError::new(format!(
                "Skill extension checkpoint contains duplicate id `{}`.",
                skill.id
            )));
        }
        if skill.has_resources || is_discovery_managed(&skill) {
            selections.push(
                SkillSelection::parse(skill.id.clone(), skill.revision.clone()).map_err(
                    |error| AgentError::new(format!("Invalid Skill checkpoint selection: {error}")),
                )?,
            );
        }
        active.insert(skill.id.clone(), skill);
    }
    validate_restored_state(&active, state.discovery.as_ref())?;
    Ok(Some((state.discovery, selections)))
}

pub(in crate::runtime) fn redact_discovery_from_snapshots(
    snapshots: &mut Vec<AgentExtensionSnapshot>,
) {
    snapshots.retain_mut(|snapshot| {
        if snapshot.extension_id != SKILL_EXTENSION_ID {
            return true;
        }
        if snapshot.version != SKILL_EXTENSION_VERSION {
            // Unknown Skill state may contain run-scoped text under a future schema. Terminal
            // records cannot be resumed, so dropping it is safer than retaining unknown fields.
            return false;
        }
        let Ok(mut state) =
            serde_json::from_value::<SkillExtensionSnapshot>(snapshot.state.clone())
        else {
            return false;
        };
        state.discovery = None;
        match serde_json::to_value(state) {
            Ok(state) => {
                snapshot.state = state;
                true
            }
            Err(_) => false,
        }
    });
}

#[cfg(test)]
mod tests;
