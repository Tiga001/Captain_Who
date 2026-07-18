//! Deterministic, side-effect-free context compaction planning.
//!
//! The planner consumes the exact classified estimate used by the capacity gate plus a
//! content-free inventory of already measured frame items. It never reads message bodies, invokes
//! a model, mutates a frame or writes storage. A later executor can apply `ContextCompactionPlan`
//! and then restart the normal assembly and measurement cycle.

use super::budget::{ContextBudgetStatus, ContextCompactionQuery};
use super::frame::{
    ContextFramePlanningItem, ContextOrigin, ContextOriginKind, ContextSource, ContextUsageClass,
};
use super::ContextJournalCursor;
use crate::llm::LlmMessageRole;
use crate::protocol::{AgentToolDefinition, AgentToolSafety};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const DEFAULT_COMPACTION_TRIGGER_PERCENT: u64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextCompactionPolicy {
    soft_trigger_percent: u64,
    base_headroom_percent: u64,
    run_growth_reserve_percent: u64,
    maximum_headroom_percent: u64,
    durable_trigger_percent: u64,
    durable_target_percent: u64,
    minimum_reclaim_tokens: u64,
    maximum_reclaim_floor_tokens: u64,
}

impl Default for ContextCompactionPolicy {
    fn default() -> Self {
        Self {
            soft_trigger_percent: DEFAULT_COMPACTION_TRIGGER_PERCENT,
            base_headroom_percent: 25,
            run_growth_reserve_percent: 50,
            maximum_headroom_percent: 40,
            durable_trigger_percent: DEFAULT_COMPACTION_TRIGGER_PERCENT,
            durable_target_percent: 15,
            minimum_reclaim_tokens: 512,
            maximum_reclaim_floor_tokens: 4_096,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextCompactionPlanStatus {
    Unconfigured,
    NotRequired,
    Required,
    InsufficientCompactableContext,
    InvalidConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextCompactionProtectionReason {
    FixedRequest,
    RequestOnly,
    CurrentUser,
    SkillInstructions,
    UserAttachment,
    RuntimeGuard,
    VisualInput,
    MixedAtomicGroup,
    UncommittedRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextCompactionItemRange {
    pub(crate) start_index: usize,
    pub(crate) end_index_exclusive: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextCompactionStep {
    pub(crate) ranges: Vec<ContextCompactionItemRange>,
    pub(crate) atomic_unit_count: usize,
    pub(crate) source_input_tokens: u64,
    /// Aspirational size of the complete replacement (semantic summary plus deterministic
    /// continuity data). It is used only for planning projections and is never an output limit.
    pub(crate) target_replacement_tokens: u64,
    pub(crate) expected_reclaimed_tokens: u64,
    pub(crate) contains_side_effects: bool,
    pub(crate) contains_errors: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) durable_prefix: Option<ContextCompactionDurablePrefix>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextCompactionDurablePrefix {
    pub(crate) previous_summary_id: Option<String>,
    pub(crate) covered_through: ContextJournalCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextCompactionProtectedEstimate {
    pub(crate) input_tokens: u64,
    pub(crate) atomic_unit_count: usize,
    pub(crate) reasons: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextCompactionPlan {
    pub(crate) status: ContextCompactionPlanStatus,
    /// Every item range in this plan addresses this exact frame revision. An executor must read
    /// all selected ranges from that snapshot before committing any replacement.
    pub(crate) context_revision: u64,
    pub(crate) persistent_revision: u64,
    pub(crate) request_input_tokens: u64,
    pub(crate) available_input_tokens: Option<u64>,
    pub(crate) soft_trigger_input_tokens: Option<u64>,
    pub(crate) target_input_tokens: Option<u64>,
    pub(crate) durable_capacity_tokens: Option<u64>,
    pub(crate) durable_trigger_input_tokens: Option<u64>,
    pub(crate) durable_target_input_tokens: Option<u64>,
    pub(crate) required_reclaimed_tokens: u64,
    pub(crate) required_durable_reclaimed_tokens: u64,
    pub(crate) planned_reclaimed_tokens: u64,
    pub(crate) projected_request_input_tokens: u64,
    pub(crate) projected_durable_input_tokens: u64,
    pub(crate) request_target_satisfied: bool,
    pub(crate) durable_target_satisfied: bool,
    pub(crate) best_effort: bool,
    pub(crate) compactable_input_tokens: u64,
    pub(crate) protected: ContextCompactionProtectedEstimate,
    pub(crate) steps: Vec<ContextCompactionStep>,
}

pub(crate) struct ContextCompactionPlanner {
    policy: ContextCompactionPolicy,
    tool_safety: BTreeMap<String, AgentToolSafety>,
}

impl ContextCompactionPlanner {
    pub(crate) fn for_tools(tools: &[AgentToolDefinition]) -> Self {
        Self {
            policy: ContextCompactionPolicy::default(),
            tool_safety: tools
                .iter()
                .map(|tool| (tool.name.clone(), tool.safety))
                .collect(),
        }
    }

    pub(crate) fn plan(
        &self,
        query: &ContextCompactionQuery,
        items: &[ContextFramePlanningItem],
        protect_current_user: bool,
    ) -> ContextCompactionPlan {
        let units = build_atomic_units(items, &self.tool_safety);
        let last_durable_user_index = items
            .iter()
            .rev()
            .find(|item| {
                item.usage_class == ContextUsageClass::Durable && item.role == LlmMessageRole::User
            })
            .map(|item| item.index);
        let mut protected_reasons = BTreeMap::<String, u64>::new();
        let mut protected_unit_count = 0_usize;
        let mut candidates = Vec::new();
        for unit in units.iter().cloned() {
            if let Some(reason) =
                absolute_protection_reason(&unit, last_durable_user_index, protect_current_user)
            {
                protected_unit_count = protected_unit_count.saturating_add(1);
                merge_reason_tokens(&mut protected_reasons, reason, unit.tokens);
                continue;
            }
            match unit.usage_class {
                ContextUsageClass::Durable => {}
                ContextUsageClass::Fixed | ContextUsageClass::RequestOnly => {
                    unreachable!("fixed and request-only units are protected above")
                }
                ContextUsageClass::RunTransient => {
                    unreachable!("run-transient units are protected above")
                }
            }
            candidates.push(CompactionCandidate { unit });
        }

        let compactable_input_tokens = candidates
            .iter()
            .map(|candidate| candidate.unit.tokens)
            .sum::<u64>();
        let protected_input_tokens = query
            .additive_input_tokens
            .saturating_sub(compactable_input_tokens);
        let inventory_protected_tokens = protected_reasons.values().copied().sum::<u64>();
        let unrepresented_fixed_tokens =
            protected_input_tokens.saturating_sub(inventory_protected_tokens);
        if unrepresented_fixed_tokens > 0 {
            let total = protected_reasons
                .entry(protection_reason_name(
                    ContextCompactionProtectionReason::FixedRequest,
                ))
                .or_default();
            *total = total.saturating_add(unrepresented_fixed_tokens);
        }
        let protected = ContextCompactionProtectedEstimate {
            input_tokens: protected_input_tokens,
            atomic_unit_count: protected_unit_count,
            reasons: protected_reasons,
        };

        let Some(available_input_tokens) = query.available_input_tokens else {
            return empty_plan(
                ContextCompactionPlanStatus::Unconfigured,
                query,
                ContextCompactionThresholds::default(),
                compactable_input_tokens,
                protected,
            );
        };
        if query.status == ContextBudgetStatus::InvalidConfiguration || available_input_tokens == 0
        {
            return empty_plan(
                ContextCompactionPlanStatus::InvalidConfiguration,
                query,
                ContextCompactionThresholds::zero(),
                compactable_input_tokens,
                protected,
            );
        }

        let soft_trigger_input_tokens =
            percent_ceil(available_input_tokens, self.policy.soft_trigger_percent);
        let target_input_tokens = self.target_input_tokens(query, available_input_tokens);
        let durable_input_tokens = query.breakdown.durable.input_tokens;
        let durable_capacity_tokens =
            available_input_tokens.saturating_sub(query.breakdown.fixed.input_tokens);
        let durable_trigger_input_tokens =
            percent_ceil(durable_capacity_tokens, self.policy.durable_trigger_percent);
        let durable_target_input_tokens =
            percent_ceil(durable_capacity_tokens, self.policy.durable_target_percent);
        let thresholds = ContextCompactionThresholds {
            request_trigger_input_tokens: Some(soft_trigger_input_tokens),
            request_target_input_tokens: Some(target_input_tokens),
            durable_capacity_tokens: Some(durable_capacity_tokens),
            durable_trigger_input_tokens: Some(durable_trigger_input_tokens),
            durable_target_input_tokens: Some(durable_target_input_tokens),
        };
        let request_pressure = query.request_input_tokens >= soft_trigger_input_tokens
            || query.status == ContextBudgetStatus::OverBudget;
        let durable_pressure = if durable_capacity_tokens == 0 {
            durable_input_tokens > 0
        } else {
            durable_input_tokens >= durable_trigger_input_tokens
        };
        let compaction_required = request_pressure || durable_pressure;
        if !compaction_required {
            return empty_plan(
                ContextCompactionPlanStatus::NotRequired,
                query,
                thresholds,
                compactable_input_tokens,
                protected,
            );
        }

        let reclaim_floor = percent_ceil(available_input_tokens, 5)
            .max(self.policy.minimum_reclaim_tokens)
            .min(self.policy.maximum_reclaim_floor_tokens)
            .min(query.request_input_tokens);
        let required_request_reclaimed_tokens = if request_pressure {
            query
                .request_input_tokens
                .saturating_sub(target_input_tokens)
                .max(reclaim_floor)
        } else {
            0
        };
        // Trigger sources only decide when compaction starts. Once started, every execution aims
        // at the same durable target so transient request pressure cannot degrade into a series of
        // tiny summaries.
        let required_durable_reclaimed_tokens =
            durable_input_tokens.saturating_sub(durable_target_input_tokens);
        let required_reclaimed_tokens =
            required_request_reclaimed_tokens.max(required_durable_reclaimed_tokens);

        let mut selected = Vec::<CompactionCandidate>::new();
        for candidate in &candidates {
            if maximum_reclaimable_tokens(&selected) >= required_reclaimed_tokens {
                break;
            }
            selected.push(candidate.clone());
        }
        normalize_stable_durable_prefix(&mut selected, &units, &candidates);

        let steps = build_steps(&selected, required_reclaimed_tokens);
        let planned_reclaimed_tokens = steps
            .iter()
            .map(|step| step.expected_reclaimed_tokens)
            .sum::<u64>();
        let projected_request_input_tokens = query
            .request_input_tokens
            .saturating_sub(planned_reclaimed_tokens);
        let projected_durable_input_tokens =
            durable_input_tokens.saturating_sub(planned_reclaimed_tokens);
        let request_target_satisfied =
            !request_pressure || projected_request_input_tokens <= target_input_tokens;
        let durable_target_satisfied =
            projected_durable_input_tokens <= durable_target_input_tokens;
        let best_effort = !request_target_satisfied || !durable_target_satisfied;
        let status = if planned_reclaimed_tokens > 0 {
            ContextCompactionPlanStatus::Required
        } else {
            ContextCompactionPlanStatus::InsufficientCompactableContext
        };

        ContextCompactionPlan {
            status,
            context_revision: query.context_revision,
            persistent_revision: query.persistent_revision,
            request_input_tokens: query.request_input_tokens,
            available_input_tokens: Some(available_input_tokens),
            soft_trigger_input_tokens: Some(soft_trigger_input_tokens),
            target_input_tokens: Some(target_input_tokens),
            durable_capacity_tokens: Some(durable_capacity_tokens),
            durable_trigger_input_tokens: Some(durable_trigger_input_tokens),
            durable_target_input_tokens: Some(durable_target_input_tokens),
            required_reclaimed_tokens,
            required_durable_reclaimed_tokens,
            planned_reclaimed_tokens,
            projected_request_input_tokens,
            projected_durable_input_tokens,
            request_target_satisfied,
            durable_target_satisfied,
            best_effort,
            compactable_input_tokens,
            protected,
            steps,
        }
    }

    fn target_input_tokens(
        &self,
        query: &ContextCompactionQuery,
        available_input_tokens: u64,
    ) -> u64 {
        let base_headroom = percent_ceil(available_input_tokens, self.policy.base_headroom_percent);
        let growth_floor = percent_ceil(available_input_tokens, 20).min(8_192);
        let run_growth = query
            .breakdown
            .run_transient
            .input_tokens
            .saturating_mul(self.policy.run_growth_reserve_percent)
            .div_ceil(100)
            .max(growth_floor);
        let maximum_headroom =
            percent_ceil(available_input_tokens, self.policy.maximum_headroom_percent);
        let desired_headroom = base_headroom.max(run_growth.min(maximum_headroom));
        available_input_tokens.saturating_sub(desired_headroom)
    }
}

#[derive(Debug, Clone)]
struct AtomicContextUnit {
    start_index: usize,
    end_index_exclusive: usize,
    usage_class: ContextUsageClass,
    tokens: u64,
    sources: BTreeSet<ContextSource>,
    contains_images: bool,
    contains_errors: bool,
    contains_side_effects: bool,
    mixed_usage_classes: bool,
    origin: Option<ContextOrigin>,
    first_role: LlmMessageRole,
    last_role: LlmMessageRole,
}

#[derive(Debug, Clone)]
struct CompactionCandidate {
    unit: AtomicContextUnit,
}

#[derive(Debug, Clone, Copy, Default)]
struct ContextCompactionThresholds {
    request_trigger_input_tokens: Option<u64>,
    request_target_input_tokens: Option<u64>,
    durable_capacity_tokens: Option<u64>,
    durable_trigger_input_tokens: Option<u64>,
    durable_target_input_tokens: Option<u64>,
}

impl ContextCompactionThresholds {
    fn zero() -> Self {
        Self {
            request_trigger_input_tokens: Some(0),
            request_target_input_tokens: Some(0),
            durable_capacity_tokens: Some(0),
            durable_trigger_input_tokens: Some(0),
            durable_target_input_tokens: Some(0),
        }
    }
}

fn build_atomic_units(
    items: &[ContextFramePlanningItem],
    tool_safety: &BTreeMap<String, AgentToolSafety>,
) -> Vec<AtomicContextUnit> {
    let mut units = Vec::new();
    let mut cursor = 0_usize;
    while cursor < items.len() {
        let first = &items[cursor];
        let end = match (
            first.usage_class == ContextUsageClass::Durable,
            first.origin.as_ref(),
        ) {
            (true, Some(origin)) => items[cursor..]
                .iter()
                .take_while(|item| {
                    item.usage_class == ContextUsageClass::Durable
                        && item.origin.as_ref() == Some(origin)
                })
                .count()
                .saturating_add(cursor),
            _ => match first.group_id.as_deref() {
                Some(group_id) => items[cursor..]
                    .iter()
                    .take_while(|item| item.group_id.as_deref() == Some(group_id))
                    .count()
                    .saturating_add(cursor),
                None => cursor.saturating_add(1),
            },
        };
        let slice = &items[cursor..end];
        let usage_class = first.usage_class;
        let tool_names = slice
            .iter()
            .flat_map(|item| item.tool_names.iter().cloned())
            .collect::<Vec<_>>();
        let contains_side_effects = tool_names.iter().any(|tool| {
            tool_safety
                .get(tool)
                .is_none_or(|safety| *safety != AgentToolSafety::ReadOnly)
        });
        units.push(AtomicContextUnit {
            start_index: first.index,
            end_index_exclusive: slice.last().map_or(first.index.saturating_add(1), |item| {
                item.index.saturating_add(1)
            }),
            usage_class,
            tokens: slice.iter().map(|item| item.estimated_tokens).sum(),
            sources: slice
                .iter()
                .flat_map(|item| item.sources.iter().copied())
                .collect(),
            contains_images: slice.iter().any(|item| item.image_count > 0),
            contains_errors: slice.iter().any(|item| item.is_error),
            contains_side_effects,
            mixed_usage_classes: slice.iter().any(|item| item.usage_class != usage_class),
            origin: first.origin.clone(),
            first_role: first.role,
            last_role: slice.last().map_or(first.role, |item| item.role),
        });
        cursor = end;
    }
    units
}

fn absolute_protection_reason(
    unit: &AtomicContextUnit,
    last_durable_user_index: Option<usize>,
    protect_current_user: bool,
) -> Option<ContextCompactionProtectionReason> {
    if unit.mixed_usage_classes {
        return Some(ContextCompactionProtectionReason::MixedAtomicGroup);
    }
    // Activated Skill instructions are an immutable run snapshot. They must survive every
    // compaction attempt exactly as selected, even if run-transient policy is refined later.
    if unit.sources.contains(&ContextSource::SkillInstructions) {
        return Some(ContextCompactionProtectionReason::SkillInstructions);
    }
    match unit.usage_class {
        ContextUsageClass::Fixed => return Some(ContextCompactionProtectionReason::FixedRequest),
        ContextUsageClass::RequestOnly => {
            return Some(ContextCompactionProtectionReason::RequestOnly)
        }
        ContextUsageClass::RunTransient => {
            return Some(ContextCompactionProtectionReason::UncommittedRun)
        }
        ContextUsageClass::Durable => {}
    }
    if protect_current_user
        && last_durable_user_index.is_some_and(|index| {
            unit.usage_class == ContextUsageClass::Durable
                && unit.start_index <= index
                && index < unit.end_index_exclusive
        })
    {
        return Some(ContextCompactionProtectionReason::CurrentUser);
    }
    if unit.sources.contains(&ContextSource::InputAttachment) {
        return Some(ContextCompactionProtectionReason::UserAttachment);
    }
    if unit.sources.contains(&ContextSource::RuntimeGuard) {
        return Some(ContextCompactionProtectionReason::RuntimeGuard);
    }
    if unit.contains_images {
        return Some(ContextCompactionProtectionReason::VisualInput);
    }
    None
}

fn maximum_reclaimable_tokens(selected: &[CompactionCandidate]) -> u64 {
    // The target is deliberately soft. Prefix selection therefore uses the theoretical maximum
    // reclaim and leaves feasibility to the executor, which knows the exact continuity cost and
    // validates the generated replacement against the measured source.
    selected
        .iter()
        .map(|candidate| candidate.unit.tokens)
        .sum::<u64>()
}

fn build_steps(
    selected: &[CompactionCandidate],
    required_reclaimed_tokens: u64,
) -> Vec<ContextCompactionStep> {
    if selected.is_empty() {
        return Vec::new();
    }
    let mut candidates = selected.iter().collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.unit.start_index);
    let source_input_tokens = candidates
        .iter()
        .map(|candidate| candidate.unit.tokens)
        .sum::<u64>();
    let target_replacement_tokens = source_input_tokens.saturating_sub(required_reclaimed_tokens);
    let expected_reclaimed_tokens = source_input_tokens.saturating_sub(target_replacement_tokens);
    if expected_reclaimed_tokens == 0 {
        return Vec::new();
    }
    vec![ContextCompactionStep {
        ranges: merge_candidate_ranges(&candidates),
        atomic_unit_count: candidates.len(),
        source_input_tokens,
        target_replacement_tokens,
        expected_reclaimed_tokens,
        contains_side_effects: candidates
            .iter()
            .any(|candidate| candidate.unit.contains_side_effects),
        contains_errors: candidates
            .iter()
            .any(|candidate| candidate.unit.contains_errors),
        durable_prefix: durable_prefix_for_candidates(&candidates),
    }]
}

fn normalize_stable_durable_prefix(
    selected: &mut Vec<CompactionCandidate>,
    units: &[AtomicContextUnit],
    candidates: &[CompactionCandidate],
) {
    let durable_units = units
        .iter()
        .filter(|unit| unit.usage_class == ContextUsageClass::Durable)
        .collect::<Vec<_>>();
    if durable_units.is_empty() || durable_units.iter().any(|unit| unit.origin.is_none()) {
        return;
    }
    let Some(furthest_selected_start) = selected
        .iter()
        .map(|candidate| candidate.unit.start_index)
        .max()
    else {
        return;
    };
    let candidates_by_start = candidates
        .iter()
        .map(|candidate| (candidate.unit.start_index, candidate))
        .collect::<BTreeMap<_, _>>();
    let mut prefix = Vec::new();
    let mut last_valid_prefix_len = 0;
    let mut reached_selected_boundary = false;
    for unit in durable_units {
        let Some(candidate) = candidates_by_start.get(&unit.start_index) else {
            break;
        };
        prefix.push((*candidate).clone());
        reached_selected_boundary |= unit.start_index >= furthest_selected_start;
        if durable_prefix_for_owned_candidates(&prefix).is_some() {
            last_valid_prefix_len = prefix.len();
        }
        if reached_selected_boundary && last_valid_prefix_len == prefix.len() {
            break;
        }
    }
    prefix.truncate(last_valid_prefix_len);
    *selected = prefix;
}

fn durable_prefix_for_candidates(
    candidates: &[&CompactionCandidate],
) -> Option<ContextCompactionDurablePrefix> {
    durable_prefix_for_units(candidates.iter().map(|candidate| &candidate.unit))
}

fn durable_prefix_for_owned_candidates(
    candidates: &[CompactionCandidate],
) -> Option<ContextCompactionDurablePrefix> {
    durable_prefix_for_units(candidates.iter().map(|candidate| &candidate.unit))
}

fn durable_prefix_for_units<'a>(
    units: impl IntoIterator<Item = &'a AtomicContextUnit>,
) -> Option<ContextCompactionDurablePrefix> {
    let mut previous_summary_id = None;
    let mut covered_through = None;
    for (index, unit) in units.into_iter().enumerate() {
        let origin = unit.origin.as_ref()?;
        match origin.kind() {
            ContextOriginKind::CompactionSummary => {
                if index != 0
                    || previous_summary_id
                        .replace(origin.id().to_string())
                        .is_some()
                {
                    return None;
                }
                if unit.first_role != LlmMessageRole::Assistant
                    || unit.last_role != LlmMessageRole::Assistant
                {
                    return None;
                }
            }
            ContextOriginKind::ConversationMessage | ContextOriginKind::ConversationTraceItem => {
                covered_through = origin.journal_cursor();
            }
            ContextOriginKind::Skill => return None,
        }
    }
    Some(ContextCompactionDurablePrefix {
        previous_summary_id,
        covered_through: covered_through?,
    })
}

fn merge_candidate_ranges(candidates: &[&CompactionCandidate]) -> Vec<ContextCompactionItemRange> {
    let mut ranges = Vec::<ContextCompactionItemRange>::new();
    for candidate in candidates {
        if let Some(previous) = ranges.last_mut() {
            if previous.end_index_exclusive == candidate.unit.start_index {
                previous.end_index_exclusive = candidate.unit.end_index_exclusive;
                continue;
            }
        }
        ranges.push(ContextCompactionItemRange {
            start_index: candidate.unit.start_index,
            end_index_exclusive: candidate.unit.end_index_exclusive,
        });
    }
    ranges
}

fn percent_ceil(value: u64, percent: u64) -> u64 {
    value.saturating_mul(percent).div_ceil(100)
}

fn merge_reason_tokens(
    reasons: &mut BTreeMap<String, u64>,
    reason: ContextCompactionProtectionReason,
    tokens: u64,
) {
    let total = reasons.entry(protection_reason_name(reason)).or_default();
    *total = total.saturating_add(tokens);
}

fn protection_reason_name(reason: ContextCompactionProtectionReason) -> String {
    match reason {
        ContextCompactionProtectionReason::FixedRequest => "fixed_request",
        ContextCompactionProtectionReason::RequestOnly => "request_only",
        ContextCompactionProtectionReason::CurrentUser => "current_user",
        ContextCompactionProtectionReason::SkillInstructions => "skill_instructions",
        ContextCompactionProtectionReason::UserAttachment => "user_attachment",
        ContextCompactionProtectionReason::RuntimeGuard => "runtime_guard",
        ContextCompactionProtectionReason::VisualInput => "visual_input",
        ContextCompactionProtectionReason::MixedAtomicGroup => "mixed_atomic_group",
        ContextCompactionProtectionReason::UncommittedRun => "uncommitted_run",
    }
    .to_string()
}

fn empty_plan(
    status: ContextCompactionPlanStatus,
    query: &ContextCompactionQuery,
    thresholds: ContextCompactionThresholds,
    compactable_input_tokens: u64,
    protected: ContextCompactionProtectedEstimate,
) -> ContextCompactionPlan {
    ContextCompactionPlan {
        status,
        context_revision: query.context_revision,
        persistent_revision: query.persistent_revision,
        request_input_tokens: query.request_input_tokens,
        available_input_tokens: query.available_input_tokens,
        soft_trigger_input_tokens: thresholds.request_trigger_input_tokens,
        target_input_tokens: thresholds.request_target_input_tokens,
        durable_capacity_tokens: thresholds.durable_capacity_tokens,
        durable_trigger_input_tokens: thresholds.durable_trigger_input_tokens,
        durable_target_input_tokens: thresholds.durable_target_input_tokens,
        required_reclaimed_tokens: 0,
        required_durable_reclaimed_tokens: 0,
        planned_reclaimed_tokens: 0,
        projected_request_input_tokens: query.request_input_tokens,
        projected_durable_input_tokens: query.breakdown.durable.input_tokens,
        request_target_satisfied: status == ContextCompactionPlanStatus::NotRequired,
        durable_target_satisfied: status == ContextCompactionPlanStatus::NotRequired,
        best_effort: false,
        compactable_input_tokens,
        protected,
        steps: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::budget::{ContextTokenBreakdown, ContextTokenCategoryEstimate};

    fn item(
        index: usize,
        usage_class: ContextUsageClass,
        tokens: u64,
        role: LlmMessageRole,
        source: ContextSource,
        origin: Option<ContextOrigin>,
    ) -> ContextFramePlanningItem {
        ContextFramePlanningItem {
            index,
            usage_class,
            estimated_tokens: tokens,
            role,
            sources: vec![source],
            group_id: None,
            tool_names: Vec::new(),
            image_count: 0,
            is_error: false,
            origin,
        }
    }

    fn query(
        status: ContextBudgetStatus,
        available: Option<u64>,
        fixed: u64,
        durable: u64,
        run_transient: u64,
        request_only: u64,
    ) -> ContextCompactionQuery {
        let category = |input_tokens| ContextTokenCategoryEstimate {
            input_tokens,
            ..ContextTokenCategoryEstimate::default()
        };
        let total = fixed
            .saturating_add(durable)
            .saturating_add(run_transient)
            .saturating_add(request_only);
        ContextCompactionQuery {
            status,
            available_input_tokens: available,
            remaining_input_tokens: available.map(|limit| {
                i64::try_from(limit).unwrap_or(i64::MAX) - i64::try_from(total).unwrap_or(i64::MAX)
            }),
            request_input_tokens: total,
            additive_input_tokens: total,
            persistent_input_tokens: fixed.saturating_add(durable),
            context_revision: 7,
            persistent_revision: 5,
            breakdown: ContextTokenBreakdown {
                fixed: category(fixed),
                durable: category(durable),
                run_transient: category(run_transient),
                request_only: category(request_only),
                total: category(total),
            },
        }
    }

    #[test]
    fn stays_inert_below_the_soft_trigger() {
        let items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                100,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                300,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old")),
            ),
            item(
                2,
                ContextUsageClass::Durable,
                100,
                LlmMessageRole::User,
                ContextSource::CurrentTurn,
                Some(ContextOrigin::conversation_message("user-current")),
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(1_000),
                100,
                400,
                0,
                0,
            ),
            &items,
            true,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::NotRequired);
        assert!(plan.steps.is_empty());
        assert_eq!(plan.soft_trigger_input_tokens, Some(900));
        assert_eq!(plan.durable_trigger_input_tokens, Some(810));
    }

    #[test]
    fn starts_compaction_at_the_ninety_percent_request_threshold() {
        let planner = ContextCompactionPlanner::for_tools(&[]);
        let below_items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                100,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                799,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old")),
            ),
        ];
        let threshold_items = vec![
            below_items[0].clone(),
            item(
                1,
                ContextUsageClass::Durable,
                800,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old")),
            ),
        ];

        let below = planner.plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(1_000),
                100,
                799,
                0,
                0,
            ),
            &below_items,
            false,
        );
        let at_threshold = planner.plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(1_000),
                100,
                800,
                0,
                0,
            ),
            &threshold_items,
            false,
        );

