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
};
use crate::tools::{AgentTool, AgentToolPermissionPolicy, ToolExecutionContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

pub(in crate::runtime) const SKILL_EXTENSION_ID: &str = "skills";
const SKILL_EXTENSION_VERSION: u32 = 2;
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
        validate_resource_authority(&active, resources.as_deref())?;
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
            RuntimeEffect::EmitEvent(AgentEvent::SkillActivated {
                run_id: self.run_id.clone(),
                activation_revision,
                activated_by: record.activated_by,
                skill: event_skill,
            }),
            RuntimeEffect::AppendRetainedContext(context),
        ])
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
        validate_resource_authority(&active, state.resources.as_deref())?;
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
                let retained_tokens =
                    activation_retained_tokens(&state, context, &result, None, &entry)?;
                consume_model_input_capacity(&mut state, retained_tokens);
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
                let retained_tokens =
                    activation_retained_tokens(&state, context, &result, None, &entry)?;
                consume_model_input_capacity(&mut state, retained_tokens);
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
        let retained_tokens =
            activation_retained_tokens(&state, context, &result, Some(&pending_context), &entry)?;
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
        consume_model_input_capacity(&mut state, retained_tokens);
        state.order.push(record.id.clone());
        state.active.insert(record.id.clone(), record.clone());
        state
            .pending_context
            .insert(record.id.clone(), pending_context);
        Ok(result)
    }
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

/// Measures exactly the successful ToolResult and newly disclosed Skill context that the runtime
/// will retain before the next model request. The check happens before resources or activation
/// state are mutated, so a capacity failure cannot leave a half-activated Skill.
fn activation_retained_tokens(
    state: &SkillActivationState,
    context: &ToolExecutionContext,
    result: &Value,
    pending_context: Option<&crate::context::ContextItem>,
    entry: &AgentDiscoverableSkill,
) -> AgentResult<u64> {
    ensure_model_input_capacity_available(state, entry)?;
    let Some(capacity) = state.model_input_capacity.as_ref() else {
        // Direct unit-level tool use does not cross a model request and therefore has no capacity
        // observation. Production runtime dispatch always records Some/None before execution.
        return Ok(0);
    };
    let tool_call_id = context.tool_call_id()?;
    let tool_result = AgentToolResult {
        call_id: tool_call_id.to_string(),
        tool: SKILL_ACTIVATE_TOOL_NAME.to_string(),
        ok: true,
        result: Some(result.clone()),
        error: None,
    };
    // Measure the same canonical projection the runtime will actually retain. The current
    // activation result is already bounded, but keeping this projection boundary shared prevents
    // future result-schema additions from silently drifting away from capacity accounting.
    let canonical = canonical_tool_result_for_context(&tool_result);
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
    if required_tokens > capacity.remaining_tokens {
        return Err(structured_activation_error(
            "skill.contextCapacityExceeded",
            format!(
                "Skill `{}` needs about {required_tokens} additional input tokens for its paired result and instructions, but this run has only {} available.",
                entry.name, capacity.remaining_tokens,
            ),
            "startNewRunOrReduceContext",
            Some(entry),
        ));
    }
    Ok(required_tokens)
}

fn consume_model_input_capacity(state: &mut SkillActivationState, tokens: u64) {
    if let Some(capacity) = state.model_input_capacity.as_mut() {
        debug_assert!(tokens <= capacity.remaining_tokens);
        capacity.remaining_tokens = capacity.remaining_tokens.saturating_sub(tokens);
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
    ActivatedSkillRecord {
        id: skill.id.clone(),
        name: skill.name.clone(),
        revision: skill.revision.clone(),
        source: skill.source.clone(),
        source_bytes: skill
            .source_bytes
            .max(u64::try_from(skill.instructions.len()).unwrap_or(u64::MAX)),
        has_resources: skill.resources.is_some(),
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
    Ok(())
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
        if record.activated_by == AgentSkillActivationActor::Model {
            let entry = discovery
                .and_then(|snapshot| snapshot.skills.iter().find(|entry| entry.id == record.id))
                .ok_or_else(|| {
                    AgentError::new(format!(
                        "restored model-activated Skill `{}` is absent from the frozen catalog.",
                        record.id
                    ))
                })?;
            if entry.revision != record.revision
                || entry.name != record.name
                || !record
                    .source
                    .starts_with(&format!("{}:", entry.source_kind))
            {
                return Err(AgentError::new(format!(
                    "restored model-activated Skill `{}` does not match the frozen catalog.",
                    record.id
                )));
            }
        }
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

fn validate_resource_authority(
    active: &BTreeMap<String, ActivatedSkillRecord>,
    resources: Option<&SkillResourceSession>,
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
    }
    for record in active.values().filter(|record| record.has_resources) {
        if !resource_packages.iter().any(|package| {
            package.skill_id().as_str() == record.id
                && package.revision().as_str() == record.revision
        }) {
            return Err(AgentError::new(format!(
                "activated Skill `{}` has resource metadata without exact run resource authority.",
                record.id
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
            Ok(None)
        }
        (None, None) => Ok(None),
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
        if skill.has_resources {
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
mod tests {
    use super::*;
    use crate::context::{ContextFrame, ContextTextBudget};
    use crate::llm::LlmMessageRole;
    use crate::protocol::{AgentActivatedSkillResources, AgentRunContext, AgentToolResult};
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillResourceKind, SkillRevision, SkillSourceId,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SECRET_INSTRUCTIONS: &str =
        "PRIVATE COMPLETE SKILL INSTRUCTIONS: always verify the generated artifact.";

    fn revision(fill: char) -> String {
        format!("skill-package-sha256-v2:{}", fill.to_string().repeat(64))
    }

    fn discoverable_skill(
        _activation_ref: &str,
        local_id: &str,
        name: &str,
        revision: &str,
    ) -> AgentDiscoverableSkill {
        let id = format!("bundled:application:{local_id}");
        AgentDiscoverableSkill {
            activation_ref: crate::skills::derive_skill_activation_ref(
                "catalog-sha256-v1:test",
                &id,
                revision,
            ),
            id,
            revision: revision.to_string(),
            name: name.to_string(),
            description: format!("Use {name} for focused work."),
            source_kind: "bundled".to_string(),
        }
    }

    fn discovery(
        skills: Vec<AgentDiscoverableSkill>,
        max_activated_skills: usize,
        max_total_source_bytes: usize,
    ) -> AgentSkillDiscoverySnapshot {
        AgentSkillDiscoverySnapshot {
            schema_version: crate::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
            catalog_revision: "catalog-sha256-v1:test".to_string(),
            prompt_token_budget: 2_000,
            skills,
            max_activated_skills,
            max_total_source_bytes,
        }
    }

    fn activated_skill(entry: &AgentDiscoverableSkill, instructions: &str) -> AgentActivatedSkill {
        AgentActivatedSkill {
            id: entry.id.clone(),
            name: entry.name.clone(),
            revision: entry.revision.clone(),
            source: "bundled:application".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: None,
        }
    }

    fn resolved_without_resources(
        entry: &AgentDiscoverableSkill,
        instructions: &str,
    ) -> AgentResolvedSkillActivation {
        AgentResolvedSkillActivation {
            skill: activated_skill(entry, instructions),
            resources: Arc::new(SkillResourceSession::empty()),
        }
    }

    fn candidate_resource_session(
        entry: &AgentDiscoverableSkill,
        resources: Vec<(String, SkillResourceKind, Vec<u8>)>,
    ) -> SkillResourceSession {
        let skill_id = SkillId::parse(entry.id.clone()).unwrap();
        let source_id = skill_id.source_id().clone();
        memory_resource_session_for_test(
            skill_id,
            SkillRevision::parse(entry.revision.clone()).unwrap(),
            source_id,
            resources,
        )
        .unwrap()
    }

    fn resolved_with_resources(
        entry: &AgentDiscoverableSkill,
        resources: Vec<(String, SkillResourceKind, Vec<u8>)>,
    ) -> AgentResolvedSkillActivation {
        let session = candidate_resource_session(entry, resources);
        let (package, resource_count, kinds) = session
            .sole_activation_binding_manifest()
            .expect("fixture binding manifest");
        let mut skill = activated_skill(entry, SECRET_INSTRUCTIONS);
        skill.resources = Some(AgentActivatedSkillResources {
            root_uri: package.to_string(),
            resource_count,
            kinds: kinds
                .iter()
                .map(|kind| kind.stable_name().to_string())
                .collect(),
        });
        AgentResolvedSkillActivation {
            skill,
            resources: Arc::new(session),
        }
    }

    fn resolver_for(
        candidates: Vec<AgentResolvedSkillActivation>,
        calls: Arc<AtomicUsize>,
    ) -> AgentSkillActivationResolver {
        Arc::new(move |selection| {
            calls.fetch_add(1, Ordering::SeqCst);
            candidates
                .iter()
                .find(|candidate| {
                    candidate.skill.id == selection.skill_id().as_str()
                        && candidate.skill.revision == selection.expected_revision().as_str()
                })
                .cloned()
                .ok_or_else(|| AgentError::new("fixture resolver received an unknown selection"))
        })
    }

    fn tool_context() -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(None::<&AgentRunContext>)
            .with_tool_call_id("activate-call-1".to_string())
    }

    fn successful_result(value: Value) -> AgentToolResult {
        AgentToolResult {
            call_id: "activate-call-1".to_string(),
            tool: SKILL_ACTIVATE_TOOL_NAME.to_string(),
            ok: true,
            result: Some(value),
            error: None,
        }
    }

    #[test]
    fn activation_tool_schema_requires_a_bounded_reason_and_rejects_invalid_reasons() {
        let extension = SkillActivationExtension::new(
            "run-1".to_string(),
            None,
            None,
            None,
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };
        let definition = tool.definition();

        assert_eq!(definition.name, "skills_activate");
        assert_eq!(definition.safety, AgentToolSafety::ReadOnly);
        assert_eq!(definition.approval_mode, AgentToolApprovalMode::Never);
        assert_eq!(
            definition.input_schema["required"],
            json!(["skillRef", "reason"])
        );
        assert_eq!(
            definition.input_schema["properties"]["reason"]["maxLength"],
            json!(MAX_ACTIVATION_REASON_CHARS)
        );
        assert_eq!(
            definition.input_schema["properties"]["skillRef"]["maxLength"],
            json!(SKILL_ACTIVATION_REF_CHARS)
        );
        assert_eq!(
            definition.input_schema["properties"]["skillRef"]["pattern"],
            json!("^s_[0-9a-f]{24}$")
        );
        assert_eq!(definition.input_schema["additionalProperties"], false);
        let valid_ref = "s_000000000000000000000000";

        let missing = tool
            .execute(&tool_context(), json!({ "skillRef": valid_ref }))
            .unwrap_err();
        assert_eq!(missing.code(), Some("skill.invalidActivationRequest"));

        let blank = tool
            .execute(
                &tool_context(),
                json!({ "skillRef": valid_ref, "reason": " \n\t " }),
            )
            .unwrap_err();
        assert_eq!(blank.code(), Some("skill.invalidActivationRequest"));

        let too_long = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": valid_ref,
                    "reason": "x".repeat(MAX_ACTIVATION_REASON_CHARS + 1),
                }),
            )
            .unwrap_err();
        assert_eq!(too_long.code(), Some("skill.activationReasonTooLong"));
    }

    #[test]
    fn activation_rejects_refs_outside_the_frozen_catalog_before_resolving() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let calls = Arc::new(AtomicUsize::new(0));
        let extension = SkillActivationExtension::new(
            "run-1".to_string(),
            Some(discovery(vec![entry.clone()], 4, 16_384)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
                calls.clone(),
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tool = SkillsActivateTool {
            state: extension.state,
        };

        let error = tool
            .execute(
                &tool_context(),
                json!({ "skillRef": "s_000000000000000000000000", "reason": "Need document guidance" }),
            )
            .unwrap_err();

        assert_eq!(error.code(), Some("skill.unknownActivationRef"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn full_instructions_are_disclosed_only_by_the_post_tool_context_effect() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-1".to_string(),
            Some(discovery(vec![entry.clone()], 4, 16_384)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
                calls,
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };

        assert!(extension
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
        let value = tool
            .execute(
                &tool_context(),
                json!({ "skillRef": entry.activation_ref.clone(), "reason": "Create a polished document" }),
            )
            .unwrap();
        let serialized_result = serde_json::to_string(&value).unwrap();
        assert_eq!(value["status"], "activated");
        assert!(!serialized_result.contains(SECRET_INSTRUCTIONS));
        assert!(extension
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());

        let expected_activation_revision = value["activationRevision"]
            .as_str()
            .expect("activation result revision")
            .to_string();
        let result = successful_result(value);
        let effects = extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted { result: &result })
            .unwrap();
        assert_eq!(effects.len(), 2);

        let mut emitted_event = false;
        let mut context = None;
        for effect in effects {
            match effect {
                RuntimeEffect::EmitEvent(AgentEvent::SkillActivated {
                    run_id,
                    activation_revision,
                    activated_by,
                    skill,
                }) => {
                    emitted_event = true;
                    assert_eq!(run_id, "run-1");
                    assert_eq!(skill.id, entry.id);
                    assert_eq!(activated_by, AgentSkillActivationActor::Model);
                    assert_eq!(activation_revision, expected_activation_revision);
                    assert_eq!(skill.source.kind, "bundled");
                    assert_eq!(skill.source.id, "bundled:application");
                }
                RuntimeEffect::AppendRetainedContext(item) => context = Some(item),
                RuntimeEffect::EmitEvent(_) => panic!("unexpected runtime event"),
            }
        }
        assert!(emitted_event);
        let frame = ContextFrame::new(vec![context.expect("missing Skill context")]);
        assert_eq!(
            frame.manifest().entries[0].sources,
            vec!["skill_instructions"]
        );
        let messages = frame.to_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, LlmMessageRole::User);
        assert!(messages[0].content.contains(SECRET_INSTRUCTIONS));
        assert!(messages[0].content.contains("<backend_activated_skill>"));
    }

    #[test]
    fn repeated_activation_is_idempotent_and_does_not_resolve_or_append_twice() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-1".to_string(),
            Some(discovery(vec![entry.clone()], 4, 16_384)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(&entry, SECRET_INSTRUCTIONS)],
                calls.clone(),
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };
        let args =
            json!({ "skillRef": entry.activation_ref.clone(), "reason": "Need document guidance" });

        let first = tool.execute(&tool_context(), args.clone()).unwrap();
        assert_eq!(first["status"], "activated");
        let effects = extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted {
                result: &successful_result(first),
            })
            .unwrap();
        assert_eq!(effects.len(), 2);

        let second = tool.execute(&tool_context(), args).unwrap();
        assert_eq!(second["status"], "alreadyActivated");
        assert_eq!(second["activatedBy"], "model");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted {
                result: &successful_result(second),
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_explicitly_selected_skill_keeps_user_attribution_when_the_model_reuses_it() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let initial_skill = activated_skill(&entry, SECRET_INSTRUCTIONS);
        let initial_activation = AgentSkillActivation {
            activation_revision: "initial-user-selection".to_string(),
            skills: vec![initial_skill],
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let extension = SkillActivationExtension::new(
            "run-user-selected".to_string(),
            Some(discovery(vec![entry.clone()], 4, 16_384)),
            Some(&initial_activation),
            Some(resolver_for(Vec::new(), calls.clone())),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tool = SkillsActivateTool {
            state: extension.state,
        };

        let result = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": entry.activation_ref,
                    "reason": "Continue using the explicitly selected Skill"
                }),
            )
            .unwrap();

        assert_eq!(result["status"], "alreadyActivated");
        assert_eq!(result["activatedBy"], "user");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn model_activation_extends_user_selection_with_the_canonical_inventory_revision() {
        let user_entry = discoverable_skill("user", "documents", "Documents", &revision('a'));
        let model_entry =
            discoverable_skill("model", "spreadsheets", "Spreadsheets", &revision('b'));
        let initial_activation = AgentSkillActivation {
            // The extension derives the current inventory revision from the frozen identities;
            // callers cannot give progressive activation a second revision meaning.
            activation_revision: "untrusted-caller-revision".to_string(),
            skills: vec![activated_skill(&user_entry, "user-selected instructions")],
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-mixed-activation".to_string(),
            Some(discovery(
                vec![user_entry.clone(), model_entry.clone()],
                4,
                16_384,
            )),
            Some(&initial_activation),
            Some(resolver_for(
                vec![resolved_without_resources(
                    &model_entry,
                    "model-activated instructions",
                )],
                calls.clone(),
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };

        let value = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": model_entry.activation_ref,
                    "reason": "Need spreadsheet guidance"
                }),
            )
            .unwrap();
        let expected_revision = activation_revision_for_identities([
            (user_entry.id.as_str(), user_entry.revision.as_str()),
            (model_entry.id.as_str(), model_entry.revision.as_str()),
        ])
        .as_str()
        .to_string();

        assert_eq!(value["activationRevision"], expected_revision);
        assert_eq!(value["activatedBy"], "model");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let effects = extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted {
                result: &successful_result(value),
            })
            .unwrap();
        let activated_event = effects
            .iter()
            .find_map(|effect| match effect {
                RuntimeEffect::EmitEvent(AgentEvent::SkillActivated {
                    activation_revision,
                    activated_by,
                    skill,
                    ..
                }) => Some((activation_revision, activated_by, skill)),
                _ => None,
            })
            .expect("model activation event");
        assert_eq!(activated_event.0, &expected_revision);
        assert_eq!(activated_event.1, &AgentSkillActivationActor::Model);
        assert_eq!(activated_event.2.id, model_entry.id);

        let snapshot = extension.snapshot_state().unwrap();
        assert_eq!(
            snapshot["skills"]
                .as_array()
                .unwrap()
                .iter()
                .map(|skill| skill["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![user_entry.id.as_str(), model_entry.id.as_str()]
        );
        assert_eq!(snapshot["skills"][0]["activatedBy"], "user");
        assert_eq!(snapshot["skills"][1]["activatedBy"], "model");
    }

    #[test]
    fn activation_enforces_skill_count_and_source_byte_budgets() {
        let first = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let second = discoverable_skill("s2", "spreadsheets", "Spreadsheets", &revision('b'));
        let count_calls = Arc::new(AtomicUsize::new(0));
        let count_extension = SkillActivationExtension::new(
            "run-count".to_string(),
            Some(discovery(vec![first.clone(), second.clone()], 1, 16_384)),
            None,
            Some(resolver_for(
                vec![
                    resolved_without_resources(&first, "first instructions"),
                    resolved_without_resources(&second, "second instructions"),
                ],
                count_calls,
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let count_tool = SkillsActivateTool {
            state: count_extension.state,
        };
        count_tool
            .execute(
                &tool_context(),
                json!({ "skillRef": first.activation_ref.clone(), "reason": "Need documents" }),
            )
            .unwrap();
        let count_error = count_tool
            .execute(
                &tool_context(),
                json!({ "skillRef": second.activation_ref.clone(), "reason": "Need spreadsheets too" }),
            )
            .unwrap_err();
        assert_eq!(count_error.code(), Some("skill.tooManySkills"));

        let byte_calls = Arc::new(AtomicUsize::new(0));
        let mut byte_candidate = resolved_without_resources(&first, "123456");
        // A host resolver cannot evade the byte budget by understating sourceBytes: the runtime
        // normalizes it against the actual instruction byte length before checking and storing.
        byte_candidate.skill.source_bytes = 1;
        let byte_extension = SkillActivationExtension::new(
            "run-bytes".to_string(),
            Some(discovery(vec![first.clone()], 1, 5)),
            None,
            Some(resolver_for(vec![byte_candidate], byte_calls)),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let byte_tool = SkillsActivateTool {
            state: byte_extension.state.clone(),
        };
        let byte_error = byte_tool
            .execute(
                &tool_context(),
                json!({ "skillRef": first.activation_ref.clone(), "reason": "Need documents" }),
            )
            .unwrap_err();
        assert_eq!(byte_error.code(), Some("skill.sourceBudgetExceeded"));
        assert!(byte_extension.state.lock().active.is_empty());
    }

    #[test]
    fn activation_rejects_instructions_that_cannot_fit_the_next_model_request() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-capacity".to_string(),
            Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(
                    &entry,
                    "complete Skill instructions that require retained context",
                )],
                calls,
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        extension.update_model_input_capacity(Some(ModelInputCapacity {
            remaining_tokens: 1,
            text_budget: ContextTextBudget::heuristic(1),
        }));
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };

        let error = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": entry.activation_ref,
                    "reason": "Need document guidance"
                }),
            )
            .unwrap_err();

        assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
        let state = extension.state.lock();
        assert!(state.active.is_empty());
        assert!(state.pending_context.is_empty());
        assert_eq!(
            state
                .model_input_capacity
                .as_ref()
                .unwrap()
                .remaining_tokens,
            1
        );
        assert!(state.resources.as_ref().unwrap().is_empty());
    }

    #[test]
    fn activation_capacity_accounts_for_the_retained_assistant_response() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-response-capacity".to_string(),
            Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(
                    &entry,
                    "complete Skill instructions that require retained context",
                )],
                calls,
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        extension.update_model_input_capacity(Some(ModelInputCapacity {
            remaining_tokens: 257,
            text_budget: ContextTextBudget::heuristic(257),
        }));
        extension.consume_model_input_capacity(256);
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };

        let error = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": entry.activation_ref,
                    "reason": "Need document guidance"
                }),
            )
            .unwrap_err();

        assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
        let state = extension.state.lock();
        assert!(state.active.is_empty());
        assert!(state.pending_context.is_empty());
        assert_eq!(
            state
                .model_input_capacity
                .as_ref()
                .unwrap()
                .remaining_tokens,
            1
        );
        assert!(state.resources.as_ref().unwrap().is_empty());
    }

    #[test]
    fn activation_capacity_includes_the_exact_paired_tool_result() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let candidate = resolved_without_resources(&entry, "x");
        let record = record_from_skill(&candidate.skill, AgentSkillActivationActor::Model);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-result-capacity".to_string(),
            Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
            None,
            Some(resolver_for(vec![candidate.clone()], calls)),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let activation_revision = extension
            .state
            .lock()
            .activation_revision_with(&record)
            .unwrap();
        let pending = activated_skill_context_item(&activation_revision, &candidate.skill).unwrap();
        let budget = ContextTextBudget::heuristic(64 * 1024);
        let instruction_tokens = ContextFrame::new(vec![pending])
            .into_messages()
            .iter()
            .map(|message| budget.estimate_message(message))
            .sum();
        extension.update_model_input_capacity(Some(ModelInputCapacity {
            remaining_tokens: instruction_tokens,
            text_budget: budget,
        }));
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };

        let error = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": entry.activation_ref,
                    "reason": "验".repeat(MAX_ACTIVATION_REASON_CHARS)
                }),
            )
            .unwrap_err();

        assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
        let state = extension.state.lock();
        assert!(state.active.is_empty());
        assert!(state.pending_context.is_empty());
        assert_eq!(
            state
                .model_input_capacity
                .as_ref()
                .unwrap()
                .remaining_tokens,
            instruction_tokens
        );
        assert!(state.resources.as_ref().unwrap().is_empty());
    }

    #[test]
    fn failed_activation_results_reduce_capacity_before_later_activations() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-failed-result-capacity".to_string(),
            Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(&entry, "instructions")],
                calls.clone(),
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let failed_result = AgentToolResult {
            call_id: "failed-activate".to_string(),
            tool: SKILL_ACTIVATE_TOOL_NAME.to_string(),
            ok: false,
            result: Some(json!({
                "type": "skillActivation",
                "code": "skill.unknownActivationRef",
                "recovery": "useAvailableRef"
            })),
            error: Some("Skill activation failed.".to_string()),
        };
        let canonical = canonical_tool_result_for_context(&failed_result);
        let budget = ContextTextBudget::heuristic(64 * 1024);
        let failed_result_tokens = budget.estimate_message(&LlmMessage::tool_result(
            &canonical.call_id,
            render_tool_observation(&canonical),
            true,
        ));
        extension.update_model_input_capacity(Some(ModelInputCapacity {
            remaining_tokens: failed_result_tokens,
            text_budget: budget,
        }));

        assert!(extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted {
                result: &failed_result,
            })
            .unwrap()
            .is_empty());
        assert_eq!(
            extension
                .state
                .lock()
                .model_input_capacity
                .as_ref()
                .unwrap()
                .remaining_tokens,
            0
        );

        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };
        let error = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": entry.activation_ref,
                    "reason": "Need document guidance"
                }),
            )
            .unwrap_err();

        assert_eq!(error.code(), Some("skill.contextCapacityExceeded"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let state = extension.state.lock();
        assert!(state.active.is_empty());
        assert!(state.pending_context.is_empty());
        assert!(state.resources.as_ref().unwrap().is_empty());
    }

    #[test]
    fn successful_activation_result_and_instructions_are_not_charged_twice() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let mut extension = SkillActivationExtension::new(
            "run-success-capacity".to_string(),
            Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(&entry, "instructions")],
                Arc::new(AtomicUsize::new(0)),
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        extension.update_model_input_capacity(Some(ModelInputCapacity {
            remaining_tokens: 64 * 1024,
            text_budget: ContextTextBudget::heuristic(64 * 1024),
        }));
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };
        let value = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": entry.activation_ref,
                    "reason": "Need document guidance"
                }),
            )
            .unwrap();
        let remaining_after_execute = extension
            .state
            .lock()
            .model_input_capacity
            .as_ref()
            .unwrap()
            .remaining_tokens;

        let effects = extension
            .on_event(&RuntimeExtensionEvent::ToolCompleted {
                result: &successful_result(value),
            })
            .unwrap();

        assert_eq!(effects.len(), 2);
        assert_eq!(
            extension
                .state
                .lock()
                .model_input_capacity
                .as_ref()
                .unwrap()
                .remaining_tokens,
            remaining_after_execute
        );
    }

    #[test]
    fn dynamic_activation_fails_closed_when_model_capacity_is_unknown() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut extension = SkillActivationExtension::new(
            "run-unknown-capacity".to_string(),
            Some(discovery(vec![entry.clone()], 4, 64 * 1024)),
            None,
            Some(resolver_for(
                vec![resolved_without_resources(&entry, "instructions")],
                calls.clone(),
            )),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        // Production runtime invokes this after every accepted model request. `None` means the
        // host supplied no authoritative context window, not that capacity is unlimited.
        extension.update_model_input_capacity(None);
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };

        let error = tool
            .execute(
                &tool_context(),
                json!({
                    "skillRef": entry.activation_ref,
                    "reason": "Need document guidance"
                }),
            )
            .unwrap_err();

        assert_eq!(error.code(), Some("skill.contextCapacityUnavailable"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(extension.state.lock().active.is_empty());
    }

    #[test]
    fn resolved_candidate_rejects_additional_or_foreign_resource_packages() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let other = discoverable_skill("s2", "spreadsheets", "Spreadsheets", &revision('b'));
        let candidate = resolved_with_resources(
            &entry,
            vec![(
                "references/workflows.md".to_string(),
                SkillResourceKind::Reference,
                b"trusted workflow".to_vec(),
            )],
        );
        let other_session = candidate_resource_session(&other, Vec::new());
        candidate.resources.extend_from(&other_session).unwrap();

        let error = validate_resolved_candidate(&entry, &candidate).unwrap_err();

        assert_eq!(error.code(), Some("skill.activationContractViolation"));
        assert!(error.to_string().contains("package bindings"));

        let foreign_only = AgentResolvedSkillActivation {
            skill: activated_skill(&entry, SECRET_INSTRUCTIONS),
            resources: Arc::new(other_session),
        };
        let error = validate_resolved_candidate(&entry, &foreign_only).unwrap_err();
        assert_eq!(error.code(), Some("skill.activationContractViolation"));
        assert!(error.to_string().contains("instead of"));
    }

    #[test]
    fn resolved_candidate_requires_resource_metadata_to_match_the_exact_binding() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let candidate = resolved_with_resources(
            &entry,
            vec![
                (
                    "assets/theme.bin".to_string(),
                    SkillResourceKind::Asset,
                    b"theme".to_vec(),
                ),
                (
                    "references/workflows.md".to_string(),
                    SkillResourceKind::Reference,
                    b"workflow".to_vec(),
                ),
            ],
        );
        assert!(validate_resolved_candidate(&entry, &candidate)
            .unwrap()
            .is_some());

        let mut wrong_root = candidate.clone();
        wrong_root.skill.resources.as_mut().unwrap().root_uri = SkillPackageUri::new(
            SkillId::parse("bundled:application:other").unwrap(),
            SkillRevision::parse(revision('c')).unwrap(),
        )
        .to_string();
        let mut wrong_count = candidate.clone();
        wrong_count.skill.resources.as_mut().unwrap().resource_count += 1;
        let mut wrong_kinds = candidate.clone();
        wrong_kinds
            .skill
            .resources
            .as_mut()
            .unwrap()
            .kinds
            .reverse();
        let mut missing_binding = candidate.clone();
        missing_binding.resources = Arc::new(SkillResourceSession::empty());

        for invalid in [wrong_root, wrong_count, wrong_kinds, missing_binding] {
            let error = validate_resolved_candidate(&entry, &invalid).unwrap_err();
            assert_eq!(error.code(), Some("skill.activationContractViolation"));
        }
    }

    #[test]
    fn resolved_candidate_without_metadata_accepts_only_no_or_exact_empty_binding() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let no_binding = resolved_without_resources(&entry, SECRET_INSTRUCTIONS);
        assert!(validate_resolved_candidate(&entry, &no_binding)
            .unwrap()
            .is_none());

        let exact_empty = AgentResolvedSkillActivation {
            skill: activated_skill(&entry, SECRET_INSTRUCTIONS),
            resources: Arc::new(candidate_resource_session(&entry, Vec::new())),
        };
        assert!(validate_resolved_candidate(&entry, &exact_empty)
            .unwrap()
            .is_none());

        let undeclared = AgentResolvedSkillActivation {
            skill: activated_skill(&entry, SECRET_INSTRUCTIONS),
            resources: Arc::new(candidate_resource_session(
                &entry,
                vec![(
                    "references/hidden.md".to_string(),
                    SkillResourceKind::Reference,
                    b"hidden".to_vec(),
                )],
            )),
        };
        let error = validate_resolved_candidate(&entry, &undeclared).unwrap_err();
        assert_eq!(error.code(), Some("skill.activationContractViolation"));
        assert!(error.to_string().contains("undeclared resources"));
    }

    #[test]
    fn checkpoint_contains_only_activation_summaries_and_restores_resource_selections() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let skill_id = SkillId::parse(entry.id.clone()).unwrap();
        let skill_revision = SkillRevision::parse(entry.revision.clone()).unwrap();
        let source_id = SkillSourceId::parse("bundled:application").unwrap();
        let candidate_session = memory_resource_session_for_test(
            skill_id,
            skill_revision,
            source_id,
            vec![(
                "references/workflows.md".to_string(),
                SkillResourceKind::Reference,
                b"trusted workflow".to_vec(),
            )],
        )
        .unwrap();
        let package_uri = candidate_session.package_uris()[0].to_string();
        let mut skill = activated_skill(&entry, SECRET_INSTRUCTIONS);
        skill.resources = Some(AgentActivatedSkillResources {
            root_uri: package_uri,
            resource_count: 1,
            kinds: vec!["reference".to_string()],
        });
        let candidate = AgentResolvedSkillActivation {
            skill,
            resources: Arc::new(candidate_session),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let extension = SkillActivationExtension::new(
            "run-1".to_string(),
            Some(discovery(vec![entry.clone()], 2, 16_384)),
            None,
            Some(resolver_for(vec![candidate], calls)),
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tool = SkillsActivateTool {
            state: extension.state.clone(),
        };
        tool.execute(
            &tool_context(),
            json!({ "skillRef": entry.activation_ref.clone(), "reason": "Need the bundled workflow" }),
        )
        .unwrap();

        let state = extension.snapshot_state().unwrap();
        let serialized = serde_json::to_string(&state).unwrap();
        assert!(!serialized.contains(SECRET_INSTRUCTIONS));
        assert!(!serialized.contains("instructions"));
        assert!(!serialized.contains("pendingContext"));
        assert_eq!(state["skills"][0]["id"], entry.id);
        assert_eq!(state["skills"][0]["hasResources"], true);

        let (_, selections) = checkpoint_authority_from_snapshots(&[AgentExtensionSnapshot {
            extension_id: SKILL_EXTENSION_ID.to_string(),
            version: SKILL_EXTENSION_VERSION,
            state,
        }])
        .unwrap()
        .expect("Skill checkpoint authority");
        assert_eq!(selections.len(), 1);
        assert_eq!(selections[0].skill_id().as_str(), entry.id);
        assert_eq!(selections[0].expected_revision().as_str(), entry.revision);
    }

    #[test]
    fn restore_rejects_model_activations_outside_the_frozen_catalog() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let mut extension = SkillActivationExtension::new(
            "run-restore".to_string(),
            Some(discovery(vec![entry.clone()], 2, 16_384)),
            None,
            None,
            Some(Arc::new(SkillResourceSession::empty())),
        )
        .unwrap();
        let tampered = SkillExtensionSnapshot {
            discovery: Some(discovery(vec![entry], 2, 16_384)),
            skills: vec![ActivatedSkillRecord {
                id: "bundled:application:spreadsheets".to_string(),
                name: "Spreadsheets".to_string(),
                revision: revision('b'),
                source: "bundled:application".to_string(),
                source_bytes: 128,
                has_resources: false,
                activated_by: AgentSkillActivationActor::Model,
            }],
        };

        let error = extension
            .restore_state(
                SKILL_EXTENSION_VERSION,
                serde_json::to_value(tampered).unwrap(),
            )
            .unwrap_err();

        assert!(error.to_string().contains("absent from the frozen catalog"));
        assert!(extension.state.lock().active.is_empty());
    }

    #[test]
    fn terminal_redaction_removes_catalog_text_and_drops_unknown_skill_state() {
        let entry = discoverable_skill("s1", "documents", "Documents", &revision('a'));
        let state = SkillExtensionSnapshot {
            discovery: Some(discovery(vec![entry.clone()], 2, 16_384)),
            skills: vec![ActivatedSkillRecord {
                id: entry.id,
                name: entry.name,
                revision: entry.revision,
                source: "bundled:application".to_string(),
                source_bytes: 128,
                has_resources: false,
                activated_by: AgentSkillActivationActor::Model,
            }],
        };
        let mut snapshots = vec![
            AgentExtensionSnapshot {
                extension_id: SKILL_EXTENSION_ID.to_string(),
                version: SKILL_EXTENSION_VERSION,
                state: serde_json::to_value(state).unwrap(),
            },
            AgentExtensionSnapshot {
                extension_id: SKILL_EXTENSION_ID.to_string(),
                version: SKILL_EXTENSION_VERSION + 1,
                state: json!({ "futureCatalog": "MUST_NOT_SURVIVE" }),
            },
            AgentExtensionSnapshot {
                extension_id: "todo".to_string(),
                version: 1,
                state: json!({ "safe": true }),
            },
        ];

        redact_discovery_from_snapshots(&mut snapshots);

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].extension_id, SKILL_EXTENSION_ID);
        assert_eq!(snapshots[0].state["discovery"], Value::Null);
        assert_eq!(snapshots[0].state["skills"].as_array().unwrap().len(), 1);
        assert_eq!(snapshots[1].extension_id, "todo");
        assert!(!serde_json::to_string(&snapshots)
            .unwrap()
            .contains("MUST_NOT_SURVIVE"));
    }
}