        assert_eq!(below.status, ContextCompactionPlanStatus::NotRequired);
        assert_eq!(at_threshold.status, ContextCompactionPlanStatus::Required);
    }

    #[test]
    fn request_pressure_alone_uses_the_durable_fifteen_percent_target() {
        let items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                1_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                2_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old-1")),
            ),
            item(
                2,
                ContextUsageClass::Durable,
                2_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old-2")),
            ),
            item(
                3,
                ContextUsageClass::Durable,
                3_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old-3")),
            ),
            item(
                4,
                ContextUsageClass::RunTransient,
                1_000,
                LlmMessageRole::Tool,
                ContextSource::ToolResult,
                None,
            ),
        ];

        // Durable usage is only 77.8% of its 9,000-token capacity. The full request reaches the
        // 90% trigger because of protected run-transient content.
        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(10_000),
                1_000,
                7_000,
                1_000,
                0,
            ),
            &items,
            false,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert_eq!(plan.durable_trigger_input_tokens, Some(8_100));
        assert_eq!(plan.durable_target_input_tokens, Some(1_350));
        assert_eq!(plan.required_durable_reclaimed_tokens, 5_650);
        assert_eq!(plan.required_reclaimed_tokens, 5_650);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].source_input_tokens, 7_000);
        assert_eq!(plan.steps[0].target_replacement_tokens, 1_350);
        assert_eq!(plan.projected_durable_input_tokens, 1_350);
        assert!(plan.durable_target_satisfied);
        assert!(!plan.best_effort);
    }

    #[test]
    fn large_prefix_keeps_the_replacement_target_as_a_projection_only() {
        let items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                10_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                200_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old")),
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::OverBudget,
                Some(210_000),
                10_000,
                200_000,
                0,
                0,
            ),
            &items,
            false,
        );

        let step = &plan.steps[0];
        assert_eq!(step.target_replacement_tokens, 30_000);
        assert_eq!(step.expected_reclaimed_tokens, 170_000);
    }

    #[test]
    fn emits_one_stable_log_cursor_for_the_selected_raw_prefix() {
        let items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                1_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                3_000,
                LlmMessageRole::User,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("user-old")),
            ),
            item(
                2,
                ContextUsageClass::Durable,
                3_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old")),
            ),
            item(
                3,
                ContextUsageClass::Durable,
                1_000,
                LlmMessageRole::User,
                ContextSource::CurrentTurn,
                Some(ContextOrigin::conversation_message("user-current")),
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(8_000),
                1_000,
                7_000,
                0,
                0,
            ),
            &items,
            true,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(
            plan.steps[0]
                .durable_prefix
                .as_ref()
                .map(|prefix| &prefix.covered_through),
            Some(&ContextJournalCursor::message("assistant-old"))
        );
    }

    #[test]
    fn a_tool_call_and_result_are_selected_as_one_atomic_log_entry() {
        let tool_origin = ContextOrigin::conversation_trace_item("assistant-current", 2);
        let items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                100,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                500,
                LlmMessageRole::Assistant,
                ContextSource::ConversationTrace,
                Some(ContextOrigin::conversation_trace_item(
                    "assistant-current",
                    0,
                )),
            ),
            item(
                2,
                ContextUsageClass::Durable,
                2_500,
                LlmMessageRole::Assistant,
                ContextSource::ConversationTrace,
                Some(tool_origin.clone()),
            ),
            item(
                3,
                ContextUsageClass::Durable,
                2_500,
                LlmMessageRole::Tool,
                ContextSource::ConversationTrace,
                Some(tool_origin),
            ),
            item(
                4,
                ContextUsageClass::Durable,
                500,
                LlmMessageRole::User,
                ContextSource::CurrentTurn,
                Some(ContextOrigin::conversation_message("user-current")),
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(6_400),
                100,
                6_000,
                0,
                0,
            ),
            &items,
            true,
        );

        let step = &plan.steps[0];
        assert_eq!(step.atomic_unit_count, 2);
        assert_eq!(step.ranges[0].start_index, 1);
        assert_eq!(step.ranges[0].end_index_exclusive, 4);
        assert_eq!(
            step.durable_prefix
                .as_ref()
                .map(|prefix| &prefix.covered_through),
            Some(&ContextJournalCursor::trace_item("assistant-current", 2))
        );
    }

    #[test]
    fn unseen_run_overlay_is_never_a_compaction_candidate() {
        let items = vec![item(
            0,
            ContextUsageClass::RunTransient,
            9_000,
            LlmMessageRole::Tool,
            ContextSource::ToolResult,
            None,
        )];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(ContextBudgetStatus::OverBudget, Some(8_000), 0, 0, 9_000, 0),
            &items,
            false,
        );

        assert_eq!(
            plan.status,
            ContextCompactionPlanStatus::InsufficientCompactableContext
        );
        assert_eq!(plan.compactable_input_tokens, 0);
        assert_eq!(plan.protected.reasons.get("uncommitted_run"), Some(&9_000));
    }

    #[test]
    fn activated_skill_snapshot_has_explicit_compaction_protection() {
        let items = vec![item(
            0,
            ContextUsageClass::RunTransient,
            2_000,
            LlmMessageRole::User,
            ContextSource::SkillInstructions,
            Some(ContextOrigin::skill("workspace:w:review")),
        )];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(ContextBudgetStatus::OverBudget, Some(1_000), 0, 0, 2_000, 0),
            &items,
            false,
        );

        assert_eq!(
            plan.status,
            ContextCompactionPlanStatus::InsufficientCompactableContext
        );
        assert_eq!(plan.compactable_input_tokens, 0);
        assert_eq!(
            plan.protected.reasons.get("skill_instructions"),
            Some(&2_000)
        );
    }

    #[test]
    fn current_user_becomes_compactable_only_after_a_successful_model_request() {
        let items = vec![item(
            0,
            ContextUsageClass::Durable,
            9_000,
            LlmMessageRole::User,
            ContextSource::CurrentTurn,
            Some(ContextOrigin::conversation_message("user-current")),
        )];
        let pressure = query(ContextBudgetStatus::OverBudget, Some(8_000), 0, 9_000, 0, 0);
        let planner = ContextCompactionPlanner::for_tools(&[]);

        let before_first_request = planner.plan(&pressure, &items, true);
        let after_first_request = planner.plan(&pressure, &items, false);

        assert_eq!(
            before_first_request.status,
            ContextCompactionPlanStatus::InsufficientCompactableContext
        );
        assert_eq!(
            after_first_request.status,
            ContextCompactionPlanStatus::Required
        );
        assert_eq!(
            after_first_request.steps[0]
                .durable_prefix
                .as_ref()
                .map(|prefix| &prefix.covered_through),
            Some(&ContextJournalCursor::message("user-current"))
        );
    }

    #[test]
    fn an_unreachable_target_still_produces_a_best_effort_plan() {
        let items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                8_500,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                1_500,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-old")),
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::OverBudget,
                Some(9_000),
                8_500,
                1_500,
                0,
                0,
            ),
            &items,
            false,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert!(plan.best_effort);
        assert!(plan.planned_reclaimed_tokens > 0);
        assert_eq!(plan.steps[0].target_replacement_tokens, 0);
        assert_eq!(plan.steps[0].expected_reclaimed_tokens, 1_500);
    }

    #[test]
    fn recursive_compaction_carries_the_previous_summary_identity() {
        let items = vec![
            item(
                0,
                ContextUsageClass::Fixed,
                500,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
                None,
            ),
            item(
                1,
                ContextUsageClass::Durable,
                2_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationSummary,
                Some(ContextOrigin::compaction_summary("summary-1")),
            ),
            item(
                2,
                ContextUsageClass::Durable,
                4_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                Some(ContextOrigin::conversation_message("assistant-new")),
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(6_800),
                500,
                6_000,
                0,
                0,
            ),
            &items,
            false,
        );
        let prefix = plan.steps[0].durable_prefix.as_ref().unwrap();

        assert_eq!(prefix.previous_summary_id.as_deref(), Some("summary-1"));
        assert_eq!(
            prefix.covered_through,
            ContextJournalCursor::message("assistant-new")
        );
    }
}
